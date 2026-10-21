//! End-to-end stdin task protocol, using real ffmpeg and isolated local files only.
use notepipe::execution::Component;
use notepipe::{
    artifact,
    config::{SlideMode, TranscriptSource},
    execution::Request,
    settings::ConfigFile,
    timeline::TranscriptEvent,
};
use std::{
    io::Write,
    path::Path,
    process::{Command, Output, Stdio},
};

fn request(root: &Path, source: &Path) -> Request {
    let mut config = ConfigFile::default();
    config.defaults.formats = Some(vec![]);
    config.defaults.transcript_source = Some(TranscriptSource::Subtitle);
    config.defaults.slide_mode = Some(SlideMode::First);
    Request {
        schema: 1,
        operation: Default::default(),
        task_id: "task-one".into(),
        note_id: "note-one".into(),
        version_id: "version-one".into(),
        source: source.display().to_string(),
        source_id: format!(
            "local:sha256:{}",
            notepipe::execution::file_digest(source).unwrap()
        ),
        title: "我确认的笔记名称".into(),
        author: String::new(),
        duration: 1.,
        subtitle: None,
        subtitle_events: Some(vec![TranscriptEvent {
            start: 0.,
            end: 1.,
            text: "这里是已选字幕中的实际正文。".into(),
            raw: None,
            translation: None,
        }]),
        config,
        work_dir: root.join("work"),
        note_dir: root.join("note"),
    }
}
fn spawn(root: &Path, request: &Request) -> std::process::Child {
    let config = root.join("config/notepipe");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(config.join("config.toml"), "[broken").unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_notepipe"))
        .arg("run-task")
        .current_dir(root)
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("NOTEPIPE_ASR_API_KEY", "environment-key-must-not-be-used")
        .env("NOTEPIPE_LLM_API_KEY", "environment-key-must-not-be-used")
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
        .write_all(&serde_json::to_vec(request).unwrap())
        .unwrap();
    child
}

fn run(root: &Path, request: &Request) -> Output {
    spawn(root, request).wait_with_output().unwrap()
}
fn events(output: &Output) -> Vec<serde_json::Value> {
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
#[cfg(unix)]
fn scanning_finishes_before_a_slow_screenshot_reports_progress_with_estimated_total() {
    use std::{
        io::{BufRead, BufReader},
        os::unix::fs::PermissionsExt,
        sync::mpsc,
        time::{Duration, Instant},
    };
    let (Some(ffmpeg), Some(python)) = (
        notepipe::runtime::which("ffmpeg"),
        notepipe::runtime::which("python3"),
    ) else {
        return;
    };
    if notepipe::runtime::which("ffprobe").is_none() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let video = root.path().join("flat-video.mp4");
    let generated = Command::new(&ffmpeg)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=blue:s=64x32:r=1",
            "-t",
            "18",
            "-pix_fmt",
            "yuv420p",
        ])
        .arg(&video)
        .output()
        .unwrap();
    assert!(
        generated.status.success(),
        "{}",
        String::from_utf8_lossy(&generated.stderr)
    );
    let bin = root.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let gate = root.path().join("extract-started");
    let release = root.path().join("extract-release");
    let wrapper = bin.join("ffmpeg");
    let py_literal = |path: &Path| serde_json::to_string(path.to_str().unwrap()).unwrap();
    std::fs::write(&wrapper,format!("#!{}\nimport os,sys,time,pathlib\nif '-frames:v' in sys.argv:\n    pathlib.Path({}).write_text('started')\n    deadline=time.monotonic()+10\n    while not pathlib.Path({}).exists() and time.monotonic()<deadline:\n        time.sleep(0.01)\nos.execv({},[{},*sys.argv[1:]])\n",python.display(),py_literal(&gate),py_literal(&release),py_literal(&ffmpeg),py_literal(&ffmpeg))).unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut paths = vec![bin];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let mut request = request(root.path(), &video);
    request.duration = 18.;
    request.config.defaults.sample_interval = Some(1.);
    let mut child = Command::new(env!("CARGO_BIN_EXE_notepipe"))
        .arg("run-task")
        .env("PATH", std::env::join_paths(paths).unwrap())
        .env("XDG_CONFIG_HOME", root.path().join("config"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&request).unwrap())
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let value: serde_json::Value = serde_json::from_str(&line.unwrap()).unwrap();
            if tx.send(value).is_err() {
                break;
            }
        }
    });
    let mut observed = Vec::new();
    loop {
        let event = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("new screenshot progress must be emitted before ffmpeg finishes");
        let initial = event["type"] == "progress" && event["stage"] == "scenes/extract";
        if initial {
            assert_eq!(event["current"], 0);
            assert_eq!(event["total"], 1);
        }
        observed.push(event);
        if initial {
            break;
        }
    }
    assert!(
        observed
            .iter()
            .any(|event| event["stage"] == "scenes/scan" && event["status"] == "done")
    );
    let scan = observed
        .iter()
        .rev()
        .find(|event| event["stage"] == "scenes/scan" && event["type"] == "progress")
        .unwrap();
    assert_eq!(scan["current"], 18);
    assert_eq!(
        scan["total"], 18,
        "total is the duration/interval estimate; CFR fixtures match exactly"
    );
    let deadline = Instant::now() + Duration::from_secs(3);
    while !gate.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        gate.exists(),
        "real screenshot subprocess should now be blocked at the gate"
    );
    std::thread::sleep(Duration::from_millis(150));
    for event in rx.try_iter() {
        assert!(
            !(event["stage"] == "scenes/extract"
                && (event["current"] == 1 || event["status"] == "done")),
            "unfinished ffmpeg output must not count as a saved screenshot"
        );
        observed.push(event);
    }
    std::fs::write(&release, b"continue").unwrap();
    let output = child.wait_with_output().unwrap();
    reader.join().unwrap();
    observed.extend(rx.try_iter());
    assert!(
        output.status.success(),
        "{observed:#?}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        observed
            .iter()
            .any(|event| event["stage"] == "scenes/extract"
                && event["current"] == 1
                && event["total"] == 1)
    );
    assert!(
        observed
            .iter()
            .any(|event| event["stage"] == "scenes/extract" && event["status"] == "done")
    );
    assert!(!observed.iter().any(|event| event["stage"] == "scenes"));
}

