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
        self.run_with_lang(args, "en")
    }
    fn run_with_lang(&self, args: &[&str], lang: &str) -> Output {
        Command::new(env!("CARGO_BIN_EXE_notepipe"))
            .args(args)
            .current_dir(self.dir.path())
            .env("XDG_CONFIG_HOME", self.dir.path().join("config"))
            .env("XDG_CACHE_HOME", self.dir.path().join("cache"))
            .env("NOTEPIPE_LANG", lang)
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
fn no_arguments_or_help_succeed_with_localized_guidance() {
    let cli = CliTest::new();
    for args in [vec![], vec!["--help"]] {
        let result = cli.run(&args);
        assert!(result.status.success());
        let text = stdout(&result);
        assert!(
            text.contains("快速开始") || text.contains("Quick start"),
            "{text}"
        );
        assert!(
            text.to_ascii_lowercase().contains("speech recognition"),
            "{text}"
        );
    }
    let english = cli.run_with_lang(&["--help"], "en");
    assert!(english.status.success());
    let text = stdout(&english);
    assert!(text.contains("Quick start"), "{text}");
    assert!(text.contains("Netscape format;"), "{text}");
    assert!(text.contains("Browser cookies"), "{text}");
    assert!(!text.contains("Cookie 文件"), "{text}");
    assert!(text.contains("Speech recognition"), "{text}");
    let traditional = cli.run_with_lang(&["--help"], "zh-Hant");
    assert!(traditional.status.success());
    assert!(stdout(&traditional).contains("Cookie 檔案"));
    for cmd in ["config", "summarize", "doctor"] {
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
fn protocol_language_defaults_to_english_and_honors_explicit_overrides() {
    let cli = CliTest::new();
    for args in [
        vec!["--json"],
        vec!["run-task"],
        vec!["--json", "--asr-mode", "invalid"],
    ] {
        for (environment, flag, english) in [
            (None, None, true),
            (Some("zh-Hans"), None, false),
            (None, Some("zh-Hans"), false),
            (Some("zh-Hans"), Some("en"), true),
        ] {
            let mut command = Command::new(env!("CARGO_BIN_EXE_notepipe"));
            command
                .args(&args)
                .current_dir(cli.dir.path())
                .env_remove("NOTEPIPE_LANG")
                .env("LC_ALL", "zh_CN.UTF-8")
                .env("XDG_CONFIG_HOME", cli.dir.path().join("config"))
                .stdin(std::process::Stdio::null());
            if let Some(value) = environment {
                command.env("NOTEPIPE_LANG", value);
            }
            if let Some(value) = flag {
                command.args(["--lang", value]);
            }
            let output = command.output().unwrap();
            let error = json_error(&output);
            let message = error["message"].as_str().unwrap();
            let has_chinese = message
                .chars()
                .any(|c| ('\u{3400}'..='\u{9fff}').contains(&c));
            assert_eq!(has_chinese, !english, "{args:?}: {message}");
        }
    }
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
    assert_eq!(value("resume "), "true");
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
        .env("NOTEPIPE_LANG", "en")
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

#[cfg(all(unix, feature = "integration"))]
#[test]
fn remote_download_output_does_not_leak_into_json_or_quiet_mode() {
    use std::os::unix::fs::PermissionsExt;
    let cli = CliTest::new();
    // Use real ffmpeg/ffprobe, with an offline yt-dlp fixture that emits noisy output.
    let video = cli.dir.path().join("fixture.mp4");
    let generated = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=blue:s=320x180:d=3",
            "-c:v",
            "libx264",
            "-y",
        ])
        .arg(&video)
        .output()
        .unwrap();
    assert!(generated.status.success(), "{}", stderr(&generated));
    let bin = cli.dir.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let cookies = cli.dir.path().join("cookies with spaces.txt");
    std::fs::write(&cookies, "saved session").unwrap();
    let calls = cli.dir.path().join("cookie-calls");
    let script = bin.join("yt-dlp");
    std::fs::write(&script, r#"#!/bin/sh
if [ "$1" = "--version" ]; then echo fixture; exit 0; fi
selected_url=''
for argument; do selected_url="$argument"; done
cookies=''
browser=''
output=''
subtitles=0
simulate=0
json=0
while [ "$#" -gt 0 ]; do
  case "$1" in
    -J|--dump-single-json) json=1 ;;
    --simulate) simulate=1 ;;
    --no-simulate) simulate=0 ;;
    -o) shift; output="$1" ;;
    --skip-download) subtitles=1 ;;
    --cookies) shift; cookies="$1" ;;
    --cookies-from-browser) shift; browser="$1" ;;
  esac
  shift
