//! File pipeline contracts with offline providers and programs.
mod support;
use serde_json::{Value, json};
use std::{
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};
use support::MockAi;

fn config() -> Value {
    json!({"pipeline":["download","transcript","screenshots","draft","markdown"],"providers":{},"stages":{
        "download":{"type":"download","input":"source","output":"media","command":["unused"]},
        "transcript":{"type":"transcript","input":"media","output":"transcript.json","sources":["subtitles"]},
        "screenshots":{"type":"screenshots","input":"media","output":"images","mode":"first"},
        "draft":{"type":"document","input":["transcript.json","images"],"output":"draft.json","publish":"note.json"},
        "markdown":{"type":"markdown","input":"draft.json","output":"note.md","publish":"note.md"}
    }})
}
fn local(root: &Path) -> PathBuf {
    let source = root.join("lecture.mp4");
    std::fs::write(&source, "media").unwrap();
    std::fs::write(
        source.with_extension("srt"),
        "1\n00:00:00,000 --> 00:00:01,000\nOriginal speech.\n",
    )
    .unwrap();
    source
}
fn video(root: &Path) -> PathBuf {
    let source = root.join("lecture.mp4");
    let output = Command::new("ffmpeg")
        .args([
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "color=c=blue:s=64x32:r=10",
        ])
        .arg("-i")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/speech.wav"
        ))
        .args(["-t", "2.4", "-pix_fmt", "yuv420p", "-c:a", "aac"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    source
}
fn python() -> String {
    which::which("python3")
        .or_else(|_| which::which("python"))
        .unwrap()
        .display()
        .to_string()
}
fn program(mode: &str) -> Value {
    json!([
        python(),
        concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/run.py"),
        mode
    ])
}
fn spawn(
    root: &Path,
    source: &str,
    config: &Value,
    flags: &[&str],
    json_mode: bool,
) -> std::process::Child {
    let mut command = Command::new(env!("CARGO_BIN_EXE_notepipe"));
    command
        .arg(source)
        .args(["--config", "-"])
        .args(flags)
        .current_dir(root)
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env_remove("RUST_LOG")
        .env_remove("OPENAI_API_KEY")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if json_mode {
        command.arg("--json");
    }
    let mut child = command.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(config).unwrap())
        .unwrap();
    child
}
fn run(root: &Path, source: &Path, config: &Value, flags: &[&str]) -> Output {
    spawn(root, source.to_str().unwrap(), config, flags, true)
        .wait_with_output()
        .unwrap()
}
fn events(output: &Output) -> Vec<Value> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}
fn done(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let done = events(output).last().unwrap().clone();
    assert_eq!(done["type"], "done");
    assert_eq!(done.as_object().unwrap().len(), 5);
    assert!(done["work_dir"].as_str().is_some());
    done
}
fn error(output: &Output) -> String {
    assert!(!output.status.success());
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let all = events(output);
    assert!(!all.iter().any(|e| e["type"] == "done"));
    all.last().unwrap()["message"].as_str().unwrap().to_owned()
}
fn document(done: &Value, name: &str) -> Value {
    serde_json::from_slice(&std::fs::read(done["files"][name].as_str().unwrap()).unwrap()).unwrap()
}
fn mock() -> MockAi {
    MockAi::respond_with(|_, body| {
        let content = if body.get("response_format").is_some() {
            let items: Value =
                serde_json::from_str(body["messages"][1]["content"][0]["text"].as_str().unwrap())
                    .unwrap();
            json!({"segments":items.as_array().unwrap().iter().map(|item|json!({"id":item["id"],"text":"Corrected speech."})).collect::<Vec<_>>()}).to_string()
        } else {
            "## Summary\n\nA recording.".into()
        };
        Some((
            200,
            json!({"choices":[{"message":{"content":content}}],"usage":{"prompt_tokens":3,"completion_tokens":2}}),
        ))
    })
}
fn with_llm(cfg: &mut Value, base: &str) {
    cfg["providers"] = json!({"ai":{"base_url":base}});
    cfg["stages"]["draft"]
        .as_object_mut()
        .unwrap()
        .remove("publish");
    cfg["pipeline"] = json!([
        "download",
        "transcript",
        "screenshots",
        "draft",
        "proofread",
        "summary",
        "markdown"
    ]);
    cfg["stages"]["proofread"] = json!({"type":"llm","input":"draft.json","output":"proofread.json","publish":"note.json","provider":"ai","prompt":"PROOFREAD"});
    cfg["stages"]["summary"] = json!({"type":"llm","input":"proofread.json","output":"summary.md","provider":"ai","prompt":"SUMMARY"});
    cfg["stages"]["markdown"]["input"] = json!(["summary.md", "proofread.json"]);
}
fn tree(root: &Path) -> String {
    fn walk(root: &Path, path: &Path, lines: &mut Vec<String>) {
        let mut entries = std::fs::read_dir(path)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect::<Vec<_>>();
        entries.sort();
        for entry in entries {
            lines.push(format!(
                "{}{}",
                entry.strip_prefix(root).unwrap().display(),
                if entry.is_dir() { "/" } else { "" }
            ));
            if entry.is_dir() {
                walk(root, &entry, lines);
            }
        }
    }
    let mut lines = Vec::new();
    walk(root, root, &mut lines);
    lines.join("\n")
}

