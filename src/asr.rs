//! HTTP transcription with earshot segmentation and resumable chunks.

use crate::config::PipelineConfig;
use crate::timeline::TranscriptEvent;
use anyhow::{Context, Result};

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

// Tuning constants; segmentation constants are next to normalize_segments.

/// HTTP attempts per request: the first try plus two retries.
const MAX_ATTEMPTS: u32 = 3;
/// Timeout of one transcription request.
const API_HTTP_TIMEOUT: Duration = Duration::from_secs(120);
/// Limit for cutting one segment with ffmpeg; real cuts take a fraction of this.
const CUT_TIMEOUT: Duration = Duration::from_secs(120);
/// Empty-result tripwire: speech detection found speech in these segments, yet the endpoint keeps returning
/// empty text without a single success. Such an endpoint cannot handle audio (wrong configuration, or a gateway
/// that drops the body), and continuing would only waste requests. A single empty segment is still valid silence.
const EMPTY_RESULT_TRIPWIRE: usize = 10;

/// Buffers empty results and resets on the first non-empty one (the endpoint works and the silence is real);
/// trips when empty results reach the threshold. The caller records buffered empty results only once it is clear
/// the tripwire did not fire, so a broken endpoint cannot mark segments as silence that a fixed rerun would skip.
#[derive(Default)]
struct EmptyTripwire {
    pending_empty: usize,
    seen_success: bool,
}

impl EmptyTripwire {
    /// Counts one result and returns whether the tripwire fired.
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
            "The speech service returned empty text for {} consecutive segments; check that the endpoint and model support audio input",
            self.pending_empty
        )
    }
}

/// Empty results are buffered until the endpoint is known to work.
/// Every chunk result passes the tripwire first. When it fires, **nothing is checkpointed**:
/// empty records would mark the segments as finished silence and hide the failure. Empty results are buffered
/// and recorded once a non-empty result arrives or the run finishes normally. Results arriving out of order are safe:
/// order only affects the buffer, and every buffered record has the same empty text.
#[derive(Default)]
struct EmptyResultTracker {
    tripwire: EmptyTripwire,
    pending_empty: Vec<(f64, f64)>,
}

impl EmptyResultTracker {
    /// Handles one chunk result: non-empty results are recorded, empty ones buffered.
    /// Returns an error when the run must stop (the tripwire fired or the checkpoint write failed).
    fn record_outcome(
        &mut self,
        cp: &mut crate::checkpoint::Checkpoint,
        start: f64,
        end: f64,
        text: &str,
    ) -> Result<()> {
        let empty = text.trim().is_empty();
        if self.tripwire.record(empty) {
            // Tripped: record nothing (see the struct comment).
            return Err(self.tripwire.tripped_error());
        }
        if empty {
            self.pending_empty.push((start, end));
            return Ok(());
        }
        self.flush_pending(cp)?;
        cp.record(start, end, text)
    }

    /// Normal completion: records the buffered empty results (real silence) so silent chunks are not redone.
    /// Do not call after an error, including the tripwire: the buffer must be discarded with the failed run.
    fn finish(self, cp: &mut crate::checkpoint::Checkpoint) -> Result<()> {
        let mut this = self;
        this.flush_pending(cp)
    }

    /// Records buffered empty results; stops at the first write failure and keeps the rest for a rerun.
    fn flush_pending(&mut self, cp: &mut crate::checkpoint::Checkpoint) -> Result<()> {
        for (start, end) in std::mem::take(&mut self.pending_empty) {
            cp.record(start, end, "")?;
        }
        Ok(())
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
    let mut cp = crate::checkpoint::Checkpoint::open(&cfg.out_dir, true, identity)?;
    let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let _cancel_on_drop = crate::runtime::CancelOnDrop(cancelled.clone());
    tokio::task::spawn_blocking(move || {
        let r = f(&mut cp, &cancelled);
        if r.is_ok() {
            cp.finish()?;
        }
        r
    })
    .await
    .context("Speech recognition task did not complete")?
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

/// Shared result when speech detection finds no speech.
pub(crate) fn no_speech_events() -> Vec<TranscriptEvent> {
    tracing::warn!("No speech detected");
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
            "Transcription cancelled."
        );
        crate::runtime::check_cancelled()
    };
    check_control()?;
    let t0 = Instant::now();
    // Credentials were resolved before the run (including environment variables); an empty key means no authentication.
    let api_key = api.api_key.trim().to_string();
    let segs = earshot_vad(wav, api, &check_control)?;
    check_control()?;
    tracing::info!(segs = segs.len(), endpoint = %api.base_url, model = %api.model, "api vad");
    if segs.is_empty() {
        return Ok(no_speech_events());
    }

