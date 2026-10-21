//! End-to-end CLI contract with stdin configuration and offline service fixtures.
use notepipe::{
    config::{SlideMode, TranscriptSource},
    settings::ConfigFile,
};
use serde_json::{Value, json};
use std::{
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

fn config() -> ConfigFile {
    let mut config = ConfigFile::default();
    config.defaults.slide_mode = Some(SlideMode::First);
    config
}
fn spawn(root: &Path, source: &str, config: &ConfigFile, flags: &[&str]) -> std::process::Child {
    let config_dir = root.join("config/notepipe");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(config_dir.join("config.toml"), "[broken").unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_notepipe"))
        .arg(source)
        .args(["--config", "-", "--json"])
        .args(flags)
        .current_dir(root)
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env_remove("NOTEPIPE_ASR_API_KEY")
        .env_remove("NOTEPIPE_LLM_API_KEY")
        .env_remove("RUST_LOG")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(config).unwrap())
        .unwrap();
    child
}
fn run(root: &Path, source: &str, config: &ConfigFile, flags: &[&str]) -> Output {
    spawn(root, source, config, flags)
        .wait_with_output()
        .unwrap()
}
fn events(output: &Output) -> Vec<Value> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| {
            let event: Value = serde_json::from_str(line).unwrap();
            assert!(
                ["log", "stage", "progress", "tokens", "done", "error"]
                    .contains(&event["type"].as_str().unwrap()),
                "{event}"
            );
            event
        })
        .collect()
}
fn done(output: &Output) -> Value {
    let events = events(output);
    assert!(
        output.status.success(),
        "{events:?} {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let done = events.last().unwrap().clone();
    assert_eq!(done["type"], "done");
    assert_eq!(done.as_object().unwrap().len(), 5);
    done
}
fn document(output: &Output) -> Value {
    serde_json::from_slice(&std::fs::read(done(output)["json"].as_str().unwrap()).unwrap()).unwrap()
}
fn files(root: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries {
            let path = entry.unwrap().path();
            if path.is_dir() {
                paths.extend(files(&path));
            } else {
                paths.push(path);
            }
        }
    }
    paths
}
fn calls(record: &Path) -> usize {
    std::fs::read_to_string(record.join("calls"))
        .unwrap_or_default()
        .lines()
        .count()
}
fn source_command(record: &Path, behavior: &str) -> Option<Vec<String>> {
    let python =
        notepipe::runtime::which("python3").or_else(|| notepipe::runtime::which("python"))?;
    Some(vec![
        python.to_string_lossy().into_owned(),
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/source_adapter.py"
        )
        .into(),
        "--record".into(),
        record.to_string_lossy().into_owned(),
        "--behavior".into(),
        behavior.into(),
    ])
}

struct MockAi {
    url: String,
    calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    bodies: std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
    headers: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl MockAi {
    fn respond_with(
        respond: impl Fn(usize, &serde_json::Value) -> Option<(u16, serde_json::Value)> + Send + 'static,
    ) -> Self {
        use std::{
            io::Read,
            sync::{
                Arc,
                atomic::{AtomicBool, AtomicUsize, Ordering},
            },
            time::Duration,
        };
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let bodies = Arc::new(std::sync::Mutex::new(Vec::new()));
        let received = bodies.clone();
        let headers = Arc::new(std::sync::Mutex::new(Vec::new()));
        let received_headers = headers.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let (count, shutdown) = (calls.clone(), stop.clone());
        let worker = std::thread::spawn(move || {
            while !shutdown.load(Ordering::SeqCst) {
                let Ok((mut stream, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                };
                // BSD may inherit the listener's nonblocking mode. The request
                // reader below uses blocking reads with a bounded timeout.
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut data = Vec::new();
                let mut buffer = [0u8; 4096];
                let (header_end, length) = loop {
                    let size = stream.read(&mut buffer).unwrap();
                    if size == 0 {
                        return;
                    }
                    data.extend_from_slice(&buffer[..size]);
                    if let Some(index) = data.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                        let header = String::from_utf8_lossy(&data[..index]);
                        let length = header
                            .lines()
                            .find_map(|line| {
                                line.split_once(':')
                                    .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                                    .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                            })
                            .unwrap();
                        break (index + 4, length);
                    }
                };
                while data.len() < header_end + length {
                    let size = stream.read(&mut buffer).unwrap();
                    if size == 0 {
                        return;
                    }
                    data.extend_from_slice(&buffer[..size]);
                }
                let number = count.fetch_add(1, Ordering::SeqCst);
                let body: serde_json::Value =
                    serde_json::from_slice(&data[header_end..header_end + length]).unwrap();
                received.lock().unwrap().push(body.clone());
                received_headers
                    .lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&data[..header_end]).into_owned());
                let Some((status, response)) = respond(number, &body) else {
                    continue;
                };
                let response = serde_json::to_vec(&response).unwrap();
                write!(stream,"HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",response.len()).unwrap();
                stream.write_all(&response).unwrap();
            }
        });
        Self {
            url,
            calls,
            bodies,
            headers,
            stop,
            worker: Some(worker),
        }
    }
}
impl Drop for MockAi {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            let result = worker.join();
            if !std::thread::panicking() {
                result.unwrap();
            }
        }
    }
}

