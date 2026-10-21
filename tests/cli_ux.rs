//! Exercise the public CLI contract without network access or real credentials.
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
            .env_remove("NOTEPIPE_ASR_API_KEY")
            .env_remove("NOTEPIPE_LLM_API_KEY")
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap()
    }
    fn config(&self, contents: &str) {
        let dir = self.dir.path().join("config/notepipe");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.toml"), contents).unwrap();
    }
}
fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}
fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}
fn json_error(output: &Output) -> serde_json::Value {
    assert!(!output.status.success());
    assert!(output.stderr.is_empty(), "{}", stderr(output));
    let lines: Vec<_> = output
        .stdout
        .split(|b| *b == b'\n')
        .filter(|line| !line.is_empty())
        .collect();
    assert_eq!(lines.len(), 1, "{}", stdout(output));
    let value: serde_json::Value = serde_json::from_slice(lines[0]).unwrap();
    assert_eq!(value["type"], "error");
    value
}

#[test]
fn no_arguments_or_help_succeed_with_guidance() {
    let cli = CliTest::new();
    for args in [vec![], vec!["--help"]] {
        let result = cli.run(&args);
        assert!(result.status.success());
        let text = stdout(&result);
        assert!(text.contains("Quick start"), "{text}");
        assert!(
            text.to_ascii_lowercase().contains("speech recognition"),
            "{text}"
        );
    }
    let help = cli.run(&["--help"]);
    assert!(help.status.success());
    let text = stdout(&help);
    assert!(text.contains("Quick start"), "{text}");
    assert!(text.contains("Speech recognition"), "{text}");
    for cmd in ["config", "doctor"] {
        assert!(cli.run(&[cmd, "--help"]).status.success(), "{cmd}");
    }
}

#[test]
fn missing_or_invalid_sources_fail_with_recovery() {
    let cli = CliTest::new();
    let missing = cli.run(&["--asr-mode", "chat"]);
    assert!(!missing.status.success());
    assert!(
        stderr(&missing).contains("Missing video source"),
        "{}",
        stderr(&missing)
    );
    let invalid = cli.run(&["./missing lecture.mp4"]);
    assert!(!invalid.status.success());
    assert!(
        stderr(&invalid).contains("Check the path"),
        "{}",
        stderr(&invalid)
    );
    assert!(invalid.stdout.is_empty());
}

#[test]
fn json_covers_missing_source_parse_and_config_errors() {
    let cli = CliTest::new();
    json_error(&cli.run(&["--json"]));
    json_error(&cli.run(&["./missing.mp4", "--json"]));
    json_error(&cli.run(&["./missing.mp4", "--json", "--asr-mode", "invalid"]));
    std::fs::write(cli.dir.path().join("lecture.mp4"), []).unwrap();
    cli.config("[defaults\n");
    let error = json_error(&cli.run(&["./lecture.mp4", "--json"]));
    assert!(error["message"].as_str().unwrap().contains("configuration"));
}

#[test]
fn validation_precedes_dependency_checks_and_downloads() {
    let cli = CliTest::new();
    std::fs::write(cli.dir.path().join("lecture.mp4"), []).unwrap();
    let result = cli.run(&["./lecture.mp4", "--sample-interval", "0", "--json"]);
    let error = json_error(&result);
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("--sample-interval")
    );
    assert!(!cli.dir.path().join("cache").exists());
}

#[test]
fn translation_accepts_language_names_and_rejects_invalid_cli_and_config_values() {
    let cli = CliTest::new();
    std::fs::write(cli.dir.path().join("lecture.mp4"), []).unwrap();
    for target in [" pt-BR ", "Arabic", "Canadian French"] {
        let error = json_error(&cli.run(&[
            "./lecture.mp4",
            "--translate",
            target,
            "--sample-interval",
            "0",
            "--json",
        ]));
        assert!(
            error["message"]
                .as_str()
                .unwrap()
                .contains("--sample-interval")
        );
    }
    for target in ["", "  ", "en\n", &"語".repeat(41)] {
        let error = json_error(&cli.run(&["./lecture.mp4", "--translate", target, "--json"]));
        assert!(
            error["message"]
                .as_str()
                .unwrap()
                .contains("Translation target")
        );
        cli.config(&format!(
            "[llm]\ntranslate = {}\n",
            serde_json::to_string(target).unwrap()
        ));
        let error = json_error(&cli.run(&["./lecture.mp4", "--json"]));
        assert!(
            error["message"]
                .as_str()
                .unwrap()
                .contains("Translation target")
        );
    }
    assert!(!cli.dir.path().join("cache").exists());
}