    // Resume: give the workers only the chunks that are not done yet.
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

    // Bounded concurrency with std::thread::scope (borrowing, no Arc); network round trips dominate.
    // Results come back over a channel. After an abort, requests in flight finish on their own and are still joined.
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

                check_control: &check_control,
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
                _ if err.is_some() => {} // already tripped or failed: drop late results
                Ok(text) => {
                    let text = text.as_deref().unwrap_or("");
                    if let Err(e) = tracker.record_outcome(cp, segs[i].start, segs[i].end, text) {
                        err = Some(e);
                        abort.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                }
                Err(e) => {
                    err =
                        Some(e.context(format!("Speech transcription failed for segment {}.", i)));
                    abort.store(true, std::sync::atomic::Ordering::Relaxed);
                }
            }
            if err.is_some()
                && let Some(adapter) = &mut adapter
            {
                adapter.stop();
            }
            // Every collected result, empty or not, advances progress exactly once, as in the sequential path.
            pb.inc(1);
        }
        // Normal completion: record buffered empty results (real silence) so silent chunks are not redone.
        if err.is_none()
            && let Err(e) = check_control().and_then(|()| tracker.finish(cp))
        {
            err = Some(e);
        }
    });
    pb.finish();
    if let Some(e) = err {
        return Err(if let Some(adapter) = &adapter {
            adapter.stderr.attach(e, "ASR adapter output")
        } else {
            e
        });
    }
    check_control()?;
    // Events come from the checkpoint, where the loop recorded them, sorted by time.
    let events: Vec<TranscriptEvent> = cp.sorted_events();
    tracing::info!(
        n = events.len(),
        secs = format_args!("{:.1}", t0.elapsed().as_secs_f64()),
        "asr done"
    );
    Ok(events)
}

/// Bounded retries: retryable errors back off with [`crate::runtime::retry_backoff`] (cancellable while waiting),
/// up to MAX_ATTEMPTS attempts; deterministic errors (authentication, bad parameters) return at once.
fn with_retry<T>(
    check_control: &dyn Fn() -> Result<()>,
    mut attempt_once: impl FnMut() -> std::result::Result<T, crate::dispatch::Failure>,
) -> Result<T> {
    let mut attempt = 1;
    loop {
        check_control()?;
        match attempt_once() {
            Ok(value) => return Ok(value),
            Err(error) if error.retryable && attempt < MAX_ATTEMPTS => {
                check_control()?;
                crate::runtime::interruptible_sleep(crate::runtime::retry_backoff(attempt))?;
                attempt += 1;
            }
            Err(error) => return Err(error.into()),
        }
    }
}

/// Send a chunk with bounded retries; only its transcription checkpoint is persisted.
fn post_cloud(t: &ApiTarget, content_type: &str, body: &[u8]) -> Result<serde_json::Value> {
    with_retry(t.check_control, || {
        crate::dispatch::json_request(
            || {
                let request = t.client.post(t.url).set("Content-Type", content_type);
                let request = if t.key.is_empty() {
                    request
                } else {
                    request.set("Authorization", &format!("Bearer {}", t.key))
                };
                (t.check_control)().map_err(crate::dispatch::Failure::local)?;
                crate::dispatch::receive(request.send_bytes(body))
            },
            |value| {
                anyhow::ensure!(
                    value.get("error").is_none_or(serde_json::Value::is_null),
                    "The speech service returned an error"
                );
                let has_text = match t.mode {
                    crate::settings::AsrMode::Transcriptions => value["text"].is_string(),
                    crate::settings::AsrMode::Chat => chat_content_has_text(value),
                };
                anyhow::ensure!(has_text, "The speech service returned no text");
                Ok(())
            },
        )
    })
}