#[test]
fn explicit_subtitles_zero_exports_and_confirmed_title_survive_real_conversion_and_resume() {
    if notepipe::runtime::which("ffmpeg").is_none() || notepipe::runtime::which("ffprobe").is_none()
    {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let video = root.path().join("different-original-name.mp4");
    let make = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=blue:s=320x240:r=1",
            "-t",
            "1",
            "-pix_fmt",
            "yuv420p",
        ])
        .arg(&video)
        .output()
        .unwrap();
    assert!(
        make.status.success(),
        "{}",
        String::from_utf8_lossy(&make.stderr)
    );
    let mut request = request(root.path(), &video);
    let first = run(root.path(), &request);
    let first_events = events(&first);
    assert!(first.status.success(), "{first_events:#?}");
    let done = first_events
        .iter()
        .find(|event| event["type"] == "done")
        .unwrap();
    assert_eq!(done["title"], request.title);
    assert_eq!(done["note_id"], request.note_id);
    assert_eq!(done["outputs"], serde_json::json!([]));
    assert!(
        !first_events
            .iter()
            .any(|event| event["stage"] == "transcribe" || event["stage"] == "audio")
    );
    let version = request.note_dir.join("versions/version-one");
    let manifest = artifact::read_manifest(&version.join("manifest.json")).unwrap();
    assert_eq!(manifest.note_id, request.note_id);
    let current: serde_json::Value =
        serde_json::from_slice(&std::fs::read(request.note_dir.join("current.json")).unwrap())
            .unwrap();
    assert_eq!(current["note_id"], request.note_id);
    assert!(!manifest.frames.is_empty());
    artifact::validate_version(&version, &manifest).unwrap();
    let original = std::fs::read(version.join("document.json")).unwrap();
    assert!(String::from_utf8_lossy(&original).contains("这里是已选字幕中的实际正文"));
    let resumed = run(root.path(), &request);
    assert!(resumed.status.success(), "{:?}", events(&resumed));
    assert_eq!(
        std::fs::read(version.join("document.json")).unwrap(),
        original
    );
    request.title = "后来在添加页填写的不同标题".into();
    assert!(!run(root.path(), &request).status.success());
    assert_eq!(
        std::fs::read(version.join("document.json")).unwrap(),
        original
    );
    let persisted = std::fs::read_to_string(request.work_dir.join("task-identity.json")).unwrap();
    assert!(!persisted.contains("environment-key-must-not-be-used"));
}

