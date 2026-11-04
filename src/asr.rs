//! HTTP transcription of speech segments, with retries and resumable chunks.

use crate::config::PipelineConfig;
use crate::timeline::TranscriptEvent;
use crate::vad::{Seg, cut_wav, earshot_vad};
use anyhow::{Context, Result};

use std::path::Path;
use std::time::{Duration, Instant};

pub async fn run(cfg: &PipelineConfig, wav: &Path) -> Result<Vec<TranscriptEvent>> {
    cfg.validate_asr()?;
    let api = cfg.asr.clone();
    let wav = wav.to_path_buf();
    let stage = cfg
        .transcript_name
        .clone()
        .unwrap_or_else(|| "transcript".into());
    let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let _cancel_on_drop = crate::runtime::CancelOnDrop(cancelled.clone());
    crate::runtime::spawn_blocking(move || run_api(&api, &wav, &stage, &cancelled))
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
    stage: &str,
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

    let identity = if api.command.is_empty() {
        crate::config::asr_endpoint(api)?
    } else {
        let suffix = match api.mode {
            crate::settings::AsrMode::Transcriptions => "/audio/transcriptions",
            crate::settings::AsrMode::Chat => "/chat/completions",
        };
        format!("adapter:{}{suffix}", api.command.join("\0"))
    };

    let tmp = tempfile::Builder::new()
        .prefix("notepipe-asr-")
        .tempdir()
        .context("Could not create a temporary working directory")?;
    let url = || {
        let mut resolved = api.clone();
        if let Some(provider) = &api.provider {
            resolved.base_url = provider.base(stage, &check_control)?;
        }
        crate::config::asr_endpoint(&resolved)
    };
    let pb = crate::progress::Bar::new(format!("{stage}/speech"), segs.len() as u64).with_template(
        "{spinner:.green} asr {pos}/{len} [{bar:32.cyan/blue}] {elapsed} {eta} {msg}",
    );

    let client = crate::dispatch::http_agent();

    // Bounded concurrency with std::thread::scope (borrowing, no Arc); network round trips dominate.
    // Results come back over a channel. After an abort, requests in flight finish on their own and are still joined.
    let (tx, rx) = std::sync::mpsc::channel::<(usize, Result<Option<String>>)>();
    let next = std::sync::atomic::AtomicUsize::new(0);
    let abort = std::sync::atomic::AtomicBool::new(false);
    let mut err: Option<anyhow::Error> = None;
    let mut events = Vec::new();
    std::thread::scope(|s| {
        for _ in 0..api.concurrency.min(segs.len()) {
            let tx = tx.clone();
            let target = ApiTarget {
                client: &client,
                identity: &identity,
                url: &url,
                model: &api.model,
                key: &api_key,
                mode: api.mode,
                prompt: &api.prompt,
                language: &api.language,
                stage,
                check_control: &check_control,
            };
            let (tmp_dir, wav) = (tmp.path(), wav);
            let (segs, next, abort) = (&segs, &next, &abort);
            let check_control = &check_control;
            s.spawn(move || {
                loop {
                    if abort.load(std::sync::atomic::Ordering::Relaxed) {
                        break;
                    }
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let Some(&seg) = segs.get(i) else {
                        break;
                    };
                    if let Err(error) = check_control() {
                        abort.store(true, std::sync::atomic::Ordering::Relaxed);
                        let _ = tx.send((i, Err(error)));
                        break;
                    }
                    let r = (|| {
                        let _permit = api
                            .limit
                            .as_ref()
                            .map(|limit| limit.acquire())
                            .transpose()?;
                        transcribe_api(&target, &tmp_dir.join(format!("c{i:04}.wav")), seg, wav)
                    })();
                    if r.is_err() {
                        abort.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                    if tx.send((i, r)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(tx);

        loop {
            if err.is_none()
                && let Err(error) = check_control()
            {
                err = Some(error);
                abort.store(true, std::sync::atomic::Ordering::Relaxed);
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
                _ if err.is_some() => {} // already failed: drop late results
                Ok(Some(text)) => events.push(TranscriptEvent {
                    extra: Default::default(),
                    start: segs[i].start,
                    end: segs[i].end,
                    text,
                    raw: None,
                }),
                Ok(None) => {}
                Err(e) => {
                    err = Some(e);
                    abort.store(true, std::sync::atomic::Ordering::Relaxed);
                }
            }
            // Every collected result, empty or not, advances progress exactly once, as in the sequential path.
            pb.inc(1);
        }
    });
    pb.finish();
    if let Some(e) = err {
        return Err(e);
    }
    check_control()?;
    events.sort_by(|a, b| a.start.total_cmp(&b.start));
    warn_empty_segments(events.len(), segs.len());
    tracing::info!(
        n = events.len(),
        secs = format_args!("{:.1}", t0.elapsed().as_secs_f64()),
        "asr done"
    );
    Ok(events)
}

fn warn_empty_segments(with_text: usize, total: usize) {
    let empty = total - with_text;
    if empty > 0 {
        crate::dispatch::warn(format!(
            "{empty} of {} speech segments returned no text",
            total
        ));
    }
}

fn post_cloud(
    t: &ApiTarget,
    audio: &[u8],
    item: &crate::dispatch::Item,
) -> Result<serde_json::Value> {
    let fields = serde_json::json!({
        "version": 1,
        "api": t.mode,
        "model": t.model,
        "prompt": if t.mode == crate::settings::AsrMode::Chat { t.prompt } else { "" },
        "language": t.language,
        "audio_sha256": crate::runtime::digest(audio),
    });
    Ok(crate::dispatch::request(item, t.check_control, || {
        crate::dispatch::cached_json_request(
            t.identity,
            &fields,
            t.key,
            t.stage,
            || {
                let (content_type, body) =
                    transcription_request(t, audio).map_err(crate::dispatch::Failure::local)?;
                let url = (t.url)().map_err(crate::dispatch::Failure::local)?;
                let mut request = t.client.post(&url).set("Content-Type", &content_type);
                if !t.key.is_empty() {
                    request = request.set("Authorization", &format!("Bearer {}", t.key));
                }
                crate::dispatch::send_http(t.check_control, request, body, t.key)
            },
            |value| {
                crate::dispatch::check_finish_reason(value)?;
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
    })?)
}

/// Transcription endpoint: address, credentials and dialect, borrowed by all workers.
struct ApiTarget<'a> {
    client: &'a ureq::Agent,
    identity: &'a str,
    url: &'a (dyn Fn() -> Result<String> + Sync),
    model: &'a str,
    key: &'a str,
    mode: crate::settings::AsrMode,
    prompt: &'a str,
    language: &'a str,
    stage: &'a str,
    check_control: &'a (dyn Fn() -> Result<()> + Sync),
}

/// Transcribes one chunk; `Ok(None)` means it contains no speech.
fn transcribe_api(t: &ApiTarget, chunk: &Path, seg: Seg, wav: &Path) -> Result<Option<String>> {
    let item = crate::dispatch::Item::speech(seg.start, seg.end);
    let prepare = (|| {
        cut_wav(wav, seg.cut_start, seg.cut_end, chunk).context("Could not split audio")?;
        (t.check_control)()?;
        let bytes = std::fs::read(chunk)
            .with_context(|| format!("Reading audio segment: {}", chunk.display()))?;
        Ok::<_, anyhow::Error>(bytes)
    })();
    let bytes = prepare.map_err(|error| anyhow::anyhow!("{}: {error:#}", item.failure))?;
    let result = post_cloud(t, &bytes, &item);
    let _ = std::fs::remove_file(chunk);
    let value = result?;
    if let Some(warning) = value["warning"]
        .as_str()
        .map(|warning| crate::dispatch::diagnostic(warning, t.key))
        .filter(|warning| !warning.is_empty())
    {
        crate::dispatch::warn(format!("{}: {warning}", item.label));
    }
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
        let mut prompt = t.prompt.to_owned();
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
    fn audio_preparation_errors_name_the_speech_segment() {
        if crate::runtime::which("ffmpeg").is_none() {
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let target = ApiTarget {
            identity: "test-provider",
            client: &ureq::agent(),
            url: &|| Ok("http://localhost".into()),
            model: "",
            key: "",
            mode: crate::settings::AsrMode::Transcriptions,
            prompt: "",
            language: "",
            stage: "transcript",
            check_control: &crate::runtime::check_cancelled,
        };
        let error = transcribe_api(
            &target,
            &root.path().join("chunk.wav"),
            Seg {
                start: 61.,
                end: 62.,
                cut_start: 0.,
                cut_end: 1.,
            },
            &root.path().join("missing.wav"),
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .starts_with("Speech recognition failed at 01:01–01:02: Could not split audio:")
        );
    }

    #[test]
    fn transcription_upload_preserves_audio_bytes() {
        let audio = b"RIFF\0\xff\r\nWAVE";
        let target = ApiTarget {
            identity: "test-provider",
            client: &ureq::agent(),
            url: &|| Ok("http://localhost".into()),
            model: "whisper-1",
            key: "",
            mode: crate::settings::AsrMode::Transcriptions,
            prompt: "",
            language: "",
            stage: "transcript",
            check_control: &crate::runtime::check_cancelled,
        };
        let (content_type, body) = transcription_request(&target, audio).unwrap();
        let boundary = content_type
            .strip_prefix("multipart/form-data; boundary=")
            .unwrap();
        assert!(body.starts_with(format!("--{boundary}\r\n").as_bytes()));
        assert!(body.windows(audio.len()).any(|slice| slice == audio));
        assert!(body.ends_with(format!("\r\n--{boundary}--\r\n").as_bytes()));
        assert!(String::from_utf8_lossy(&body).contains("name=\"file\"; filename=\"audio.wav\""));
        let another = transcription_request(&target, audio).unwrap();
        assert_ne!(content_type, another.0);
        assert_ne!(body, another.1);
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
        let error =
            crate::dispatch::request(&crate::dispatch::Item::speech(0., 1.), &check, || {
                attempts += 1;
                cancelled.store(true, std::sync::atomic::Ordering::Relaxed);
                let mut error = crate::dispatch::Failure::local("retryable response");
                error.retryable = true;
                Err::<(), _>(error)
            })
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "Speech recognition failed at 00:00–00:01: test retry cancellation"
        );
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
    fn empty_warning_counts_only_the_current_segments() {
        use crate::test_support::MockAi;
        use std::sync::atomic::{AtomicBool, Ordering};
        let _dispatch = crate::runtime::lock_recover(&crate::dispatch::TEST_LOCK);
        if crate::runtime::which("ffmpeg").is_none() {
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let cache = crate::dispatch::install(root.path()).unwrap();
        let wav = Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/speech.wav"
        ));
        let mut api = Asr {
            mode: AsrMode::Chat,
            command: vec!["adapter".into(), "--model".into(), "fixture".into()],
            max_speech: 1.,
            concurrency: 3,
            ..Default::default()
        };
        let segments = earshot_vad(wav, &api, &|| Ok(())).unwrap();
        assert!(segments.len() > 1);
        let first_count = segments.len();
        let mock = MockAi::respond_with(move |call, _| {
            Some((
                200,
                serde_json::json!({
                    "choices": [{"message": {"content": if call < first_count { "Speech" } else { "" }}}]
                }),
            ))
        });
        api.base_url = mock.url.clone();
        let cancelled = AtomicBool::new(false);
        let first = run_api(&api, wav, "transcript", &cancelled).unwrap();
        assert_eq!(first.len(), segments.len());
        for (event, seg) in first.iter().zip(&segments) {
            assert_eq!((event.start, event.end), (seg.start, seg.end));
            assert_eq!(event.text, "Speech");
        }
        assert!(cache.take_warnings().is_empty());
        api.max_speech = 30.;
        let current = earshot_vad(wav, &api, &|| Ok(())).unwrap();
        assert_eq!(current.len(), 1);
        assert!(
            run_api(&api, wav, "transcript", &cancelled)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            cache.take_warnings(),
            ["1 of 1 speech segments returned no text"]
        );
        let requests = mock.calls.load(Ordering::SeqCst);
        api.base_url = "http://127.0.0.1:1".into();
        assert!(
            run_api(&api, wav, "transcript", &cancelled)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            cache.take_warnings(),
            ["1 of 1 speech segments returned no text"]
        );
        api.max_speech = 1.;
        let cached = run_api(&api, wav, "renamed-transcript", &cancelled).unwrap();
        assert_eq!(
            serde_json::to_value(cached).unwrap(),
            serde_json::to_value(first).unwrap()
        );
        assert!(cache.take_warnings().is_empty());
        assert_eq!(mock.calls.load(Ordering::SeqCst), requests);
    }

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
                identity: "test-provider",
                client: &ureq::agent(),
                url: &|| Ok(format!("{}/chat/completions", mock.url)),
                model: "",
                key: "",
                mode: AsrMode::Chat,
                prompt: "Transcribe verbatim.",
                language: "",
                stage: "transcript",
                check_control: &crate::runtime::check_cancelled,
            };
            let error =
                post_cloud(&target, b"audio", &crate::dispatch::Item::speech(0., 1.)).unwrap_err();
            let failure = error.downcast_ref::<crate::dispatch::Failure>().unwrap();
            assert!(!failure.retryable);
            assert_eq!(
                error.to_string(),
                "Speech recognition failed at 00:00–00:01: the model stopped at its output limit"
            );
            assert_eq!(mock.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
            assert!(!root.path().join("responses").exists());
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
            identity: "test-provider",
            client: &crate::runtime::http_agent(Duration::from_millis(100)),
            url: &|| Ok(url.clone()),
            model: "",
            key: "",
            mode: AsrMode::Transcriptions,
            prompt: "",
            language: "",
            stage: "transcript",
            check_control: &check,
        };
        let error =
            post_cloud(&target, b"audio", &crate::dispatch::Item::speech(0., 1.)).unwrap_err();
        assert!(error.to_string().contains("test send cancellation"));
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }

    #[test]
    fn fast_responses_obey_cancellation_before_dispatch_and_caching() {
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
            let _cache = crate::dispatch::install(dir.path()).unwrap();
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
                    "transcript",
                    &cancelled,
                );
                done.store(true, Ordering::Relaxed);
                assert!(result.is_err());
                assert_eq!(server.join().unwrap(), 1);
                assert!(!dir.path().join("responses").exists());
            });
        }
    }

    #[test]
    fn dialects_cache_by_audio_and_preserve_requests_and_responses() {
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
                    let mut requests = Vec::new();
                    for index in 0..2 {
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
                            if let Some(offset) = request.windows(4).position(|w| w == b"\r\n\r\n")
                            {
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
                        let text = if index == 0 {
                            " language English<asr_text>Recognized.</asr_text> "
                        } else {
                            "Different audio."
                        };
                        let response = if mode == AsrMode::Chat {
                            serde_json::json!({"choices":[{"message":{field:text}}]})
                        } else {
                            serde_json::json!({field:text})
                        }
                        .to_string();
                        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}", response.len()).unwrap();
                        requests.push((head, request[offset..].to_vec()));
                    }
                    requests
                });
                let api = Asr {
                    mode,
                    base_url: base,
                    ..Default::default()
                };
                let url = crate::config::asr_endpoint(&api).unwrap();
                let target = ApiTarget {
                    identity: "test-provider",
                    client: &crate::dispatch::http_agent(),
                    url: &|| Ok(url.clone()),
                    model: if with_options { "test-model" } else { "" },
                    key: if with_options { "test-key" } else { "" },
                    mode,
                    prompt: "  User transcription rules.\nKeep every word.  ",
                    language: if with_options { "ko" } else { "" },
                    stage: "transcript",
                    check_control: &crate::runtime::check_cancelled,
                };
                let dir = tempfile::tempdir().unwrap();
                let _cache = crate::dispatch::install(dir.path()).unwrap();
                let wav = Path::new(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tests/fixtures/speech.wav"
                ));
                let seg = Seg {
                    start: 0.,
                    end: 2.4,
                    cut_start: 0.,
                    cut_end: 2.4,
                };
                let chunk = dir.path().join("chunk.wav");
                for _ in 0..2 {
                    assert_eq!(
                        transcribe_api(&target, &chunk, seg, wav)
                            .unwrap()
                            .as_deref(),
                        Some(" language English<asr_text>Recognized.</asr_text> ")
                    );
                }
                let mut changed = std::fs::read(wav).unwrap();
                let data = changed
                    .windows(4)
                    .position(|bytes| bytes == b"data")
                    .unwrap()
                    + 8;
                changed[data + 1000] ^= 0xff;
                let other = dir.path().join("other.wav");
                std::fs::write(&other, changed).unwrap();
                assert_eq!(
                    transcribe_api(&target, &chunk, seg, &other)
                        .unwrap()
                        .as_deref(),
                    Some("Different audio.")
                );
                let requests = server.join().unwrap();
                assert_eq!(requests.len(), 2);
                let (head, body) = &requests[0];
                assert!(head.starts_with(&format!("post /v1{suffix} ")), "{head}");
                assert_eq!(head.contains("authorization:"), with_options);
                if with_options {
                    assert!(head.contains("authorization: bearer test-key"));
                }
                if mode == AsrMode::Chat {
                    let json: serde_json::Value = serde_json::from_slice(body).unwrap();
                    assert_eq!(json.get("model").is_some(), with_options);
                    assert_eq!(json["temperature"], 0);
                    let prompt = json["messages"][0]["content"][0]["text"].as_str().unwrap();
                    assert_eq!(
                        prompt,
                        if with_options {
                            format!("{} The speech is in ko.", target.prompt)
                        } else {
                            target.prompt.into()
                        }
                    );
                    assert_eq!(
                        json["messages"][0]["content"][1]["input_audio"]["format"],
                        "wav"
                    );
                } else {
                    let text = String::from_utf8_lossy(body);
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
                identity: "test-provider",
                client: &client,
                url: &|| Ok(String::new()),
                model: "",
                key: "",
                mode: AsrMode::Chat,
                prompt: "Transcribe verbatim.",
                language,

                stage: "transcript",
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
}