/// Instruction for audio chat models: a faithful transcript with natural punctuation, like dedicated ASR output.
const CHAT_TRANSCRIBE_PROMPT: &str = r#"Transcribe this audio. Output only the transcript itself with natural punctuation, and no explanation, summary or formatting. If there is no speech, output an empty string."#;

/// Transcription endpoint: address, credentials and dialect, borrowed by all workers.
struct ApiTarget<'a> {
    client: &'a ureq::Agent,
    url: &'a str,
    model: &'a str,
    key: &'a str,
    mode: crate::settings::AsrMode,
    language: &'a str,
    check_control: &'a (dyn Fn() -> Result<()> + Sync),
}

/// Transcribes one chunk; `Ok(None)` means it contains no speech.
fn transcribe_api(t: &ApiTarget, chunk: &Path, seg: Seg, wav: &Path) -> Result<Option<String>> {
    cut_wav(wav, seg.cut_start, seg.cut_end, chunk).context("Could not split audio")?;
    (t.check_control)()?;
    let bytes = std::fs::read(chunk)
        .with_context(|| format!("Reading audio segment: {}", chunk.display()))?;
    let (content_type, body) = transcription_request(t, &bytes)?;
    let result = post_cloud(t, &content_type, &body);
    let _ = std::fs::remove_file(chunk);
    let value = result?;
    let text = match t.mode {
        crate::settings::AsrMode::Transcriptions => {
            value["text"].as_str().unwrap_or_default().to_owned()
        }
        crate::settings::AsrMode::Chat => parse_chat_content(&value),
    };
    Ok((!text.trim().is_empty()).then_some(text))
}

/// Options precede the binary audio part in transcription requests.
fn transcription_request(t: &ApiTarget, audio: &[u8]) -> Result<(String, Vec<u8>)> {
    use crate::settings::AsrMode;
    use base64::Engine as _;
    if t.mode == AsrMode::Chat {
        let mut prompt = CHAT_TRANSCRIBE_PROMPT.to_owned();
        if !t.language.is_empty() {
            prompt.push_str(&format!(" The speech is in {}.", t.language));
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
    for (name, value) in [("model", t.model), ("language", t.language)] {
        if !value.is_empty() {
            options.push_str(&format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            ));
        }
    }
    let mut body = format!("{options}--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"audio.wav\"\r\nContent-Type: audio/wav\r\n\r\n").into_bytes();
    body.extend_from_slice(audio);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    Ok((format!("multipart/form-data; boundary={boundary}"), body))
}

/// Whether a chat/completions message.content carries text: either a string, or an array of
/// parts [{type:"text", text:...}] in which some part has a text field.
fn chat_content_has_text(v: &serde_json::Value) -> bool {
    let content = &v["choices"][0]["message"]["content"];
    content.is_string()
        || content
            .as_array()
            .is_some_and(|parts| parts.iter().any(|part| part["text"].is_string()))
}

/// Text of a chat/completions response: content is usually a string; some audio-capable endpoints
/// return an array of [{type:"text", text:...}] parts, which are concatenated.
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
    let samples = read_pcm_samples(wav, check_control, |sample| {
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
        check_control,
    )
}

/// A segment sent for transcription: `start`/`end` are its timeline times, `cut_*` the audio range including padding.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Seg {
    pub start: f64,
    pub end: f64,
    pub cut_start: f64,
    pub cut_end: f64,
}