#[test]
fn default_template_publishes_and_keeps_every_named_work_output() {
    let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("target");
    let temporary = tempfile::Builder::new()
        .prefix("default-template-")
        .tempdir_in(&parent)
        .unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let source = video(&root);
    std::fs::write(
        source.with_extension("srt"),
        "1\n00:00:00,000 --> 00:00:02,000\nOriginal speech.\n",
    )
    .unwrap();
    let provider = mock();
    let mut cfg = serde_json::to_value(
        toml::from_str::<toml::Value>(include_str!("../src/config.toml")).unwrap(),
    )
    .unwrap();
    assert!(cfg["stages"]["download"].get("command").is_none());
    cfg["out"] = json!(root.join("out"));
    cfg["providers"]["openai"]["base_url"] = json!(provider.url);
    cfg["providers"]["openai"]["api_key_env"] = json!("");
    let output = run(&root, &source, &cfg, &["-v"]);
    let completed = done(&output);
    let note = Path::new(completed["note_dir"].as_str().unwrap());
    let work = Path::new(completed["work_dir"].as_str().unwrap());
    assert_eq!(note, root.join("out/lecture"));
    for path in ["note.md", "note.json", "images/0001.jpg", ".notepipe.json"] {
        assert!(note.join(path).is_file(), "{path}");
    }
    for path in [
        "media/info.json",
        "transcript.json",
        "images/images.json",
        "draft.json",
        "proofread.json",
        "summary.md",
        "note.md",
    ] {
        assert!(work.join(path).is_file(), "{path}");
    }
    let doc = document(&completed, "proofread");
    assert_eq!(doc["schema"], 3);
    assert!(doc.get("top").is_none());
    assert!(doc.get("warnings").is_none());
    assert_eq!(doc["sections"][0]["speech"][0]["raw"], "Original speech.");
    let info: Value =
        serde_json::from_slice(&std::fs::read(work.join("media/info.json")).unwrap()).unwrap();
    assert_eq!(info["media"], source.to_str().unwrap());
    assert_eq!(
        info["subtitles"][0]["path"],
        source.with_extension("srt").to_str().unwrap()
    );
    assert!(
        events(&output)
            .iter()
            .any(|e| e["message"] == format!("Work folder: {}", work.display()))
    );
    let md = std::fs::read_to_string(note.join("note.md")).unwrap();
    assert_eq!(
        md,
        "## Summary\n\nA recording.\n\n![](<images/0001.jpg>)\n\nCorrected speech.\n"
    );
    let first_calls = provider.calls.load(std::sync::atomic::Ordering::SeqCst);
    let second = done(&run(&root, &source, &cfg, &[]));
    assert_eq!(second["note_dir"], completed["note_dir"]);
    assert_eq!(
        provider.calls.load(std::sync::atomic::Ordering::SeqCst),
        first_calls
    );
    assert!(source.is_file());
}
#[test]
fn skipped_proofread_is_published_verbatim_and_skipped_summary_is_omitted() {
    let root = tempfile::tempdir().unwrap();
    let source = local(root.path());
    let provider = mock();
    let mut cfg = config();
    with_llm(&mut cfg, &provider.url);
    let output = run(
        root.path(),
        &source,
        &cfg,
        &["--skip", "screenshots,proofread,summary"],
    );
    let completed = done(&output);
    for skipped in ["screenshots", "proofread", "summary"] {
        assert!(!events(&output).iter().any(|event| {
            event["stage"]
                .as_str()
                .is_some_and(|name| name == skipped || name.starts_with(&format!("{skipped}/")))
        }));
    }
    let human = spawn(
        root.path(),
        source.to_str().unwrap(),
        &cfg,
        &["--skip", "screenshots,proofread,summary"],
        false,
    )
    .wait_with_output()
    .unwrap();
    assert!(human.status.success());
    let stderr = String::from_utf8_lossy(&human.stderr);
    for skipped in ["screenshots", "proofread", "summary"] {
        assert!(!stderr.contains(&format!("Running {skipped}…")), "{stderr}");
    }
    let work = Path::new(completed["work_dir"].as_str().unwrap());
    assert_eq!(
        std::fs::read(work.join("draft.json")).unwrap(),
        std::fs::read(completed["files"]["proofread"].as_str().unwrap()).unwrap()
    );
    assert_eq!(
        std::fs::read_to_string(completed["files"]["markdown"].as_str().unwrap()).unwrap(),
        "Original speech.\n"
    );
    assert_eq!(provider.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    let completed = done(&run(
        root.path(),
        &source,
        &cfg,
        &["--skip", "screenshots,summary"],
    ));
    assert_eq!(
        std::fs::read_to_string(completed["files"]["markdown"].as_str().unwrap()).unwrap(),
        "Corrected speech.\n"
    );
    assert!(
        !completed["files"]
            .as_object()
            .unwrap()
            .contains_key("summary")
    );
}
#[test]
fn summaries_read_the_selected_document_transcript_or_markdown() {
    let root = tempfile::tempdir().unwrap();
    let source = local(root.path());
    let provider = mock();
    let mut cfg = config();
    with_llm(&mut cfg, &provider.url);
    done(&run(root.path(), &source, &cfg, &["--skip", "screenshots"]));
    cfg["stages"]["summary"]["input"] = json!("transcript.json");
    done(&run(root.path(), &source, &cfg, &["--skip", "screenshots"]));
    cfg["pipeline"]
        .as_array_mut()
        .unwrap()
        .insert(5, json!("existing"));
    let mut command = program("text");
    command
        .as_array_mut()
        .unwrap()
        .push(json!(" \nExisting **Markdown**.\r\n"));
    cfg["stages"]["existing"] =
        json!({"type":"run","output":"existing.md","produces":"markdown","command":command});
    cfg["stages"]["summary"]["input"] = json!("existing.md");
    done(&run(root.path(), &source, &cfg, &["--skip", "screenshots"]));
    let bodies = provider.bodies.lock().unwrap();
    let users = bodies
        .iter()
        .filter(|b| b.get("response_format").is_none())
        .map(|b| b["messages"][1]["content"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(users[0].starts_with("Title: lecture\n\nTranscript"));
    assert!(users[0].contains("[00:00] Corrected speech."));
    assert!(users[1].contains("[00:00] Original speech."));
    assert_eq!(users[2], " \nExisting **Markdown**.\r\n");
}
#[test]
fn llm_errors_stop_publication_and_keep_the_previous_note() {
    for mode in ["invalid", "ids", "truncated", "empty", "retry", "summary"] {
        let root = tempfile::tempdir().unwrap();
        let source = local(root.path());
        let original = done(&run(
            root.path(),
            &source,
            &config(),
            &["--skip", "screenshots"],
        ));
        let path = Path::new(original["files"]["draft"].as_str().unwrap());
        let previous = std::fs::read(path).unwrap();
        let provider = MockAi::respond_with(move |_, body| {
            if mode == "retry" {
                return Some((500, json!({})));
            }
            let content = match mode {
                "invalid" => "invalid".into(),
                "ids" => json!({"segments":[{"id":99,"text":"bad"}]}).to_string(),
                "empty" => json!({"segments":[{"id":0,"text":""}]}).to_string(),
                _ => json!({"segments":[{"id":0,"text":"Corrected"}]}).to_string(),
            };
            if mode == "summary" && body.get("response_format").is_none() {
                return Some((400, json!({})));
            }
            Some((
                200,
                json!({"choices":[{"finish_reason":if mode=="truncated"{"length"}else{"stop"},"message":{"content":content}}]}),
            ))
        });
        let mut cfg = config();
        with_llm(&mut cfg, &provider.url);
        let message = error(&run(root.path(), &source, &cfg, &["--skip", "screenshots"]));
        if mode == "empty" {
            assert_eq!(message, "Stage proofread would remove every paragraph");
        }
        if mode == "retry" {
            assert_eq!(provider.calls.load(std::sync::atomic::Ordering::SeqCst), 3);
        }
        assert_eq!(std::fs::read(path).unwrap(), previous, "{mode}: {message}");
    }
}
fn with_run(cfg: &mut Value, command: Value, produces: Option<&str>) {
    cfg["pipeline"] = json!([
        "download",
        "transcript",
        "screenshots",
        "draft",
        "custom",
        "markdown"
    ]);
    cfg["stages"]["draft"]
        .as_object_mut()
        .unwrap()
        .remove("publish");
    cfg["stages"]["custom"] = json!({"type":"run","input":"draft.json","output":"custom.json","publish":"note.json","command":command});
    if let Some(produces) = produces {
        cfg["stages"]["custom"]["produces"] = json!(produces);
    }
    cfg["stages"]["markdown"]["input"] = json!("custom.json");
}
#[test]
fn run_expands_whole_path_arguments_and_supports_stdin_stdout_without_caching() {
    let root = tempfile::tempdir().unwrap();
    let source = local(root.path());
    let mut cfg = config();
    let mut command = program("files");
    command
        .as_array_mut()
        .unwrap()
        .extend([json!("{input}"), json!("{output}")]);
    with_run(&mut cfg, command, Some("document"));
    let completed = done(&run(root.path(), &source, &cfg, &["--skip", "screenshots"]));
    assert_eq!(
        document(&completed, "custom")["sections"][0]["speech"][0]["text"],
        "Original speech."
    );
    let counter = root.path().join("counter");
    let mut command = program("counter");
    command.as_array_mut().unwrap().push(json!(counter));
    cfg["stages"]["custom"]["command"] = command;
    for _ in 0..2 {
        done(&run(root.path(), &source, &cfg, &["--skip", "screenshots"]));
    }
    assert_eq!(std::fs::read_to_string(counter).unwrap(), "run\nrun\n");
}
#[test]
fn run_validates_document_images_and_preserves_unknown_fields_through_llm() {
    let root = tempfile::tempdir().unwrap();
    let source = local(root.path());
    let provider = mock();
    let mut cfg = config();
    with_run(&mut cfg, program("document"), Some("document"));
    cfg["pipeline"]
        .as_array_mut()
        .unwrap()
        .insert(5, json!("proofread"));
    cfg["stages"]["custom"]
        .as_object_mut()
        .unwrap()
        .remove("publish");
    cfg["stages"]["proofread"] = json!({"type":"llm","input":"custom.json","output":"proofread.json","publish":"note.json","provider":"ai"});
    cfg["providers"] = json!({"ai":{"base_url":provider.url}});
    cfg["stages"]["markdown"]["input"] = json!("proofread.json");
    let completed = done(&run(root.path(), &source, &cfg, &["--skip", "screenshots"]));
    let doc = document(&completed, "proofread");
    assert_eq!(doc["extension"], json!({"keep":[1,2,3]}));
    assert_eq!(doc["meta"]["extension"], "metadata");
    assert_eq!(doc["sections"][0]["extension"], "section");
    assert_eq!(doc["sections"][0]["speech"][0]["extension"], "paragraph");
    for mode in ["outside", "bad-time", "schema"] {
        cfg["stages"]["custom"]["command"] = program(mode);
        let message = error(&run(root.path(), &source, &cfg, &["--skip", "screenshots"]));
        assert!(message.contains("run fixture diagnostics"), "{message}");
    }
}
#[test]
fn run_failures_include_stderr_and_missing_output_is_an_error() {
    let root = tempfile::tempdir().unwrap();
    let source = local(root.path());
    for mode in ["exit", "missing"] {
        let mut cfg = config();
        let mut command = program(mode);
        command.as_array_mut().unwrap().push(json!("{output}"));
        with_run(&mut cfg, command, Some("document"));
        let message = error(&run(root.path(), &source, &cfg, &["--skip", "screenshots"]));
        assert!(message.contains("run fixture diagnostics"), "{message}");
        assert!(
            message.contains(if mode == "exit" {
                "custom failed (code=Some(7))"
            } else {
                "Stage custom: the program did not write custom.json"
            }),
            "{message}"
        );
        assert!(!root.path().join("lecture").exists());
    }
}
#[test]
fn run_accepts_multiple_inputs_zero_inputs_and_no_metadata_substitution() {
    let root = tempfile::tempdir().unwrap();
    let source = local(root.path());
    let mut cfg = config();
    let mut command = program("files");
    command
        .as_array_mut()
        .unwrap()
        .extend([json!("{input}"), json!("{output}")]);
    cfg["pipeline"].as_array_mut().unwrap().push(json!("join"));
    cfg["stages"]["join"] = json!({"type":"run","input":["note.md","note.md"],"output":"joined.md","publish":"joined.md","produces":"markdown","command":command});
    let completed = done(&run(root.path(), &source, &cfg, &["--skip", "screenshots"]));
    assert_eq!(
        std::fs::read_to_string(completed["files"]["join"].as_str().unwrap()).unwrap(),
        "Original speech.\nOriginal speech.\n"
    );
    let mut command = program("text");
    command
        .as_array_mut()
        .unwrap()
        .push(json!("{title} {author} {source}"));
    cfg["stages"]["join"]["command"] = command;
    cfg["stages"]["join"]["input"] = json!([]);
    let completed = done(&run(root.path(), &source, &cfg, &["--skip", "screenshots"]));
    assert_eq!(
        std::fs::read_to_string(completed["files"]["join"].as_str().unwrap()).unwrap(),
        "{title} {author} {source}"
    );
    cfg["stages"]["join"]
        .as_object_mut()
        .unwrap()
        .remove("produces");
    done(&run(root.path(), &source, &cfg, &["--skip", "screenshots"]));
    cfg["pipeline"]
        .as_array_mut()
        .unwrap()
        .push(json!("consumer"));
    cfg["stages"]["consumer"] =
        json!({"type":"run","input":"joined.md","output":"consumer","command":program("stdin")});
    assert_eq!(
        error(&run(root.path(), &source, &cfg, &["--skip", "screenshots"])),
        "Stage consumer reads joined.md, which is file; a run stage reads media or transcript or images or document or markdown"
    );
}
#[test]
fn stdout_is_printed_after_commit_and_only_stdout_writes_no_folder() {
    use std::io::Read;
    let root = tempfile::tempdir().unwrap();
    let source = local(root.path());
    let mut cfg = config();
    cfg["stages"]["markdown"]["publish"] = json!("-");
    let mut child = spawn(
        root.path(),
        source.to_str().unwrap(),
        &cfg,
        &["--skip", "screenshots", "-q"],
        false,
    );
    let mut stdout = child.stdout.take().unwrap();
    let mut first = [0; 1];
    stdout.read_exact(&mut first).unwrap();
    assert!(root.path().join("lecture/.notepipe.json").is_file());
    let mut text = Vec::from(first);
    stdout.read_to_end(&mut text).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert_eq!(text, b"Original speech.\n");
    let only = tempfile::tempdir().unwrap();
    let source = local(only.path());
    cfg["stages"]["draft"]
        .as_object_mut()
        .unwrap()
        .remove("publish");
    let output = spawn(
        only.path(),
        source.to_str().unwrap(),
        &cfg,
        &["--skip", "screenshots"],
        false,
    )
    .wait_with_output()
    .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"Original speech.\n");
    assert!(!only.path().join("lecture").exists());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("Notes ready:"));
    assert_eq!(
        error(&run(only.path(), &source, &cfg, &["--skip", "screenshots"])),
        "Publishing to standard output and --json both use standard output; use one of them"
    );
}
#[test]
fn stdout_is_empty_when_a_later_stage_fails() {
    let root = tempfile::tempdir().unwrap();
    let source = local(root.path());
    let mut cfg = config();
    cfg["stages"]["markdown"]["publish"] = json!("-");
    cfg["pipeline"]
        .as_array_mut()
        .unwrap()
        .push(json!("failure"));
    cfg["stages"]["failure"] =
        json!({"type":"run","input":"note.md","output":"failed","command":program("exit")});
    let output = spawn(
        root.path(),
        source.to_str().unwrap(),
        &cfg,
        &["--skip", "screenshots"],
        false,
    )
    .wait_with_output()
    .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!root.path().join("lecture").exists());
}
#[test]
fn ownership_marker_replaces_its_folder_and_leaves_unowned_folders_alone() {
    let root = tempfile::tempdir().unwrap();
    let source = local(root.path());
    let cfg = config();
    let first = done(&run(root.path(), &source, &cfg, &["--skip", "screenshots"]));
    let note = Path::new(first["note_dir"].as_str().unwrap());
    std::fs::write(note.join("user-added"), "will be replaced").unwrap();
    let second = done(&run(root.path(), &source, &cfg, &["--skip", "screenshots"]));
    assert_eq!(first["note_dir"], second["note_dir"]);
    assert!(!note.join("user-added").exists());
    let other = root.path().join("other");
    std::fs::create_dir(&other).unwrap();
    let source = local(&other);
    let third = done(&run(root.path(), &source, &cfg, &["--skip", "screenshots"]));
    assert_eq!(
        third["note_dir"],
        json!(root.path().canonicalize().unwrap().join("lecture-2"))
    );
    std::fs::remove_file(note.join(".notepipe.json")).unwrap();
    std::fs::write(note.join("user-added"), "keep").unwrap();
    let source = root.path().join("lecture.mp4");
    let fourth = done(&run(root.path(), &source, &cfg, &["--skip", "screenshots"]));
    assert_eq!(
        fourth["note_dir"],
        json!(root.path().canonicalize().unwrap().join("lecture-3"))
    );
    assert!(note.join("user-added").is_file());
}
#[test]
fn transcript_and_screenshots_overlap_and_two_llm_stages_share_one_provider_budget() {
    let root = tempfile::tempdir().unwrap();
    let source = video(root.path());
    let record = root.path().join("provider");
    let argv = json!([
        python(),
        concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/asr_adapter.py"),
        "--record",
        record,
        "--behavior",
        "parallel"
    ]);
    let mut cfg = config();
    cfg["providers"] = json!({"p":{"command":argv,"concurrency":1}});
    cfg["stages"]["transcript"]["sources"] = json!(["speech"]);
    cfg["stages"]["transcript"]["speech"] = json!({"provider":"p"});
    cfg["pipeline"] = json!([
        "download",
        "transcript",
        "screenshots",
        "draft",
        "summary",
        "outline",
        "markdown"
    ]);
    for name in ["summary", "outline"] {
        cfg["stages"][name] = json!({"type":"llm","input":"transcript.json","output":format!("{name}.md"),"provider":"p","prompt":name});
    }
    cfg["stages"]["markdown"]["input"] = json!(["summary.md", "outline.md", "draft.json"]);
    let output = run(root.path(), &source, &cfg, &[]);
    done(&output);
    let all = events(&output);
    let index = |name: &str, status: &str| {
        all.iter()
            .position(|e| e["type"] == "stage" && e["stage"] == name && e["status"] == status)
            .unwrap()
    };
    assert!(index("transcript", "start") < index("screenshots", "done"));
    assert!(index("screenshots", "start") < index("transcript", "done"));
    assert!(index("summary", "start") < index("outline", "done"));
    assert!(index("outline", "start") < index("summary", "done"));
    assert_eq!(
        std::fs::read_to_string(record.join("max-active")).unwrap(),
        "1"
    );
    assert_eq!(
        std::fs::read_to_string(record.join("starts")).unwrap(),
        "start\n"
    );
    #[cfg(unix)]
    assert!(record.join("terminated").is_file());
    assert_eq!(
        std::fs::read_to_string(record.join("calls"))
            .unwrap()
            .lines()
            .count(),
        3
    );
}
#[test]
fn downloaded_media_is_temporary_and_completed_caches_survive_llm_changes() {
    let root = tempfile::tempdir().unwrap();
    let record = root.path().join("download");
    let mut cfg = config();
    cfg["stages"]["download"]["command"] = json!([
        python(),
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/source_adapter.py"
        ),
        "--record",
        record
    ]);
    let provider = mock();
    with_llm(&mut cfg, &provider.url);
    let source = Path::new("https://example.test/media");
    let first = done(&run(root.path(), source, &cfg, &[]));
    let work = Path::new(first["work_dir"].as_str().unwrap());
    assert!(!work.join("media/media.mp4").exists());
    assert!(!work.join(".cache/download/files/media.mp4").exists());
    cfg["stages"]["summary"]["prompt"] = json!("CHANGED SUMMARY");
    done(&run(root.path(), source, &cfg, &[]));
    assert_eq!(
        std::fs::read_to_string(record.join("calls"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    let info: Value =
        serde_json::from_slice(&std::fs::read(work.join("media/info.json")).unwrap()).unwrap();
    assert_eq!(info["author"], "Fixture author");
    std::fs::remove_file(
        work.join(".cache/download/files")
            .join(info["subtitles"][0]["path"].as_str().unwrap()),
    )
    .unwrap();
    done(&run(root.path(), source, &cfg, &[]));
    assert_eq!(
        std::fs::read_to_string(record.join("calls"))
            .unwrap()
            .lines()
            .count(),
        2
    );
}

fn adapter(record: &Path, behavior: &str) -> Vec<String> {
    vec![
        python(),
        concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/asr_adapter.py").into(),
        "--record".into(),
        record.display().to_string(),
        "--behavior".into(),
        behavior.into(),
    ]
}
fn speech_config(record: &Path, behavior: &str) -> Value {
    let mut cfg = config();
    cfg["providers"] = json!({"speech":{"command":adapter(record,behavior),"concurrency":1}});
    cfg["stages"]["transcript"]["sources"] = json!(["speech"]);
    cfg["stages"]["transcript"]["speech"] = json!({"provider":"speech","max_segment":1.0});
    cfg
}
fn count(record: &Path) -> usize {
    std::fs::read_to_string(record.join("calls"))
        .unwrap_or_default()
        .lines()
        .count()
}
fn checkpoint(work: &Path) -> PathBuf {
    let path = std::fs::read_dir(work.join(".cache"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.file_name().unwrap().to_string_lossy().starts_with("asr-"))
        .unwrap();
    path.join("asr.jsonl")
}
fn wait_for_file(child: &mut std::process::Child, path: &Path) {
    let start = std::time::Instant::now();
    while !path.is_file() {
        assert!(
            child.try_wait().unwrap().is_none(),
            "child exited before {}",
            path.display()
        );
        if start.elapsed() > std::time::Duration::from_secs(120) {
            let _ = child.kill();
            panic!("timed out waiting for {}", path.display());
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}
fn transaction(root: &Path, phase: &str) {
    std::fs::write(root.join(".notepipe-transaction.json"),serde_json::to_vec(&json!({"name":"lecture","staging":".lecture.notepipe-new-recorded","old":".lecture.notepipe-old-recorded","phase":phase})).unwrap()).unwrap();
}
#[test]
fn failed_transcription_recovers_the_previous_note_and_resumes_completed_chunks() {
    let root = tempfile::tempdir().unwrap();
    let source = video(root.path());
    let record = root.path().join("asr");
    let cfg = speech_config(&record, "fail-after-first");
    let subtitle = root.path().join("previous.srt");
    std::fs::write(
        &subtitle,
        "1\n00:00:00,000 --> 00:00:02,000\nPrevious note.\n",
    )
    .unwrap();
    let previous = done(&run(
        root.path(),
        &source,
        &cfg,
        &["--subtitle", subtitle.to_str().unwrap()],
    ));
    let path = Path::new(previous["files"]["draft"].as_str().unwrap());
    let bytes = std::fs::read(path).unwrap();
    let note = Path::new(previous["note_dir"].as_str().unwrap());
    let old = root.path().join(".lecture.notepipe-old-recorded");
    std::fs::rename(note, &old).unwrap();
    transaction(root.path(), "old-moved");
    error(&run(root.path(), &source, &cfg, &[]));
    assert_eq!(std::fs::read(path).unwrap(), bytes);
    assert!(!old.exists());
    let cp = checkpoint(Path::new(previous["work_dir"].as_str().unwrap()));
    assert_eq!(std::fs::read_to_string(&cp).unwrap().lines().count(), 1);
    std::fs::write(record.join("release"), "ready").unwrap();
    done(&run(root.path(), &source, &cfg, &[]));
    assert_eq!(
        count(&record),
        std::fs::read_to_string(cp).unwrap().lines().count() + 1
    );
}
#[cfg(unix)]
#[test]
fn a_failed_sibling_stops_the_scheduler_owned_speech_provider() {
    let root = tempfile::tempdir().unwrap();
    let source = video(root.path());
    let record = root.path().join("asr");
    let mut cfg = speech_config(&record, "hang");
    cfg["pipeline"].as_array_mut().unwrap().push(json!("fail"));
    cfg["stages"]["fail"] = json!({"type":"run","output":"failure","command":[python(),"-c","import pathlib,sys,time;r=pathlib.Path(sys.argv[1]);\nwhile not r.exists():time.sleep(.01)\nsys.exit(7)",record.join("transcribing")]});
    let output = run(root.path(), &source, &cfg, &["--skip", "screenshots"]);
    assert!(error(&output).contains("fail"));
    assert!(record.join("terminated").is_file());
    assert_eq!(count(&record), 1);
    assert!(!root.path().join("lecture").exists());
}
#[cfg(unix)]
#[test]
fn signals_cancel_adapters_and_resume_completed_asr_chunks() {
    for signal in [libc::SIGINT, libc::SIGTERM] {
        let root = tempfile::tempdir().unwrap();
        let source = video(root.path());
        let record = root.path().join("asr");
        let cfg = speech_config(&record, "resume");
        let mut child = spawn(root.path(), source.to_str().unwrap(), &cfg, &[], true);
        wait_for_file(&mut child, &record.join("transcribing"));
        assert_eq!(unsafe { libc::kill(child.id() as i32, signal) }, 0);
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.status.code(), Some(130));
        assert!(!events(&output).iter().any(|e| e["type"] == "done"));
        assert!(record.join("terminated").is_file());
        let work = std::fs::read_dir(root.path().join("cache/notepipe/work"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| p.is_dir())
            .unwrap();
        assert!(!work.join(".cache/audio-transcript.wav").exists());
        let cp = checkpoint(&work);
        assert_eq!(std::fs::read_to_string(&cp).unwrap().lines().count(), 1);
        std::fs::write(record.join("release"), "ready").unwrap();
        done(&run(root.path(), &source, &cfg, &[]));
        assert_eq!(
            count(&record),
            std::fs::read_to_string(cp).unwrap().lines().count() + 1
        );
    }
}
#[cfg(unix)]
#[test]
fn signals_clean_media_without_waiting_for_http_and_forced_exits_are_cleaned_on_subtitle_rerun() {
    use std::time::Duration;
    for (signal, forced) in [
        (libc::SIGINT, false),
        (libc::SIGTERM, false),
        (libc::SIGTERM, true),
    ] {
        let root = tempfile::tempdir().unwrap();
        let received = root.path().join("received");
        let release = root.path().join("release");
        let ready = received.clone();
        let gate = release.clone();
        let service = MockAi::respond_with(move |_, _| {
            std::fs::write(&ready, "ready").unwrap();
            let start = std::time::Instant::now();
            while !gate.exists() {
                assert!(start.elapsed() < Duration::from_secs(15));
                std::thread::sleep(Duration::from_millis(10));
            }
            None
        });
        let mut cfg = json!({"pipeline":["download","transcript","summary"],"providers":{"ai":{"base_url":service.url}},"stages":{
            "download":{"type":"download","input":"source","output":"media","command":[python(),concat!(env!("CARGO_MANIFEST_DIR"),"/tests/fixtures/source_adapter.py"),"--record",root.path().join("download"),"--behavior","files"]},
            "transcript":{"type":"transcript","input":"media","output":"transcript.json","sources":["subtitles"],"publish":"transcript.json"},
            "summary":{"type":"llm","input":"transcript.json","output":"summary.md","publish":"note.md","provider":"ai"}
        }});
        let stubborn = root.path().join("stubborn-ready");
        if forced {
            cfg["pipeline"]
                .as_array_mut()
                .unwrap()
                .push(json!("stubborn"));
            cfg["stages"]["stubborn"] = json!({"type":"run","output":"stubborn","command":[python(),"-c","import signal,pathlib,sys,time;signal.signal(signal.SIGTERM,signal.SIG_IGN);pathlib.Path(sys.argv[1]).write_text('ready');time.sleep(60)",stubborn]});
        }
        let url = "https://example.test/cancel";
        let mut child = spawn(root.path(), url, &cfg, &[], true);
        wait_for_file(&mut child, &received);
        if forced {
            wait_for_file(&mut child, &stubborn);
        }
        let work = std::fs::read_dir(root.path().join("cache/notepipe/work"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| p.is_dir())
            .unwrap();
        let audio = work.join(".cache/audio-removed-stage.wav");
        std::fs::write(&audio, "leftover audio").unwrap();
        assert_eq!(unsafe { libc::kill(child.id() as i32, signal) }, 0);
        if forced {
            std::thread::sleep(Duration::from_millis(200));
            assert!(
                child.try_wait().unwrap().is_none(),
                "the child program is still stopping"
            );
            assert_eq!(unsafe { libc::kill(child.id() as i32, signal) }, 0);
        } else {
            let start = std::time::Instant::now();
            while child.try_wait().unwrap().is_none() {
                assert!(
                    start.elapsed() < Duration::from_secs(2),
                    "waited for the HTTP response"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
            std::fs::write(&release, "ready").unwrap();
        }
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.status.code(), Some(130));
        assert!(!events(&output).iter().any(|e| e["type"] == "done"));
        assert!(!root.path().join("Fixture-source").exists());
        if forced {
            assert!(audio.exists());
            std::fs::write(&release, "ready").unwrap();
            let subtitle = root.path().join("selected.srt");
            std::fs::write(
                &subtitle,
                "1\n00:00:00,000 --> 00:00:01,000\nSelected speech.\n",
            )
            .unwrap();
            done(
                &spawn(
                    root.path(),
                    url,
                    &cfg,
                    &[
                        "--subtitle",
                        subtitle.to_str().unwrap(),
                        "--skip",
                        "summary,stubborn",
                    ],
                    true,
                )
                .wait_with_output()
                .unwrap(),
            );
        }
        for path in [
            ".cache/download/files/media.mp4",
            "media/media.mp4",
            ".cache/audio-removed-stage.wav",
        ] {
            assert!(!work.join(path).exists(), "{path}");
        }
    }
}
#[test]
fn work_outputs_can_change_between_files_and_directories() {
    let root = tempfile::tempdir().unwrap();
    let source = local(root.path());
    let directory = json!({"pipeline":["download","note"],"stages":{
        "download":{"type":"download","input":"source","output":"artifact","command":["unused"]},
        "note":{"type":"run","output":"note.md","publish":"note.md","command":[python(),"-c","print('Note')"]}
    }});
    let file = json!({"pipeline":["file"],"stages":{
        "file":{"type":"run","output":"artifact","publish":"note.md","command":[python(),"-c","import sys; sys.stdout.buffer.write(b'New body\\n')"]}
    }});
    let completed = done(&run(root.path(), &source, &directory, &[]));
    let output = Path::new(completed["work_dir"].as_str().unwrap()).join("artifact");
    assert!(output.join("info.json").is_file());
    done(&run(root.path(), &source, &file, &[]));
    assert_eq!(std::fs::read_to_string(&output).unwrap(), "New body\n");
    done(&run(root.path(), &source, &directory, &[]));
    assert!(output.join("info.json").is_file());
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let outside = root.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        let victim = outside.join("data");
        std::fs::write(&victim, "user data").unwrap();
        for cfg in [&file, &directory] {
            if output.is_dir() {
                std::fs::remove_dir_all(&output).unwrap();
            } else {
                std::fs::remove_file(&output).unwrap();
            }
            symlink(&outside, &output).unwrap();
            done(&run(root.path(), &source, cfg, &[]));
            assert!(!output.symlink_metadata().unwrap().is_symlink());
            assert_eq!(std::fs::read_to_string(&victim).unwrap(), "user data");
        }
        std::fs::remove_dir_all(&output).unwrap();
        symlink(outside.join("missing"), &output).unwrap();
        done(&run(root.path(), &source, &directory, &[]));
        assert!(output.join("info.json").is_file());
        assert!(outside.is_dir());
    }
}
#[test]
fn an_active_download_finishes_before_independent_stages_start() {
    let root = tempfile::tempdir().unwrap();
    let transcript =
        json!({"language":"en","events":[{"start":0,"end":1,"text":"Imported words."}]})
            .to_string();
    let fetch = "import time,sys,json,pathlib;time.sleep(.2);o=pathlib.Path(sys.argv[-1]);(o/'media.wav').write_text('media');print(json.dumps({'media':'media.wav','title':'Downloaded title','duration':3}))";
    let mut cfg = json!({"pipeline":["fetch","import","document"],"stages":{
        "fetch":{"type":"download","input":"source","output":"media","command":[python(),"-c",fetch]},
        "import":{"type":"run","output":"transcript.json","produces":"transcript","command":[python(),"-c","import sys;sys.stdout.write(sys.argv[1])",transcript]},
        "document":{"type":"document","input":"transcript.json","output":"document.json","publish":"note.json"}
    }});
    let output = spawn(
        root.path(),
        "https://example.test/metadata",
        &cfg,
        &[],
        true,
    )
    .wait_with_output()
    .unwrap();
    let completed = done(&output);
    assert_eq!(
        document(&completed, "document")["meta"]["title"],
        "Downloaded title"
    );
    let all = events(&output);
    let finished = all
        .iter()
        .position(|e| e["stage"] == "fetch" && e["status"] == "done")
        .unwrap();
    for stage in ["import", "document"] {
        assert!(
            all.iter()
                .position(|e| e["stage"] == stage && e["status"] == "start")
                .unwrap()
                > finished
        );
    }
    let source = local(root.path());
    cfg["stages"]["fetch"]["command"] = json!(["not-a-program"]);
    done(&run(root.path(), &source, &cfg, &["--skip", "fetch"]));
}
#[test]
fn a_failed_sibling_stops_paragraph_batches_chunks_and_markdown_retries() {
    use std::{sync::atomic::Ordering, time::Duration};
    for mode in ["paragraphs", "chunks", "markdown"] {
        let root = tempfile::tempdir().unwrap();
        let source = local(root.path());
        let received = root.path().join("received");
        let failed = root.path().join("failed");
        let ready = received.clone();
        let failure = failed.clone();
        let service = MockAi::respond_with(move |_, body| {
            std::fs::write(&ready, "ready").unwrap();
            let start = std::time::Instant::now();
            while !failure.exists() {
                assert!(start.elapsed() < Duration::from_secs(10));
                std::thread::sleep(Duration::from_millis(10));
            }
            std::thread::sleep(Duration::from_millis(200));
            Some(if mode == "markdown" {
                (503, json!({"error":"retry"}))
            } else {
                let content = if body.get("response_format").is_some() {
                    json!({"segments":[{"id":0,"text":"Rewritten."}]}).to_string()
                } else {
                    "Summary.".into()
                };
                (200, json!({"choices":[{"message":{"content":content}}]}))
            })
        });
        let events = (0..3)
            .map(|i| json!({"start":i*10,"end":i*10+1,"text":"x".repeat(990)}))
            .collect::<Vec<_>>();
        let (kind, input, output, text) = match mode {
            "paragraphs" => ("document", "input.json", "answer.json", json!({"schema":3,"meta":{"title":"Title","author":"","duration":30,"source":"source"},"sections":[{"t":0,"end":30,"image":"","speech":events}]}).to_string()),
            "chunks" => ("transcript", "input.json", "answer.md", json!({"language":"en","events":events}).to_string()),
            _ => ("markdown", "input.md", "answer.md", "Exact input".into()),
        };
        let mut cfg = json!({"pipeline":["seed","answer","fail"],"providers":{"ai":{"base_url":service.url,"concurrency":1}},"stages":{
            "seed":{"type":"run","output":input,"produces":kind,"command":[python(),"-c","import sys;sys.stdout.write(sys.argv[1])",text]},
            "answer":{"type":"llm","input":input,"output":output,"publish":output,"provider":"ai"},
            "fail":{"type":"run","output":"failure","command":[python(),"-c","import pathlib,sys,time;r=pathlib.Path(sys.argv[1]);\nwhile not r.exists():time.sleep(.01)\npathlib.Path(sys.argv[2]).write_text('failed');sys.exit(7)",received,failed]}
        }});
        cfg["stages"]["answer"][if mode == "paragraphs" {
            "batch_size"
        } else {
            "chunk_size"
        }] = json!(if mode == "paragraphs" { 1 } else { 1000 });
        assert!(error(&run(root.path(), &source, &cfg, &[])).contains("fail"));
        assert_eq!(service.calls.load(Ordering::SeqCst), 1, "{mode}");
        assert!(!root.path().join("lecture").exists());
    }
}
#[cfg(unix)]
#[test]
fn signals_and_a_failed_sibling_cancel_run_programs() {
    for signal in [Some(libc::SIGINT), None] {
        let root = tempfile::tempdir().unwrap();
        let source = local(root.path());
        let ready = root.path().join("ready");
        let mut command = program("wait");
        command.as_array_mut().unwrap().push(json!(ready));
        let mut cfg = config();
        cfg["pipeline"].as_array_mut().unwrap().push(json!("wait"));
        cfg["stages"]["wait"] = json!({"type":"run","output":"waiting","command":command});
        if signal.is_none() {
            let mut fail = program("exit");
            fail.as_array_mut()
                .unwrap()
                .extend([json!("{input}"), json!("{output}")]);
            cfg["pipeline"].as_array_mut().unwrap().push(json!("fail"));
            cfg["stages"]["fail"] =
                json!({"type":"run","input":"note.md","output":"failure","command":fail});
        }
        let mut child = spawn(
            root.path(),
            source.to_str().unwrap(),
            &cfg,
            &["--skip", "screenshots"],
            true,
        );
        wait_for_file(&mut child, &ready);
        if let Some(signal) = signal {
            assert_eq!(unsafe { libc::kill(child.id() as i32, signal) }, 0);
        }
        let start = std::time::Instant::now();
        while child.try_wait().unwrap().is_none() {
            if start.elapsed() > std::time::Duration::from_secs(10) {
                let _ = child.kill();
                panic!("program was not cancelled");
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let output = child.wait_with_output().unwrap();
        assert!(!output.status.success());
        assert!(!root.path().join("lecture").exists());
    }
}
#[cfg(unix)]
#[test]
fn cleanup_warnings_reach_the_done_event_and_stderr() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let source = local(root.path());
    let cfg = config();
    done(&run(root.path(), &source, &cfg, &["--skip", "screenshots"]));
    let old = root.path().join(".lecture.notepipe-old-recorded");
    std::fs::create_dir(&old).unwrap();
    std::fs::write(old.join("content"), "previous").unwrap();
    std::fs::set_permissions(&old, std::fs::Permissions::from_mode(0o500)).unwrap();
    transaction(root.path(), "published");
    let output = run(root.path(), &source, &cfg, &["--skip", "screenshots"]);
    std::fs::set_permissions(&old, std::fs::Permissions::from_mode(0o700)).unwrap();
    let completed = done(&output);
    assert!(completed["warnings"].as_array().unwrap().iter().any(|w| {
        w.as_str()
            .unwrap()
            .contains(".lecture.notepipe-old-recorded")
    }));
    assert!(String::from_utf8_lossy(&output.stderr).contains(".lecture.notepipe-old-recorded"));
    assert!(!root.path().join(".notepipe-transaction.json").exists());
}
#[test]
fn stages_send_named_environment_keys_and_skips_do_not_require_them() {
    let root = tempfile::tempdir().unwrap();
    let source = local(root.path());
    let provider = mock();
    let mut cfg = config();
    with_llm(&mut cfg, &provider.url);
    cfg["providers"]["ai"]["api_key_env"] = json!("NOTEPIPE_TEST_SERVICE_KEY");
    let settings = root.path().join("settings.json");
    std::fs::write(&settings, serde_json::to_vec(&cfg).unwrap()).unwrap();
    for key in [Some("environment-key"), None, Some("")] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_notepipe"));
        command
            .arg(&source)
            .arg("--config")
            .arg(&settings)
            .args(["--json", "--skip", "screenshots"])
            .current_dir(root.path())
            .env("XDG_CACHE_HOME", root.path().join("cache"));
        if let Some(key) = key {
            command.env("NOTEPIPE_TEST_SERVICE_KEY", key);
        } else {
            command.env_remove("NOTEPIPE_TEST_SERVICE_KEY");
        }
        let output = command.output().unwrap();
        if key == Some("environment-key") {
            done(&output);
            assert!(
                provider.headers.lock().unwrap()[0]
                    .contains("Authorization: Bearer environment-key")
            );
        } else {
            assert_eq!(
                error(&output),
                "Provider ai: the environment variable NOTEPIPE_TEST_SERVICE_KEY is not set"
            );
        }
    }
    done(&run(
        root.path(),
        &source,
        &cfg,
        &["--skip", "screenshots,proofread,summary"],
    ));
}
#[test]
fn run_transcripts_are_validated_and_document_accepts_reversed_inputs() {
    let root = tempfile::tempdir().unwrap();
    let source = video(root.path());
    let mut cfg = config();
    let transcript = json!({"language":"en","events":[{"start":0,"end":1,"text":"Generated transcript.","extension":"paragraph"}],"extension":"kept"});
    let mut command = program("text");
    command
        .as_array_mut()
        .unwrap()
        .push(json!(transcript.to_string()));
    cfg["stages"]["transcript"] = json!({"type":"run","output":"transcript.json","publish":"transcript.json","produces":"transcript","command":command});
    cfg["stages"]["draft"]["input"] = json!(["images", "transcript.json"]);
    let completed = done(&run(root.path(), &source, &cfg, &[]));
    assert_eq!(document(&completed, "draft")["extension"], "kept");
    assert_eq!(
        document(&completed, "draft")["sections"][0]["speech"][0]["extension"],
        "paragraph"
    );
    assert_eq!(document(&completed, "transcript")["language"], "en");
    cfg["stages"]["transcript"]["command"][3] =
        json!(r#"{"language":"en","events":[{"start":-1,"end":1,"text":"invalid"}]}"#);
    assert!(error(&run(root.path(), &source, &cfg, &[])).contains("Invalid subtitle timestamps"));
}
#[test]
fn explicit_subtitles_are_the_only_transcript_source_and_still_emit_the_stage() {
    let root = tempfile::tempdir().unwrap();
    let source = local(root.path());
    let mut cfg = config();
    let subtitle = root.path().join("selected.srt");
    std::fs::write(
        &subtitle,
        "1\n00:00:00,000 --> 00:00:01,000\nSelected speech.\n",
    )
    .unwrap();
    cfg["providers"] =
        json!({"p":{"command":["must-not-start"],"api_key_env":"NOTEPIPE_TEST_UNUSED_KEY"}});
    cfg["stages"]["transcript"]["sources"] = json!(["speech"]);
    cfg["stages"]["transcript"]["speech"] = json!({"provider":"p"});
    let output = run(
        root.path(),
        &source,
        &cfg,
        &[
            "--skip",
            "screenshots",
            "--subtitle",
            subtitle.to_str().unwrap(),
            "--language",
            "ko",
        ],
    );
    let completed = done(&output);
    let work = Path::new(completed["work_dir"].as_str().unwrap());
    let transcript: Value =
        serde_json::from_slice(&std::fs::read(work.join("transcript.json")).unwrap()).unwrap();
    assert_eq!(transcript["language"], "ko");
    assert_eq!(transcript["events"][0]["text"], "Selected speech.");
    for status in ["start", "done"] {
        assert!(
            events(&output)
                .iter()
                .any(|e| e["stage"] == "transcript" && e["status"] == status)
        );
    }
}
#[test]
fn only_a_published_document_still_copies_its_images_and_no_resume_discards_caches() {
    let root = tempfile::tempdir().unwrap();
    let source = video(root.path());
    std::fs::write(
        source.with_extension("srt"),
        "1\n00:00:00,000 --> 00:00:02,000\nA recording.\n",
    )
    .unwrap();
    let mut cfg = config();
    cfg["pipeline"] = json!(["download", "transcript", "screenshots", "draft"]);
    let first = done(&run(root.path(), &source, &cfg, &[]));
    let note = Path::new(first["note_dir"].as_str().unwrap());
    assert!(note.join("images/0001.jpg").is_file());
    assert!(!note.join("note.md").exists());
    let work = Path::new(first["work_dir"].as_str().unwrap());
    std::fs::write(work.join(".cache/stale"), "discard").unwrap();
    done(&run(root.path(), &source, &cfg, &["--no-resume"]));
    assert!(!work.join(".cache/stale").exists());
    assert!(source.is_file());
}

#[test]
fn an_empty_paragraph_is_removed_and_later_rewrites_keep_the_first_original() {
    let root = tempfile::tempdir().unwrap();
    let source = local(root.path());
    std::fs::write(source.with_extension("srt"),"1\n00:00:00,000 --> 00:00:01,000\nFiller.\n\n2\n00:00:06,000 --> 00:00:07,000\nSubstantive speech.\n").unwrap();
    let provider = MockAi::respond_with(|_, body| {
        let translate = body["messages"][0]["content"]
            .as_str()
            .unwrap()
            .starts_with("TRANSLATE");
        let content = if translate {
            json!({"segments":[{"id":0,"text":"Translated speech."}]})
        } else {
            json!({"segments":[{"id":0,"text":""},{"id":1,"text":"Corrected speech."}]})
        };
        Some((
            200,
            json!({"choices":[{"message":{"content":content.to_string()}}]}),
        ))
    });
    let mut cfg = config();
    with_llm(&mut cfg, &provider.url);
    cfg["pipeline"] = json!([
        "download",
        "transcript",
        "screenshots",
        "draft",
        "proofread",
        "translate",
        "markdown"
    ]);
    cfg["stages"]["proofread"]
        .as_object_mut()
        .unwrap()
        .remove("publish");
    cfg["stages"]["translate"] = json!({"type":"llm","input":"proofread.json","output":"translated.json","publish":"note.json","provider":"ai","prompt":"TRANSLATE"});
    cfg["stages"]["markdown"]["input"] = json!("translated.json");
    let completed = done(&run(root.path(), &source, &cfg, &["--skip", "screenshots"]));
    let doc = document(&completed, "translate");
    let speech = doc["sections"][0]["speech"].as_array().unwrap();
    assert_eq!(speech.len(), 1);
    assert_eq!(speech[0]["text"], "Translated speech.");
    assert_eq!(speech[0]["raw"], "Substantive speech.");
}
#[test]
fn repeated_transcript_and_screenshot_stages_share_caches_without_corruption() {
    let root = tempfile::tempdir().unwrap();
    let source = video(root.path());
    let record = root.path().join("asr");
    let mut cfg = speech_config(&record, "parallel");
    cfg["pipeline"] = json!([
        "download",
        "transcript",
        "second-transcript",
        "screenshots",
        "second-screenshots",
        "draft",
        "markdown"
    ]);
    cfg["stages"]["second-transcript"] = cfg["stages"]["transcript"].clone();
    cfg["stages"]["second-transcript"]["output"] = json!("second-transcript.json");
    cfg["stages"]["second-screenshots"] = cfg["stages"]["screenshots"].clone();
    cfg["stages"]["second-screenshots"]["output"] = json!("second-images");
    cfg["stages"]["draft"]["input"] = json!(["second-transcript.json", "second-images"]);
    let completed = done(&run(root.path(), &source, &cfg, &[]));
    let work = Path::new(completed["work_dir"].as_str().unwrap());
    let first: Value =
        serde_json::from_slice(&std::fs::read(work.join("transcript.json")).unwrap()).unwrap();
    let second: Value =
        serde_json::from_slice(&std::fs::read(work.join("second-transcript.json")).unwrap())
            .unwrap();
    assert_eq!(
        first["events"].as_array().unwrap().len(),
        second["events"].as_array().unwrap().len()
    );
    for (a, b) in first["events"]
        .as_array()
        .unwrap()
        .iter()
        .zip(second["events"].as_array().unwrap())
    {
        assert_eq!(a["text"], b["text"]);
        for key in ["start", "end"] {
            assert!((a[key].as_f64().unwrap() - b[key].as_f64().unwrap()).abs() < 1e-9);
        }
    }
    let cp = checkpoint(work);
    assert_eq!(
        count(&record),
        std::fs::read_to_string(cp).unwrap().lines().count()
    );
    assert!(
        Path::new(completed["note_dir"].as_str().unwrap())
            .join("second-images/0001.jpg")
            .is_file()
    );
    let frames: Value =
        serde_json::from_slice(&std::fs::read(work.join("second-images/images.json")).unwrap())
            .unwrap();
    assert_eq!(frames[0]["image"], "0001.jpg");
}

#[test]
fn an_explicitly_published_images_directory_can_share_document_assets() {
    let root = tempfile::tempdir().unwrap();
    let source = video(root.path());
    std::fs::write(
        source.with_extension("srt"),
        "1\n00:00:00,000 --> 00:00:02,000\nA recording.\n",
    )
    .unwrap();
    let mut cfg = config();
    cfg["stages"]["screenshots"]["publish"] = json!("images");
    let completed = done(&run(root.path(), &source, &cfg, &[]));
    let images = Path::new(completed["files"]["screenshots"].as_str().unwrap());
    assert!(images.join("0001.jpg").is_file());
    assert!(images.join("images.json").is_file());
}
#[test]
fn broken_media_still_publishes_subtitles_with_screenshot_warnings() {
    let root = tempfile::tempdir().unwrap();
    let source = local(root.path());
    let output = run(root.path(), &source, &config(), &[]);
    let completed = done(&output);
    assert!(!completed["warnings"].as_array().unwrap().is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Warning:"));
    assert_eq!(document(&completed, "draft")["sections"][0]["image"], "");
    assert!(
        !Path::new(completed["note_dir"].as_str().unwrap())
            .join("images")
            .exists()
    );
}

#[test]
fn out_is_relative_to_starting_directory_and_cli_out_overrides_it() {
    let root = tempfile::tempdir().unwrap();
    let source = local(root.path());
    let mut cfg = config();
    cfg["out"] = json!("from-config");
    let first = done(&run(root.path(), &source, &cfg, &["--skip", "screenshots"]));
    assert_eq!(
        Path::new(first["note_dir"].as_str().unwrap()),
        root.path()
            .join("from-config/lecture")
            .canonicalize()
            .unwrap()
    );
    let second = done(&run(
        root.path(),
        &source,
        &cfg,
        &["--skip", "screenshots", "-o", "from-cli"],
    ));
    assert_eq!(
        Path::new(second["note_dir"].as_str().unwrap()),
        root.path().join("from-cli/lecture").canonicalize().unwrap()
    );
    let output = spawn(
        root.path(),
        source.to_str().unwrap(),
        &cfg,
        &["--skip", "screenshots", "-q", "-v"],
        false,
    )
    .wait_with_output()
    .unwrap();
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[test]
fn edited_explicit_subtitles_rebuild_the_note_in_the_same_folder() {
    let root = tempfile::tempdir().unwrap();
    let source = local(root.path());
    let subtitle = root.path().join("selected.srt");
    let mut previous = Value::Null;
    for text in ["Selected speech.", "Updated selected speech."] {
        std::fs::write(
            &subtitle,
            format!("1\n00:00:00,000 --> 00:00:01,000\n{text}\n"),
        )
        .unwrap();
        let completed = done(&run(
            root.path(),
            &source,
            &config(),
            &[
                "--skip",
                "screenshots",
                "--subtitle",
                subtitle.to_str().unwrap(),
            ],
        ));
        if !previous.is_null() {
            assert_eq!(completed["note_dir"], previous);
        }
        previous = completed["note_dir"].clone();
        let doc = document(&completed, "draft");
        assert_eq!(doc["meta"]["source"], json!(source.canonicalize().unwrap()));
        assert_eq!(doc["sections"][0]["speech"][0]["text"], text);
        assert_eq!(
            std::fs::read_to_string(completed["files"]["markdown"].as_str().unwrap()).unwrap(),
            format!("{text}\n")
        );
    }
}

#[test]
fn skipping_screenshots_avoids_image_work_and_events_for_a_video() {
    let root = tempfile::tempdir().unwrap();
    let source = video(root.path());
    std::fs::write(
        source.with_extension("srt"),
        "1\n00:00:00,000 --> 00:00:01,000\nText only.\n",
    )
    .unwrap();
    let output = run(root.path(), &source, &config(), &["--skip", "screenshots"]);
    let completed = done(&output);
    let doc = document(&completed, "draft");
    assert_eq!(doc["sections"].as_array().unwrap().len(), 1);
    assert_eq!(doc["sections"][0]["image"], "");
    assert_eq!(completed["warnings"], json!([]));
    assert!(!events(&output).iter().any(|event| {
        event["stage"]
            .as_str()
            .is_some_and(|name| name.starts_with("screenshots"))
    }));
    let work = Path::new(completed["work_dir"].as_str().unwrap());
    assert!(!work.join("images").exists());
    assert!(!tree(work).contains(".jpg"));
    assert!(!tree(work).contains("frames-"));
}

#[test]
fn llm_changes_reuse_download_asr_and_frames_and_rebuild_named_outputs() {
    let root = tempfile::tempdir().unwrap();
    let download = root.path().join("download");
    let asr = root.path().join("asr");
    let mut cfg = speech_config(&asr, "healthy");
    cfg["stages"]["download"]["command"] = source_adapter_command(&download, "speech");
    let source = Path::new("https://example.test/cached-speech");
    let first = done(&run(root.path(), source, &cfg, &[]));
    let work = Path::new(first["work_dir"].as_str().unwrap());
    let calls = count(&asr);
    assert!(calls > 0);
    let frames = std::fs::read_dir(work.join(".cache"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("frames-")
        })
        .unwrap()
        .join("complete.json");
    let frame_time = frames.metadata().unwrap().modified().unwrap();
    // Rebuild stage outputs from caches, including after a run stranded downloaded media.
    let stranded = work.join(".cache/download/files/media.mp4");
    std::fs::write(&stranded, "leftover media").unwrap();
    std::fs::remove_file(work.join("transcript.json")).unwrap();
    std::fs::remove_dir_all(work.join("images")).unwrap();
    let speech = cfg["providers"]["speech"].clone();
    let provider = mock();
    with_llm(&mut cfg, &provider.url);
    cfg["providers"]["speech"] = speech;
    for _ in 0..2 {
        let completed = done(&run(root.path(), source, &cfg, &[]));
        assert_eq!(completed["note_dir"], first["note_dir"]);
        assert_eq!(count(&download), 1);
        assert_eq!(count(&asr), calls);
        assert_eq!(frames.metadata().unwrap().modified().unwrap(), frame_time);
        assert_eq!(provider.calls.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert!(work.join("transcript.json").is_file());
        assert!(work.join("images/0001.jpg").is_file());
        assert!(!stranded.exists());
        assert!(!work.join("media/media.mp4").exists());
    }
    std::fs::create_dir(&stranded).unwrap();
    let output = run(root.path(), source, &cfg, &[]);
    let completed = done(&output);
    assert!(
        completed["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| {
                warning
                    .as_str()
                    .unwrap()
                    .contains("Could not remove cached media")
            })
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("Could not remove cached media"));
    assert!(
        Path::new(completed["note_dir"].as_str().unwrap())
            .join("images/0001.jpg")
            .is_file()
    );
    done(&run(root.path(), source, &cfg, &["--no-resume"]));
    assert_eq!(count(&download), 2);
    assert!(count(&asr) > calls);
    assert_eq!(provider.calls.load(std::sync::atomic::Ordering::SeqCst), 4);
}

fn source_adapter_command(record: &Path, behavior: &str) -> Value {
    json!([
        python(),
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/source_adapter.py"
        ),
        "--record",
        record,
        "--behavior",
        behavior
    ])
}

fn silence(root: &Path) -> PathBuf {
    let source = root.join("silence.wav");
    let output = Command::new("ffmpeg")
        .args([
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "anullsrc=r=16000:cl=mono",
            "-t",
            "1",
        ])
        .arg(&source)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    source
}

#[test]
fn transcript_sources_follow_order_and_subtitles_only_fail_when_absent() {
    let root = tempfile::tempdir().unwrap();
    let source = video(root.path());
    let subtitle = source.with_extension("srt");
    std::fs::write(
        &subtitle,
        "1\n00:00:00,000 --> 00:00:02,000\nSidecar words.\n",
    )
    .unwrap();
    let record = root.path().join("asr");
    let mut cfg = speech_config(&record, "healthy");
    for (sources, expected) in [
        (json!(["subtitles", "speech"]), "Sidecar words."),
        (json!(["speech", "subtitles"]), "Fixture transcript."),
        (json!(["speech"]), "Fixture transcript."),
        (json!(["subtitles"]), "Sidecar words."),
    ] {
        cfg["stages"]["transcript"]["sources"] = sources;
        let completed = done(&run(root.path(), &source, &cfg, &["--skip", "screenshots"]));
        assert!(
            document(&completed, "draft")["sections"][0]["speech"]
                .to_string()
                .contains(expected)
        );
    }
    std::fs::remove_file(subtitle).unwrap();
    assert_eq!(
        error(&run(root.path(), &source, &cfg, &["--skip", "screenshots"])),
        "No subtitles found. Select existing subtitles, or use speech recognition."
    );
}

#[test]
fn silent_audio_reports_no_speech_and_speech_first_falls_back_to_subtitles() {
    let root = tempfile::tempdir().unwrap();
    let source = silence(root.path());
    let record = root.path().join("asr");
    let mut cfg = speech_config(&record, "healthy");
    assert_eq!(
        error(&run(root.path(), &source, &cfg, &["--skip", "screenshots"])),
        "No speech was recognized in the recording"
    );
    assert_eq!(count(&record), 0);
    std::fs::write(
        source.with_extension("srt"),
        "1\n00:00:00,000 --> 00:00:01,000\nFallback subtitles.\n",
    )
    .unwrap();
    cfg["stages"]["transcript"]["sources"] = json!(["speech", "subtitles"]);
    for _ in 0..2 {
        let completed = done(&run(root.path(), &source, &cfg, &["--skip", "screenshots"]));
        assert_eq!(
            document(&completed, "draft")["sections"][0]["speech"][0]["text"],
            "Fallback subtitles."
        );
    }
    assert_eq!(count(&record), 0);
}

#[test]
fn explicit_subtitles_override_downloaded_tracks_without_starting_the_provider() {
    let root = tempfile::tempdir().unwrap();
    let mut cfg = config();
    cfg["stages"]["download"]["command"] =
        source_adapter_command(&root.path().join("source"), "files");
    cfg["providers"] =
        json!({"speech":{"command":["must-not-start"],"api_key_env":"NOTEPIPE_TEST_UNUSED_KEY"}});
    cfg["stages"]["transcript"]["sources"] = json!(["subtitles", "speech"]);
    cfg["stages"]["transcript"]["speech"] = json!({"provider":"speech"});
    let subtitle = root.path().join("selected.srt");
    std::fs::write(
        &subtitle,
        "1\n00:00:00,000 --> 00:00:01,000\nSelected words.\n",
    )
    .unwrap();
    let output = run(
        root.path(),
        Path::new("https://example.test/media"),
        &cfg,
        &[
            "--skip",
            "screenshots",
            "--subtitle",
            subtitle.to_str().unwrap(),
        ],
    );
    assert_eq!(
        document(&done(&output), "draft")["sections"][0]["speech"][0]["text"],
        "Selected words."
    );
    assert!(!events(&output).iter().any(|e| {
        e["stage"]
            .as_str()
            .is_some_and(|name| name.starts_with("transcript/"))
    }));
}

#[test]
fn speech_output_limit_fails_without_retrying() {
    let root = tempfile::tempdir().unwrap();
    let source = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/speech.wav"
    ));
    let record = root.path().join("asr");
    let mut cfg = speech_config(&record, "output-limit");
    cfg["stages"]["transcript"]["speech"]["max_segment"] = json!(30.0);
    let output = spawn(
        root.path(),
        source.to_str().unwrap(),
        &cfg,
        &["--skip", "screenshots"],
        false,
    )
    .wait_with_output()
    .unwrap();
    assert!(!output.status.success());
    assert_eq!(count(&record), 1);
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("The model stopped at its output limit.")
    );
}

#[test]
fn managed_asr_stops_after_a_transcription_error() {
    let root = tempfile::tempdir().unwrap();
    let source = video(root.path());
    let record = root.path().join("asr");
    let cfg = speech_config(&record, "error");
    let output = run(root.path(), &source, &cfg, &["--skip", "screenshots"]);
    assert!(error(&output).contains("400"));
    #[cfg(unix)]
    assert!(record.join("terminated").is_file());
    assert_eq!(
        std::fs::read_to_string(record.join("authorization")).unwrap(),
        ""
    );
    assert_eq!(
        std::fs::read_to_string(record.join("endpoint")).unwrap(),
        "/v1/audio/transcriptions"
    );
    assert!(!root.path().join("lecture").exists());
}

struct RemoteAsr {
    child: std::process::Child,
    base: String,
}
impl RemoteAsr {
    fn start(record: &Path, behavior: &str) -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let argv = adapter(record, behavior);
        let child = Command::new(&argv[0])
            .args(&argv[1..])
            .args(["--host", "127.0.0.1", "--port", &port.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut service = Self {
            child,
            base: format!("http://127.0.0.1:{port}/v1"),
        };
        wait_for_file(&mut service.child, &record.join("ready"));
        service
    }
}
impl Drop for RemoteAsr {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(unix)]
#[test]
fn first_signal_abandons_slow_chat_and_speech_responses_before_cleanup() {
    use std::time::{Duration, Instant};
    for speech in [false, true] {
        for signal in [libc::SIGINT, libc::SIGTERM] {
            let root = tempfile::tempdir().unwrap();
            let record = root.path().join("remote");
            let mut service = RemoteAsr::start(&record, "slow");
            let mut cfg = json!({"pipeline":["download","transcript"],"providers":{"p":{"base_url":service.base,"concurrency":1}},"stages":{
                "download":{"type":"download","input":"source","output":"media","command":source_adapter_command(&root.path().join("download"),"speech-audio")},
                "transcript":{"type":"transcript","input":"media","output":"transcript.json","sources":[if speech {"speech"} else {"subtitles"}],"speech":{"provider":"p","max_segment":30.0},"publish":"transcript.json"}
            }});
            if !speech {
                cfg["pipeline"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!("summary"));
                cfg["stages"]["summary"] = json!({"type":"llm","input":"transcript.json","output":"summary.md","publish":"note.md","provider":"p"});
            }
            let mut child = spawn(root.path(), "https://example.test/slow", &cfg, &[], true);
            wait_for_file(&mut child, &record.join("transcribing"));
            let start = Instant::now();
            assert_eq!(unsafe { libc::kill(child.id() as i32, signal) }, 0);
            while child.try_wait().unwrap().is_none() {
                if start.elapsed() > Duration::from_secs(2) {
                    let _ = child.kill();
                    panic!("waited for a 30-second response; speech={speech}");
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            let output = child.wait_with_output().unwrap();
            assert_eq!(output.status.code(), Some(130));
            assert!(!record.join("responded").exists());
            assert!(service.child.try_wait().unwrap().is_none());
            assert!(!events(&output).iter().any(|e| e["type"] == "done"));
            let work = std::fs::read_dir(root.path().join("cache/notepipe/work"))
                .unwrap()
                .map(|e| e.unwrap().path())
                .find(|p| p.is_dir())
                .unwrap();
            assert!(!work.join(".cache/download/files/media.wav").exists());
            assert!(!work.join("media/media.wav").exists());
            assert!(!work.join(".cache/audio-transcript.wav").exists());
            assert!(!work.join(".cache/llm").exists());
            if speech {
                assert!(!checkpoint(&work).exists());
            }
            assert!(!root.path().join("Fixture-source").exists());
        }
    }
}

#[test]
fn keyless_remote_asr_obeys_the_provider_concurrency_limit() {
    for concurrency in [1, 3] {
        let root = tempfile::tempdir().unwrap();
        let source = video(root.path());
        let record = root.path().join("asr");
        let service = RemoteAsr::start(&record, "parallel");
        let mut cfg = speech_config(&record, "parallel");
        cfg["providers"]["speech"] = json!({"base_url":service.base,"concurrency":concurrency});
        done(&run(root.path(), &source, &cfg, &["--skip", "screenshots"]));
        assert_eq!(
            std::fs::read_to_string(record.join("authorization")).unwrap(),
            ""
        );
        let maximum: usize = std::fs::read_to_string(record.join("max-active"))
            .unwrap()
            .parse()
            .unwrap();
        assert!(maximum <= concurrency);
        assert_eq!(maximum > 1, concurrency > 1);
    }
}

#[test]
fn speech_language_and_model_select_separate_checkpoints() {
    let root = tempfile::tempdir().unwrap();
    let source = video(root.path());
    let record = root.path().join("asr");
    let mut cfg = speech_config(&record, "healthy");
    let first = done(&run(root.path(), &source, &cfg, &["--skip", "screenshots"]));
    let chunks = count(&record);
    assert!(chunks > 0);
    done(&run(root.path(), &source, &cfg, &["--skip", "screenshots"]));
    assert_eq!(count(&record), chunks);
    cfg["stages"]["transcript"]["language"] = json!("ko");
    done(&run(root.path(), &source, &cfg, &["--skip", "screenshots"]));
    assert_eq!(count(&record), chunks * 2);
    cfg["stages"]["transcript"]["speech"]["model"] = json!("another-model");
    done(&run(root.path(), &source, &cfg, &["--skip", "screenshots"]));
    assert_eq!(count(&record), chunks * 3);
    let work = Path::new(first["work_dir"].as_str().unwrap());
    let checkpoints = std::fs::read_dir(work.join(".cache"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("asr-")
        })
        .collect::<Vec<_>>();
    assert_eq!(checkpoints.len(), 3);
    for path in checkpoints {
        assert!(path.join("asr.jsonl").is_file());
        assert!(!path.join(".asr_identity").exists());
        assert!(!path.join(".asr_done").exists());
    }
}

#[test]
fn audio_only_caches_reuse_missing_media_and_vision_stages_send_text() {
    let root = tempfile::tempdir().unwrap();
    let download_record = root.path().join("source");
    let asr_record = root.path().join("asr");
    let mut cfg = speech_config(&asr_record, "healthy");
    cfg["stages"]["download"]["command"] = source_adapter_command(&download_record, "speech-audio");
    let source = Path::new("https://example.test/audio");
    let first = done(&run(root.path(), source, &cfg, &[]));
    assert_eq!(first["warnings"], json!([]));
    let work = Path::new(first["work_dir"].as_str().unwrap());
    assert!(!work.join("media/media.wav").exists());
    assert!(!work.join(".cache/download/files/media.wav").exists());
    let images: Value =
        serde_json::from_slice(&std::fs::read(work.join("images/images.json")).unwrap()).unwrap();
    assert_eq!(images, json!([]));
    let marker = std::fs::read_dir(work.join(".cache"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("frames-")
        })
        .unwrap()
        .join("complete.json");
    assert_eq!(std::fs::read(&marker).unwrap(), b"null");
    let modified = marker.metadata().unwrap().modified().unwrap();
    let chunks = count(&asr_record);
    let provider = mock();
    with_llm(&mut cfg, &provider.url);
    cfg["providers"]["speech"] = json!({"command":adapter(&asr_record, "healthy"),"concurrency":1});
    cfg["stages"]["proofread"]["images"] = json!(true);
    for _ in 0..2 {
        let output = run(root.path(), source, &cfg, &[]);
        let completed = done(&output);
        assert_eq!(completed["warnings"], json!([]));
        assert_eq!(
            document(&completed, "proofread")["sections"][0]["image"],
            ""
        );
        assert_eq!(
            document(&completed, "proofread")["sections"][0]["speech"][0]["text"],
            "Corrected speech."
        );
        assert!(!events(&output).iter().any(|e| {
            e["stage"]
                .as_str()
                .is_some_and(|name| name.starts_with("screenshots/"))
        }));
        assert!(
            !Path::new(completed["note_dir"].as_str().unwrap())
                .join("images")
                .exists()
        );
    }
    let bodies = provider.bodies.lock().unwrap();
    let content = bodies[0]["messages"][1]["content"].as_array().unwrap();
    assert_eq!(content.len(), 1);
    assert_eq!(content[0]["type"], "text");
    assert_eq!(bodies.len(), 2);
    assert_eq!(count(&download_record), 1);
    assert_eq!(count(&asr_record), chunks);
    assert_eq!(marker.metadata().unwrap().modified().unwrap(), modified);
}

#[cfg(unix)]
#[test]
fn new_note_folders_and_files_follow_umask_on_replacement() {
    use std::os::unix::{fs::PermissionsExt, process::CommandExt};
    let root = tempfile::tempdir().unwrap();
    let source = video(root.path());
    std::fs::write(
        source.with_extension("srt"),
        "1\n00:00:00,000 --> 00:00:02,000\nSelected words.\n",
    )
    .unwrap();
    let settings = root.path().join("settings.json");
    std::fs::write(&settings, serde_json::to_vec(&config()).unwrap()).unwrap();
    let mut previous = None;
    for mask in [0o022u32, 0o077u32] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_notepipe"));
        command
            .arg(&source)
            .arg("--config")
            .arg(&settings)
            .arg("--json")
            .current_dir(root.path())
            .env("XDG_CONFIG_HOME", root.path().join("config"))
            .env("XDG_CACHE_HOME", root.path().join("cache"))
            .env_remove("RUST_LOG");
        unsafe {
            command.pre_exec(move || {
                libc::umask(mask as libc::mode_t);
                Ok(())
            });
        }
        let completed = done(&command.output().unwrap());
        let note = PathBuf::from(completed["note_dir"].as_str().unwrap());
        let work = Path::new(completed["work_dir"].as_str().unwrap());
        if let Some(previous) = previous {
            assert_eq!(note, previous);
        }
        for path in [
            note.clone(),
            note.join("images"),
            note.join("note.md"),
            note.join("note.json"),
            note.join("images/0001.jpg"),
            work.join("images"),
            work.join("images/0001.jpg"),
            work.join("media/info.json"),
        ] {
            let expected = if path.is_dir() { 0o777 } else { 0o666 } & !mask;
            assert_eq!(
                path.metadata().unwrap().permissions().mode() & 0o777,
                expected,
                "{}",
                path.display()
            );
        }
        previous = Some(note);
    }
}

#[test]
fn source_command_changes_separate_caches_and_preserve_adapter_seek_templates() {
    let root = tempfile::tempdir().unwrap();
    let record = root.path().join("source");
    let mut cfg = config();
    let source = Path::new("https://EXAMPLE.test:443/media?episode=1&t=12#part");
    let mut works = Vec::new();
    for behavior in ["healthy", "seek", "healthy"] {
        cfg["stages"]["download"]["command"] = source_adapter_command(&record, behavior);
        let completed = done(&run(root.path(), source, &cfg, &[]));
        let doc = document(&completed, "draft");
        assert_eq!(doc["meta"]["source"], source.to_str().unwrap());
        if behavior == "seek" {
            assert_eq!(
                doc["meta"]["seek_url"],
                "https://example.test/play?at={t}#part"
            );
        } else {
            assert!(doc["meta"].get("seek_url").is_none());
        }
        works.push(completed["work_dir"].clone());
    }
    assert_ne!(works[0], works[1]);
    assert_eq!(works[0], works[2]);
    assert_eq!(count(&record), 2);
}

#[test]
fn run_start_recovers_only_the_recorded_transaction_before_source_failure() {
    for phase in ["staged", "old-moved", "published"] {
        let root = tempfile::tempdir().unwrap();
        let notes = root.path().join("notes");
        let note = notes.join("lecture");
        let new = notes.join(".lecture.notepipe-new-recorded");
        let old = notes.join(".lecture.notepipe-old-recorded");
        std::fs::create_dir_all(&note).unwrap();
        std::fs::write(note.join("note.md"), "Previous note.").unwrap();
        std::fs::create_dir(&new).unwrap();
        std::fs::write(new.join("note.md"), "New note.").unwrap();
        if phase != "staged" {
            std::fs::rename(&note, &old).unwrap();
        }
        if phase == "published" {
            std::fs::rename(&new, &note).unwrap();
        }
        transaction(&notes, phase);
        let unrelated = [
            notes.join(".other.notepipe-new-recorded"),
            notes.join(".other.notepipe-old-recorded"),
            notes.join("another-note"),
        ];
        for path in &unrelated {
            std::fs::create_dir(path).unwrap();
            std::fs::write(path.join("user-file"), "Unrelated content.").unwrap();
        }
        let record = root.path().join("source");
        let mut cfg = config();
        cfg["out"] = json!(notes);
        cfg["stages"]["download"]["command"] = source_adapter_command(&record, "exit");
        assert!(
            error(&run(
                root.path(),
                Path::new("https://example.test/unrelated"),
                &cfg,
                &[]
            ))
            .contains("7")
        );
        assert_eq!(count(&record), 1);
        assert!(!notes.join(".notepipe-transaction.json").exists());
        assert!(!new.exists() && !old.exists());
        assert_eq!(
            std::fs::read_to_string(note.join("note.md")).unwrap(),
            if phase == "published" {
                "New note."
            } else {
                "Previous note."
            }
        );
        for path in unrelated {
            assert_eq!(
                std::fs::read_to_string(path.join("user-file")).unwrap(),
                "Unrelated content."
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn a_second_signal_kills_the_source_tree_before_exiting_immediately() {
    use std::time::{Duration, Instant};
    let root = tempfile::tempdir().unwrap();
    let record = root.path().join("source");
    let mut cfg = config();
    cfg["stages"]["download"]["command"] = source_adapter_command(&record, "hang");
    let mut child = spawn(root.path(), "https://example.test/media", &cfg, &[], true);
    wait_for_file(&mut child, &record.join("child-ready"));
    let pid: i32 = std::fs::read_to_string(record.join("pid"))
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGINT) }, 0);
    wait_for_file(&mut child, &record.join("child-stopped"));
    let start = Instant::now();
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGTERM) }, 0);
    while child.try_wait().unwrap().is_none() {
        if start.elapsed() > Duration::from_secs(5) {
            let _ = child.kill();
            panic!("the second signal did not stop the CLI immediately");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(130));
    assert!(!events(&output).iter().any(|event| event["type"] == "done"));
    let start = Instant::now();
    while unsafe { libc::kill(pid, 0) } == 0 {
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "the download adapter was orphaned"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(unix)]
#[test]
fn cancellation_during_audio_cutting_sends_no_http_request() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let source = video(root.path());
    let record = root.path().join("asr");
    let cfg = speech_config(&record, "healthy");
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
    sleep 30
    ;;
esac
exec "$REAL_FFMPEG" "$@"
"#,
    )
    .unwrap();
    std::fs::set_permissions(wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_notepipe"))
        .arg(&source)
        .args(["--config", "-", "--json", "--skip", "screenshots"])
        .current_dir(root.path())
        .env(
            "PATH",
            format!(
                "{}:{}",
                bin.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .env("REAL_FFMPEG", which::which("ffmpeg").unwrap())
        .env("CUT_STARTED", root.path().join("cutting"))
        .env("XDG_CONFIG_HOME", root.path().join("config"))
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
    let start = std::time::Instant::now();
    while child.try_wait().unwrap().is_none() {
        if start.elapsed() > std::time::Duration::from_secs(10) {
            let _ = child.kill();
            panic!("cancellation did not stop audio cutting");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(130));
    assert!(!events(&output).iter().any(|event| event["type"] == "done"));
    assert!(!record.join("request").exists());
    assert!(record.join("terminated").is_file());
}

#[test]
fn distinct_command_providers_start_once_each_and_stop_after_their_stages() {
    let root = tempfile::tempdir().unwrap();
    let source = local(root.path());
    let mut cfg = config();
    cfg["pipeline"] = json!([
        "download",
        "transcript",
        "screenshots",
        "draft",
        "first",
        "second",
        "third",
        "markdown"
    ]);
    for (name, provider) in [("first", "one"), ("second", "two"), ("third", "one")] {
        cfg["providers"][provider] =
            json!({"command":adapter(&root.path().join(provider),"healthy")});
        cfg["stages"][name] = json!({"type":"llm","input":"transcript.json","output":format!("{name}.md"),"provider":provider,"prompt":name});
    }
    cfg["stages"]["markdown"]["input"] = json!(["first.md", "second.md", "third.md"]);
    let output = run(root.path(), &source, &cfg, &["--skip", "screenshots"]);
    let completed = done(&output);
    assert_eq!(
        std::fs::read_to_string(completed["files"]["markdown"].as_str().unwrap()).unwrap(),
        "Fixture summary.\n\nFixture summary.\n\nFixture summary.\n"
    );
    for (provider, requests) in [("one", 2), ("two", 1)] {
        let record = root.path().join(provider);
        assert_eq!(
            std::fs::read_to_string(record.join("starts")).unwrap(),
            "start\n"
        );
        assert_eq!(count(&record), requests);
        #[cfg(unix)]
        assert!(record.join("terminated").is_file());
    }
    for status in ["start", "done"] {
        assert_eq!(
            events(&output)
                .iter()
                .filter(|event| event["type"] == "stage"
                    && event["stage"]
                        .as_str()
                        .is_some_and(|stage| stage.ends_with("/provider"))
                    && event["status"] == status)
                .count(),
            2
        );
    }
}

#[test]
fn custom_paragraph_requests_cache_a_validated_response_format_fallback() {
    let root = tempfile::tempdir().unwrap();
    let source = video(root.path());
    std::fs::write(
        source.with_extension("srt"),
        "1\n00:00:00,000 --> 00:00:02,000\nOriginal speech.\n",
    )
    .unwrap();
    let provider = MockAi::respond_with(|_, body| {
        assert!(
            body["messages"][0]["content"]
                .as_str()
                .unwrap()
                .starts_with("CUSTOM-RULE-42 ")
        );
        assert_eq!(body["model"], "custom-model");
        assert_eq!(body["temperature"], 0.2);
        assert_eq!(body["top_p"], 0.8);
        assert!(body.get("max_tokens").is_none());
        assert_eq!(body["messages"][1]["content"][1]["type"], "image_url");
        Some(if body.get("response_format").is_some() {
            (422, json!({"detail":"response_format: private-test-key"}))
        } else {
            (
                200,
                json!({"choices":[{"message":{"content":r#"{"segments":[{"id":0,"text":"Cached fallback."}]}"#}}]}),
            )
        })
    });
    let mut cfg = config();
    with_llm(&mut cfg, &provider.url);
    cfg["stages"]["proofread"]["prompt"] = json!("CUSTOM-RULE-42");
    cfg["stages"]["proofread"]["model"] = json!("custom-model");
    cfg["stages"]["proofread"]["images"] = json!(true);
    cfg["stages"]["proofread"]["request"] = json!({"temperature":0.2,"top_p":0.8});
    for _ in 0..2 {
        let completed = done(&run(root.path(), &source, &cfg, &["--skip", "summary"]));
        let doc = document(&completed, "proofread");
        assert_eq!(doc["sections"][0]["speech"][0]["text"], "Cached fallback.");
        assert_eq!(doc["sections"][0]["speech"][0]["raw"], "Original speech.");
        assert_eq!(provider.calls.load(std::sync::atomic::Ordering::SeqCst), 2);
        let cache = Path::new(completed["work_dir"].as_str().unwrap()).join(".cache/llm");
        for entry in std::fs::read_dir(cache).unwrap() {
            assert!(
                !std::fs::read_to_string(entry.unwrap().path())
                    .unwrap()
                    .contains("private-test-key")
            );
        }
    }
}

#[test]
fn pipeline_stage_names_reach_boundaries_progress_and_token_events() {
    let root = tempfile::tempdir().unwrap();
    let source = video(root.path());
    let record = root.path().join("speech");
    let provider = mock();
    let mut cfg = config();
    with_llm(&mut cfg, &provider.url);
    cfg["providers"]["speech"] = json!({"command":adapter(&record,"healthy")});
    cfg["stages"]["transcript"]["sources"] = json!(["speech"]);
    cfg["stages"]["transcript"]["speech"] = json!({"provider":"speech"});
    let output = run(root.path(), &source, &cfg, &[]);
    done(&output);
    let all = events(&output);
    for stage in [
        "download",
        "transcript",
        "transcript/audio",
        "transcript/speech",
        "transcript/provider",
        "screenshots",
        "screenshots/scan",
        "screenshots/extract",
        "draft",
        "proofread",
        "summary",
        "markdown",
    ] {
        for status in ["start", "done"] {
            assert!(
                all.iter().any(|event| event["type"] == "stage"
                    && event["stage"] == stage
                    && event["status"] == status),
                "missing {stage}/{status}: {all:?}"
            );
        }
    }
    assert!(
        all.iter()
            .any(|event| event["type"] == "progress" && event["stage"] == "transcript/speech")
    );
    for stage in ["proofread", "summary"] {
        assert!(all.iter().any(|event| event["type"] == "tokens"
            && event["stage"] == stage
            && event["prompt"] == 3
            && event["completion"] == 2));
    }
    assert_eq!(
        all.iter()
            .filter(|event| event["type"] == "stage"
                && event["status"] == "start"
                && event["stage"]
                    .as_str()
                    .is_some_and(|stage| stage.ends_with("/provider")))
            .count(),
        1
    );
    assert!(!all.iter().any(|event| {
        event["stage"]
            .as_str()
            .is_some_and(|stage| stage.starts_with("model/"))
    }));
    assert!(!all.iter().any(|event| matches!(
        event["stage"].as_str(),
        Some("fetch" | "subtitle" | "audio" | "transcribe" | "render" | "adapter/start")
    )));
}
