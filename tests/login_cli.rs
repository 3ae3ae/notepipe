use clap::Parser;
use notepipe::cli::Cli;

#[test]
fn login_is_standalone_or_precedes_video_processing() {
    let login = Cli::try_parse_from(["notepipe", "--login", "codex"]).unwrap();
    assert!(login.login.is_some());
    assert!(login.source.is_none());
    let login = Cli::try_parse_from([
        "notepipe",
        "--login",
        "codex",
        "https://example.invalid/video/one",
    ])
    .unwrap();
    assert!(login.source.is_some());
    for platform in ["ollama", "codex"] {
        assert!(
            Cli::try_parse_from(["notepipe", "--login", platform])
                .unwrap()
                .login
                .is_some(),
            "rejected --login {platform}"
        );
        assert!(
            Cli::try_parse_from(["notepipe", "--logout", platform])
                .unwrap()
                .logout
                .is_some(),
            "rejected --logout {platform}"
        );
    }
    for args in [
        vec!["notepipe", "--login", "youtube"],
        vec!["notepipe", "--login", "codex", "--logout", "codex"],
        vec!["notepipe", "--login", "codex", "--json"],
        vec![
            "notepipe",
            "--logout",
            "codex",
            "https://example.invalid/video/one",
        ],
    ] {
        assert!(Cli::try_parse_from(&args).is_err(), "accepted {args:?}");
    }
}

#[test]
fn invalid_source_does_not_start_login() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_notepipe"))
        .env("NOTEPIPE_LANG", "en")
        .args(["--login", "codex", "doctor"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("video URL"));
    assert!(out.stdout.is_empty());
}

#[test]
fn initial_config_does_not_create_credentials() {
    let dir = tempfile::tempdir().unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_notepipe"))
        .env("XDG_CONFIG_HOME", dir.path())
        .env("NOTEPIPE_LANG", "en")
        .args(["config", "init"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let message = String::from_utf8_lossy(&out.stdout);
    assert!(message.contains("config.toml"));
    assert!(dir.path().join("notepipe/config.toml").is_file());
    assert!(!dir.path().join("notepipe/auth").exists());
}
