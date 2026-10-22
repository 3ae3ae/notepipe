//! Exercise the public CLI contract without network access or real credentials.
use serde_json::{Value, json};
use std::process::{Command, Output};
struct CliTest {
    dir: tempfile::TempDir,
}
impl CliTest {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
        }
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_notepipe"))
            .args(args)
            .current_dir(self.dir.path())
            .env("XDG_CONFIG_HOME", self.dir.path().join("config"))
            .env("XDG_CACHE_HOME", self.dir.path().join("cache"))
            .env_remove("RUST_LOG")
            .env_remove("OPENAI_API_KEY")
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap()
    }
    fn path(&self) -> std::path::PathBuf {
        self.dir
            .path()
            .join("config")
            .join("notepipe")
            .join("config.toml")
    }
    fn config(&self, text: &str) {
        std::fs::create_dir_all(self.path().parent().unwrap()).unwrap();
        std::fs::write(self.path(), text).unwrap();
    }
    fn config_value(&self, value: &Value) {
        self.config(&value.to_string());
    }
}
fn config() -> Value {
    json!({"pipeline":["download","transcript","screenshots","draft","markdown"],"providers":{},"stages":{
        "download":{"type":"download","input":"source","output":"media","command":["notepipe-source-ytdlp"]},
        "transcript":{"type":"transcript","input":"media","output":"transcript.json","sources":["subtitles"]},
        "screenshots":{"type":"screenshots","input":"media","output":"images"},
        "draft":{"type":"document","input":["transcript.json","images"],"output":"draft.json","publish":"note.json"},
        "markdown":{"type":"markdown","input":"draft.json","output":"note.md","publish":"note.md"}
    }})
}
fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}
fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}
fn events(output: &Output) -> Vec<Value> {
    stdout(output)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}