#[test]
fn broken_video_preserves_readable_subtitles_as_partial_note_without_phantom_images() {
    if notepipe::runtime::which("ffmpeg").is_none() || notepipe::runtime::which("ffprobe").is_none()
    {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let video = root.path().join("broken.mp4");
    std::fs::write(&video, b"broken media").unwrap();
    let request = request(root.path(), &video);
    let output = run(root.path(), &request);
    let observed = events(&output);
    assert!(output.status.success(), "{observed:#?}");
    let manifest =
        artifact::read_manifest(&request.note_dir.join("versions/version-one/manifest.json"))
            .unwrap();
    assert!(manifest.partial);
    assert!(manifest.frames.is_empty());
    assert_eq!(
        manifest.outcomes.screenshots.status,
        artifact::Status::Failed
    );
    let body =
        std::fs::read_to_string(request.note_dir.join("versions/version-one/note.md")).unwrap();
    assert!(body.contains("这里是已选字幕"));
    assert!(!body.contains("![]"));
    assert!(!body.contains("本段无语音"));
}

#[test]
fn missing_frozen_asr_destination_fails_before_download_and_ignores_environment() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("local.mp4");
    std::fs::write(&source, b"input").unwrap();
    let mut request = request(root.path(), &source);
    request.source = "https://example.invalid/video".into();
    request.source_id = "online:example:video".into();
    request.subtitle_events = None;
    request.config.defaults.transcript_source = Some(TranscriptSource::Asr);
    let output = run(root.path(), &request);
    let observed = events(&output);
    assert!(!output.status.success());
    assert!(!observed.iter().any(|event| event["stage"] == "download"));
    assert!(observed.iter().any(|event| {
        event["type"] == "error"
            && event["message"]
                .as_str()
                .unwrap_or_default()
                .contains("[asr]")
    }));
    assert!(!request.work_dir.join("media.mp4").exists());
    assert!(!request.note_dir.join("current.json").exists());
}

#[test]
fn changed_local_source_is_rejected_before_using_the_confirmed_subtitles() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("changed.mp4");
    std::fs::write(&source, b"confirmed content").unwrap();
    let request = request(root.path(), &source);
    std::fs::write(&source, b"replacement content").unwrap();
    let result = run(root.path(), &request);
    assert!(!result.status.success());
    let observed = events(&result);
    assert!(observed.iter().any(|event| {
        event["message"]
            .as_str()
            .is_some_and(|s| s.contains("Source content changed after confirmation"))
    }));
    assert!(!request.note_dir.join("current.json").exists());
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

fn summary_response() -> serde_json::Value {
    serde_json::json!({"choices":[{"message":{"content":serde_json::json!({"tldr":"这份摘要来自保留的正文。","key_points":["正文内容"],"outline":[{"t":0,"title":"开头","detail":"正文内容"}]}).to_string()}}]})
}