fn speech_video(root: &Path) -> Option<std::path::PathBuf> {
    let ffmpeg = notepipe::runtime::which("ffmpeg")?;
    notepipe::runtime::which("ffprobe")?;
    let video = root.join("speech.mp4");
    let generated = Command::new(ffmpeg)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=blue:s=64x32:r=10",
            "-i",
        ])
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/speech.wav"
        ))
        .args(["-t", "2.4", "-pix_fmt", "yuv420p", "-c:a", "aac"])
        .arg(&video)
        .output()
        .unwrap();
    assert!(
        generated.status.success(),
        "{}",
        String::from_utf8_lossy(&generated.stderr)
    );
    Some(video)
}

fn adapter_command(record: &Path, behavior: &str) -> Option<Vec<String>> {
    let python =
        notepipe::runtime::which("python3").or_else(|| notepipe::runtime::which("python"))?;
    Some(vec![
        python.display().to_string(),
        "-u".into(),
        concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/asr_adapter.py").into(),
        "--record".into(),
        record.display().to_string(),
        "--behavior".into(),
        behavior.into(),
    ])
}

#[cfg(unix)]
fn wait_for_file(child: &mut std::process::Child, path: &Path) {
    let start = std::time::Instant::now();
    while !path.is_file() {
        if start.elapsed() >= std::time::Duration::from_secs(15)
            || child.try_wait().unwrap().is_some()
        {
            let _ = child.kill();
            let _ = child.wait();
            panic!("fixture never reached {}", path.display());
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[cfg(unix)]
fn wait_for_cancelled(mut child: std::process::Child) -> Output {
    let start = std::time::Instant::now();
    while child.try_wait().unwrap().is_none() {
        if start.elapsed() >= std::time::Duration::from_secs(10) {
            let _ = child.kill();
            let _ = child.wait();
            panic!("cancellation did not stop the CLI");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(130));
    output
}

#[test]
fn explicit_subtitles_publish_plain_notes_and_replace_only_the_same_source() {
    let root = tempfile::tempdir().unwrap();
    let Some(video) = speech_video(root.path()) else {
        return;
    };
    let subtitle = root.path().join("selected.srt");
    std::fs::write(
        &subtitle,
        "1\n00:00:00,000 --> 00:00:02,000\nSelected words.\n",
    )
    .unwrap();
    std::fs::write(video.with_extension("srt"), "bad sidecar ignored").unwrap();
    let config = config();
    let flags = ["--subtitle", subtitle.to_str().unwrap()];
    let first = run(root.path(), video.to_str().unwrap(), &config, &flags);
    let published = done(&first);
    assert!(published["warnings"].as_array().unwrap().is_empty());
    let note_dir = Path::new(published["note_dir"].as_str().unwrap());
    assert_eq!(note_dir.file_name().unwrap(), "speech");
    let note = std::fs::read_to_string(note_dir.join("note.md")).unwrap();
    assert!(note.starts_with("---\ntitle: \"speech\"\nsource: "));
    assert!(note.contains("Selected words.") && note.contains("![00:00](images/0001.jpg)"));
    assert!(!note.contains("Screenshot of video") && !note.contains("Generated by"));
    let doc = document(&first);
    assert_eq!(
        doc["meta"]["source"],
        video.canonicalize().unwrap().to_str().unwrap()
    );
    assert_eq!(doc["meta"].as_object().unwrap().len(), 4);
    assert_eq!(doc["sections"][0]["image"], "images/0001.jpg");
    assert!(note_dir.join("images/0001.jpg").is_file());
    let names = std::fs::read_dir(note_dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert_eq!(names.len(), 3);
    std::fs::write(
        &subtitle,
        "1\n00:00:00,000 --> 00:00:02,000\nUpdated selected words.\n",
    )
    .unwrap();
    let second = run(root.path(), video.to_str().unwrap(), &config, &flags);
    assert_eq!(done(&second)["note_dir"], published["note_dir"]);
    assert!(
        std::fs::read_to_string(note_dir.join("note.md"))
            .unwrap()
            .contains("Updated selected words.")
    );
    let other = root.path().join("other");
    std::fs::create_dir(&other).unwrap();
    let other_video = other.join("speech.mp4");
    std::fs::copy(&video, &other_video).unwrap();
    let third = run(root.path(), other_video.to_str().unwrap(), &config, &flags);
    assert!(
        done(&third)["note_dir"]
            .as_str()
            .unwrap()
            .ends_with("speech-2")
    );
    assert!(note_dir.join("note.json").is_file());
}

#[test]
fn broken_media_publishes_subtitles_with_matching_warnings_and_no_phantom_images() {
    if notepipe::runtime::which("ffmpeg").is_none() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let video = root.path().join("broken.mp4");
    let subtitle = root.path().join("transcript.vtt");
    std::fs::write(&video, "broken video").unwrap();
    std::fs::write(
        &subtitle,
        "WEBVTT\n\n00:00.000 --> 00:02.000\nReadable transcript.\n",
    )
    .unwrap();
    let output = run(
        root.path(),
        video.to_str().unwrap(),
        &config(),
        &["--subtitle", subtitle.to_str().unwrap()],
    );
    let done = done(&output);
    assert!(!done["warnings"].as_array().unwrap().is_empty());
    let doc = document(&output);
    assert_eq!(doc["warnings"], done["warnings"]);
    assert_eq!(doc["sections"][0]["image"], "");
    assert!(
        !std::fs::read_to_string(done["markdown"].as_str().unwrap())
            .unwrap()
            .contains("![")
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("Warning:"));
}

#[test]
fn source_command_changes_separate_caches_and_preserve_adapter_seek_templates() {
    let root = tempfile::tempdir().unwrap();
    if notepipe::runtime::which("ffmpeg").is_none() {
        return;
    }
    let record = root.path().join("source");
    let mut cfg = config();
    let url = "https://EXAMPLE.test:443/media?episode=1&t=12#part";
    for (behavior, link) in [
        ("healthy", url),
        ("seek", "https://example.test/play?at=0#part"),
        ("healthy", url),
    ] {
        let Some(command) = source_command(&record, behavior) else {
            return;
        };
        cfg.source.command = command;
        let output = run(root.path(), url, &cfg, &[]);
        let doc = document(&output);
        assert_eq!(doc["meta"]["source"], url);
        let markdown =
            std::fs::read_to_string(done(&output)["markdown"].as_str().unwrap()).unwrap();
        assert!(
            markdown.contains(&format!("## [00:00]({link})")),
            "{markdown}"
        );
        if behavior == "seek" {
            assert_eq!(
                doc["meta"]["seek_url"],
                "https://example.test/play?at={t}#part"
            );
        } else {
            assert!(doc["meta"].get("seek_url").is_none());
        }
    }
    assert_eq!(
        calls(&record),
        2,
        "each command needs its own source and stage caches"
    );
}

#[test]
fn llm_only_changes_reuse_source_frames_and_transcript_without_download_or_asr() {
    let root = tempfile::tempdir().unwrap();
    if notepipe::runtime::which("ffmpeg").is_none() {
        return;
    }
    let source_record = root.path().join("source");
    let asr_record = root.path().join("asr");
    let (Some(source), Some(asr)) = (
        source_command(&source_record, "speech"),
        adapter_command(&asr_record, "healthy"),
    ) else {
        return;
    };
    let mock = MockAi::respond_with(|_, body| {
        assert!(body.get("response_format").is_none());
        Some((
            200,
            json!({"choices":[{"message":{"content":"## A custom summary\n\nVerbatim **Markdown**."}}]}),
        ))
    });
    let mut cfg = config();
    cfg.source.command = source;
    cfg.asr.command = asr;
    cfg.defaults.transcript_source = Some(TranscriptSource::Asr);
    let url = "https://example.test/media?episode=1";
    let first = run(root.path(), url, &cfg, &[]);
    let first_done = done(&first);
    assert_eq!(calls(&source_record), 1);
    let asr_calls = calls(&asr_record);
    assert!(asr_calls > 0);
    let cached = files(&root.path().join("cache/notepipe/work"));
    assert!(
        !cached
            .iter()
            .any(|path| path.file_name().unwrap() == "media.mp4"
                || path.file_name().unwrap() == "audio.wav")
    );
    let frame_marker = cached
        .iter()
        .find(|path| {
            path.file_name().unwrap() == "complete.json"
                && path
                    .parent()
                    .unwrap()
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("frames-")
        })
        .unwrap();
    let frame_time = frame_marker.metadata().unwrap().modified().unwrap();
    // A failed publication can leave media after both stage caches are complete.
    let source_marker = cached
        .iter()
        .find(|path| {
            path.file_name().unwrap() == "complete.json"
                && path.parent().unwrap().file_name().unwrap() == "source"
        })
        .unwrap();
    let stranded_media = source_marker.parent().unwrap().join("files/media.mp4");
    std::fs::write(&stranded_media, "cached media").unwrap();
    cfg.llm
        .stages
        .push(notepipe::llm::Stage::builtin("summary").unwrap());
    cfg.llm.base_url = mock.url.clone();
    cfg.llm.model = "fixture".into();
    for _ in 0..2 {
        let second = run(root.path(), url, &cfg, &[]);
        assert_eq!(done(&second)["note_dir"], first_done["note_dir"]);
        assert!(
            document(&second)["top"][0]["text"]
                .as_str()
                .unwrap()
                .contains("Verbatim **Markdown**.")
        );
    }
    assert!(!stranded_media.exists());
    assert_eq!(mock.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(calls(&source_record), 1);
    assert_eq!(calls(&asr_record), asr_calls);
    assert_eq!(
        frame_marker.metadata().unwrap().modified().unwrap(),
        frame_time
    );
    // A directory at the obsolete audio path makes cleanup fail on every platform.
    let audio = source_marker
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("audio.wav");
    std::fs::create_dir(&audio).unwrap();
    let warning_run = run(root.path(), url, &cfg, &[]);
    let warning_doc = document(&warning_run);
    assert!(
        warning_doc["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning
                .as_str()
                .unwrap()
                .contains("Could not remove cached media"))
    );
    assert_eq!(warning_doc["warnings"], done(&warning_run)["warnings"]);
    for section in warning_doc["sections"].as_array().unwrap() {
        let image = section["image"].as_str().unwrap();
        assert!(image.starts_with("images/"));
        assert!(
            Path::new(first_done["note_dir"].as_str().unwrap())
                .join(image)
                .is_file()
        );
    }
    std::fs::remove_dir(audio).unwrap();
    let reset = run(root.path(), url, &cfg, &["--no-resume"]);
    done(&reset);
    assert_eq!(calls(&source_record), 2);
    assert!(calls(&asr_record) > asr_calls);
    assert_eq!(mock.calls.load(std::sync::atomic::Ordering::SeqCst), 2);
}

#[test]
fn managed_asr_uses_keyless_http_and_stops_after_success_or_error() {
    for behavior in ["healthy", "error"] {
        let root = tempfile::tempdir().unwrap();
        let Some(video) = speech_video(root.path()) else {
            return;
        };
        let record = root.path().join("asr");
        let Some(command) = adapter_command(&record, behavior) else {
            return;
        };
        let mut cfg = config();
        cfg.asr.command = command;
        cfg.defaults.transcript_source = Some(TranscriptSource::Asr);
        let output = run(root.path(), video.to_str().unwrap(), &cfg, &[]);
        assert_eq!(
            output.status.success(),
            behavior == "healthy",
            "{:?}",
            events(&output)
        );
        assert!(record.join("terminated").is_file());
        assert_eq!(
            std::fs::read_to_string(record.join("authorization")).unwrap(),
            ""
        );
        assert_eq!(
            std::fs::read_to_string(record.join("endpoint")).unwrap(),
            "/v1/audio/transcriptions"
        );
        if behavior == "healthy" {
            assert!(
                document(&output)
                    .to_string()
                    .contains("Fixture transcript.")
            );
        } else {
            assert_eq!(events(&output).last().unwrap()["type"], "error");
        }
    }
}

#[test]
fn proofreading_uses_custom_rules_and_caches_a_validated_response_format_fallback() {
    let root = tempfile::tempdir().unwrap();
    let Some(video) = speech_video(root.path()) else {
        return;
    };
    std::fs::write(
        video.with_extension("srt"),
        "1\n00:00:00,000 --> 00:00:02,000\nOriginal transcript.\n",
    )
    .unwrap();
    let mock = MockAi::respond_with(|_, body| {
        assert!(body.to_string().contains("CUSTOM-RULE-42"));
        Some(if body.get("response_format").is_some() {
            (
                422,
                json!({"error":{"param":"response_format","code":"unsupported_parameter","message":"private-test-key"}}),
            )
        } else {
            (
                200,
                json!({"choices":[{"message":{"content":r#"{"segments":[{"id":0,"text":"Cached fallback."}]}"#}}]}),
            )
        })
    });
    let mut cfg = config();
    cfg.llm
        .stages
        .push(notepipe::llm::Stage::builtin("proofread").unwrap());
    cfg.llm.base_url = mock.url.clone();
    cfg.llm.model = "fixture".into();
    cfg.llm.stages[0].prompt = "CUSTOM-RULE-42".into();
    cfg.llm.api_key = "private-test-key".into();
    cfg.llm.stages[0].images = true;
    for _ in 0..2 {
        let output = run(root.path(), video.to_str().unwrap(), &cfg, &[]);
        assert!(document(&output).to_string().contains("Cached fallback."));
        assert_eq!(mock.calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    }
    assert!(
        mock.bodies.lock().unwrap()[0]
            .to_string()
            .contains("image_url")
    );
    for path in files(&root.path().join("cache")) {
        if path
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            assert!(
                !std::fs::read_to_string(path)
                    .unwrap()
                    .contains("private-test-key")
            );
        }
    }
}

#[test]
fn failed_transcription_preserves_the_previous_note_and_completed_chunks() {
    let root = tempfile::tempdir().unwrap();
    let Some(video) = speech_video(root.path()) else {
        return;
    };
    let record = root.path().join("asr");
    let Some(command) = adapter_command(&record, "fail-after-first") else {
        return;
    };
    let subtitle = root.path().join("complete.srt");
    std::fs::write(
        &subtitle,
        "1\n00:00:00,000 --> 00:00:02,000\nComplete previous note.\n",
    )
    .unwrap();
    let mut cfg = config();
    let previous = done(&run(
        root.path(),
        video.to_str().unwrap(),
        &cfg,
        &["--subtitle", subtitle.to_str().unwrap()],
    ));
    let note = Path::new(previous["markdown"].as_str().unwrap());
    let json = Path::new(previous["json"].as_str().unwrap());
    let old_note = std::fs::read(note).unwrap();
    let old_json = std::fs::read(json).unwrap();
    cfg.asr.command = command;
    cfg.asr.concurrency = 1;
    cfg.asr.max_speech = 1.;
    cfg.defaults.transcript_source = Some(TranscriptSource::Asr);
    let failed = run(root.path(), video.to_str().unwrap(), &cfg, &[]);
    assert!(!failed.status.success());
    assert_eq!(events(&failed).last().unwrap()["type"], "error");
    assert_eq!(std::fs::read(note).unwrap(), old_note);
    assert_eq!(std::fs::read(json).unwrap(), old_json);
    let cache = files(&root.path().join("cache/notepipe/work"));
    let checkpoint = cache
        .iter()
        .find(|path| path.file_name().unwrap() == "asr.jsonl")
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(checkpoint).unwrap().lines().count(),
        1
    );
    assert!(!checkpoint.parent().unwrap().join(".asr_done").exists());
    std::fs::write(record.join("release"), "ready").unwrap();
    done(&run(root.path(), video.to_str().unwrap(), &cfg, &[]));
    let chunks = std::fs::read_to_string(checkpoint).unwrap().lines().count();
    assert_eq!(
        calls(&record),
        chunks + 1,
        "completed chunks must be reused"
    );
}

#[cfg(unix)]
#[test]
fn sigint_and_sigterm_cancel_adapters_and_resume_completed_asr_chunks() {
    for signal in [libc::SIGINT, libc::SIGTERM] {
        let root = tempfile::tempdir().unwrap();
        let Some(video) = speech_video(root.path()) else {
            return;
        };
        let record = root.path().join("asr");
        let Some(command) = adapter_command(&record, "resume") else {
            return;
        };
        let mut cfg = config();
        cfg.asr.command = command;
        cfg.asr.concurrency = 1;
        cfg.asr.max_speech = 1.;
        cfg.defaults.transcript_source = Some(TranscriptSource::Asr);
        let mut child = spawn(root.path(), video.to_str().unwrap(), &cfg, &[]);
        wait_for_file(&mut child, &record.join("transcribing"));
        assert_eq!(unsafe { libc::kill(child.id() as i32, signal) }, 0);
        let output = wait_for_cancelled(child);
        assert!(!events(&output).iter().any(|event| event["type"] == "done"));
        assert!(record.join("terminated").is_file());
        let cache = files(&root.path().join("cache/notepipe/work"));
        let checkpoint = cache
            .iter()
            .find(|path| path.file_name().unwrap() == "asr.jsonl")
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(checkpoint).unwrap().lines().count(),
            1
        );
        assert!(
            !cache
                .iter()
                .any(|path| path.file_name().unwrap() == ".asr_done")
        );
        std::fs::write(record.join("release"), "ready").unwrap();
        let resumed = run(root.path(), video.to_str().unwrap(), &cfg, &[]);
        done(&resumed);
        let complete_chunks = std::fs::read_to_string(checkpoint).unwrap().lines().count();
        assert_eq!(
            calls(&record),
            complete_chunks + 1,
            "completed chunks must not be sent again"
        );
    }
}

#[cfg(unix)]
#[test]
fn a_second_signal_kills_the_source_tree_before_exiting_immediately() {
    use std::time::{Duration, Instant};
    let root = tempfile::tempdir().unwrap();
    let record = root.path().join("source");
    let Some(command) = source_command(&record, "hang") else {
        return;
    };
    let mut cfg = config();
    cfg.source.command = command;
    let mut child = spawn(root.path(), "https://example.test/media", &cfg, &[]);
    wait_for_file(&mut child, &record.join("child-ready"));
    let pid: i32 = std::fs::read_to_string(record.join("pid"))
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGINT) }, 0);
    wait_for_file(&mut child, &record.join("child-stopped"));
    let start = Instant::now();
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGTERM) }, 0);
    wait_for_cancelled(child);
    assert!(start.elapsed() < Duration::from_secs(1));
    let start = Instant::now();
    while unsafe { libc::kill(pid, 0) } == 0 {
        assert!(
            start.elapsed() < Duration::from_secs(3),
            "source adapter was orphaned"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn keyless_remote_asr_and_bounded_adapter_concurrency_still_work() {
    for concurrency in [1, 3] {
        let root = tempfile::tempdir().unwrap();
        let Some(video) = speech_video(root.path()) else {
            return;
        };
        let record = root.path().join("asr");
        let Some(command) = adapter_command(&record, "parallel") else {
            return;
        };
        let adapter = notepipe::adapter::Adapter::start(&command).unwrap();
        let mut cfg = config();
        cfg.defaults.transcript_source = Some(TranscriptSource::Asr);
        cfg.asr.base_url = format!("{}/v1", adapter.base);
        cfg.asr.max_speech = 1.;
        cfg.asr.concurrency = concurrency;
        let output = run(root.path(), video.to_str().unwrap(), &cfg, &[]);
        done(&output);
        assert_eq!(
            std::fs::read_to_string(record.join("authorization")).unwrap(),
            ""
        );
        let maximum: usize = std::fs::read_to_string(record.join("max-active"))
            .unwrap()
            .parse()
            .unwrap();
        assert!(maximum <= concurrency);
        if concurrency == 1 {
            assert_eq!(maximum, 1);
        } else {
            assert!(maximum > 1);
        }
    }
}

#[test]
fn proofreading_keys_prefer_flags_then_config_then_environment() {
    for (configured, flag, expected) in [
        ("file-key", Some("flag-key"), "flag-key"),
        ("file-key", None, "file-key"),
        ("", None, "environment-key"),
    ] {
        let root = tempfile::tempdir().unwrap();
        let Some(video) = speech_video(root.path()) else {
            return;
        };
        std::fs::write(
            video.with_extension("srt"),
            "1\n00:00:00,000 --> 00:00:02,000\nOriginal.\n",
        )
        .unwrap();
        let mock = MockAi::respond_with(|_, _| {
            Some((
                200,
                json!({"choices":[{"message":{"content":r#"{"segments":[{"id":0,"text":"Proofread."}]}"#}}]}),
            ))
        });
        let mut cfg = config();
        cfg.llm
            .stages
            .push(notepipe::llm::Stage::builtin("proofread").unwrap());
        cfg.llm.base_url = mock.url.clone();
        cfg.llm.model = "fixture".into();
        cfg.llm.api_key = configured.into();
        let path = root.path().join("settings.json");
        std::fs::write(&path, serde_json::to_vec(&cfg).unwrap()).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_notepipe"));
        command
            .arg(&video)
            .args(["--config", path.to_str().unwrap(), "--json"])
            .current_dir(root.path())
            .env("XDG_CACHE_HOME", root.path().join("cache"))
            .env("NOTEPIPE_LLM_API_KEY", "environment-key")
            .env_remove("RUST_LOG")
            .stdin(Stdio::null());
        if let Some(flag) = flag {
            command.args(["--llm-api-key", flag]);
        }
        done(&command.output().unwrap());
        assert!(
            mock.headers.lock().unwrap()[0].contains(&format!("Authorization: Bearer {expected}"))
        );
    }
}

#[test]
fn retryable_and_terminal_llm_errors_preserve_text_and_never_enter_the_cache() {
    let root = tempfile::tempdir().unwrap();
    let Some(video) = speech_video(root.path()) else {
        return;
    };
    std::fs::write(
        video.with_extension("srt"),
        "1\n00:00:00,000 --> 00:00:02,000\nOriginal words.\n",
    )
    .unwrap();
    let mock = MockAi::respond_with(|number, _| {
        Some(if number < 2 {
            (
                if number == 0 { 503 } else { 400 },
                json!({"error":{"message":"private-test-key"}}),
            )
        } else {
            (
                200,
                json!({"choices":[{"message":{"content":r#"{"segments":[{"id":0,"text":"Retried words."}]}"#}}]}),
            )
        })
    });
    let mut cfg = config();
    cfg.llm
        .stages
        .push(notepipe::llm::Stage::builtin("proofread").unwrap());
    cfg.llm.base_url = mock.url.clone();
    cfg.llm.model = "fixture".into();
    let first = run(root.path(), video.to_str().unwrap(), &cfg, &[]);
    let doc = document(&first);
    assert_eq!(mock.calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert!(doc.to_string().contains("Original words."));
    assert!(!doc["warnings"].as_array().unwrap().is_empty());
    assert!(!doc.to_string().contains("private-test-key"));
    assert!(
        !files(&root.path().join("cache/notepipe/work"))
            .iter()
            .any(|path| path.parent().unwrap().file_name().unwrap() == "llm")
    );
    for _ in 0..2 {
        let retry = run(root.path(), video.to_str().unwrap(), &cfg, &[]);
        assert!(document(&retry).to_string().contains("Retried words."));
        assert_eq!(mock.calls.load(std::sync::atomic::Ordering::SeqCst), 3);
    }
}

#[cfg(unix)]
#[test]
fn cancellation_during_audio_cutting_sends_no_http_request() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let Some(video) = speech_video(root.path()) else {
        return;
    };
    let ffmpeg = notepipe::runtime::which("ffmpeg").unwrap();
    let record = root.path().join("asr");
    let Some(command) = adapter_command(&record, "healthy") else {
        return;
    };
    let adapter = notepipe::adapter::Adapter::start(&command).unwrap();
    let mut cfg = config();
    cfg.defaults.transcript_source = Some(TranscriptSource::Asr);
    cfg.asr.base_url = format!("{}/v1", adapter.base);
    let bin = root.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let wrapper = bin.join("ffmpeg");
    std::fs::write(
        &wrapper,
        r#"#!/bin/sh
for output; do :; done
case "$output" in
  */c????.wav)
    : > "$CUT_STARTED"
    count=0
    while [ "$count" -lt 3000 ]; do sleep 0.01; count=$((count + 1)); done
    ;;
esac
exec "$REAL_FFMPEG" "$@"
"#,
    )
    .unwrap();
    std::fs::set_permissions(wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_notepipe"))
        .arg(&video)
        .args(["--config", "-", "--json"])
        .current_dir(root.path())
        .env(
            "PATH",
            format!(
                "{}:{}",
                bin.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .env("REAL_FFMPEG", ffmpeg)
        .env("CUT_STARTED", root.path().join("cutting"))
        .env("XDG_CACHE_HOME", root.path().join("cache"))
        .env_remove("RUST_LOG")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&cfg).unwrap())
        .unwrap();
    wait_for_file(&mut child, &root.path().join("cutting"));
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGTERM) }, 0);
    let output = wait_for_cancelled(child);
    assert!(!events(&output).iter().any(|event| event["type"] == "done"));
    assert!(
        !record.join("request").exists(),
        "HTTP was sent after cancellation"
    );
}

#[test]
fn generic_stages_preserve_order_overrides_and_markdown_outputs() {
    use std::sync::{Arc, Mutex, atomic::Ordering};
    let root = tempfile::tempdir().unwrap();
    let Some(video) = speech_video(root.path()) else {
        return;
    };
    std::fs::write(
        video.with_extension("srt"),
        "1\n00:00:00,000 --> 00:00:02,000\nOriginal words.\n",
    )
    .unwrap();
    let order = Arc::new(Mutex::new(Vec::new()));
    let main_order = order.clone();
    let main = MockAi::respond_with(move |_, body| {
        let system = body["messages"][0]["content"].as_str().unwrap();
        assert_eq!(body["model"], "default-model");
        assert_eq!(body["temperature"], 0.2);
        assert_eq!(body["top_p"], 0.8);
        assert!(body.get("max_tokens").is_none());
        let content = if system.starts_with("You proofread") {
            main_order.lock().unwrap().push("proofread");
            r#"{"segments":[{"id":0,"text":"Proofread words."}]}"#
        } else if system.starts_with("CUSTOM-TOP") {
            main_order.lock().unwrap().push("outline");
            assert!(!system.contains("language of the transcript"));
            assert!(
                body["messages"][1]["content"]
                    .as_str()
                    .unwrap()
                    .contains("Proofread words.")
                    || body["messages"][1]["content"]
                        .as_str()
                        .unwrap()
                        .contains("Original words.")
            );
            "## First block\n\nCustom **Markdown**."
        } else {
            assert!(system.starts_with("You summarize recordings"));
            main_order.lock().unwrap().push("summary");
            "## Second block\n\nSummary text."
        };
        Some((200, json!({"choices":[{"message":{"content":content}}]})))
    });
    let below_order = order.clone();
    let below = MockAi::respond_with(move |_, body| {
        below_order.lock().unwrap().push("gloss");
        assert_eq!(body["model"], "below-model");
        assert_eq!(body["temperature"], 0.7);
        assert_eq!(body["top_p"], 0.8);
        assert_eq!(body["top_k"], 5);
        assert!(
            body["messages"][0]["content"]
                .as_str()
                .unwrap()
                .starts_with("CUSTOM-BELOW Output a JSON object")
        );
        assert!(
            body["messages"][1]["content"]
                .to_string()
                .contains("Proofread words.")
        );
        Some((
            200,
            json!({"choices":[{"message":{"content":r#"{"segments":[{"id":0,"text":"Des mots en français.\nMore detail."}]}"#}}]}),
        ))
    });
    let mut cfg = config();
    cfg.llm = serde_json::from_value(json!({
        "base_url":main.url,"api_key":"default-key","model":"default-model",
        "request":{"temperature":0.2,"top_p":0.8},
        "stages":[
            {"name":"proofread"},
            {"name":"gloss","input":"paragraphs","output":"below","prompt":"CUSTOM-BELOW","base_url":below.url,"api_key":"stage-key","model":"below-model","request":{"temperature":0.7,"top_k":5}},
            {"name":"outline","input":"transcript","output":"top","prompt":"CUSTOM-TOP"},
            {"name":"summary"}
        ]
    })).unwrap();
    let output = run(root.path(), video.to_str().unwrap(), &cfg, &[]);
    let doc = document(&output);
    assert_eq!(
        *order.lock().unwrap(),
        ["proofread", "gloss", "outline", "summary"]
    );
    assert_eq!(doc["top"][0]["stage"], "outline");
    assert_eq!(doc["top"][1]["stage"], "summary");
    assert!(doc.get("summary").is_none());
    let event = &doc["sections"][0]["speech"][0];
    assert_eq!(event["text"], "Proofread words.");
    assert_eq!(event["raw"], "Original words.");
    assert_eq!(event["below"][0]["stage"], "gloss");
    assert!(event.get("translation").is_none());
    let markdown = std::fs::read_to_string(done(&output)["markdown"].as_str().unwrap()).unwrap();
    assert!(markdown.find("## First block").unwrap() < markdown.find("## Second block").unwrap());
    assert!(markdown.find("## Second block").unwrap() < markdown.find("Proofread words.").unwrap());
    assert!(markdown.contains("> Des mots en français.\n> More detail.\n"));
    assert!(below.headers.lock().unwrap()[0].contains("Authorization: Bearer stage-key"));
    assert!(
        main.headers
            .lock()
            .unwrap()
            .iter()
            .all(|header| header.contains("Authorization: Bearer default-key"))
    );
    let selected = run(
        root.path(),
        video.to_str().unwrap(),
        &cfg,
        &["--stages", "outline"],
    );
    let doc = document(&selected);
    assert_eq!(doc["top"].as_array().unwrap().len(), 1);
    assert_eq!(doc["top"][0]["stage"], "outline");
    assert_eq!(doc["sections"][0]["speech"][0]["text"], "Original words.");
    assert!(doc["sections"][0]["speech"][0].get("below").is_none());
    let calls = main.calls.load(Ordering::SeqCst);
    cfg.llm.base_url = "invalid unused URL".into();
    cfg.llm.model.clear();
    let disabled = run(root.path(), video.to_str().unwrap(), &cfg, &["--no-stages"]);
    assert!(document(&disabled)["top"].as_array().unwrap().is_empty());
    assert_eq!(main.calls.load(Ordering::SeqCst), calls);
    assert_eq!(below.calls.load(Ordering::SeqCst), 1);
}

#[test]
fn paragraph_stages_keep_partial_successes_and_roll_back_an_unreadable_replacement() {
    if notepipe::runtime::which("ffmpeg").is_none() || notepipe::runtime::which("ffprobe").is_none()
    {
        return;
    }
    for blank in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let video = root.path().join("broken.mp4");
        std::fs::write(&video, "broken media, readable subtitles").unwrap();
        let cue = |n| {
            format!(
                "{n}\n00:{:02}:{:02},000 --> 00:{:02}:{:02},000\nOriginal {n}.\n\n",
                n * 5 / 60,
                n * 5 % 60,
                (n * 5 + 1) / 60,
                (n * 5 + 1) % 60
            )
        };
        std::fs::write(
            video.with_extension("srt"),
            (0..21).map(cue).collect::<String>(),
        )
        .unwrap();
        let mock = MockAi::respond_with(move |_, body| {
            let payload = body["messages"][1]["content"][0]["text"].as_str().unwrap();
            let items: Vec<Value> = serde_json::from_str(payload).unwrap();
            if !blank && items.len() == 1 {
                return Some((400, json!({"error":{"message":"rejected"}})));
            }
            let segments = items.iter().map(|item| json!({"id":item["id"],"text":if blank { String::new() } else { format!("Processed {}", item["text"].as_str().unwrap()) }})).collect::<Vec<_>>();
            Some((
                200,
                json!({"choices":[{"message":{"content":json!({"segments":segments}).to_string()}}]}),
            ))
        });
        let mut cfg = config();
        cfg.llm.base_url = mock.url.clone();
        cfg.llm.model = "fixture".into();
        cfg.llm.concurrency = 1;
        cfg.llm
            .stages
            .push(notepipe::llm::Stage::builtin("proofread").unwrap());
        let output = run(root.path(), video.to_str().unwrap(), &cfg, &[]);
        let doc = document(&output);
        let paragraphs = doc["sections"][0]["speech"].as_array().unwrap();
        assert_eq!(paragraphs.len(), 21);
        assert_eq!(paragraphs[20]["text"], "Original 20.");
        if blank {
            assert_eq!(paragraphs[0]["text"], "Original 0.");
        } else {
            assert_eq!(paragraphs[0]["text"], "Processed Original 0.");
        }
        assert!(
            doc["warnings"]
                .as_array()
                .unwrap()
                .iter()
                .any(|warning| warning.as_str().unwrap().contains("proofread incomplete"))
        );
    }
}

#[test]
fn transcript_map_reduce_keeps_part_order_and_uses_the_fixed_merge_template() {
    let mock = MockAi::respond_with(|_, body| {
        assert!(body.get("temperature").is_none() && body.get("max_tokens").is_none());
        let user = body["messages"][1]["content"].as_str().unwrap();
        let content = if user.contains("Below are summaries of consecutive parts") {
            assert!(
                user.find("== Part 1 ==\nFirst part").unwrap()
                    < user.find("== Part 2 ==\nSecond part").unwrap()
            );
            assert!(user.ends_with("Merge them into one summary of the whole recording, following the same instructions."));
            "## Combined\n\nExact final Markdown."
        } else {
            assert!(user.starts_with("Title: Recording\nAuthor: Author\n\nTranscript (each line starts with its start time as [mm:ss]):\n\n"));
            if user.contains("[00:00]") {
                "First part"
            } else {
                "Second part"
            }
        };
        Some((200, json!({"choices":[{"message":{"content":content}}]})))
    });
    let stage = notepipe::llm::Stage::builtin("summary").unwrap();
    let settings = notepipe::llm::LlmSettings {
        base_url: mock.url.clone(),
        model: "fixture".into(),
        concurrency: 2,
        ..Default::default()
    };
    let events = (0..2)
        .map(|index| notepipe::timeline::TranscriptEvent {
            start: f64::from(index * 60),
            end: f64::from(index * 60 + 1),
            text: "x".repeat(15_000),
            raw: None,
            below: Vec::new(),
        })
        .collect::<Vec<_>>();
    let meta = notepipe::source::MediaMeta {
        title: "Recording".into(),
        author: "Author".into(),
        duration: 120.,
        source: "https://example.test/media".into(),
        seek_url: None,
    };
    let result =
        notepipe::runtime::block_on(notepipe::summarize::run(&settings, &stage, &events, &meta))
            .unwrap()
            .unwrap();
    assert_eq!(result, "## Combined\n\nExact final Markdown.");
    assert_eq!(mock.calls.load(std::sync::atomic::Ordering::SeqCst), 3);
}
