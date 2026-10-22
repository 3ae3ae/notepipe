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
            .env_remove("NOTEPIPE_SPEECH_API_KEY")
            .env_remove("NOTEPIPE_AI_API_KEY")
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
        assert!(text.contains("Examples:"), "{text}");
        assert!(text.contains("[speech]"), "{text}");
    }
    let help = cli.run(&["--help"]);
    assert!(help.status.success());
    let text = stdout(&help);
    assert!(text.contains("Examples:"), "{text}");
    assert!(text.contains("Speech recognition"), "{text}");
    assert!(text.contains("config: screenshots.sensitivity"), "{text}");
    assert!(text.contains("notepipe config init"), "{text}");
    for description in [
        "Subtitles when there are any, otherwise speech recognition",
        "Subtitles only; fail when there are none",
        "Speech recognition only, even when subtitles exist",
    ] {
        assert!(text.contains(description), "{text}");
    }
    assert!(
        !text.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)),
        "{text}"
    );
    for cmd in ["config", "doctor"] {
        assert!(cli.run(&[cmd, "--help"]).status.success(), "{cmd}");
    }
    let config = cli.run(&["config"]);
    let text = format!("{}{}", stdout(&config), stderr(&config));
    assert!(
        text.contains("Create, find and show the configuration file"),
        "{text}"
    );
    // clap names the binary notepipe.exe on Windows.
    assert!(text.contains(" config <COMMAND>"), "{text}");
}

#[test]
fn config_path_and_missing_file_guidance_use_the_configuration_path() {
    let cli = CliTest::new();
    let path = cli
        .dir
        .path()
        .join("config")
        .join("notepipe")
        .join("config.toml");
    let result = cli.run(&["config", "path"]);
    assert!(result.status.success());
    assert!(result.stderr.is_empty(), "{}", stderr(&result));
    assert_eq!(stdout(&result), format!("{}\n", path.display()));
    let shown = cli.run(&["config", "show"]);
    assert!(shown.status.success());
    assert!(stdout(&shown).contains("notepipe config init"));
    for args in [vec![], vec!["-h"], vec!["--help"]] {
        let output = cli.run(&args);
        assert!(output.status.success());
        assert!(
            stdout(&output).contains(&format!(
                "File: {} (created on the first run)",
                path.display()
            )),
            "{}",
            stdout(&output)
        );
    }
    assert!(cli.run(&["config", "init"]).status.success());
    let result = cli.run(&["config", "path"]);
    assert_eq!(stdout(&result), format!("{}\n", path.display()));
    for args in [vec![], vec!["-h"], vec!["--help"]] {
        let output = cli.run(&args);
        assert!(output.status.success());
        assert!(
            stdout(&output).contains(&format!("File: {}\n", path.display())),
            "{}",
            stdout(&output)
        );
    }
}

#[test]
fn missing_or_invalid_sources_fail_with_recovery() {
    let cli = CliTest::new();
    let missing = cli.run(&["--speech-api", "chat"]);
    assert!(!missing.status.success());
    assert!(
        stderr(&missing).contains("Missing video or audio source"),
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
    json_error(&cli.run(&["./missing.mp4", "--json", "--speech-api", "invalid"]));
    std::fs::write(cli.dir.path().join("lecture.mp4"), []).unwrap();
    cli.config("[output\n");
    let error = json_error(&cli.run(&["./lecture.mp4", "--json"]));
    assert!(error["message"].as_str().unwrap().contains("configuration"));
}

#[test]
fn validation_precedes_dependency_checks_and_downloads() {
    let cli = CliTest::new();
    cli.config("");
    std::fs::write(cli.dir.path().join("lecture.mp4"), []).unwrap();
    let result = cli.run(&["./lecture.mp4", "--frame-interval", "0", "--json"]);
    let error = json_error(&result);
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("--frame-interval")
    );
    assert!(!cli.dir.path().join("cache").exists());
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
    assert_eq!(value("dir "), ".");
    assert_eq!(value("settle "), "0.8");
    assert_eq!(value("mode "), "stable");
}