#[test]
fn removed_login_and_setup_commands_fail_without_saving() {
    let cli = CliTest::new();
    for args in [
        vec!["--login", "codex"],
        vec!["--logout", "ollama"],
        vec!["--no-llm-hint"],
        vec!["llm", "setup"],
        vec!["llm", "status"],
        vec!["llm", "disable"],
        vec!["remove", "--asr"],
    ] {
        assert!(!cli.run(&args).status.success(), "{args:?}");
    }
    let help = stdout(&cli.run(&["--help"]));
    for removed in ["--login", "--logout", "--no-llm-hint", "llm setup"] {
        assert!(!help.contains(removed), "{help}");
    }
    assert!(!cli.dir.path().join("config/notepipe").exists());
}

#[test]
fn config_show_reports_the_defaults_conversion_uses() {
    let cli = CliTest::new();
    let result = cli.run(&["config", "show"]);
    assert!(result.status.success());
    let text = stdout(&result);
    let value = |key: &str| {
        text.lines()
            .find(|line| line.trim_start().starts_with(key))
            .and_then(|line| line.split(':').nth(1))
            .map(str::trim)
            .unwrap_or_default()
            .to_owned()
    };
    assert_eq!(value("out "), ".");
    assert_eq!(value("stable_secs "), "0.8");
}

#[test]
fn settings_never_print_saved_keys() {
    let cli = CliTest::new();
    cli.config("[llm]\napi_key = 'secret-llm-test-key'\n[asr]\napi_key = 'secret-asr-test-key'\n");
    let result = cli.run(&["config", "show"]);
    assert!(result.status.success());
    assert!(!stdout(&result).contains("secret-"));
}

#[test]
fn generated_template_parses_and_does_not_overwrite_existing_config() {
    let cli = CliTest::new();
    assert!(cli.run(&["config", "init"]).status.success());
    assert!(cli.run(&["config", "show"]).status.success());
    cli.config("[defaults]\nsimilarity = 0.7\n");
    assert!(!cli.run(&["config", "init"]).status.success());
    let file = std::fs::read_to_string(cli.dir.path().join("config/notepipe/config.toml")).unwrap();
    assert!(file.contains("similarity = 0.7"));
}

#[test]
fn doctor_reports_required_failures_with_nonzero_exit() {
    let cli = CliTest::new();
    let result = Command::new(env!("CARGO_BIN_EXE_notepipe"))
        .arg("doctor")
        .env("PATH", "")
        .env("XDG_CONFIG_HOME", cli.dir.path().join("config"))
        .env("XDG_CACHE_HOME", cli.dir.path().join("cache"))
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(
        stdout(&result).contains("Required tools"),
        "{}",
        stdout(&result)
    );
    assert!(
        stderr(&result).contains("required-tool or configuration issues"),
        "{}",
        stderr(&result)
    );
}

#[test]
fn version_flags_include_only_an_explicit_build_commit() {
    let cli = CliTest::new();
    let suffix = option_env!("NOTEPIPE_COMMIT_HASH")
        .map(|hash| format!(" ({hash})"))
        .unwrap_or_default();
    let expected = format!("notepipe {}{suffix}\n", env!("CARGO_PKG_VERSION"));
    for flag in ["-V", "--version"] {
        let result = cli.run(&[flag]);
        assert!(result.status.success());
        assert!(result.stderr.is_empty());
        assert_eq!(stdout(&result), expected);
    }
}

fn source_json_error(output: &Output) -> serde_json::Value {
    assert!(!output.status.success());
    assert!(output.stderr.is_empty(), "{}", stderr(output));
    let events: Vec<serde_json::Value> = stdout(output)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let error = events.last().unwrap().clone();
    assert_eq!(error["type"], "error");
    error
}

fn source_config(cli: &CliTest, behavior: &str) -> Option<std::path::PathBuf> {
    let python =
        notepipe::runtime::which("python3").or_else(|| notepipe::runtime::which("python"))?;
    let record = cli.dir.path().join("source record");
    let command = vec![
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
    let mut config = notepipe::settings::ConfigFile::default();
    config.source.command = command;
    cli.config(&toml::to_string(&config).unwrap());
    Some(record)
}

#[test]
fn missing_source_adapter_reports_generic_guidance() {
    let cli = CliTest::new();
    cli.config("[source]\ncommand = ['notepipe-test-missing-source-adapter']\n");
    let error = source_json_error(&cli.run(&["https://example.test/video", "--json"]));
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains(notepipe::source::ADAPTER_HINT)
    );
}

