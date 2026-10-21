//! HTTP transcription with earshot segmentation and resumable chunks.

use crate::config::PipelineConfig;
use crate::timeline::TranscriptEvent;
use anyhow::{Context, Result};

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

// ---------- 超参数（VAD 切分参数见 normalize_segments 附近） ----------

/// HTTP 请求最多尝试次数（1 次首发 + 2 次重试）
const MAX_ATTEMPTS: u32 = 3;
/// 云端 STT 单次请求超时
const API_HTTP_TIMEOUT: Duration = Duration::from_secs(120);
/// ffmpeg 单段切音频上限（chunk 最长 max_speech 秒，处理应远快于此）
const CUT_TIMEOUT: Duration = Duration::from_secs(120);
/// 空结果熔断：VAD 已判定这些分片含语音，端点却在没有任何成功的情况下
/// 连续返回空文本——端点实际上无法处理音频输入（配置错误/网关丢音频体），
/// 继续只会空烧请求。单片为空仍是合法静音（见 record 处注释）。
const EMPTY_RESULT_TRIPWIRE: usize = 10;

/// 空结果熔断器：缓冲空结果，出现任何非空结果即复位（端点正常，静音属实）；
/// 空结果连续达到阈值时熔断。缓冲的空结果由调用方在确认不熔断后再落 checkpoint，
/// 避免熔断后空记录把分片标成"已完成静音"，修好之后重跑也救不回来。
#[derive(Default)]
struct EmptyTripwire {
    pending_empty: usize,
    seen_success: bool,
}

impl EmptyTripwire {
    /// 记录一条结果，返回是否已熔断。
    fn record(&mut self, empty: bool) -> bool {
        if empty {
            self.pending_empty += 1;
        } else {
            self.seen_success = true;
        }
        !self.seen_success && self.pending_empty >= EMPTY_RESULT_TRIPWIRE
    }

    fn tripped_error(&self) -> anyhow::Error {
        anyhow::anyhow!(
            "语音服务连续 {} 个分片返回空文本；请检查语音服务地址与模型是否支持音频输入 / The speech service returned empty text for {} consecutive segments; check that the endpoint and model support audio input",
            self.pending_empty,
            self.pending_empty
        )
    }
}

/// Empty results are buffered until the endpoint is known to work.
/// 每个 chunk 结果先经 tripwire 计数，连续空结果熔断时**不落任何 checkpoint**
/// （空记录会把分片标成「已完成静音」，掩盖真实的识别失败）；空结果先缓冲，
/// 出现非空结果（或正常收尾）才批量落账。并行路径结果乱序到达也安全：
/// 乱序只改变 pending 的相对顺序，批量落账全部写同一空文本，checkpoint 内容一致。
#[derive(Default)]
struct EmptyResultTracker {
    tripwire: EmptyTripwire,
    pending_empty: Vec<(f64, f64)>,
}

impl EmptyResultTracker {
    /// 处理一个 chunk 的转写结果：非空结果落账，空结果缓冲。
    /// Err = 需要中止（熔断触发或 checkpoint 写盘失败），错误原样上抛。
    fn record_outcome(
        &mut self,
        cp: &mut crate::checkpoint::Checkpoint,
        start: f64,
        end: f64,
        text: &str,
    ) -> Result<()> {
        let empty = text.trim().is_empty();
        if self.tripwire.record(empty) {
            // 熔断不落账（见 struct 注释）
            return Err(self.tripwire.tripped_error());
        }
        if empty {
            self.pending_empty.push((start, end));
            return Ok(());
        }
        self.flush_pending(cp)?;
        cp.record(start, end, text)
    }

    /// 正常收尾：把缓冲的空结果（真实静音）落 checkpoint，避免静音 chunk 反复重跑。
    /// 出错（含熔断）时调用方不得调用本方法——pending 必须随任务失败丢弃。
    fn finish(self, cp: &mut crate::checkpoint::Checkpoint) -> Result<()> {
        let mut this = self;
        this.flush_pending(cp)
    }

    /// 批量落账缓冲的空结果；写盘失败即中断，剩余 pending 保留待重跑。
    fn flush_pending(&mut self, cp: &mut crate::checkpoint::Checkpoint) -> Result<()> {
        for (start, end) in std::mem::take(&mut self.pending_empty) {
            cp.record(start, end, "")?;
        }
        Ok(())
    }
}

/// 清洗 Qwen3 转写中的提示词残留。
pub fn sanitize_qwen_text(s: &str) -> String {
    let mut t = s.trim();
    if let Some(p) = t.find("</asr_text>") {
        t = t[..p].trim();
    }
    if let Some(p) = t.rfind("<asr_text>") {
        t = t[p + "<asr_text>".len()..].trim();
    }
    t.to_string()
}