/// Stream the pipeline's 16 kHz mono s16le WAV, returning its actual sample count.
fn read_pcm_samples(
    wav: &Path,
    mut check_control: impl FnMut() -> Result<()>,
    mut on_sample: impl FnMut(i16) -> Result<()>,
) -> Result<u64> {
    use std::io::{BufReader, Read, Seek};
    let file = std::fs::File::open(wav)
        .with_context(|| format!("Could not read audio: {}", wav.display()))?;
    let mut reader = BufReader::new(file);
    // Read the RIFF header and scan chunk headers to find the PCM data chunk.
    let mut header = [0u8; 12];
    reader
        .read_exact(&mut header)
        .ok()
        .filter(|_| &header[0..4] == b"RIFF" && &header[8..12] == b"WAVE")
        .ok_or_else(|| anyhow::anyhow!("Cannot parse wav PCM data"))?;
    let data_size = loop {
        let mut ch = [0u8; 8];
        if reader.read_exact(&mut ch).is_err() {
            anyhow::bail!("Cannot parse wav PCM data");
        }
        let size = u32::from_le_bytes([ch[4], ch[5], ch[6], ch[7]]) as u64;
        if &ch[0..4] == b"data" {
            break size;
        }
        // Skip other chunks, including the padding byte after odd sizes.
        reader.seek(std::io::SeekFrom::Current((size + (size & 1)) as i64))?;
    };
    let mut buf = vec![0u8; 128 * 1024];
    let mut half: Option<u8> = None; // low byte of a sample split across reads
    let mut samples = 0;
    let mut emit = |sample| {
        on_sample(sample)?;
        samples += 1;
        Ok::<(), anyhow::Error>(())
    };
    let mut remaining = data_size;
    while remaining > 0 {
        check_control()?;
        let want = remaining.min(buf.len() as u64) as usize;
        let n = reader.read(&mut buf[..want])?;
        if n == 0 {
            break; // truncated file: the data chunk claims more than exists; use what was read
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

/// RMS energy per 100 ms, used to find the quietest point near a planned cut.
pub struct Energy {
    hop: f64,
    rms: Vec<f32>,
}

impl Energy {
    /// Streams the 16 kHz mono 16-bit WAV produced by extract_audio and keeps only the RMS values,
    /// accumulating squares per 100 ms window, so long recordings are not loaded into memory
    /// (two hours of audio would be about 1 GB).
    pub fn load(wav: &Path, check_control: impl FnMut() -> Result<()>) -> Result<Self> {
        let mut rms = Vec::new();
        let mut sum = 0.0f64;
        let mut n_in_hop = 0usize;
        read_pcm_samples(wav, check_control, |sample| {
            rms_accumulate(sample, &mut sum, &mut n_in_hop, &mut rms);
            Ok(())
        })?;
        if n_in_hop > 0 {
            rms.push((sum / n_in_hop as f64).sqrt() as f32);
        }
        Ok(Self { hop: 0.1, rms })
    }

    /// The quietest moment within [a, b] seconds, or None without data.
    fn quietest(&self, a: f64, b: f64) -> Option<f64> {
        let i0 = (a / self.hop).ceil() as usize;
        let i1 = (b / self.hop).floor() as usize;
        if i1 <= i0 || i0 >= self.rms.len() {
            return None;
        }
        let i1 = i1.min(self.rms.len() - 1);
        // total_cmp: RMS values cannot be NaN in theory, but this never panics.
        let (bi, _) = self.rms[i0..=i1]
            .iter()
            .enumerate()
            .min_by(|x, y| x.1.total_cmp(y.1))?;
        Some((i0 + bi) as f64 * self.hop + self.hop / 2.0)
    }
}

/// Adds one sample to the current 100 ms window and closes the window when it is full.
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

const PAD: f64 = 0.25; // seconds of silence added on each outer side of a cut
const SPLIT_WINDOW: f64 = 3.0; // seconds before a planned cut searched for the quietest point
const MIN_PIECE: f64 = 1.0; // shortest piece a forced cut may produce
/// Minimum distance of a cut from the segment start, so odd energy curves cannot produce endless tiny pieces.
const MIN_HARD_CUT: f64 = 0.5;

/// After speech detection: split long segments at quiet points and pad with silence.
/// - A segment longer than max_speech is cut at the quietest point in [target - 3 s, target], avoiding mid-word cuts.
/// - Outer edges get 0.25 s of silence, never neighbouring speech, so no text is duplicated.
///
/// `dur` is probed once by the caller.
pub fn normalize_segments(
    speech: Vec<(f64, f64)>,
    max_speech: f64,
    wav: &Path,
    dur: f64,
    check_control: &dyn Fn() -> Result<()>,
) -> Result<Vec<Seg>> {
    // No speech, or only fragments too short to keep: no segments.
    // Never fall back to sending the whole file: models hallucinate on silence.
    let mut raw = speech;
    raw.retain(|(a, b)| b - a >= 0.2);
    if raw.is_empty() {
        return Ok(vec![]);
    }
    // Read the energy curve only when a segment must be split; it scans the whole file.
    let energy = if raw.iter().any(|(a, b)| b - a > max_speech) {
        let mut interrupted = false;
        let result = Energy::load(wav, || {
            let result = check_control();
            interrupted |= result.is_err();
            result
        });
        match result {
            Ok(energy) => Some(energy),
            Err(error) if interrupted => return Err(error),
            Err(e) => {
                tracing::warn!(
                    "Could not read audio energy; long segments are split at fixed lengths: {e:#}"
                );
                None
            }
        }
    } else {
        None
    };

    let mut pieces: Vec<(f64, f64)> = vec![];
    for &(s, e) in &raw {
        split_smart(s, e, max_speech, energy.as_ref(), &mut pieces);
    }

    // Padding: outer edges extend 0.25 s into silence, limited by the neighbouring speech segments;
    // internal cuts already sit at quiet points and get no padding, so neighbouring chunks do not repeat text.
    let segs = pieces
        .iter()
        .map(|&(s, e)| {
            let host_idx = raw
                .iter()
                .position(|&(rs, re)| s >= rs - 1e-6 && e <= re + 1e-6);
            // Only the first and last piece of a detected segment have outer edges to pad.
            let (is_first_of_host, is_last_of_host) = match host_idx {
                Some(h) => {
                    let (rs, re) = raw[h];
                    let first = (s - rs).abs() < 1e-6;
                    let last = (re - e).abs() < 1e-6;
                    (first, last)
                }
                None => (true, true),
            };
            // End of the previous detected segment: the limit for padding into the gap.
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

/// Splits recursively at the quietest point, or at a fixed length when there is none.
fn split_smart(s: f64, e: f64, max: f64, energy: Option<&Energy>, out: &mut Vec<(f64, f64)>) {
    if e - s <= max {
        out.push((s, e));
        return;
    }
    let target = s + max;
    // Search only [target - 3 s, target], so no piece exceeds max_speech; speech services often
    // limit input length.
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
        anyhow::bail!("ffmpeg could not split audio");
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
        assert!(
            t.record(true),
            "consecutive empty results trip the tripwire"
        );
        assert!(t.tripped_error().to_string().contains("empty text"));

        // After a non-empty result the endpoint works, so silence is real and the tripwire stays open.
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
                check_control: &crate::runtime::check_cancelled,
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
        let count = read_pcm_samples(
            path,
            || Ok(()),
            |sample| {
                samples.push(sample);
                Ok(())
            },
        )
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
    fn energy_scan_propagates_control_errors_but_keeps_io_fallback() {
        let wav = test_wav(&vec![0; 160_000]);
        let polls = std::cell::Cell::new(0);
        let error = normalize_segments(vec![(0.0, 10.0)], 2.0, wav.path(), 10.0, &|| {
            polls.set(polls.get() + 1);
            anyhow::ensure!(polls.get() != 2, "test energy cancellation");
            Ok(())
        })
        .unwrap_err();
        assert_eq!(error.to_string(), "test energy cancellation");
        assert_eq!(polls.get(), 2);
        let missing = wav.path().with_extension("missing");
        assert_eq!(
            normalize_segments(vec![(0.0, 10.0)], 2.0, &missing, 10.0, &|| Ok(()))
                .unwrap()
                .len(),
            5
        );
    }

    #[test]
    fn retries_obey_the_drop_flag() {
        let _dispatch = crate::runtime::lock_recover(&crate::dispatch::TEST_LOCK);
        let cancelled = std::sync::atomic::AtomicBool::new(false);
        let check = || {
            anyhow::ensure!(
                !cancelled.load(std::sync::atomic::Ordering::Relaxed),
                "test retry cancellation"
            );
            Ok(())
        };
        let mut attempts = 0;
        let error = with_retry(&check, || {
            attempts += 1;
            cancelled.store(true, std::sync::atomic::Ordering::Relaxed);
            let mut error = crate::dispatch::Failure::local("retryable response");
            error.retryable = true;
            Err::<(), _>(error)
        })
        .unwrap_err();
        assert_eq!(error.to_string(), "test retry cancellation");
        assert_eq!(attempts, 1);
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
            read_pcm_samples(
                wav.path(),
                || Ok(()),
                |sample| {
                    decoded.push(sample);
                    Ok(())
                }
            )
            .unwrap(),
            16_017
        );
        assert_eq!(decoded, samples);
        assert_eq!(
            Energy::load(wav.path(), || Ok(())).unwrap().rms,
            vec![0.5; 11]
        );
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
    fn chat_content_parsing() {
        // String content (most OpenAI-compatible endpoints)
        let v = serde_json::json!({"choices":[{"message":{"content":"你好世界"}}]});
        assert_eq!(parse_chat_content(&v), "你好世界");
        // Array of parts (some audio-capable endpoints)
        let v = serde_json::json!({"choices":[{"message":{"content":[
            {"type":"text","text":"你好"},
            {"type":"text","text":"世界"}
        ]}}]});
        assert_eq!(parse_chat_content(&v), "你好世界");
        // Missing or malformed content becomes empty text, treated as no speech.
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
    fn cancellation_is_checked_immediately_before_the_http_send() {
        let _dispatch = crate::runtime::lock_recover(&crate::dispatch::TEST_LOCK);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!(
            "http://{}/v1/audio/transcriptions",
            listener.local_addr().unwrap()
        );
        let polls = std::sync::atomic::AtomicUsize::new(0);
        let check = || {
            anyhow::ensure!(
                polls.fetch_add(1, std::sync::atomic::Ordering::Relaxed) == 0,
                "test send cancellation"
            );
            Ok(())
        };
        let target = ApiTarget {
            client: &crate::runtime::http_agent(Duration::from_millis(100)),
            url: &url,
            model: "",
            key: "",
            mode: AsrMode::Transcriptions,
            language: "",
            check_control: &check,
        };
        let error = post_cloud(&target, "application/json", b"{}").unwrap_err();
        assert!(error.to_string().contains("test send cancellation"));
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }

    #[test]
    fn fast_responses_obey_cancellation_before_dispatch_and_checkpointing() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let _dispatch = crate::runtime::lock_recover(&crate::dispatch::TEST_LOCK);
        if crate::runtime::which("ffmpeg").is_none() {
            return;
        }
        {
            let dir = tempfile::tempdir().unwrap();
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
                        cancelled.store(true, Ordering::Relaxed);
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
                assert!(result.is_err());
                assert_eq!(server.join().unwrap(), 1);
                assert!(cp.sorted_events().is_empty());
            });
        }
    }

    #[test]
    fn dialects_send_optional_fields_before_audio_and_preserve_responses() {
        let _dispatch = crate::runtime::lock_recover(&crate::dispatch::TEST_LOCK);
        if crate::runtime::which("ffmpeg").is_none() {
            return;
        }
        for (mode, suffix, field) in [
            (AsrMode::Transcriptions, "/audio/transcriptions", "text"),
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
                    let text = " language English<asr_text>Recognized.</asr_text> ";
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
                    check_control: &crate::runtime::check_cancelled,
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
                assert_eq!(
                    text.as_deref(),
                    Some(" language English<asr_text>Recognized.</asr_text> ")
                );
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
                            format!("{CHAT_TRANSCRIBE_PROMPT} The speech is in ko.")
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
                    for name in ["model", "language"] {
                        let position = text.find(&format!("name=\"{name}\""));
                        assert_eq!(position.is_some(), with_options);
                        if let Some(position) = position {
                            assert!(position < audio);
                        }
                    }
                    assert!(text.contains("name=\"file\";"));
                    assert!(body.windows(4).any(|part| part == b"RIFF"));
                }
            }
        }
    }

    #[test]
    fn chat_requests_pass_language_tags_through() {
        let client = ureq::agent();
        for language in ["zh", "pt", "pt-BR", "zh-Hant", "xx-custom", " KO_kr "] {
            let target = ApiTarget {
                client: &client,
                url: "",
                model: "",
                key: "",
                mode: AsrMode::Chat,
                language,

                check_control: &crate::runtime::check_cancelled,
            };
            let (_, body) = transcription_request(&target, b"audio").unwrap();
            let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert!(
                body["messages"][0]["content"][0]["text"]
                    .as_str()
                    .unwrap()
                    .ends_with(&format!(" The speech is in {language}."))
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
            while !gate.is_file() && start.elapsed() < Duration::from_secs(60) {
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