#[test]
fn cli_llm_keys_prefer_flags_then_config_then_environment_without_affecting_saved_tasks() {
    if notepipe::runtime::which("ffmpeg").is_none() || notepipe::runtime::which("ffprobe").is_none()
    {
        return;
    }
    let mock = MockAi::respond_with(|_, body| {
        Some((
            200,
            if body.to_string().contains("tldr") {
                summary_response()
            } else {
                serde_json::json!({"choices":[{"message":{"content":"{\"segments\":[{\"id\":0,\"text\":\"Proofread.\"}]}"}}]})
            },
        ))
    });
    let root = tempfile::tempdir().unwrap();
    let video = root.path().join("lecture.mp4");
    let generated = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=blue:s=64x32:r=1",
            "-t",
            "1",
            "-pix_fmt",
            "yuv420p",
        ])
        .arg(&video)
        .output()
        .unwrap();
    assert!(generated.status.success());
    std::fs::write(
        video.with_extension("srt"),
        "1\n00:00:00,000 --> 00:00:01,000\nOriginal.\n",
    )
    .unwrap();
    for (index, (saved, environment, flag, expected)) in [
        (
            "",
            Some("environment-test-key"),
            None,
            Some("environment-test-key"),
        ),
        (
            "  ",
            Some("environment-test-key"),
            None,
            Some("environment-test-key"),
        ),
        (
            "saved-test-key",
            Some("environment-test-key"),
            None,
            Some("saved-test-key"),
        ),
        (
            "saved-test-key",
            Some("environment-test-key"),
            Some("cli-test-key"),
            Some("cli-test-key"),
        ),
        ("", None, None, None),
        ("", Some("  "), None, None),
    ]
    .into_iter()
    .enumerate()
    {
        let case = root.path().join(index.to_string());
        let config = case.join("config/notepipe/config.toml");
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        let contents = format!(
            "[llm]\nenabled = true\nbase_url = {:?}\nmodel = 'mock'\napi_key = {saved:?}\n",
            mock.url
        );
        std::fs::write(&config, &contents).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_notepipe"));
        command
            .arg(&video)
            .args([
                "--json",
                "--formats",
                "md",
                "--slide-mode",
                "first",
                "--transcript-source",
                "subtitle",
                "-o",
            ])
            .arg(case.join("notes"))
            .env("XDG_CONFIG_HOME", case.join("config"))
            .env("XDG_CACHE_HOME", case.join("cache"))
            .env_remove("RUST_LOG")
            .env_remove("NOTEPIPE_LLM_API_KEY")
            .stdin(Stdio::null());
        if let Some(value) = environment {
            command.env("NOTEPIPE_LLM_API_KEY", value);
        }
        if let Some(value) = flag {
            command.args(["--llm-api-key", value]);
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        let headers = mock.headers.lock().unwrap().last().unwrap().clone();
        let auth = headers.lines().find_map(|line| {
            line.split_once(':')
                .filter(|(name, _)| name.eq_ignore_ascii_case("authorization"))
                .map(|(_, value)| value.trim().to_owned())
        });
        assert_eq!(auth, expected.map(|key| format!("Bearer {key}")));
        assert_eq!(std::fs::read_to_string(&config).unwrap(), contents);
        for bytes in [&output.stdout, &output.stderr] {
            assert!(!String::from_utf8_lossy(bytes).contains("test-key"));
        }
        if index == 0 {
            let done = events(&output)
                .into_iter()
                .find(|event| event["type"] == "done")
                .unwrap();
            let summary = Command::new(env!("CARGO_BIN_EXE_notepipe"))
                .args(["summarize", done["out_dir"].as_str().unwrap()])
                .env("XDG_CONFIG_HOME", case.join("config"))
                .env("XDG_CACHE_HOME", case.join("cache"))
                .env("NOTEPIPE_LLM_API_KEY", "environment-test-key")
                .stdin(Stdio::null())
                .output()
                .unwrap();
            assert!(
                summary.status.success(),
                "{}",
                String::from_utf8_lossy(&summary.stderr)
            );
            assert!(
                mock.headers
                    .lock()
                    .unwrap()
                    .last()
                    .unwrap()
                    .contains("Bearer environment-test-key")
            );
        }
    }
    let mut frozen = request(&root.path().join("frozen"), &video);
    frozen.config.llm.enabled = true;
    frozen.config.llm.base_url = mock.url.clone();
    frozen.config.llm.model = "mock".into();
    let output = run(root.path(), &frozen);
    assert!(output.status.success(), "{:?}", events(&output));
    assert!(
        !mock
            .headers
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .to_ascii_lowercase()
            .contains("authorization:")
    );
}

#[test]
fn proofreading_images_and_custom_rules_match_the_requested_outputs() {
    if notepipe::runtime::which("ffmpeg").is_none() || notepipe::runtime::which("ffprobe").is_none()
    {
        return;
    }
    let mock = MockAi::respond_with(|_, body| {
        if body.to_string().contains("tldr") {
            return Some((200, summary_response()));
        }
        let user = &body["messages"][1]["content"];
        let text = user.as_str().map(str::to_owned).unwrap_or_else(|| {
            user.as_array()
                .unwrap()
                .iter()
                .filter_map(|part| part["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n")
        });
        let segments: serde_json::Value = serde_json::from_str(&text).unwrap();
        Some((
            200,
            serde_json::json!({"choices":[{"message":{"content":serde_json::json!({"segments":segments}).to_string()}}]}),
        ))
    });
    let root = tempfile::tempdir().unwrap();
    let video = root.path().join("slides.mp4");
    let generated = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=blue:s=320x240:r=1",
            "-t",
            "1",
            "-pix_fmt",
            "yuv420p",
        ])
        .arg(&video)
        .output()
        .unwrap();
    assert!(generated.status.success());
    let mut initial = request(root.path(), &video);
    initial.config.llm.enabled = true;
    initial.config.llm.summarize = true;
    initial.config.llm.vision = true;
    initial.config.llm.prompt = Some("UX-RULE-42: preserve terminology.".into());
    initial.config.llm.base_url = mock.url.clone();
    initial.config.llm.model = "proofread-vision".into();
    initial.config.llm.api_key = "private-task-key".into();
    initial.config.defaults.formats = Some(vec![notepipe::config::OutputFormat::Json]);
    let output = run(root.path(), &initial);
    assert!(output.status.success(), "{:?}", events(&output));
    let manifest =
        artifact::read_manifest(&initial.note_dir.join("versions/version-one/manifest.json"))
            .unwrap();
    assert!(!manifest.partial, "{manifest:?}");
    assert_eq!(
        manifest.outcomes.proofreading.status,
        artifact::Status::Succeeded
    );
    assert_eq!(
        manifest.outcomes.summary.status,
        artifact::Status::Succeeded
    );
    assert_eq!(manifest.outputs, vec!["exports/structured.json"]);
    assert!(!manifest.frames.is_empty());
    let bodies = mock.bodies.lock().unwrap();
    assert_eq!(bodies.len(), 2);
    assert!(bodies[0].to_string().contains("UX-RULE-42"));
    assert!(bodies[0].to_string().contains("image_url"));
    assert!(!bodies[1].to_string().contains("image_url"));
    assert!(bodies.iter().all(
        |body| body["model"] == "proofread-vision" && !body.to_string().contains("input_audio")
    ));
}