struct CancelOnDrop(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Open the checkpoint and mark it complete only after successful transcription.
async fn run_with_cp<F>(
    cfg: &PipelineConfig,
    identity: &crate::checkpoint::AsrIdentity,
    f: F,
) -> Result<Vec<TranscriptEvent>>
where
    F: FnOnce(
            &mut crate::checkpoint::Checkpoint,
            &std::sync::atomic::AtomicBool,
        ) -> Result<Vec<TranscriptEvent>>
        + Send
        + 'static,
{
    let mut cp = crate::checkpoint::Checkpoint::open(&cfg.out_dir, cfg.resume, identity)?;
    let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let _cancel_on_drop = CancelOnDrop(cancelled.clone());
    tokio::task::spawn_blocking(move || {
        let r = f(&mut cp, &cancelled);
        if r.is_ok() {
            cp.finish()?;
        }
        r
    })
    .await
    .context(crate::tr!(
        "语音识别任务未能完成 / Speech recognition task did not complete"
    ))?
}

pub async fn run(cfg: &PipelineConfig, wav: &Path) -> Result<Vec<TranscriptEvent>> {
    cfg.validate_asr()?;
    let identity = crate::checkpoint::AsrIdentity::new(&cfg.asr);
    let api = cfg.asr.clone();
    let wav = wav.to_path_buf();
    run_with_cp(cfg, &identity, move |cp, cancelled| {
        run_api(&api, &wav, cp, cancelled)
    })
    .await
}

/// 所有后端共用的「VAD 后没有语音」收尾：只保留截图，并记录空结果。
pub(crate) fn no_speech_events() -> Vec<TranscriptEvent> {
    tracing::warn!(
        "未检测到语音，将仅保留截图 / No speech detected; keeping slides without a transcript"
    );
    Vec::new()
}

/// The same bounded HTTP chunk pipeline serves remote and managed endpoints.
fn run_api(
    api: &crate::settings::Asr,
    wav: &Path,
    cp: &mut crate::checkpoint::Checkpoint,
    cancelled: &std::sync::atomic::AtomicBool,
) -> Result<Vec<TranscriptEvent>> {
    let check_control = || {
        anyhow::ensure!(
            !cancelled.load(std::sync::atomic::Ordering::Relaxed),
            crate::tr!("Transcription cancelled.")
        );
        crate::dispatch::check_control()
    };
    check_control()?;
    let t0 = Instant::now();
    // 凭据已在任务预检时解析（含环境变量）；空 key = 明确选择的无鉴权服务
    let api_key = api.api_key.trim().to_string();
    let segs = earshot_vad(wav, api, &check_control)?;
    check_control()?;
    tracing::info!(segs = segs.len(), endpoint = %api.base_url, model = %api.model, "api vad");
    if segs.is_empty() {
        return Ok(no_speech_events());
    }

    // 断点续跑：预先过滤出未完成的 chunk（worker 只拿真正需要执行的任务）
    let pending: Vec<usize> = (0..segs.len())
        .filter(|&i| !cp.is_done(segs[i].start, segs[i].end))
        .collect();
    if pending.is_empty() {
        return Ok(cp.sorted_events());
    }

    let tmp = crate::runtime::TempWorkDir::new("asr")?;
    let mut api = api.clone();
    let mut adapter = if api.command.is_empty() {
        None
    } else {
        let adapter = crate::adapter::Adapter::start_with_control(&api.command, &check_control)?;
        api.base_url = format!("{}/v1", adapter.base);
        Some(adapter)
    };
    let url = crate::config::asr_endpoint(&api)?;
    let pb = crate::progress::Bar::new("transcribe", segs.len() as u64).with_template(
        "{spinner:.green} asr {pos}/{len} [{bar:32.cyan/blue}] {elapsed} {eta} {msg}",
    );

    let client = crate::runtime::http_agent(API_HTTP_TIMEOUT);
    pb.set_position((segs.len() - pending.len()) as u64);
    crate::progress::emit(
        serde_json::json!({"type":"workers", "stage":"transcribe", "workers": api.concurrency.clamp(1, 16).min(pending.len())}),
    );

    // 有界并发（std::thread::scope + 借用，无需 Arc）：网络往返是主要瓶颈；
    // 结果经 channel 回收后记录。abort 后 in-flight 请求自然结束，无人 join 不到。
    let (tx, rx) = std::sync::mpsc::channel::<(usize, Result<Option<String>>)>();
    let next = std::sync::atomic::AtomicUsize::new(0);
    let abort = std::sync::atomic::AtomicBool::new(false);
    let mut err: Option<anyhow::Error> = None;
    std::thread::scope(|s| {
        for _ in 0..api.concurrency.clamp(1, 16).min(pending.len()) {
            let tx = tx.clone();
            let target = ApiTarget {
                client: &client,
                url: &url,
                model: &api.model,
                key: &api_key,
                mode: api.mode,
                language: &api.language,
                managed: adapter.is_some(),
            };
            let (tmp_dir, wav) = (tmp.path(), wav);
            let (segs, pending, next, abort) = (&segs, &pending, &next, &abort);
            let check_control = &check_control;
            s.spawn(move || {
                loop {
                    if abort.load(std::sync::atomic::Ordering::Relaxed) {
                        break;
                    }
                    let idx = next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let Some(i) = pending.get(idx).copied() else {
                        break;
                    };
                    if let Err(error) = check_control() {
                        abort.store(true, std::sync::atomic::Ordering::Relaxed);
                        let _ = tx.send((i, Err(error)));
                        break;
                    }
                    let seg = segs[i];
                    let r =
                        transcribe_api(&target, &tmp_dir.join(format!("c{i:04}.wav")), seg, wav);
                    if tx.send((i, r)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(tx);

        let mut tracker = EmptyResultTracker::default();
        loop {
            if err.is_none()
                && let Err(error) = check_control()
            {
                err = Some(error);
                abort.store(true, std::sync::atomic::Ordering::Relaxed);
                if let Some(adapter) = &mut adapter {
                    adapter.stop();
                }
            }
            let (i, r) = match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(result) => result,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            };
            if err.is_none()
                && let Err(error) = check_control()
            {
                err = Some(error);
                abort.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            match r {
                _ if err.is_some() => {} // 已熔断/已出错：丢弃迟到结果
                Ok(text) => {
                    let text = text.as_deref().unwrap_or("");
                    if let Err(e) = tracker.record_outcome(cp, segs[i].start, segs[i].end, text) {
                        err = Some(e);
                        abort.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                }
                Err(e) => {
                    err = Some(
                        e.context(crate::tr!("Speech transcription failed for segment {}.", i)),
                    );
                    abort.store(true, std::sync::atomic::Ordering::Relaxed);
                }
            }
            if err.is_some()
                && let Some(adapter) = &mut adapter
            {
                adapter.stop();
            }
            // 每个回收的结果（含空结果）恰好推进一次，与串行路径一致
            pb.inc(1);
        }
        // 正常收尾：缓冲的空结果（真实静音）落 checkpoint，避免静音 chunk 反复重跑
        if err.is_none()
            && let Err(e) = check_control().and_then(|()| tracker.finish(cp))
        {
            err = Some(e);
        }
    });
    pb.finish();
    if let Some(e) = err {
        return Err(if let Some(adapter) = &adapter {
            adapter.stderr.attach(e, crate::tr!("ASR adapter output"))
        } else {
            e
        });
    }
    check_control()?;
    // 事件统一来自 checkpoint（收集循环里已 record），按时间排序
    let events: Vec<TranscriptEvent> = cp.sorted_events();
    tracing::info!(
        n = events.len(),
        secs = format_args!("{:.1}", t0.elapsed().as_secs_f64()),
        "asr done"
    );
    Ok(events)
}

/// 有界重试：可重试错误按 [`crate::runtime::retry_backoff`] 退避（退避期间可取消），
/// 共尝试 MAX_ATTEMPTS 次；确定性错误（鉴权/参数）直接返回。
fn with_retry<T>(
    mut attempt_once: impl FnMut() -> std::result::Result<T, crate::dispatch::Failure>,
) -> Result<T> {
    let mut attempt = 1;
    loop {
        crate::dispatch::check_control()?;
        match attempt_once() {
            Ok(value) => return Ok(value),
            Err(error) if error.retryable && attempt < MAX_ATTEMPTS => {
                crate::runtime::interruptible_sleep(crate::runtime::retry_backoff(attempt))?;
                attempt += 1;
            }
            Err(error) => return Err(error.into()),
        }
    }
}

/// 云端识别：每次发送前先落请求账本（结果不确定时绝不自动重发）。
/// 响应在账本里按模式校验，取值时不必再检查一遍。
fn post_cloud(
    t: &ApiTarget,
    seg: Seg,
    content_type: &str,
    body: &[u8],
    identity: &serde_json::Value,
) -> Result<serde_json::Value> {
    let description = crate::tr!(
        "识别视频 {}–{} 的声音 / Transcribe video audio {}–{}",
        crate::render::fmt_ts(seg.start),
        crate::render::fmt_ts(seg.end),
        crate::render::fmt_ts(seg.start),
        crate::render::fmt_ts(seg.end)
    );
    with_retry(|| {
        let send = || {
            let request = t.client.post(t.url).set("Content-Type", content_type);
            let request = if t.key.is_empty() {
                request
            } else {
                request.set("Authorization", &format!("Bearer {}", t.key))
            };
            crate::dispatch::receive(request.send_bytes(body))
        };
        crate::dispatch::json_request_described(
            "asr",
            "transcription",
            &description,
            t.url,
            identity,
            if t.managed {
                crate::dispatch::Resend::Free
            } else {
                crate::dispatch::Resend::AskUser
            },
            send,
            |value| {
                anyhow::ensure!(
                    value.get("error").is_none_or(serde_json::Value::is_null),
                    crate::tr!("语音服务返回了错误 / The speech service returned an error")
                );
                let has_text = match t.mode {
                    crate::settings::AsrMode::Transcriptions | crate::settings::AsrMode::Xai => {
                        value["text"].is_string()
                    }
                    crate::settings::AsrMode::Meta => value["transcript"].is_string(),
                    crate::settings::AsrMode::Chat => chat_content_has_text(value),
                };
                anyhow::ensure!(
                    has_text,
                    crate::tr!("语音服务没有返回文字 / The speech service returned no text")
                );
                Ok(())
            },
        )
    })
}

/// chat 模式下让多模态 LLM 转录的指令（保持与本地 ASR 输出风格一致：忠实、带自然标点）。
const CHAT_TRANSCRIBE_PROMPT: &str = "Transcribe this audio. Output only the transcript itself with natural punctuation, and no explanation, summary or formatting. If there is no speech, output an empty string.";

/// 云端转录端点：地址、凭据与请求模式（worker 间共享借用）。
struct ApiTarget<'a> {
    client: &'a ureq::Agent,
    url: &'a str,
    model: &'a str,
    key: &'a str,
    mode: crate::settings::AsrMode,
    language: &'a str,
    managed: bool,
}

/// 转写单个 chunk；Ok(None) = 无语音内容。
fn transcribe_api(t: &ApiTarget, chunk: &Path, seg: Seg, wav: &Path) -> Result<Option<String>> {
    cut_wav(wav, seg.cut_start, seg.cut_end, chunk)
        .context(crate::tr!("无法切分音频 / Could not split audio"))?;
    let bytes = std::fs::read(chunk)
        .with_context(|| crate::tr!("Reading audio segment: {}", chunk.display()))?;
    let identity = serde_json::json!({"model":t.model,"mode":t.mode,"language":t.language,"audio_sha256":crate::execution::digest(&bytes),"segment_start":seg.start,"segment_end":seg.end,"cut_start":seg.cut_start,"cut_end":seg.cut_end});
    let (content_type, body) = transcription_request(t, &bytes)?;
    let result = post_cloud(t, seg, &content_type, &body, &identity);
    let _ = std::fs::remove_file(chunk);
    let value = result?;
    let text = match t.mode {
        crate::settings::AsrMode::Transcriptions | crate::settings::AsrMode::Xai => {
            value["text"].as_str().unwrap_or_default().to_owned()
        }
        crate::settings::AsrMode::Meta => {
            value["transcript"].as_str().unwrap_or_default().to_owned()
        }
        crate::settings::AsrMode::Chat => parse_chat_content(&value),
    };
    let text = sanitize_qwen_text(&text);
    Ok((!text.is_empty()).then_some(text))
}

/// Options precede the binary audio part in every multipart dialect.
fn transcription_request(t: &ApiTarget, audio: &[u8]) -> Result<(String, Vec<u8>)> {
    use crate::settings::AsrMode;
    use base64::Engine as _;
    if t.mode == AsrMode::Chat {
        let mut prompt = CHAT_TRANSCRIBE_PROMPT.to_owned();
        if !t.language.is_empty() {
            prompt.push_str(&format!(
                " The speech is in {}.",
                crate::language::english_name(t.language)
            ));
        }
        let mut body = serde_json::json!({
            "temperature": 0,
            "messages": [{"role":"user", "content":[
                {"type":"text", "text":prompt},
                {"type":"input_audio", "input_audio":{"data":base64::engine::general_purpose::STANDARD.encode(audio), "format":"wav"}}
            ]}]
        });
        if !t.model.is_empty() {
            body["model"] = t.model.into();
        }
        return Ok(("application/json".into(), serde_json::to_vec(&body)?));
    }
    let boundary = format!("notepipe-{}", crate::random_b64url(24)?);
    let mut options = String::new();
    let audio_part = if t.mode == AsrMode::Meta {
        let mut request = serde_json::json!({"audioEncoding":"WAV"});
        if !t.model.is_empty() {
            request["model"] = t.model.into();
        }
        if !t.language.is_empty() {
            request["languageBias"] =
                serde_json::json!([crate::language::english_name(t.language)]);
        }
        options.push_str(&format!("--{boundary}\r\nContent-Disposition: form-data; name=\"request\"\r\nContent-Type: application/json\r\n\r\n{request}\r\n"));
        "audio"
    } else {
        for (name, value) in [("model", t.model), ("language", t.language)] {
            if !value.is_empty() {
                options.push_str(&format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"));
            }
        }
        "file"
    };
    let mut body = format!("{options}--{boundary}\r\nContent-Disposition: form-data; name=\"{audio_part}\"; filename=\"audio.wav\"\r\nContent-Type: audio/wav\r\n\r\n").into_bytes();
    body.extend_from_slice(audio);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    Ok((format!("multipart/form-data; boundary={boundary}"), body))
}

/// chat/completions 响应的 message.content 是否携带文本：字符串形式，或
/// 多模态分片数组 [{type:"text", text:...}] 中任一片含 text 字段。
fn chat_content_has_text(v: &serde_json::Value) -> bool {
    let content = &v["choices"][0]["message"]["content"];
    content.is_string()
        || content
            .as_array()
            .is_some_and(|parts| parts.iter().any(|part| part["text"].is_string()))
}

/// 从 chat/completions 响应取文本：content 通常是字符串；部分多模态端点
/// 返回 [{type:"text", text:...}] 分片数组，拼起来即可。
fn parse_chat_content(v: &serde_json::Value) -> String {
    let content = &v["choices"][0]["message"]["content"];
    if let Some(s) = content.as_str() {
        return s.to_string();
    }
    content
        .as_array()
        .map(|parts| {
            parts
                .iter()
                .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default()
}

/// Stream 16 kHz mono s16le PCM in 256-sample frames; pad only the final frame.
fn earshot_vad(
    wav: &Path,
    asr: &crate::settings::Asr,
    check_control: &dyn Fn() -> Result<()>,
) -> Result<Vec<Seg>> {
    let mut detector = earshot::Detector::default();
    let mut speech: Vec<(u64, u64)> = Vec::new();
    let mut detect = |frame: &[i16], start: u64, end: u64| {
        if detector.predict_i16(frame) >= asr.vad_threshold {
            if let Some(last) = speech.last_mut()
                && (start == last.1 || start - last.1 < (asr.min_silence as f64 * 16_000.0) as u64)
            {
                last.1 = end;
            } else {
                speech.push((start, end));
            }
        }
    };
    let mut frame = [0i16; 256];
    let mut filled = 0;
    let mut offset = 0u64;
    let samples = read_pcm_samples(wav, |sample| {
        frame[filled] = sample;
        filled += 1;
        if filled == frame.len() {
            if offset.is_multiple_of(256 * 32) {
                check_control()?;
            }
            detect(&frame, offset, offset + 256);
            offset += 256;
            filled = 0;
        }
        Ok(())
    })?;
    if filled > 0 {
        frame[filled..].fill(0);
        detect(&frame, offset, samples);
    }
    let speech = speech
        .into_iter()
        .map(|(start, end)| (start as f64 / 16_000.0, end as f64 / 16_000.0))
        .collect();
    normalize_segments(
        speech,
        asr.max_speech as f64,
        wav,
        samples as f64 / 16_000.0,
    )
}

/// 最终送入 ASR 的分段：`start/end` 是事件时间（用于时间线），`cut_*` 是切音频范围（含静音填充）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Seg {
    pub start: f64,
    pub end: f64,
    pub cut_start: f64,
    pub cut_end: f64,
}

/// Stream the pipeline's 16 kHz mono s16le WAV, returning its actual sample count.
fn read_pcm_samples(wav: &Path, mut on_sample: impl FnMut(i16) -> Result<()>) -> Result<u64> {
    use std::io::{BufReader, Read, Seek};
    let file = std::fs::File::open(wav)
        .with_context(|| format!("无法读取音频 / Could not read audio: {}", wav.display()))?;
    let mut reader = BufReader::new(file);
    // RIFF 头 + 逐 chunk 头扫描，流式定位 PCM data 块。
    let mut header = [0u8; 12];
    reader
        .read_exact(&mut header)
        .ok()
        .filter(|_| &header[0..4] == b"RIFF" && &header[8..12] == b"WAVE")
        .ok_or_else(|| {
            anyhow::anyhow!(crate::tr!(
                "无法解析 wav PCM 数据 / Cannot parse wav PCM data"
            ))
        })?;
    let data_size = loop {
        let mut ch = [0u8; 8];
        if reader.read_exact(&mut ch).is_err() {
            anyhow::bail!(crate::tr!(
                "无法解析 wav PCM 数据 / Cannot parse wav PCM data"
            ));
        }
        let size = u32::from_le_bytes([ch[4], ch[5], ch[6], ch[7]]) as u64;
        if &ch[0..4] == b"data" {
            break size;
        }
        // 跳过其他 chunk（含奇数对齐字节）
        reader.seek(std::io::SeekFrom::Current((size + (size & 1)) as i64))?;
    };
    let mut buf = vec![0u8; 128 * 1024];
    let mut half: Option<u8> = None; // 跨 read 边界的样本低字节
    let mut samples = 0;
    let mut emit = |sample| {
        on_sample(sample)?;
        samples += 1;
        Ok::<(), anyhow::Error>(())
    };
    let mut remaining = data_size;
    while remaining > 0 {
        let want = remaining.min(buf.len() as u64) as usize;
        let n = reader.read(&mut buf[..want])?;
        if n == 0 {
            break; // data 块声明长度超出文件实际（截断文件）：按实际读到的计
        }
        remaining -= n as u64;
        let mut i = 0;
        if let Some(lo) = half.take() {
            emit(i16::from_le_bytes([lo, buf[0]]))?;
            i = 1;
        }
        while i + 1 < n {
            emit(i16::from_le_bytes([buf[i], buf[i + 1]]))?;
            i += 2;
        }
        if i < n {
            half = Some(buf[i]);
        }
    }
    Ok(samples)
}

/// 音频逐 100ms 的 RMS 能量（用于在目标切点附近找最近的静音最低点）。
pub struct Energy {
    hop: f64,
    rms: Vec<f32>,
}

impl Energy {
    /// 流式读取 16k 单声道 s16 wav（extract_audio 的固定产物）：
    /// 分块读入并滑动累计每个 100ms 窗口的平方和，只保留 rms 数组——
    /// 不把整文件和全部样本读进内存（2h 音频约 1GB）。
    pub fn load(wav: &Path) -> Result<Self> {
        let mut rms = Vec::new();
        let mut sum = 0.0f64;
        let mut n_in_hop = 0usize;
        read_pcm_samples(wav, |sample| {
            rms_accumulate(sample, &mut sum, &mut n_in_hop, &mut rms);
            Ok(())
        })?;
        if n_in_hop > 0 {
            rms.push((sum / n_in_hop as f64).sqrt() as f32);
        }
        Ok(Self { hop: 0.1, rms })
    }

    /// [a,b]（秒）内能量最低的时刻；无数据时返回 None。
    fn quietest(&self, a: f64, b: f64) -> Option<f64> {
        let i0 = (a / self.hop).ceil() as usize;
        let i1 = (b / self.hop).floor() as usize;
        if i1 <= i0 || i0 >= self.rms.len() {
            return None;
        }
        let i1 = i1.min(self.rms.len() - 1);
        // total_cmp：rms 来自非负能量的 sqrt，理论上无 NaN，但不赌 partial_cmp 不 panic
        let (bi, _) = self.rms[i0..=i1]
            .iter()
            .enumerate()
            .min_by(|x, y| x.1.total_cmp(y.1))?;
        Some((i0 + bi) as f64 * self.hop + self.hop / 2.0)
    }
}

/// 累计一个样本进当前 100ms 窗口；窗口满则写入 rms 并归零重开。
fn rms_accumulate(sample: i16, sum: &mut f64, n_in_hop: &mut usize, rms: &mut Vec<f32>) {
    const HOP: usize = 1600; // 100ms @16k
    *sum += (sample as f64 / 32768.0).powi(2);
    *n_in_hop += 1;
    if *n_in_hop == HOP {
        rms.push((*sum / HOP as f64).sqrt() as f32);
        *sum = 0.0;
        *n_in_hop = 0;
    }
}

const PAD: f64 = 0.25; // 切音频时向两侧静音各延展的秒数
const SPLIT_WINDOW: f64 = 3.0; // 在目标切点 ± 此窗口内寻找静音最低点
const MIN_PIECE: f64 = 1.0; // 硬切产生的最短片段
/// 切点距段起点的最小秒数：防止能量异常时切点贴到起点，产生超短 chunk 无限递归
const MIN_HARD_CUT: f64 = 0.5;

/// VAD 后处理：能量感知切分 + 静音填充。
/// - 超过 max_speech 的段在 [target-3s, target+3s] 窗口内选能量最低点切（避开词中切断）
/// - 切音频时向两侧静音各填充 0.25s（只进静音、不进相邻语音，故无重复文本）
///
/// `dur` 由调用方一次性探测传入（避免每段各 ffprobe 一次）。
pub fn normalize_segments(
    speech: Vec<(f64, f64)>,
    max_speech: f64,
    wav: &Path,
    dur: f64,
) -> Result<Vec<Seg>> {
    // VAD 成功但无语音（或全被短段过滤）→ 空分段。
    // 不把整段音频当语音兜底：静音课件会诱发 ASR 幻觉。
    let mut raw = speech;
    raw.retain(|(a, b)| b - a >= 0.2);
    if raw.is_empty() {
        return Ok(vec![]);
    }
    // 能量曲线只在有超长段需要切分时才读（要流式扫描整个音频）
    let energy = if raw.iter().any(|(a, b)| b - a > max_speech) {
        Energy::load(wav)
            .inspect_err(|e| {
                tracing::warn!(
                    "无法读取音频能量，长段将按固定长度切分 / Could not read audio energy; long segments are split at fixed lengths: {e:#}"
                )
            })
            .ok()
    } else {
        None
    };

    let mut pieces: Vec<(f64, f64)> = vec![];
    for &(s, e) in &raw {
        split_smart(s, e, max_speech, energy.as_ref(), &mut pieces);
    }

    // 填充：VAD 外边界向真实静音扩展 0.25s（限制来自相邻原始语音段，而非本段自身）；
    // max_speech 内部切点已在能量最低点，不额外填充（避免相邻 chunk 重复文本）。
    let segs = pieces
        .iter()
        .map(|&(s, e)| {
            let host_idx = raw
                .iter()
                .position(|&(rs, re)| s >= rs - 1e-6 && e <= re + 1e-6);
            // 该 piece 是否是其所在 raw 段的第一片/最后一片（外边界才能 pad）
            let (is_first_of_host, is_last_of_host) = match host_idx {
                Some(h) => {
                    let (rs, re) = raw[h];
                    let first = (s - rs).abs() < 1e-6;
                    let last = (re - e).abs() < 1e-6;
                    (first, last)
                }
                None => (true, true),
            };
            // 前一个原始语音段的终点（跨段静音的上限）
            let speech_lo = match host_idx {
                Some(h) if h > 0 => raw[h - 1].1,
                _ => 0.0,
            };
            let speech_hi = match host_idx {
                Some(h) if h + 1 < raw.len() => raw[h + 1].0,
                _ => dur,
            };
            let cut_start = if is_first_of_host {
                (s - PAD).max(speech_lo).max(0.0)
            } else {
                s
            };
            let cut_end = if is_last_of_host {
                (e + PAD).min(speech_hi)
            } else {
                e
            };
            Seg {
                start: s,
                end: e,
                cut_start,
                cut_end: cut_end.max(e),
            }
        })
        .collect();
    Ok(segs)
}

/// 递归切分：优先在静音最低点切，找不到则回退硬切。
fn split_smart(s: f64, e: f64, max: f64, energy: Option<&Energy>, out: &mut Vec<(f64, f64)>) {
    if e - s <= max {
        out.push((s, e));
        return;
    }
    let target = s + max;
    // 只在 [target-3s, target] 内找静音最低点：任何 piece 都不超过 max（硬上限，
    // ASR 后端常有上下文长度限制）
    let w0 = (target - SPLIT_WINDOW).max(s + MIN_PIECE.min(max / 2.0));
    let w1 = target;
    let cut = energy
        .and_then(|en| en.quietest(w0.max(s), w1))
        .unwrap_or(target);
    let cut = cut.clamp(s + MIN_HARD_CUT, target);
    out.push((s, cut));
    split_smart(cut, e, max, energy, out);
}

pub fn cut_wav(src: &Path, start: f64, end: f64, dest: &Path) -> Result<()> {
    let dur = (end - start).max(0.05);
    let mut cmd = Command::new("ffmpeg");
    cmd.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-nostdin",
        "-y",
        "-ss",
    ])
    .arg(format!("{start:.3}"))
    .arg("-t")
    .arg(format!("{dur:.3}"))
    .arg("-i")
    .arg(src)
    .args(["-ac", "1", "-ar", "16000", "-c:a", "pcm_s16le"])
    .arg(dest);
    let out = crate::runtime::run_bounded("ffmpeg", &mut cmd, CUT_TIMEOUT)?;
    if !out.status.success() {
        anyhow::bail!(crate::tr!("无法切分音频 / ffmpeg could not split audio"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_tripwire_fires_only_without_any_success() {
        let mut t = EmptyTripwire::default();
        for _ in 0..EMPTY_RESULT_TRIPWIRE - 1 {
            assert!(!t.record(true));
        }
        assert!(t.record(true), "连续空结果达到阈值必须熔断");
        assert!(t.tripped_error().to_string().contains("空文本"));

        // 一旦出现非空结果（端点其实正常），静音属实，不再熔断
        let mut t = EmptyTripwire::default();
        assert!(!t.record(true));
        assert!(!t.record(false));
        for _ in 0..EMPTY_RESULT_TRIPWIRE * 2 {
            assert!(!t.record(true));
        }
    }

    #[test]
    fn transcription_upload_preserves_audio_bytes() {
        let audio = b"RIFF\0\xff\r\nWAVE";
        let (content_type, body) = transcription_request(
            &ApiTarget {
                client: &ureq::agent(),
                url: "http://localhost",
                model: "whisper-1",
                key: "",
                mode: crate::settings::AsrMode::Transcriptions,
                language: "",
                managed: false,
            },
            audio,
        )
        .unwrap();
        let boundary = content_type
            .strip_prefix("multipart/form-data; boundary=")
            .unwrap();
        assert!(body.starts_with(format!("--{boundary}\r\n").as_bytes()));
        assert!(body.windows(audio.len()).any(|slice| slice == audio));
        assert!(body.ends_with(format!("\r\n--{boundary}--\r\n").as_bytes()));
        assert!(String::from_utf8_lossy(&body).contains("name=\"file\"; filename=\"audio.wav\""));
    }

    fn speech_fixture_samples() -> Vec<i16> {
        let path = Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/speech.wav"
        ));
        let mut samples = Vec::new();
        let count = read_pcm_samples(path, |sample| {
            samples.push(sample);
            Ok(())
        })
        .unwrap();
        assert_eq!(count, samples.len() as u64);
        samples
    }

    fn test_wav(samples: &[i16]) -> tempfile::NamedTempFile {
        use std::io::Write as _;
        let pcm: Vec<u8> = samples
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect();
        let size = u32::try_from(pcm.len()).unwrap();
        let header = [
            b"RIFF".as_slice(),
            &(size + 36).to_le_bytes(),
            b"WAVEfmt ",
            &16u32.to_le_bytes(),
            &1u16.to_le_bytes(),
            &1u16.to_le_bytes(),
            &16_000u32.to_le_bytes(),
            &32_000u32.to_le_bytes(),
            &2u16.to_le_bytes(),
            &16u16.to_le_bytes(),
            b"data",
            &size.to_le_bytes(),
        ]
        .concat();
        let mut wav = tempfile::NamedTempFile::new().unwrap();
        wav.write_all(&header).unwrap();
        wav.write_all(&pcm).unwrap();
        wav
    }

    #[test]
    fn earshot_detects_the_speech_fixture_between_silent_seconds() {
        let mut samples = vec![0; 16_000];
        samples.extend(speech_fixture_samples());
        samples.extend([0; 16_000]);
        let wav = test_wav(&samples);
        let segments = earshot_vad(wav.path(), &Default::default(), &|| Ok(())).unwrap();
        assert_eq!(segments.len(), 1, "{segments:?}");
        assert!((segments[0].start - 1.0).abs() <= 0.35, "{segments:?}");
        assert!((segments[0].end - 3.4).abs() <= 0.35, "{segments:?}");
        assert!(segments[0].cut_start >= 0.0);
        assert!(segments[0].cut_end <= samples.len() as f64 / 16_000.0);
    }

    #[test]
    fn vad_checks_control_during_pcm_scanning() {
        let wav = test_wav(&vec![0; 16_000]);
        let polls = std::cell::Cell::new(0);
        let error = earshot_vad(wav.path(), &Default::default(), &|| {
            polls.set(polls.get() + 1);
            anyhow::ensure!(polls.get() < 2, "test cancellation");
            Ok(())
        })
        .unwrap_err();
        assert_eq!(error.to_string(), "test cancellation");
        assert_eq!(polls.get(), 2);
    }

    #[test]
    fn earshot_silence_has_no_segments_including_a_partial_final_frame() {
        let wav = test_wav(&vec![0; 16_000]);
        assert!(
            earshot_vad(wav.path(), &Default::default(), &|| Ok(()))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn earshot_merges_short_pauses_and_keeps_long_pauses_separate() {
        let speech = speech_fixture_samples();
        for (silence_samples, expected) in [(1_600, 1), (16_000, 2)] {
            let mut samples = vec![0; 16_000];
            samples.extend_from_slice(&speech);
            samples.resize(samples.len() + silence_samples, 0);
            samples.extend_from_slice(&speech);
            samples.extend([0; 16_000]);
            let wav = test_wav(&samples);
            let segments = earshot_vad(wav.path(), &Default::default(), &|| Ok(())).unwrap();
            assert_eq!(
                segments.len(),
                expected,
                "pause={silence_samples}: {segments:?}"
            );
        }
    }

    #[test]
    fn shared_pcm_reader_preserves_energy_and_actual_sample_count() {
        let samples = vec![16_384; 16_017];
        let wav = test_wav(&samples);
        let mut decoded = Vec::new();
        assert_eq!(
            read_pcm_samples(wav.path(), |sample| {
                decoded.push(sample);
                Ok(())
            })
            .unwrap(),
            16_017
        );
        assert_eq!(decoded, samples);
        assert_eq!(Energy::load(wav.path()).unwrap().rms, vec![0.5; 11]);
    }

    #[test]
    fn vad_settings_control_scores_merging_and_maximum_length() {
        let wav = test_wav(&vec![0; 32_000]);
        let asr = crate::settings::Asr {
            vad_threshold: 0.0,
            min_silence: 0.0,
            max_speech: 1.0,
            ..Default::default()
        };
        let uncut = crate::settings::Asr {
            max_speech: 20.0,
            ..asr.clone()
        };
        assert_eq!(
            earshot_vad(wav.path(), &uncut, &|| Ok(())).unwrap().len(),
            1
        );
        let segments = earshot_vad(wav.path(), &asr, &|| Ok(())).unwrap();
        assert!(segments.len() >= 2);
        assert!(segments.iter().all(|seg| seg.end - seg.start <= 1.0));
        let mut samples = speech_fixture_samples();
        samples.extend([0; 16_000]);
        samples.extend(speech_fixture_samples());
        let wav = test_wav(&samples);
        let asr = crate::settings::Asr {
            min_silence: 2.0,
            ..Default::default()
        };
        assert_eq!(earshot_vad(wav.path(), &asr, &|| Ok(())).unwrap().len(), 1);
    }

    #[test]
    fn sanitize_markers() {
        assert_eq!(
            sanitize_qwen_text("**language Chinese<asr_text>你好世界。"),
            "你好世界。"
        );
        assert_eq!(sanitize_qwen_text("内容</asr_text>尾巴"), "内容");
        for text in ["Language models help.", "language Chinese\u{3000}你好"] {
            assert_eq!(sanitize_qwen_text(text), text);
        }
        assert_eq!(
            sanitize_qwen_text("language English<asr_text>Language models help.</asr_text>"),
            "Language models help."
        );
    }

    #[test]
    fn chat_content_parsing() {
        // 字符串形式（主流 OpenAI 兼容端点）
        let v = serde_json::json!({"choices":[{"message":{"content":"你好世界"}}]});
        assert_eq!(parse_chat_content(&v), "你好世界");
        // 分片数组形式（部分多模态端点）
        let v = serde_json::json!({"choices":[{"message":{"content":[
            {"type":"text","text":"你好"},
            {"type":"text","text":"世界"}
        ]}}]});
        assert_eq!(parse_chat_content(&v), "你好世界");
        // 缺字段/异常响应 → 空串（调用方按无语音处理）
        let v = serde_json::json!({"choices":[]});
        assert_eq!(parse_chat_content(&v), "");
    }
}

#[cfg(test)]
mod http_tests {
    use super::*;
    use crate::settings::{Asr, AsrMode};
    use std::io::{Read, Write};

    #[test]
    fn fast_responses_obey_cancellation_before_dispatch_and_checkpointing() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let _dispatch = crate::runtime::lock_recover(&crate::dispatch::TEST_LOCK);
        if crate::runtime::which("ffmpeg").is_none() {
            return;
        }
        for intent in [
            None,
            Some(crate::dispatch::Intent::Pause),
            Some(crate::dispatch::Intent::Cancel),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let control = dir.path().join("control.json");
            let _ledger =
                crate::dispatch::install(dir.path(), Some(&control), &Default::default()).unwrap();
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let api = Asr {
                base_url: format!("http://{}/v1", listener.local_addr().unwrap()),
                concurrency: 1,
                max_speech: 1.0,
                ..Default::default()
            };
            let mut cp = crate::checkpoint::Checkpoint::open(
                dir.path(),
                false,
                &crate::checkpoint::AsrIdentity::new(&api),
            )
            .unwrap();
            let cancelled = AtomicBool::new(false);
            let done = AtomicBool::new(false);
            std::thread::scope(|scope| {
                let server = scope.spawn(|| {
                    let mut requests = 0;
                    while !done.load(Ordering::Relaxed) {
                        let mut stream = match listener.accept() {
                            Ok((stream, _)) => stream,
                            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                                std::thread::sleep(Duration::from_millis(1));
                                continue;
                            }
                            Err(error) => panic!("{error}"),
                        };
                        stream.set_nonblocking(false).unwrap();
                        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                        let mut request = Vec::new();
                        let mut buffer = [0; 4096];
                        loop {
                            let n = stream.read(&mut buffer).unwrap();
                            assert_ne!(n, 0);
                            request.extend_from_slice(&buffer[..n]);
                            if let Some(offset) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                                let head = String::from_utf8_lossy(&request[..offset]).to_ascii_lowercase();
                                let length: usize = head.lines().find_map(|line| line.strip_prefix("content-length:")).unwrap().trim().parse().unwrap();
                                if request.len() >= offset + 4 + length { break; }
                            }
                        }
                        requests += 1;
                        if let Some(intent) = intent {
                            let value = crate::dispatch::Control { intent, ..Default::default() };
                            crate::checkpoint::atomic_write(&control, &serde_json::to_vec(&value).unwrap()).unwrap();
                        } else {
                            cancelled.store(true, Ordering::Relaxed);
                        }
                        let body = r#"{"text":"Recognized."}"#;
                        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
                    }
                    requests
                });
                let result = run_api(
                    &api,
                    Path::new(concat!(
                        env!("CARGO_MANIFEST_DIR"),
                        "/tests/fixtures/speech.wav"
                    )),
                    &mut cp,
                    &cancelled,
                );
                done.store(true, Ordering::Relaxed);
                assert!(result.is_err(), "{intent:?}");
                assert_eq!(server.join().unwrap(), 1, "{intent:?}");
                assert!(cp.sorted_events().is_empty(), "{intent:?}");
            });
        }
    }

    #[test]
    fn dialects_send_optional_fields_before_audio_and_sanitize_responses() {
        let _dispatch = crate::runtime::lock_recover(&crate::dispatch::TEST_LOCK);
        if crate::runtime::which("ffmpeg").is_none() {
            return;
        }
        for (mode, suffix, field) in [
            (AsrMode::Transcriptions, "/audio/transcriptions", "text"),
            (AsrMode::Xai, "/stt", "text"),
            (AsrMode::Meta, "/asr/transcribe", "transcript"),
            (AsrMode::Chat, "/chat/completions", "content"),
        ] {
            for with_options in [false, true] {
                let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
                let base = format!("http://{}/v1", listener.local_addr().unwrap());
                let server = std::thread::spawn(move || {
                    let (mut stream, _) = listener.accept().unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(10)))
                        .unwrap();
                    let mut request = Vec::new();
                    let mut buffer = [0; 4096];
                    let offset = loop {
                        let read = stream.read(&mut buffer).unwrap();
                        assert_ne!(read, 0);
                        request.extend_from_slice(&buffer[..read]);
                        if let Some(offset) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                            break offset + 4;
                        }
                    };
                    let head = String::from_utf8_lossy(&request[..offset]).to_ascii_lowercase();
                    let length: usize = head
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length:"))
                        .unwrap()
                        .trim()
                        .parse()
                        .unwrap();
                    while request.len() < offset + length {
                        let read = stream.read(&mut buffer).unwrap();
                        assert_ne!(read, 0);
                        request.extend_from_slice(&buffer[..read]);
                    }
                    let text = "language English<asr_text>Recognized.</asr_text>";
                    let response = if mode == AsrMode::Chat {
                        serde_json::json!({"choices":[{"message":{field:text}}]})
                    } else {
                        serde_json::json!({field:text})
                    }
                    .to_string();
                    write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}", response.len()).unwrap();
                    (head, request[offset..].to_vec())
                });
                let api = Asr {
                    mode,
                    base_url: base,
                    ..Default::default()
                };
                let url = crate::config::asr_endpoint(&api).unwrap();
                let target = ApiTarget {
                    client: &crate::runtime::http_agent(API_HTTP_TIMEOUT),
                    url: &url,
                    model: if with_options { "test-model" } else { "" },
                    key: if with_options { "test-key" } else { "" },
                    mode,
                    language: if with_options { "ko" } else { "" },
                    managed: false,
                };
                let dir = tempfile::tempdir().unwrap();
                let text = transcribe_api(
                    &target,
                    &dir.path().join("chunk.wav"),
                    Seg {
                        start: 0.0,
                        end: 2.4,
                        cut_start: 0.0,
                        cut_end: 2.4,
                    },
                    Path::new(concat!(
                        env!("CARGO_MANIFEST_DIR"),
                        "/tests/fixtures/speech.wav"
                    )),
                )
                .unwrap();
                assert_eq!(text.as_deref(), Some("Recognized."));
                let (head, body) = server.join().unwrap();
                assert!(head.starts_with(&format!("post /v1{suffix} ")), "{head}");
                assert_eq!(head.contains("authorization:"), with_options);
                if with_options {
                    assert!(head.contains("authorization: bearer test-key"));
                }
                if mode == AsrMode::Chat {
                    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
                    assert_eq!(json.get("model").is_some(), with_options);
                    assert_eq!(json["temperature"], 0);
                    let prompt = json["messages"][0]["content"][0]["text"].as_str().unwrap();
                    assert_eq!(
                        prompt,
                        if with_options {
                            format!("{CHAT_TRANSCRIBE_PROMPT} The speech is in Korean.")
                        } else {
                            CHAT_TRANSCRIBE_PROMPT.into()
                        }
                    );
                    assert_eq!(
                        json["messages"][0]["content"][1]["input_audio"]["format"],
                        "wav"
                    );
                } else {
                    let text = String::from_utf8_lossy(&body);
                    let audio = text.find("filename=\"audio.wav\"").unwrap();
                    if mode == AsrMode::Meta {
                        let option = text.find("name=\"request\"").unwrap();
                        assert!(option < audio);
                        assert!(text.contains("Content-Type: application/json\r\n"));
                        let json_start = text[option..].find("\r\n\r\n").unwrap() + option + 4;
                        let json_end = text[json_start..].find("\r\n").unwrap() + json_start;
                        let json: serde_json::Value =
                            serde_json::from_str(&text[json_start..json_end]).unwrap();
                        assert_eq!(json["audioEncoding"], "WAV");
                        assert_eq!(json.get("model").is_some(), with_options);
                        assert_eq!(json.get("languageBias").is_some(), with_options);
                        if with_options {
                            assert_eq!(json["languageBias"], serde_json::json!(["Korean"]));
                        }
                        assert!(text.contains("name=\"audio\";"));
                    } else {
                        for name in ["model", "language"] {
                            let position = text.find(&format!("name=\"{name}\""));
                            assert_eq!(position.is_some(), with_options);
                            if let Some(position) = position {
                                assert!(position < audio);
                            }
                        }
                        assert!(text.contains("name=\"file\";"));
                    }
                    assert!(body.windows(4).any(|part| part == b"RIFF"));
                }
            }
        }
    }

    #[test]
    fn language_names_are_shared_by_chat_and_meta_requests() {
        let client = ureq::agent();
        for (code, name) in [
            ("zh", "Chinese"),
            ("pt", "Portuguese"),
            ("pt-BR", "Portuguese"),
            ("zh-Hant", "Traditional Chinese"),
            ("xx-custom", "xx-custom"),
        ] {
            let mut target = ApiTarget {
                client: &client,
                url: "",
                model: "",
                key: "",
                mode: AsrMode::Chat,
                language: code,
                managed: false,
            };
            let (_, body) = transcription_request(&target, b"audio").unwrap();
            let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert!(
                body["messages"][0]["content"][0]["text"]
                    .as_str()
                    .unwrap()
                    .ends_with(&format!(" The speech is in {name}."))
            );
            target.mode = AsrMode::Meta;
            let (_, body) = transcription_request(&target, b"audio").unwrap();
            assert!(
                String::from_utf8(body)
                    .unwrap()
                    .contains(&format!("\"languageBias\":[\"{name}\"]"))
            );
        }
    }
    #[cfg(unix)]
    #[test]
    fn dropping_the_async_transcription_stops_its_blocking_adapter() {
        let _dispatch = crate::runtime::lock_recover(&crate::dispatch::TEST_LOCK);
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            if crate::runtime::which("ffmpeg").is_none() {
                return;
            }
            let Some(python) = crate::runtime::which("python3") else {
                return;
            };
            let dir = tempfile::tempdir().unwrap();
            let record = dir.path().join("adapter");
            let mut cfg =
                crate::options::resolve("video".into(), &Default::default(), &Default::default())
                    .unwrap();
            cfg.out_dir = dir.path().join("work");
            cfg.asr.command = vec![
                python.display().to_string(),
                "-u".into(),
                concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/asr_adapter.py").into(),
                "--record".into(),
                record.display().to_string(),
                "--behavior".into(),
                "hang".into(),
            ];
            let task = tokio::spawn(async move {
                run(
                    &cfg,
                    Path::new(concat!(
                        env!("CARGO_MANIFEST_DIR"),
                        "/tests/fixtures/speech.wav"
                    )),
                )
                .await
            });
            let gate = record.join("transcribing");
            let start = Instant::now();
            while !gate.is_file() && start.elapsed() < Duration::from_secs(10) {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            let reached_request = gate.is_file();
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
            assert!(reached_request);
            let start = Instant::now();
            while !record.join("terminated").is_file() && start.elapsed() < Duration::from_secs(5) {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            assert!(record.join("terminated").is_file());
            assert!(!dir.path().join("work/.asr_done").exists());
        });
    }
}