#[test]
fn invalid_source_adapter_output_is_rejected() {
    for behavior in ["malformed", "parent", "absolute", "exit"] {
        let cli = CliTest::new();
        if source_config(&cli, behavior).is_none() {
            return;
        }
        let error = source_json_error(&cli.run(&["https://example.test/video", "--json"]));
        let message = error["message"].as_str().unwrap();
        assert!(
            message.contains("source adapter") || message.contains("Source adapter"),
            "{message}"
        );
        assert!(
            message.contains("source adapter fixture diagnostics"),
            "{message}"
        );
        assert!(!cli.dir.path().join("out/web").exists());
    }
}

#[test]
fn source_adapter_generates_notes_and_reuses_its_completed_output() {
    if notepipe::runtime::which("ffmpeg").is_none() || notepipe::runtime::which("ffprobe").is_none()
    {
        return;
    }
    let cli = CliTest::new();
    let Some(record) = source_config(&cli, "healthy") else {
        return;
    };
    let url = "https://example.test/video?episode=2";
    let invalid = source_json_error(&cli.run(&[url, "--summarize", "--json"]));
    assert!(
        invalid["message"]
            .as_str()
            .unwrap()
            .contains("[llm].base_url")
    );
    assert!(!record.join("calls").exists());
    for flag in ["--json", "--quiet"] {
        let output = cli.run(&[
            url,
            "--transcript-source",
            "subtitle",
            "--slide-mode",
            "first",
            flag,
        ]);
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
        let events: Vec<serde_json::Value> = stdout(&output)
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert!(
            events
                .iter()
                .any(|event| event["type"] == "progress" && event["stage"] == "download")
        );
        let done = events.last().unwrap();
        assert_eq!(done["type"], "done");
        assert_eq!(done["warnings"], serde_json::json!([]));
        let note = cli.dir.path().join(done["markdown"].as_str().unwrap());
        let note = std::fs::read_to_string(note).unwrap();
        assert!(note.contains("Welcome to the source adapter."), "{note}");
        assert!(!note.contains("Automatic captions."));
        assert!(note.contains("Fixture author"));
        assert!(note.contains(url));
    }
    let calls = std::fs::read_to_string(record.join("calls")).unwrap();
    let calls: Vec<serde_json::Value> = calls
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["url"], url);
    assert_eq!(
        calls[0]["extra"],
        serde_json::json!(["--format", "fixture quality"])
    );
}

#[test]
fn forced_asr_settings_are_checked_before_running_a_source_adapter() {
    let cli = CliTest::new();
    let Some(record) = source_config(&cli, "healthy") else {
        return;
    };
    let error = source_json_error(&cli.run(&[
        "https://example.test/media",
        "--transcript-source",
        "asr",
        "--json",
    ]));
    assert!(error["message"].as_str().unwrap().contains("[asr]"));
    assert!(!record.join("calls").exists());
}

#[cfg(unix)]
#[test]
fn unconfigured_sources_discover_a_single_adapter_and_report_ambiguity() {
    use std::os::unix::fs::PermissionsExt;
    let Some(python) =
        notepipe::runtime::which("python3").or_else(|| notepipe::runtime::which("python"))
    else {
        return;
    };
    let cli = CliTest::new();
    cli.config("[source]\ncommand = []\n");
    let bin = cli.dir.path().join("bin");
    let record = cli.dir.path().join("record");
    std::fs::create_dir(&bin).unwrap();
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_notepipe"))
            .args(["https://example.test/media", "--json"])
            .env("PATH", &bin)
            .env("XDG_CONFIG_HOME", cli.dir.path().join("config"))
            .env("XDG_CACHE_HOME", cli.dir.path().join("cache"))
            .env_remove("RUST_LOG")
            .current_dir(cli.dir.path())
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap()
    };
    let missing = source_json_error(&run());
    assert!(
        missing["message"]
            .as_str()
            .unwrap()
            .contains(notepipe::source::ADAPTER_HINT)
    );
    let quote = |value: &std::path::Path| {
        format!("'{}'", value.display().to_string().replace('\'', "'\\''"))
    };
    let script = format!(
        "#!/bin/sh\nexec {} {} --record {} --behavior exit \"$@\"\n",
        quote(&python),
        quote(std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/source_adapter.py"
        ))),
        quote(&record)
    );
    let install = |name: &str| {
        let path = bin.join(name);
        std::fs::write(&path, &script).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    };
    install("notepipe-source-first");
    let invoked = source_json_error(&run());
    assert!(
        invoked["message"]
            .as_str()
            .unwrap()
            .contains("Source adapter failed")
    );
    install("notepipe-source-second");
    let ambiguous = source_json_error(&run());
    let message = ambiguous["message"].as_str().unwrap();
    for text in [
        "notepipe-source-first",
        "notepipe-source-second",
        "[source].command",
    ] {
        assert!(message.contains(text), "{message}");
    }
    assert_eq!(
        std::fs::read_to_string(record.join("calls"))
            .unwrap()
            .lines()
            .count(),
        1
    );
}

