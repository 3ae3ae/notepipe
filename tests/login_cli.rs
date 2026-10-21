use clap::Parser;
use notepipe::cli::Cli;

#[test]
fn login_is_standalone_or_precedes_video_processing() {
    let login = Cli::try_parse_from(["notepipe", "--login", "bilibili"]).unwrap();
    assert!(login.login.is_some());
    assert!(login.source.is_none());
    let login = Cli::try_parse_from([
        "notepipe",
        "--login",
        "bilibili",
        "https://www.bilibili.com/video/BV1",
    ])
    .unwrap();
    assert!(login.source.is_some());
    for platform in ["bilibili", "ollama", "codex"] {
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
        vec!["notepipe", "--login", "bilibili", "--logout", "bilibili"],
        vec!["notepipe", "--login", "bilibili", "--json"],
        vec!["notepipe", "--logout", "bilibili", "https://b23.tv/test"],
    ] {
        assert!(Cli::try_parse_from(&args).is_err(), "accepted {args:?}");
    }
}

#[test]
fn invalid_source_does_not_start_login() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_notepipe"))
        .env("NOTEPIPE_LANG", "zh-Hans")
        .args(["--login", "bilibili", "doctor"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("只能跟视频链接"));
    assert!(out.stdout.is_empty());
}

#[test]
fn initial_config_recommends_bilibili_login_without_creating_credentials() {
    let dir = tempfile::tempdir().unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_notepipe"))
        .env("XDG_CONFIG_HOME", dir.path())
        .env("NOTEPIPE_LANG", "zh-Hans")
        .args(["config", "init"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let message = String::from_utf8_lossy(&out.stdout);
    assert!(message.contains("notepipe --login bilibili"));
    assert!(message.contains("账号权限范围内更高清晰度"));
    assert!(dir.path().join("notepipe/config.toml").is_file());
    assert!(!dir.path().join("notepipe/auth").exists());
}