#[test]
fn config_show_reports_ai_adapter_after_base_url() {
    let cli = CliTest::new();
    cli.config("[ai]\nadapter = ['my-ai-adapter', '--option']\n");
    let result = cli.run(&["config", "show"]);
    assert!(result.status.success());
    let text = stdout(&result);
    let ai = text.split_once("[ai]\n").unwrap().1;
    let mut lines = ai.lines();
    assert_eq!(lines.next().unwrap(), format!("  {:<17}: ", "base_url"));
    assert_eq!(
        lines.next().unwrap(),
        format!("  {:<17}: [\"my-ai-adapter\", \"--option\"]", "adapter")
    );
}

#[test]
fn screenshot_mode_is_configurable_and_unknown_keys_are_rejected() {
    let cli = CliTest::new();
    let help = stdout(&cli.run(&["--help"]));
    assert!(help.contains("--screenshot-mode") && help.contains("Screenshots"));
    cli.config("[screenshots]\nmode = 'first'\n");
    let shown = cli.run(&["config", "show"]);
    assert!(shown.status.success());
    assert!(
        stdout(&shown)
            .lines()
            .any(|line| line.contains("mode") && line.ends_with("first"))
    );
    std::fs::write(cli.dir.path().join("media.mp4"), "placeholder").unwrap();
    cli.config("[screenshots]\nunknown_key = 1\n");
    let error = json_error(&cli.run(&["media.mp4", "--json"]));
    assert!(error["message"].as_str().unwrap().contains("configuration"));
}

#[test]
fn settings_never_print_saved_keys() {
    let cli = CliTest::new();
    cli.config(
        "[ai]\napi_key = 'secret-llm-test-key'\n[speech]\napi_key = 'secret-asr-test-key'\n",
    );
    let result = cli.run(&["config", "show"]);
    assert!(result.status.success());
    assert!(!stdout(&result).contains("secret-"));
}

#[test]
fn generated_template_parses_and_does_not_overwrite_existing_config() {
    let cli = CliTest::new();
    let created = cli.run(&["config", "init"]);
    assert!(created.status.success());
    let path = cli
        .dir
        .path()
        .join("config")
        .join("notepipe")
        .join("config.toml");
    assert_eq!(
        stdout(&created),
        format!(
            "Created {}\nEvery value in it is in effect; edit it to change how notes are made.\n",
            path.display()
        )
    );
    assert!(cli.run(&["config", "show"]).status.success());
    cli.config("[screenshots]\nsensitivity = 0.7\n");
    let existing = cli.run(&["config", "init"]);
    assert!(!existing.status.success());
    assert!(stderr(&existing).contains(&format!(
        "{} already exists. Run `notepipe config init --force` to replace it.",
        path.display()
    )));
    let file = std::fs::read_to_string(
        cli.dir
            .path()
            .join("config")
            .join("notepipe")
            .join("config.toml"),
    )
    .unwrap();
    assert!(file.contains("sensitivity = 0.7"));
}

#[test]
fn runs_create_the_default_configuration_once_and_explicit_configs_create_nothing() {
    for mode in ["", "--json", "--quiet"] {
        let cli = CliTest::new();
        let path = cli
            .dir
            .path()
            .join("config")
            .join("notepipe")
            .join("config.toml");
        let notice = format!(
            "Created {} with the default settings. Edit it to change how notes are made.",
            path.display()
        );
        let mut args = vec!["https://example.test/media", "--frame-interval", "0"];
        if !mode.is_empty() {
            args.push(mode);
        }
        let first = cli.run(&args);
        assert!(path.is_file());
        assert!(!first.status.success());
        if mode == "--json" {
            let events: Vec<serde_json::Value> = stdout(&first)
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            assert_eq!(
                events[0],
                serde_json::json!({"type":"log","level":"info","message":notice})
            );
            assert!(first.stderr.is_empty());
        } else {
            assert_eq!(stderr(&first).contains(&notice), mode != "--quiet");
        }
        let contents = std::fs::read(&path).unwrap();
        let second = cli.run(&args);
        assert!(!format!("{}{}", stdout(&second), stderr(&second)).contains(&notice));
        assert_eq!(std::fs::read(&path).unwrap(), contents);
    }
    let cli = CliTest::new();
    std::fs::write(cli.dir.path().join("explicit.toml"), "").unwrap();
    let error = json_error(&cli.run(&[
        "https://example.test/media",
        "--config",
        "explicit.toml",
        "--frame-interval",
        "0",
        "--json",
    ]));
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("--frame-interval")
    );
    assert!(!cli.dir.path().join("config").exists());
}