#[test]
fn summary_only_reprocessing_retries_known_failure_once_without_repeating_source_work() {
    use std::sync::atomic::Ordering;
    if notepipe::runtime::which("ffmpeg").is_none() || notepipe::runtime::which("ffprobe").is_none()
    {
        return;
    }
    let mock = MockAi::respond_with(|number, _| {
        if number == 0 {
            // An explicit response_format rejection sanctions exactly one relaxed resend.
            Some((
                422,
                serde_json::json!({"error":{"param":"response_format","code":"unsupported_parameter","message":"Unsupported response_format; private-task-key"}}),
            ))
        } else if number < 3 {
            Some((
                400,
                serde_json::json!({"error":{"message":"Invalid model; private-task-key must not be copied"}}),
            ))
        } else {
            Some((200, summary_response()))
        }
    });
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("video.mp4");
    std::fs::write(&source, b"broken video, readable selected subtitles").unwrap();
    let mut initial = request(root.path(), &source);
    initial.config.defaults.formats = Some(vec![notepipe::config::OutputFormat::Html]);
    initial.config.llm.enabled = false;
    initial.config.llm.summarize = true;
    initial.config.llm.base_url = mock.url.clone();
    initial.config.llm.model = "summary-model".into();
    initial.config.llm.api_key = "private-task-key".into();
    let first = run(root.path(), &initial);
    assert!(first.status.success(), "{:?}", events(&first));
    assert_eq!(
        mock.calls.load(Ordering::SeqCst),
        2,
        "an explicit response_format rejection allows one resend without it; the following arbitrary HTTP 400 must not change and resend the payload"
    );
    let base = initial.note_dir.join("versions/version-one");
    let first_manifest = artifact::read_manifest(&base.join("manifest.json")).unwrap();
    assert_eq!(
        first_manifest.outcomes.summary.status,
        artifact::Status::Failed
    );
    assert_eq!(
        first_manifest.outcomes.exports["html"].status,
        artifact::Status::Succeeded
    );
    let reason = first_manifest.outcomes.summary.message.as_deref().unwrap();
    assert!(reason.contains("HTTP 400"));
    assert!(!reason.contains(" / ") && !reason.contains("private-task-key"));
    std::fs::write(
        base.join("note.md"),
        "# 我的人工补充\n\n旧版本不能被补摘要覆盖。\n",
    )
    .unwrap();
    let original_files = std::fs::read_dir(&base)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|p| p.is_file())
        .map(|p| {
            let bytes = std::fs::read(&p).unwrap();
            (p, bytes)
        })
        .collect::<Vec<_>>();
    std::fs::remove_file(&source).unwrap();
    let mut retry = initial.clone();
    retry.task_id = "summary-retry".into();
    retry.version_id = "summary-retry".into();
    retry.work_dir = root.path().join("retry-work");
    retry.operation = notepipe::execution::Operation::Reprocess {
        base_version_dir: base.clone(),
        components: vec![Component::Summary],
    };
    let second = run(root.path(), &retry);
    let second_events = events(&second);
    assert!(second.status.success(), "{second_events:#?}");
    assert_eq!(
        mock.calls.load(Ordering::SeqCst),
        3,
        "the selected known-failed summary must be attempted again"
    );
    assert!(
        second_events
            .iter()
            .any(|event| event["type"] == "stage" && event["stage"] == "summary")
    );
    assert!(!second_events.iter().any(|event| {
        ["audio", "transcribe", "llm", "download", "scenes"]
            .iter()
            .any(|stage| event["stage"] == *stage)
            || event["stage"]
                .as_str()
                .is_some_and(|stage| stage.starts_with("scenes/"))
    }));
    let resumed = run(root.path(), &retry);
    assert!(resumed.status.success(), "{:?}", events(&resumed));
    assert_eq!(
        mock.calls.load(Ordering::SeqCst),
        3,
        "normal continuation must not renew an explicit retry"
    );
    let mut final_retry = retry.clone();
    final_retry.task_id = "summary-final".into();
    final_retry.version_id = "summary-final".into();
    final_retry.work_dir = root.path().join("final-work");
    final_retry.operation = notepipe::execution::Operation::Reprocess {
        base_version_dir: initial.note_dir.join("versions/summary-retry"),
        components: vec![Component::Summary],
    };
    let third = run(root.path(), &final_retry);
    assert!(third.status.success(), "{:?}", events(&third));
    assert_eq!(mock.calls.load(Ordering::SeqCst), 4);
    let final_dir = initial.note_dir.join("versions/summary-final");
    let manifest = artifact::read_manifest(&final_dir.join("manifest.json")).unwrap();
    assert_eq!(
        manifest.outcomes.summary.status,
        artifact::Status::Succeeded
    );
    let document: artifact::Document =
        serde_json::from_slice(&std::fs::read(final_dir.join("document.json")).unwrap()).unwrap();
    assert_eq!(document.summary.unwrap().tldr, "这份摘要来自保留的正文。");
    for (path, bytes) in original_files {
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
    let bodies = mock.bodies.lock().unwrap();
    // The single sanctioned resend drops response_format and changes nothing else.
    assert!(bodies[0].get("response_format").is_some());
    assert!(bodies[1].get("response_format").is_none());
    let mut relaxed = bodies[0].clone();
    relaxed.as_object_mut().unwrap().remove("response_format");
    assert_eq!(bodies[1], relaxed);
    // Later tasks resend the full payload, byte-identical to the first request.
    assert_eq!(bodies[0], bodies[2]);
    assert_eq!(bodies[2], bodies[3]);
}

