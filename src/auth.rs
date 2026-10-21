//! Private yt-dlp cookie snapshots. User cookie files are never passed to yt-dlp.

use anyhow::{Context, Result, ensure};
use std::{io::Write, path::Path};

fn validate(cookies: Option<&Path>, cookies_from_browser: Option<&str>) -> Result<()> {
    if let Some(path) = cookies {
        ensure!(
            path.is_file(),
            crate::tr!(
                "Cookie 文件必须存在且为普通文件 / Cookie file must exist and be a regular file"
            )
        );
    }
    if let Some(browser) = cookies_from_browser {
        ensure!(
            !browser.is_empty() && !browser.chars().any(char::is_control),
            crate::tr!(
                "浏览器 Cookie 来源不能为空或包含控制字符 / Browser cookie spec must be non-empty and contain no control characters"
            )
        );
    }
    Ok(())
}

/// yt-dlp rewrites cookie files on exit. Keep this private snapshot alive until
/// the subprocess exits, including when multiple requests run at the same time.
pub fn configure_ytdlp(
    command: &mut std::process::Command,
    cookies: Option<&Path>,
    cookies_from_browser: Option<&str>,
) -> Result<Option<tempfile::NamedTempFile>> {
    validate(cookies, cookies_from_browser)?;
    let snapshot = if let Some(path) = cookies {
        let bytes = std::fs::read(path)
            .context(crate::tr!("无法读取 Cookie 文件 / Cannot read cookie file"))?;
        let mut file = tempfile::NamedTempFile::new()?;
        file.write_all(&bytes)?;
        file.flush()?;
        command.arg("--cookies").arg(file.path());
        Some(file)
    } else {
        None
    };
    if let Some(browser) = cookies_from_browser {
        command.arg("--cookies-from-browser").arg(browser);
    }
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookie_snapshots_are_private_independent_and_never_rewrite_the_original() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cookies.txt");
        std::fs::write(&path, "saved session").unwrap();
        let mut first = std::process::Command::new("yt-dlp");
        let a = configure_ytdlp(&mut first, Some(&path), Some("firefox:default-release"))
            .unwrap()
            .unwrap();
        let mut second = std::process::Command::new("yt-dlp");
        let b = configure_ytdlp(&mut second, Some(&path), None)
            .unwrap()
            .unwrap();
        let args: Vec<_> = first.get_args().collect();
        assert_eq!(
            args,
            [
                std::ffi::OsStr::new("--cookies"),
                a.path().as_os_str(),
                std::ffi::OsStr::new("--cookies-from-browser"),
                std::ffi::OsStr::new("firefox:default-release")
            ]
        );
        assert_ne!(a.path(), path);
        assert_ne!(a.path(), b.path());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                a.as_file().metadata().unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        std::fs::write(a.path(), "rewritten by yt-dlp").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "saved session");
        assert_eq!(std::fs::read_to_string(b.path()).unwrap(), "saved session");
    }

    #[test]
    fn invalid_inputs_are_rejected_before_adding_arguments() {
        let dir = tempfile::tempdir().unwrap();
        for path in [dir.path().to_path_buf(), dir.path().join("missing")] {
            let mut command = std::process::Command::new("yt-dlp");
            assert!(configure_ytdlp(&mut command, Some(&path), None).is_err());
            assert_eq!(command.get_args().count(), 0);
        }
        let path = dir.path().join("cookies.txt");
        std::fs::write(&path, "saved session").unwrap();
        for browser in ["", "chrome\n", "chrome\u{7f}", "chrome\t", "chrome\0"] {
            let mut command = std::process::Command::new("yt-dlp");
            assert!(configure_ytdlp(&mut command, Some(&path), Some(browser)).is_err());
            assert_eq!(command.get_args().count(), 0);
        }
    }
}