#[test]
fn failed_default_configuration_creation_warns_and_uses_defaults() {
    let cli = CliTest::new();
    std::fs::write(cli.dir.path().join("config"), "blocked").unwrap();
    let output = cli.run(&["https://example.test/media", "--frame-interval", "0"]);
    let text = stderr(&output);
    assert!(text.contains(&format!(
        "Could not create {}:",
        cli.dir.path().join("config").join("notepipe").join("config.toml").display()
    )));
    assert!(text.contains(". Continuing with the default settings."));
    assert!(text.contains("--frame-interval"));
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
    let text = stdout(&result);
    assert!(text.contains("https://ffmpeg.org/download.html"));
    for removed in [
        "brew install",
        "winget install",
        "apt install",
        "apt-get",
        "pacman",
        "install.sh",
    ] {
        assert!(!text.contains(removed), "{text}");
    }
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
fn doctor_reports_missing_llm_settings_for_enabled_stages_offline() {
    let cli = CliTest::new();
    cli.config("[ai]\nbase_url = 'http://127.0.0.1:9/v1'\n[[stages]]\nname = 'summary'\ninput = 'transcript'\noutput = 'top'\nprompt = 'CUSTOM'\n");
    let result = cli.run(&["doctor"]);
    assert!(!result.status.success());
    let text = stdout(&result);
    assert!(
        text.contains("✗ Settings values") && text.contains("AI model is not set"),
        "{text}"
    );
    assert!(stderr(&result).contains("required-tool or configuration issues"));
}

#[test]
fn doctor_reports_ai_adapter_executable_after_speech() {
    let cli = CliTest::new();
    let program = env!("CARGO_BIN_EXE_notepipe");
    cli.config(&serde_json::json!({"ai": {"adapter": [program]}}).to_string());
    let result = cli.run(&["doctor"]);
    let text = stdout(&result);
    let speech = text.find("- Speech recognition").unwrap();
    let adapter = text.find(&format!("✓ AI adapter  {program}")).unwrap();
    assert!(adapter > speech, "{text}");

    let program = "notepipe-test-missing-ai-adapter";
    cli.config(&serde_json::json!({"ai": {"adapter": [program]}}).to_string());
    let result = cli.run(&["doctor"]);
    assert!(!result.status.success());
    assert!(
        stdout(&result).contains(&format!("✗ AI adapter {program}")),
        "{}",
        stdout(&result)
    );
    assert!(stderr(&result).contains("required-tool or configuration issues"));
    cli.config(&serde_json::json!({"stages":[{"name":"custom","input":"transcript","output":"top","adapter":[program]}]}).to_string());
    let result = cli.run(&["doctor"]);
    assert!(!result.status.success());
    assert!(stdout(&result).contains(&format!("✗ AI adapter {program}")));
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
    cli.config(&serde_json::json!({"download":{"adapter":command}}).to_string());
    Some(record)
}

#[test]
fn missing_source_adapter_reports_generic_guidance() {
    let cli = CliTest::new();
    cli.config("[download]\nadapter = ['notepipe-test-missing-source-adapter']\n");
    let error = source_json_error(&cli.run(&["https://example.test/video", "--json"]));
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("URLs need a download adapter")
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
            message.contains("download adapter") || message.contains("Download adapter"),
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
    if which::which("ffmpeg").ok().is_none() || which::which("ffprobe").ok().is_none() {
        return;
    }
    let cli = CliTest::new();
    let Some(record) = source_config(&cli, "healthy") else {
        return;
    };
    let url = "https://example.test/video?episode=2";
    let invalid = source_json_error(&cli.run(&[url, "--stages", "summary", "--json"]));
    assert!(
        invalid["message"]
            .as_str()
            .unwrap()
            .contains("Unknown stage: summary")
    );
    assert!(!record.join("calls").exists());
    for flag in ["--json", "--quiet"] {
        let output = cli.run(&[
            url,
            "--transcript",
            "subtitles",
            "--screenshot-mode",
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
        "--transcript",
        "speech",
        "--json",
    ]));
    assert!(error["message"].as_str().unwrap().contains("[speech]"));
    assert!(!record.join("calls").exists());
}

#[cfg(unix)]
#[test]
fn unconfigured_sources_discover_a_single_adapter_and_report_ambiguity() {
    use std::os::unix::fs::PermissionsExt;
    let Some(python) = which::which("python3")
        .ok()
        .or_else(|| which::which("python").ok())
    else {
        return;
    };
    let cli = CliTest::new();
    cli.config("[download]\nadapter = []\n");
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
            .contains("URLs need a download adapter")
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
            .contains("Download adapter failed")
    );
    install("notepipe-source-second");
    let ambiguous = source_json_error(&run());
    let message = ambiguous["message"].as_str().unwrap();
    for text in [
        "notepipe-source-first",
        "notepipe-source-second",
        "[download].adapter",
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
fn source_discovery_never_uses_unset_or_empty_path_entries_as_the_current_directory() {
    let cli = CliTest::new();
    cli.config("[download]\nadapter = []\n");
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
            .env_remove("NOTEPIPE_AI_API_KEY")
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
                .contains("URLs need a download adapter"),
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
        "[screenshots]\nframe_interval = 0.5\n",
        "  {\"screenshots\":{\"frame_interval\":0.5}}",
    ] {
        for input in ["settings.conf", "-"] {
            std::fs::write(cli.dir.path().join("settings.conf"), text).unwrap();
            let mut child = Command::new(env!("CARGO_BIN_EXE_notepipe"))
                .args([
                    "./media.mp4",
                    "--config",
                    input,
                    "--frame-interval",
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
                    .contains("--frame-interval")
            );
        }
    }
    for invalid in [
        "[ai]\napi_key = 'private-test-key",
        "{\"ai\":{\"api_key\":\"private-test-key",
    ] {
        std::fs::write(cli.dir.path().join("invalid.conf"), invalid).unwrap();
        let error = json_error(&cli.run(&["./media.mp4", "--config", "invalid.conf", "--json"]));
        assert!(error["message"].as_str().unwrap().contains("configuration"));
        assert!(!error.to_string().contains("private-test-key"));
    }
}

#[test]
fn stage_selection_is_explicit_and_validated() {
    let cli = CliTest::new();
    assert!(
        !cli.run(&[
            "media.mp4",
            "--stages",
            "proofread",
            "--no-stages",
            "--json"
        ])
        .status
        .success()
    );
    std::fs::write(cli.dir.path().join("media.mp4"), "placeholder").unwrap();
    cli.config("");
    let error = json_error(&cli.run(&["media.mp4", "--stages", "unknown", "--json"]));
    assert!(error["message"].as_str().unwrap().contains("Unknown stage"));
    cli.config("[ai]\nbase_url = 'invalid unused URL'\nmodel = ''\n");
    let error = json_error(&cli.run(&["media.mp4", "--frame-interval", "0", "--json"]));
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("--frame-interval")
    );
}

#[test]
fn llm_request_tables_are_validated_before_any_source_work() {
    let cli = CliTest::new();
    for request in [
        "1",
        "[]",
        "'text'",
        "{ model = 'override' }",
        "{ messages = [] }",
        "{ stream = false }",
        "{ response_format = {} }",
    ] {
        for config in [
            format!("[ai]\nrequest = {request}\n"),
            format!(
                "[[stages]]\nname = 'summary'\ninput = 'transcript'\noutput = 'top'\nprompt = 'CUSTOM'\nrequest = {request}\n"
            ),
        ] {
            cli.config(&config);
            let error = json_error(&cli.run(&["https://example.test/media", "--json"]));
            assert!(error["message"].as_str().unwrap().contains("configuration"));
        }
    }
    assert!(!cli.dir.path().join("cache").exists());
}

#[test]
fn duplicate_stage_names_are_rejected_before_selection_or_source_work() {
    let cli = CliTest::new();
    cli.config("[[stages]]\nname = 'summary'\ninput = 'transcript'\noutput = 'top'\nprompt = 'CUSTOM'\n[[stages]]\nname = 'summary'\ninput = 'transcript'\noutput = 'top'\nprompt = 'CUSTOM'\n");
    for selection in [vec![], vec!["--stages", "summary"], vec!["--no-stages"]] {
        let mut args = vec!["https://example.test/media", "--json"];
        args.extend(selection);
        let error = json_error(&cli.run(&args));
        assert!(error["message"].as_str().unwrap().contains("configuration"));
    }
    assert!(!cli.dir.path().join("cache").exists());
}