#[test]
fn removed_output_controls_and_commands_are_rejected() {
    let cli = CliTest::new();
    for args in [
        vec!["run-task"],
        vec!["summarize"],
        vec!["source", "--resume"],
        vec!["source", "--no-download"],
        vec!["source", "--formats", "md"],
    ] {
        assert!(!cli.run(&args).status.success(), "{args:?}");
    }
    for key in ["formats = ['md']", "no_download = true", "resume = true"] {
        cli.config(&format!("[defaults]\n{key}\n"));
        assert!(!cli.run(&["config", "show"]).status.success(), "{key}");
    }
}

#[test]
fn source_discovery_never_uses_unset_or_empty_path_entries_as_the_current_directory() {
    let cli = CliTest::new();
    cli.config("[source]\ncommand = []\n");
    let adapter = cli.dir.path().join(if cfg!(windows) {
        "notepipe-source-untrusted.cmd"
    } else {
        "notepipe-source-untrusted"
    });
    std::fs::write(&adapter, "fixture must never run").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&adapter, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let separator = if cfg!(windows) { ";" } else { ":" };
    let empty_components = format!("{separator}{separator}");
    for path in [None, Some(""), Some(empty_components.as_str())] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_notepipe"));
        command
            .args(["https://example.test/media", "--json"])
            .current_dir(cli.dir.path())
            .env("XDG_CONFIG_HOME", cli.dir.path().join("config"))
            .env("XDG_CACHE_HOME", cli.dir.path().join("cache"))
            .env_remove("RUST_LOG")
            .env_remove("NOTEPIPE_LLM_API_KEY")
            .stdin(std::process::Stdio::null());
        if let Some(path) = path {
            command.env("PATH", path);
        } else {
            command.env_remove("PATH");
        }
        let error = source_json_error(&command.output().unwrap());
        assert!(
            error["message"]
                .as_str()
                .unwrap()
                .contains(notepipe::source::ADAPTER_HINT),
            "{error}"
        );
    }
}

#[test]
fn explicit_config_accepts_json_and_toml_from_files_and_stdin_with_flag_overrides() {
    use std::io::Write;
    let cli = CliTest::new();
    cli.config("[invalid ambient configuration");
    std::fs::write(cli.dir.path().join("media.mp4"), "placeholder").unwrap();
    for text in [
        "[defaults]\nsample_interval = 0.5\n",
        "  {\"defaults\":{\"sample_interval\":0.5}}",
    ] {
        for input in ["settings.conf", "-"] {
            std::fs::write(cli.dir.path().join("settings.conf"), text).unwrap();
            let mut child = Command::new(env!("CARGO_BIN_EXE_notepipe"))
                .args([
                    "./media.mp4",
                    "--config",
                    input,
                    "--sample-interval",
                    "0",
                    "--json",
                ])
                .current_dir(cli.dir.path())
                .env("XDG_CONFIG_HOME", cli.dir.path().join("config"))
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
            let error = json_error(&child.wait_with_output().unwrap());
            assert!(
                error["message"]
                    .as_str()
                    .unwrap()
                    .contains("--sample-interval")
            );
        }
    }
    for invalid in [
        "[llm]\napi_key = 'private-test-key",
        "{\"llm\":{\"api_key\":\"private-test-key",
    ] {
        std::fs::write(cli.dir.path().join("invalid.conf"), invalid).unwrap();
        let error = json_error(&cli.run(&["./media.mp4", "--config", "invalid.conf", "--json"]));
        assert!(error["message"].as_str().unwrap().contains("configuration"));
        assert!(!error.to_string().contains("private-test-key"));
    }
}
