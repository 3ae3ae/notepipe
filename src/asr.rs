//! HTTP transcription of speech segments, with retries and resumable chunks.

use crate::config::PipelineConfig;
use crate::timeline::TranscriptEvent;
use crate::vad::{Seg, cut_wav, earshot_vad};
use anyhow::{Context, Result};

use std::path::Path;
use std::time::{Duration, Instant};

// Tuning constants; segmentation constants are next to normalize_segments.

/// HTTP attempts per request: the first try plus two retries.
const MAX_ATTEMPTS: u32 = 3;
/// Timeout of one transcription request.
const API_HTTP_TIMEOUT: Duration = Duration::from_secs(120);
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

pub async fn run(
    cfg: &PipelineConfig,
    wav: &Path,
) -> Result<(Vec<TranscriptEvent>, Option<crate::adapter::Adapter>)> {
    cfg.validate_asr()?;
    let api = cfg.asr.clone();
    let wav = wav.to_path_buf();
    let stage = cfg
        .transcript_name
        .clone()
        .unwrap_or_else(|| "transcript".into());
    let mut cp = crate::checkpoint::Checkpoint::open(&cfg.out_dir)?;
    let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let _cancel_on_drop = crate::runtime::CancelOnDrop(cancelled.clone());
    tokio::task::spawn_blocking(move || run_api(&api, &wav, &mut cp, &stage, &cancelled))
        .await
        .context("Speech recognition task did not complete")?
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
    stage: &str,
    cancelled: &std::sync::atomic::AtomicBool,
) -> Result<(Vec<TranscriptEvent>, Option<crate::adapter::Adapter>)> {
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
        return Ok((no_speech_events(), None));
    }

    // Resume: give the workers only the chunks that are not done yet.
    let pending: Vec<usize> = (0..segs.len())
        .filter(|&i| !cp.is_done(segs[i].start, segs[i].end))
        .collect();
    if pending.is_empty() {
        return Ok((cp.sorted_events(), None));
    }

    let tmp = tempfile::Builder::new()
        .prefix("notepipe-asr-")
        .tempdir()
        .context("Could not create a temporary working directory")?;
    let mut api = api.clone();
    let mut adapter = if api.command.is_empty() {
        None
    } else {
        let adapter =
            crate::adapter::Adapter::start_with_control(&api.command, stage, &check_control)?;
        api.base_url = format!("{}/v1", adapter.base);
        Some(adapter)
    };
    let url = crate::config::asr_endpoint(&api)?;
    let pb = crate::progress::Bar::new(format!("{stage}/speech"), segs.len() as u64).with_template(
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
        for _ in 0..api.concurrency.min(pending.len()) {
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
            adapter.stderr.attach(e, "Speech adapter output")
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
    Ok((events, adapter))
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
                    crate::settings::AsrMode::Chat => {
                        anyhow::ensure!(
                            value["choices"][0]["finish_reason"] != "length",
                            "The speech model stopped at its output limit"
                        );
                        chat_content_has_text(value)
                    }
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
    fn truncated_speech_chat_is_not_retried_or_cached() {
        use crate::test_support::MockAi;
        use serde_json::json;
        let _dispatch = crate::runtime::lock_recover(&crate::dispatch::TEST_LOCK);
        for content in [
            json!("Recognized."),
            json!([{"type":"text","text":"Recognized."}]),
        ] {
            let root = tempfile::tempdir().unwrap();
            let _cache = crate::dispatch::install(root.path()).unwrap();
            let mock = MockAi::respond_with(move |_, _| {
                Some((
                    200,
                    json!({"choices":[{"finish_reason":"length","message":{"content":content}}]}),
                ))
            });
            let target = ApiTarget {
                client: &ureq::agent(),
                url: &format!("{}/chat/completions", mock.url),
                model: "",
                key: "",
                mode: AsrMode::Chat,
                language: "",
                check_control: &crate::runtime::check_cancelled,
            };
            let error = post_cloud(&target, "application/json", b"{}").unwrap_err();
            let failure = error.downcast_ref::<crate::dispatch::Failure>().unwrap();
            assert!(!failure.retryable);
            assert_eq!(
                error.to_string(),
                "The speech model stopped at its output limit"
            );
            assert_eq!(mock.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
            assert!(!root.path().join("llm").exists());
        }
    }

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
            let mut cp = crate::checkpoint::Checkpoint::open(dir.path()).unwrap();
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
                    "transcript",
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
            assert!(matches!(task.await, Err(error) if error.is_cancelled()));
            assert!(reached_request);
            let start = Instant::now();
            while !record.join("terminated").is_file() && start.elapsed() < Duration::from_secs(30)
            {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            assert!(record.join("terminated").is_file());
            assert!(!dir.path().join("work/asr.jsonl").exists());
        });
    }
}