done
[ "$selected_url" = "$CLI_TEST_SOURCE" ] &&
[ "$cookies" != "$CLI_TEST_COOKIES" ] &&
[ "$(cat "$cookies")" = "saved session" ] &&
[ "$browser" = "chrome:Profile 1" ] || exit 1
if [ "$simulate" -eq 1 ]; then
  phase=probe
elif [ "$subtitles" -eq 1 ]; then
  phase=subtitle
  printf '1\n00:00:00,000 --> 00:00:02,500\nWelcome to the lecture.\n' > "$output.en.srt"
else
  phase=download
  /bin/cp "$CLI_TEST_VIDEO" "$output"
fi
printf '%s|%s\n' "$phase" "$cookies" >> "$CLI_TEST_CALLS"
printf 'rewritten' > "$cookies"
if [ "$json" -eq 1 ]; then
  printf '%s\n' '{"title":"Test lecture","id":"test","webpage_url":"https://example.test/video","duration":3,"extractor":"test","subtitles":{"en":[{"ext":"srt","url":"https://example.test/en.srt"}]},"automatic_captions":{}}'
else
  printf 'raw downloader output\n'
fi
printf 'raw downloader stderr\n' >&2
"#).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    for flag in ["--json", "--quiet"] {
        let output_dir = cli.dir.path().join(flag.trim_start_matches('-'));
        let result = Command::new(env!("CARGO_BIN_EXE_notepipe"))
            .env("NOTEPIPE_LANG", "en")
            .args([
                "https://example.test/video?episode=2",
                "--transcript-source",
                "subtitle",
                flag,
                "-o",
            ])
            .arg(&output_dir)
            .arg("--cookies")
            .arg(&cookies)
            .args(["--cookies-from-browser", "chrome:Profile 1"])
            .env("CLI_TEST_COOKIES", &cookies)
            .env("CLI_TEST_CALLS", &calls)
            .env("CLI_TEST_SOURCE", "https://example.test/video?episode=2")
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    bin.display(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            )
            .env("CLI_TEST_VIDEO", &video)
            .env("XDG_CONFIG_HOME", cli.dir.path().join("config"))
            .env("XDG_CACHE_HOME", cli.dir.path().join("cache"))
            .env("RUST_LOG", "info")
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{} {}",
            stdout(&result),
            stderr(&result)
        );
        assert!(result.stderr.is_empty(), "{}", stderr(&result));
        if flag == "--quiet" {
            assert!(result.stdout.is_empty(), "{}", stdout(&result));
        } else {
            let events: Vec<serde_json::Value> = stdout(&result)
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            let done = events.last().unwrap();
            assert_eq!(done["type"], "done");
            let notes = std::path::Path::new(done["out_dir"].as_str().unwrap()).join("note.md");
            let notes = std::fs::read_to_string(notes).unwrap();
            assert!(notes.contains("Welcome to the lecture."));
            assert!(notes.contains("https://example.test/video?episode=2"));
        }
    }
    assert!(video.is_file());
    assert_eq!(std::fs::read_to_string(&cookies).unwrap(), "saved session");
    let calls = std::fs::read_to_string(calls).unwrap();
    let calls: Vec<_> = calls
        .lines()
        .map(|line| line.split_once('|').unwrap())
        .collect();
    assert_eq!(
        calls.iter().map(|(phase, _)| *phase).collect::<Vec<_>>(),
        [
            "probe", "subtitle", "download", "probe", "subtitle", "download"
        ]
    );
    assert_eq!(
        calls
            .iter()
            .map(|(_, path)| path)
            .collect::<std::collections::HashSet<_>>()
            .len(),
        6
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