#[test]
fn export_only_recovery_uses_the_saved_version_after_source_and_subtitle_are_gone() {
    use notepipe::config::OutputFormat;
    if notepipe::runtime::which("ffmpeg").is_none() || notepipe::runtime::which("ffprobe").is_none()
    {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("video.mp4");
    std::fs::write(&source, b"broken").unwrap();
    let original = request(root.path(), &source);
    assert!(run(root.path(), &original).status.success());
    let base = original.note_dir.join("versions/version-one");
    let pointer = std::fs::read(original.note_dir.join("current.json")).unwrap();
    let mut export = original.clone();
    export.task_id = "exports".into();
    export.version_id = "unused-new-body".into();
    export.work_dir = root.path().join("export-work");
    export.subtitle_events = None;
    export.subtitle = Some(root.path().join("deleted-subtitles.srt"));
    export.operation = notepipe::execution::Operation::Reprocess {
        base_version_dir: base,
        components: vec![Component::Exports],
    };
    export.config.defaults.formats = Some(vec![
        OutputFormat::Md,
        OutputFormat::Html,
        OutputFormat::Json,
    ]);
    for attempt in 0..2 {
        if attempt == 1 {
            std::fs::remove_file(&source).unwrap();
        }
        let result = run(root.path(), &export);
        let events = events(&result);
        assert!(result.status.success(), "{events:#?}");
        let done = events.iter().find(|event| event["type"] == "done").unwrap();
        assert_eq!(done["partial"], false);
        assert_eq!(done["outputs"].as_array().unwrap().len(), 3);
    }
    assert_eq!(
        std::fs::read(original.note_dir.join("current.json")).unwrap(),
        pointer
    );
    assert!(!original.note_dir.join("versions/unused-new-body").exists());
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

#[test]
fn keyless_task_transcribes_over_http_without_reading_ambient_credentials() {
    let root = tempfile::tempdir().unwrap();
    let Some(video) = speech_video(root.path()) else {
        return;
    };
    let record = root.path().join("adapter");
    let Some(command) = adapter_command(&record, "healthy") else {
        return;
    };
    let adapter = notepipe::adapter::Adapter::start(&command).unwrap();
    let mut request = request(root.path(), &video);
    request.subtitle_events = None;
    request.duration = 2.4;
    request.config.defaults.transcript_source = Some(TranscriptSource::Asr);
    request.config.asr.base_url = format!("{}/v1", adapter.base);
    let output = run(root.path(), &request);
    let observed = events(&output);
    assert!(output.status.success(), "{observed:#?}");
    assert_eq!(
        std::fs::read_to_string(record.join("authorization")).unwrap(),
        ""
    );
    assert_eq!(
        std::fs::read_to_string(record.join("endpoint")).unwrap(),
        "/v1/audio/transcriptions"
    );
    let doc: serde_json::Value = serde_json::from_slice(
        &std::fs::read(request.note_dir.join("versions/version-one/document.json")).unwrap(),
    )
    .unwrap();
    assert!(doc.to_string().contains("Fixture transcript."));
    assert!(!doc.to_string().contains("<asr_text>"));
}

#[test]
fn managed_adapter_uses_http_and_stops_after_success_or_error() {
    for behavior in ["healthy", "error"] {
        let root = tempfile::tempdir().unwrap();
        let Some(video) = speech_video(root.path()) else {
            return;
        };
        let record = root.path().join("adapter");
        let Some(command) = adapter_command(&record, behavior) else {
            return;
        };
        let mut request = request(root.path(), &video);
        request.subtitle_events = None;
        request.duration = 2.4;
        request.config.defaults.transcript_source = Some(TranscriptSource::Asr);
        request.config.asr.command = command;
        request.config.asr.base_url = "ignored invalid remote URL".into();
        request.config.asr.language = "ko".into();
        let output = run(root.path(), &request);
        let observed = events(&output);
        assert_eq!(
            output.status.success(),
            behavior == "healthy",
            "{observed:#?}"
        );
        if behavior == "healthy" {
            let manifest = artifact::read_manifest(
                &request.note_dir.join("versions/version-one/manifest.json"),
            )
            .unwrap();
            assert_eq!(
                manifest.outcomes.transcript.status,
                artifact::Status::Succeeded
            );
        } else {
            assert!(observed.iter().any(|event| {
                event["type"] == "error"
                    && event["message"]
                        .as_str()
                        .unwrap_or_default()
                        .contains("adapter stderr")
            }));
            assert!(!request.work_dir.join(".asr_done").exists());
        }
        let body = std::fs::read(record.join("request")).unwrap();
        assert!(String::from_utf8_lossy(&body).contains("name=\"language\"\r\n\r\nko"));
        #[cfg(unix)]
        assert!(record.join("terminated").exists());
    }
}

#[test]
fn asr_concurrency_bounds_the_number_of_http_requests_in_flight() {
    for concurrency in [1, 3] {
        let root = tempfile::tempdir().unwrap();
        let Some(video) = speech_video(root.path()) else {
            return;
        };
        let record = root.path().join("adapter");
        let Some(command) = adapter_command(&record, "parallel") else {
            return;
        };
        let adapter = notepipe::adapter::Adapter::start(&command).unwrap();
        let mut request = request(root.path(), &video);
        request.subtitle_events = None;
        request.config.defaults.transcript_source = Some(TranscriptSource::Asr);
        request.config.asr.base_url = format!("{}/v1", adapter.base);
        request.config.asr.concurrency = concurrency;
        request.config.asr.max_speech = 1.0;
        let output = run(root.path(), &request);
        assert!(output.status.success(), "{:#?}", events(&output));
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
fn retryable_llm_failures_are_retried_without_persisting_error_bodies() {
    use std::sync::atomic::Ordering;
    if notepipe::runtime::which("ffmpeg").is_none() || notepipe::runtime::which("ffprobe").is_none()
    {
        return;
    }
    let mock = MockAi::respond_with(|number, _| {
        Some(if number == 0 {
            (
                503,
                serde_json::json!({"error":{"message":"private-test-key"}}),
            )
        } else {
            (
                200,
                serde_json::json!({"choices":[{"message":{"content":r#"{"segments":[{"id":0,"text":"Cached transcript."}]}"#}}]}),
            )
        })
    });
    let root = tempfile::tempdir().unwrap();
    let media = root.path().join("broken.mp4");
    std::fs::write(&media, b"broken media; selected subtitles remain usable").unwrap();
    let mut request = request(root.path(), &media);
    request.config.llm.enabled = true;
    request.config.llm.base_url = mock.url.clone();
    request.config.llm.model = "fixture".into();
    request.config.llm.api_key = "private-test-key".into();
    let output = run(root.path(), &request);
    assert!(output.status.success(), "{:?}", events(&output));
    assert_eq!(mock.calls.load(Ordering::SeqCst), 2);
    let files = std::fs::read_dir(request.work_dir.join("llm"))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(files.len(), 1);
    let saved = std::fs::read_to_string(files[0].path()).unwrap();
    assert!(saved.contains("Cached transcript."));
    assert!(!saved.contains("private-test-key"));
    assert!(!request.work_dir.join("requests").exists());
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

#[cfg(unix)]
#[test]
fn sigint_and_sigterm_stop_a_running_asr_adapter() {
    for signal in [libc::SIGINT, libc::SIGTERM] {
        let root = tempfile::tempdir().unwrap();
        let Some(video) = speech_video(root.path()) else {
            return;
        };
        let record = root.path().join("adapter");
        let Some(command) = adapter_command(&record, "hang") else {
            return;
        };
        let mut request = request(root.path(), &video);
        request.subtitle_events = None;
        request.config.defaults.transcript_source = Some(TranscriptSource::Asr);
        request.config.asr.command = command;
        let mut child = spawn(root.path(), &request);
        wait_for_file(&mut child, &record.join("transcribing"));
        assert_eq!(unsafe { libc::kill(child.id() as i32, signal) }, 0);
        let output = wait_for_cancelled(child);
        let observed = events(&output);
        assert!(!observed.iter().any(|event| event["type"] == "done"
            || event["type"] == "blocked"
            || event["type"] == "workers"));
        assert!(record.join("terminated").is_file());
        assert!(!request.work_dir.join(".asr_done").exists());
    }
}

#[cfg(unix)]
#[test]
fn a_second_signal_exits_without_waiting_for_adapter_shutdown() {
    use std::time::{Duration, Instant};
    let Some(python) =
        notepipe::runtime::which("python3").or_else(|| notepipe::runtime::which("python"))
    else {
        return;
    };
    let root = tempfile::tempdir().unwrap();
    let media = root.path().join("placeholder");
    std::fs::write(&media, b"placeholder").unwrap();
    let record = root.path().join("source");
    let mut request = request(root.path(), &media);
    request.source = "https://example.test/media".into();
    request.source_id = "online:fixture".into();
    request.subtitle_events = None;
    request.config.source.command = vec![
        python.display().to_string(),
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/source_adapter.py"
        )
        .into(),
        "--record".into(),
        record.display().to_string(),
        "--behavior".into(),
        "hang".into(),
    ];
    let mut child = spawn(root.path(), &request);
    wait_for_file(&mut child, &record.join("child-ready"));
    let adapter_pid: i32 = std::fs::read_to_string(record.join("pid"))
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGINT) }, 0);
    wait_for_file(&mut child, &record.join("child-stopped"));
    let start = Instant::now();
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGTERM) }, 0);
    while child.try_wait().unwrap().is_none() && start.elapsed() < Duration::from_secs(3) {
        std::thread::sleep(Duration::from_millis(10));
    }
    let elapsed = start.elapsed();
    // Immediate exit intentionally skips Drop; the test owns the remaining fixture group.
    unsafe {
        libc::kill(-adapter_pid, libc::SIGKILL);
    }
    let output = wait_for_cancelled(child);
    assert!(
        elapsed < Duration::from_secs(1),
        "second signal took {elapsed:?}"
    );
    assert!(output.stderr.is_empty());
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
    let record = root.path().join("adapter");
    let Some(command) = adapter_command(&record, "healthy") else {
        return;
    };
    let adapter = notepipe::adapter::Adapter::start(&command).unwrap();
    let mut request = request(root.path(), &video);
    request.subtitle_events = None;
    request.config.defaults.transcript_source = Some(TranscriptSource::Asr);
    request.config.asr.base_url = format!("{}/v1", adapter.base);
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
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_notepipe"))
        .arg("run-task")
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
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&request).unwrap())
        .unwrap();
    wait_for_file(&mut child, &root.path().join("cutting"));
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGTERM) }, 0);
    let output = wait_for_cancelled(child);
    assert!(!events(&output).iter().any(|event| event["type"] == "done"));
    assert!(
        !record.join("request").exists(),
        "HTTP was sent after cancellation"
    );
    assert!(!request.work_dir.join(".asr_done").exists());
}