fn source_json_error(output: &Output) -> Value {
    assert!(!output.status.success());
    assert!(output.stderr.is_empty(), "{}", stderr(output));
    let error = events(output).last().unwrap().clone();
    assert_eq!(error["type"], "error");
    error
}
fn json_error(output: &Output) -> Value {
    let error = source_json_error(output);
    assert_eq!(events(output).len(), 1, "{}", stdout(output));
    error
}
#[test]
fn no_arguments_or_help_succeed_with_guidance_and_only_supported_flags() {
    let cli = CliTest::new();
    for args in [vec![], vec!["--help"]] {
        let output = cli.run(&args);
        assert!(output.status.success());
        let text = stdout(&output);
        for expected in [
            "Examples:",
            "The transcript stage tries its sources in order:",
            "See notepipe config show for the pipeline in effect.",
            "--skip proofread,summary",
        ] {
            assert!(text.contains(expected), "{text}");
        }
    }
    let text = stdout(&cli.run(&["--help"]));
    for option in [
        "--out",
        "--config",
        "--no-resume",
        "--subtitle",
        "--language",
        "--screenshot-mode",
        "--sensitivity",
        "--frame-interval",
        "--min-gap",
        "--settle",
        "--region",
        "--skip",
        "--verbose",
        "--quiet",
        "--json",
    ] {
        assert!(text.contains(option), "{option}");
    }
    for option in [
        "--stages",
        "--no-stages",
        "--transcript",
        "--max-segment",
        "--print",
    ] {
        assert!(!text.contains(option), "{option}");
        assert!(!cli.run(&["media.mp4", option, "value"]).status.success());
    }
    assert!(text.contains(
        "Leave these stages out of the pipeline for this run, such as proofread,summary"
    ));
    for command in ["config", "doctor"] {
        assert!(cli.run(&[command, "--help"]).status.success());
    }
    let output = cli.run(&["config"]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains(" config <COMMAND>"));
}
#[test]
fn config_path_init_and_show_share_the_literal_template() {
    let cli = CliTest::new();
    let path = cli.path();
    assert_eq!(
        stdout(&cli.run(&["config", "path"])),
        format!("{}\n", path.display())
    );
    assert!(stdout(&cli.run(&["--help"])).contains(&format!(
        "File: {} (created on the first run)",
        path.display()
    )));
    let shown = cli.run(&["config", "show"]);
    assert!(shown.status.success(), "{}", stderr(&shown));
    let text = stdout(&shown);
    assert!(text.starts_with(&format!("Configuration file: {}\n", path.display())));
    let parsed: toml::Value = toml::from_str(text.split_once('\n').unwrap().1).unwrap();
    assert_eq!(
        parsed["stages"]["screenshots"]["settle"].as_float(),
        Some(0.8)
    );
    assert_eq!(parsed["out"].as_str(), Some("."));
    assert!(!path.exists());
    let created = cli.run(&["config", "init"]);
    assert!(created.status.success());
    assert_eq!(
        stdout(&created),
        format!(
            "Created {}\nEvery value in it is in effect; edit it to change how notes are made.\n",
            path.display()
        )
    );
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        include_str!("../src/config.toml")
    );
    cli.config("preserve this");
    assert!(stderr(&cli.run(&["config", "init"])).contains("already exists"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "preserve this");
    assert!(cli.run(&["config", "init", "--force"]).status.success());
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        include_str!("../src/config.toml")
    );
    assert!(stdout(&cli.run(&["--help"])).contains(&format!("File: {}\n", path.display())));
}
#[test]
fn config_show_serializes_defaults_without_resolving_keys() {
    let cli = CliTest::new();
    let mut cfg = config();
    cfg["providers"]["p"] = json!({"command":["program"],"api_key_env":"NOTEPIPE_UNSET_SHOW_KEY"});
    cfg["stages"]["screenshots"]["mode"] = json!("first");
    cli.config_value(&cfg);
    let output = cli.run(&["config", "show"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let value: toml::Value = toml::from_str(stdout(&output).split_once('\n').unwrap().1).unwrap();
    assert_eq!(value["providers"]["p"]["concurrency"].as_integer(), Some(4));
    assert!(value["providers"]["p"].get("args").is_none());
    assert_eq!(
        value["stages"]["screenshots"]["mode"].as_str(),
        Some("first")
    );
    assert_eq!(
        value["stages"]["transcript"]["language"].as_str(),
        Some("auto")
    );
}
#[test]
fn missing_and_invalid_sources_and_arguments_have_json_errors() {
    let cli = CliTest::new();
    assert!(stderr(&cli.run(&["--language", "ko"])).contains("Missing video or audio source"));
    assert!(stderr(&cli.run(&["missing lecture.mp4"])).contains("Check the path"));
    for args in [
        vec!["--json"],
        vec!["missing.mp4", "--json"],
        vec!["missing.mp4", "--unknown", "--json"],
    ] {
        json_error(&cli.run(&args));
    }
    std::fs::write(cli.dir.path().join("media.mp4"), []).unwrap();
    cli.config("[broken");
    assert!(
        json_error(&cli.run(&["media.mp4", "--json"]))["message"]
            .as_str()
            .unwrap()
            .contains("configuration")
    );
}
#[test]
fn validation_and_skip_errors_precede_source_work() {
    let cli = CliTest::new();
    cli.config_value(&config());
    for (flags, expected) in [
        (vec!["--frame-interval", "0"], "--frame-interval"),
        (
            vec!["--skip", "unknown"],
            "--skip: no stage named unknown in the pipeline",
        ),
        (
            vec!["--skip", "draft,markdown"],
            "Nothing is published; set publish on a stage",
        ),
        (
            vec!["--skip", "transcript"],
            "--skip transcript: draft needs transcript.json",
        ),
    ] {
        let mut args = vec!["https://example.test/media", "--json"];
        args.extend(flags);
        assert!(
            json_error(&cli.run(&args))["message"]
                .as_str()
                .unwrap()
                .contains(expected)
        );
    }
    for (option, value) in [
        ("--screenshot-mode", "first"),
        ("--sensitivity", "0.5"),
        ("--frame-interval", "1"),
        ("--min-gap", "1"),
        ("--settle", "1"),
        ("--region", "0%,0%-100%,100%"),
    ] {
        let error = json_error(&cli.run(&[
            "https://example.test/media",
            "--skip",
            "screenshots",
            option,
            value,
            "--json",
        ]));
        assert_eq!(
            error["message"],
            format!("{option} needs a screenshots stage in the pipeline")
        );
    }
    assert!(!cli.dir.path().join("cache").exists());
}
#[test]
fn auto_creation_notices_and_fallback_use_the_built_in_template() {
    for mode in ["", "--json", "--quiet"] {
        let cli = CliTest::new();
        let notice = format!(
            "Created {}. Speech recognition and the proofread and summary stages use OpenAI with the key in OPENAI_API_KEY; edit the file to change that.",
            cli.path().display()
        );
        let mut args = vec!["https://example.test/media", "--frame-interval", "0"];
        if !mode.is_empty() {
            args.push(mode);
        }
        let first = cli.run(&args);
        assert!(!first.status.success());
        assert_eq!(
            std::fs::read_to_string(cli.path()).unwrap(),
            include_str!("../src/config.toml")
        );
        if mode == "--json" {
            assert_eq!(
                events(&first)[0],
                json!({"type":"log","level":"info","message":notice})
            );
            assert!(first.stderr.is_empty());
        } else {
            assert_eq!(stderr(&first).contains(&notice), mode != "--quiet");
        }
        let second = cli.run(&args);
        assert!(!format!("{}{}", stdout(&second), stderr(&second)).contains(&notice));
    }
    let cli = CliTest::new();
    std::fs::write(cli.dir.path().join("config"), "blocked").unwrap();
    let output = cli.run(&["https://example.test/media"]);
    let text = stderr(&output);
    assert!(text.contains(". Continuing with the default settings."));
    assert!(
        text.contains("Provider openai: the environment variable OPENAI_API_KEY is not set"),
        "{text}"
    );
    assert!(!cli.dir.path().join("cache").exists());
}
#[test]
fn explicit_configs_accept_json_and_toml_from_files_and_stdin() {
    use std::io::Write;
    let cli = CliTest::new();
    cli.config("[broken ambient config");
    for text in [
        serde_json::to_string(&config()).unwrap(),
        toml::to_string(&config()).unwrap(),
    ] {
        for input in ["settings.conf", "-"] {
            std::fs::write(cli.dir.path().join("settings.conf"), &text).unwrap();
            let mut child = Command::new(env!("CARGO_BIN_EXE_notepipe"))
                .args([
                    "https://example.test/media",
                    "--config",
                    input,
                    "--frame-interval",
                    "0",
                    "--json",
                ])
                .current_dir(cli.dir.path())
                .env("XDG_CACHE_HOME", cli.dir.path().join("cache"))
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(text.as_bytes())
                .unwrap();
            assert!(
                json_error(&child.wait_with_output().unwrap())["message"]
                    .as_str()
                    .unwrap()
                    .contains("--frame-interval")
            );
        }
    }
    for text in [
        "[providers.p]\napi_key = 'private-test-key",
        "{\"providers\":{\"p\":{\"api_key\":\"private-test-key",
    ] {
        std::fs::write(cli.dir.path().join("invalid"), text).unwrap();
        let error = json_error(&cli.run(&[
            "https://example.test/media",
            "--config",
            "invalid",
            "--json",
        ]));
        assert!(error["message"].as_str().unwrap().contains("configuration"));
        assert!(!error.to_string().contains("private-test-key"));
    }
    let cli = CliTest::new();
    std::fs::write(
        cli.dir.path().join("explicit"),
        serde_json::to_vec(&config()).unwrap(),
    )
    .unwrap();
    json_error(&cli.run(&[
        "https://example.test/media",
        "--config",
        "explicit",
        "--frame-interval",
        "0",
        "--json",
    ]));
    assert!(!cli.path().exists());
}
#[test]
fn doctor_reports_programs_keys_and_required_tools_offline() {
    let cli = CliTest::new();
    let mut cfg = config();
    let program = env!("CARGO_BIN_EXE_notepipe");
    cfg["providers"] = json!({"p":{"command":[program]},"unused":{"command":[program]}});
    cfg["stages"]["transcript"] = json!({"type":"transcript","input":"media","output":"transcript.json","speech":{"provider":"p"}});
    for kind in ["download", "run"] {
        cfg["stages"][kind] = json!({"type":kind,"input":if kind == "download" {json!("source")} else {json!([])},"output":kind,"command":[format!("missing-{kind}-program")]});
    }
    cli.config_value(&cfg);
    let output = cli.run(&["doctor"]);
    let text = stdout(&output);
    assert!(!output.status.success());
    assert_eq!(text.matches(&format!("✓ AI adapter  {program}")).count(), 1);
    for kind in ["download", "run"] {
        assert!(text.contains(&format!("missing-{kind}-program")), "{text}");
    }
    cfg["providers"]["p"]["api_key_env"] = json!("NOTEPIPE_UNSET_DOCTOR_KEY");
    cli.config_value(&cfg);
    assert!(
        stdout(&cli.run(&["doctor"]))
            .contains("Provider p: the environment variable NOTEPIPE_UNSET_DOCTOR_KEY is not set")
    );
    cfg["stages"]["transcript"]["speech"]["provider"] = json!("missing");
    cli.config_value(&cfg);
    assert!(
        stdout(&cli.run(&["doctor"]))
            .contains("Stage transcript: no provider named missing in [providers]")
    );
    let output = Command::new(env!("CARGO_BIN_EXE_notepipe"))
        .arg("doctor")
        .env("PATH", "")
        .env("XDG_CONFIG_HOME", cli.dir.path().join("config"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(stdout(&output).contains("https://ffmpeg.org/download.html"));
    assert!(stderr(&output).contains("required-tool or configuration issues"));
}
#[test]
fn version_flags_include_only_an_explicit_build_commit() {
    let suffix = option_env!("NOTEPIPE_COMMIT_HASH")
        .map(|hash| format!(" ({hash})"))
        .unwrap_or_default();
    for flag in ["-V", "--version"] {
        let output = CliTest::new().run(&[flag]);
        assert!(output.status.success());
        assert_eq!(
            stdout(&output),
            format!("notepipe {}{suffix}\n", env!("CARGO_PKG_VERSION"))
        );
    }
}
fn source_config(cli: &CliTest, behavior: &str) -> Option<std::path::PathBuf> {
    let python = which::which("python3")
        .ok()
        .or_else(|| which::which("python").ok())?;
    let record = cli.dir.path().join("source record");
    let command: Vec<String> = vec![
        python.display().to_string(),
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/source_adapter.py"
        )
        .into(),
        "--record".into(),
        record.display().to_string(),
        "--behavior".into(),
        behavior.into(),
        "--".into(),
        "--format".into(),
        "fixture quality".into(),
    ];
    let mut cfg = config();
    cfg["stages"]["download"] =
        serde_json::json!({"type":"download","input":"source","output":"media","command":command});
    cli.config_value(&cfg);
    Some(record)
}

#[test]
fn urls_require_an_explicit_download_stage_without_path_discovery() {
    let cli = CliTest::new();
    cli.config_value(&json!({"pipeline":["custom"],"stages":{"custom":{"type":"run","output":"file","publish":"file","command":["must-not-start"]}}}));
    let error = source_json_error(&cli.run(&["https://example.test/media", "--json"]));
    assert_eq!(
        error["message"],
        "URLs need a download stage in the pipeline; install one with uv tool install \"notepipe[ytdlp]\""
    );
    assert!(!cli.dir.path().join("cache").exists());
}
#[test]
fn source_adapter_errors_include_diagnostics() {
    for behavior in ["malformed", "parent", "absolute", "exit"] {
        let cli = CliTest::new();
        if source_config(&cli, behavior).is_none() {
            return;
        }
        let error = source_json_error(&cli.run(&["https://example.test/media", "--json"]));
        let message = error["message"].as_str().unwrap();
        assert!(
            message.to_lowercase().contains("download adapter"),
            "{message}"
        );
        assert!(
            message.contains("source adapter fixture diagnostics"),
            "{message}"
        );
    }
}
#[test]
fn source_adapter_generates_notes_and_reuses_completed_output() {
    if which::which("ffmpeg").is_err() || which::which("ffprobe").is_err() {
        return;
    }
    let cli = CliTest::new();
    let Some(record) = source_config(&cli, "healthy") else {
        return;
    };
    let url = "https://example.test/video?episode=2";
    for flag in ["--json", "--quiet"] {
        let output = cli.run(&[url, "--screenshot-mode", "first", flag]);
        assert!(
            output.status.success(),
            "{} {}",
            stdout(&output),
            stderr(&output)
        );
        assert!(output.stderr.is_empty(), "{}", stderr(&output));
        if flag == "--quiet" {
            assert!(output.stdout.is_empty());
            continue;
        }
        let all = events(&output);
        assert!(
            all.iter()
                .any(|e| e["type"] == "progress" && e["stage"] == "download")
        );
        let done = all.last().unwrap();
        assert_eq!(done["type"], "done");
        assert_eq!(done["warnings"], json!([]));
        let note = std::fs::read_to_string(done["files"]["markdown"].as_str().unwrap()).unwrap();
        assert!(note.contains("Welcome to the source adapter."));
        assert!(!note.contains("Automatic captions."));
        let doc: Value = serde_json::from_slice(
            &std::fs::read(done["files"]["draft"].as_str().unwrap()).unwrap(),
        )
        .unwrap();
        assert_eq!(doc["meta"]["author"], "Fixture author");
        assert_eq!(doc["meta"]["source"], url);
    }
    let calls = std::fs::read_to_string(record.join("calls")).unwrap();
    let calls: Vec<Value> = calls
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["url"], url);
    assert_eq!(calls[0]["extra"], json!(["--format", "fixture quality"]));
}
#[test]
fn missing_or_empty_active_provider_keys_fail_before_source_or_cache_work() {
    for key in [None, Some("")] {
        let cli = CliTest::new();
        let Some(record) = source_config(&cli, "healthy") else {
            return;
        };
        let mut cfg: Value = serde_json::from_slice(&std::fs::read(cli.path()).unwrap()).unwrap();
        cfg["providers"]["p"] =
            json!({"command":["must-not-start"],"api_key_env":"NOTEPIPE_MISSING_EARLY_KEY"});
        cfg["stages"]["transcript"] = json!({"type":"transcript","input":"media","output":"transcript.json","sources":["speech"],"speech":{"provider":"p"}});
        cli.config_value(&cfg);
        let mut command = Command::new(env!("CARGO_BIN_EXE_notepipe"));
        command
            .args(["https://example.test/media", "--json"])
            .current_dir(cli.dir.path())
            .env("XDG_CONFIG_HOME", cli.dir.path().join("config"))
            .env("XDG_CACHE_HOME", cli.dir.path().join("cache"));
        if let Some(key) = key {
            command.env("NOTEPIPE_MISSING_EARLY_KEY", key);
        } else {
            command.env_remove("NOTEPIPE_MISSING_EARLY_KEY");
        }
        let error = source_json_error(&command.output().unwrap());
        assert_eq!(
            error["message"],
            "Provider p: the environment variable NOTEPIPE_MISSING_EARLY_KEY is not set"
        );
        assert!(!cli.dir.path().join("cache").exists());
        assert!(!record.exists());
    }
}

#[test]
fn configuration_errors_name_the_path_line_and_key_without_source_snippets() {
    let cli = CliTest::new();
    let cases = [
        (
            "pipeline = []\nout = 'private-test-value'\nunexpected = true\n",
            3,
            "unknown field `unexpected`",
        ),
        (
            "pipeline = []\nout = 'private-test-value'\n[providers.p]\nconcurrency = 'wrong-type'\n",
            4,
            "invalid type: string",
        ),
        ("pipeline = [\n", 2, "invalid array"),
        (
            "{\n  \"pipeline\": [],\n  \"out\": \"private-test-value\",\n  \"unexpected\": true\n}\n",
            4,
            "unknown field `unexpected`",
        ),
        (
            "{\n  \"pipeline\": [],\n  \"out\": false\n}",
            3,
            "invalid type: boolean",
        ),
        ("{\n  \"pipeline\": [\n", 3, "EOF while parsing a list"),
    ];
    for (text, line, message) in cases {
        cli.config(text);
        let ambient = stderr(&cli.run(&["config", "show"]));
        assert!(
            ambient.starts_with(&format!(
                "Error: Fix the configuration and retry: {}: line {line}: ",
                cli.path().display()
            )),
            "{ambient}"
        );
        assert!(ambient.contains(message), "{ambient}");
        assert!(!ambient.contains("private-test-value"), "{ambient}");
        assert!(!ambient.contains(" | "), "{ambient}");
        let path = cli.dir.path().join("explicit.conf");
        std::fs::write(&path, text).unwrap();
        let explicit = json_error(&cli.run(&[
            "https://example.test/media",
            "--config",
            "explicit.conf",
            "--json",
        ]));
        let explicit = explicit["message"].as_str().unwrap();
        assert!(
            explicit.starts_with(&format!(
                "Fix the configuration and retry: explicit.conf: line {line}: "
            )),
            "{explicit}"
        );
        assert!(explicit.contains(message), "{explicit}");
        assert!(!explicit.contains("private-test-value"), "{explicit}");
        assert!(!explicit.contains(" | "), "{explicit}");
    }
}

#[test]
fn doctor_treats_only_download_programs_as_optional_and_deduplicates_provider_keys() {
    let cli = CliTest::new();
    let mut cfg = config();
    let program = "notepipe-missing-doctor-program";
    cfg["stages"]["download"]["command"] = json!([program]);
    cli.config_value(&cfg);
    let output = cli.run(&["doctor"]);
    assert!(
        output.status.success(),
        "{} {}",
        stdout(&output),
        stderr(&output)
    );
    assert!(
        stdout(&output).lines().any(|line| line
            == format!("- Download adapter {program}  Not installed; needed only for URLs"))
    );
    assert!(!stdout(&output).contains("Optional tools by use"));
    cfg["stages"]["custom"] = json!({"type":"run","output":"custom","command":[program]});
    cli.config_value(&cfg);
    let output = cli.run(&["doctor"]);
    assert!(!output.status.success());
    assert!(stdout(&output).contains(&format!("✗ Adapter {program}")));
    cfg["stages"].as_object_mut().unwrap().remove("custom");
    cfg["providers"]["p"] = json!({"command":[program]});
    cli.config_value(&cfg);
    let output = cli.run(&["doctor"]);
    assert!(!output.status.success());
    assert!(stdout(&output).contains(&format!("✗ AI adapter {program}")));
    cfg["providers"]["p"] =
        json!({"base_url":"http://127.0.0.1:9/v1","api_key_env":"NOTEPIPE_UNSET_DOCTOR_KEY"});
    cfg["stages"]["transcript"]["sources"] = json!(["speech"]);
    cfg["stages"]["transcript"]["speech"] = json!({"provider":"p"});
    cfg["pipeline"] = json!([
        "download",
        "transcript",
        "screenshots",
        "draft",
        "one",
        "two",
        "markdown"
    ]);
    for name in ["one", "two"] {
        cfg["stages"][name] =
            json!({"type":"llm","input":"draft.json","output":format!("{name}.md"),"provider":"p"});
    }
    cli.config_value(&cfg);
    let output = cli.run(&["doctor"]);
    assert!(!output.status.success());
    assert_eq!(
        stdout(&output)
            .matches("Provider p: the environment variable NOTEPIPE_UNSET_DOCTOR_KEY is not set")
            .count(),
        1
    );
}
