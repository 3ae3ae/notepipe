//! yt-dlp lookup: explicit override, executable directory, PATH, then common installation locations.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// 显式指定 yt-dlp 二进制的环境变量；指向不可执行文件时忽略并继续按清单搜索。
pub const ENV_OVERRIDE: &str = "NOTEPIPE_YTDLP";

fn binary_name() -> String {
    format!("yt-dlp{}", std::env::consts::EXE_SUFFIX)
}

/// 存在且可执行（Windows 只验证是普通文件）。
fn is_exec(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .map(|m| m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// 随程序安装的 yt-dlp：与可执行文件同目录。
fn bundled_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        candidates.push(dir.join(binary_name()));
    }
    candidates
}

/// `base` 下形如 `3.x`/`Python3xx` 的解释器目录：macOS `~/Library/Python/3.*/bin`
/// 与 Windows `Python3*/Scripts` 同为 `pip --user` 的 yt-dlp 落点。
fn interpreter_dirs(base: &Path, leaf: &str, prefix: &str) -> Vec<PathBuf> {
    std::fs::read_dir(base)
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.file_name().to_string_lossy().starts_with(prefix))
                .map(|e| e.path().join(leaf))
                .collect()
        })
        .unwrap_or_default()
}

/// PATH 之外的常见安装位置。
fn fallback_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    let home = crate::config::home_dir();
    #[cfg(unix)]
    {
        if let Some(home) = &home {
            for rel in [
                ".local/bin", // pipx / uv tool / pip --user（Linux）
                ".local/share/mise/shims",
                ".asdf/shims",
                ".nix-profile/bin",
                "anaconda3/bin",
                "miniconda3/bin",
                "miniforge3/bin",
                "mambaforge/bin",
                "opt/anaconda3/bin",
                "opt/miniconda3/bin",
            ] {
                candidates.push(home.join(rel).join(binary_name()));
            }
            for dir in interpreter_dirs(&home.join("Library/Python"), "bin", "3.") {
                candidates.push(dir.join(binary_name()));
            }
        }
        for abs in [
            "/opt/homebrew/bin",
            "/usr/local/bin",
            "/opt/local/bin", // MacPorts
            "/opt/anaconda3/bin",
            "/opt/miniconda3/bin",
            "/nix/var/nix/profiles/default/bin",
        ] {
            candidates.push(Path::new(abs).join(binary_name()));
        }
    }
    #[cfg(windows)]
    {
        let name = binary_name();
        if let Some(local) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
            candidates.push(local.join(r"Microsoft\WinGet\Links").join(&name));
            for dir in interpreter_dirs(&local.join(r"Programs\Python"), "Scripts", "Python3") {
                candidates.push(dir.join(&name));
            }
        }
        if let Some(home) = &home {
            candidates.push(home.join(r"scoop\shims").join(&name));
        }
        if let Some(data) = std::env::var_os("ProgramData").map(PathBuf::from) {
            candidates.push(data.join(r"chocolatey\bin").join(&name));
        }
        if let Some(roaming) = std::env::var_os("APPDATA").map(PathBuf::from) {
            for dir in interpreter_dirs(&roaming.join("Python"), "Scripts", "Python3") {
                candidates.push(dir.join(&name));
            }
        }
    }
    candidates
}

/// A candidate that exits successfully for `--version`. A bundled one-file
/// PyInstaller build can be present and executable yet fail after macOS signing
/// (the extracted Python has a different Team ID); treat that as unavailable and
/// continue to the user's PATH/Homebrew installation.
fn runs(path: &Path) -> bool {
    let mut command = std::process::Command::new(path);
    command.arg("--version");
    crate::runtime::run_bounded("yt-dlp", &mut command, std::time::Duration::from_secs(3))
        .is_ok_and(|output| output.status.success())
}

fn candidates_in(path: Option<&OsStr>) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(custom) = std::env::var_os(ENV_OVERRIDE).filter(|v| !v.is_empty()) {
        candidates.push(PathBuf::from(custom));
    }
    candidates.extend(bundled_candidates());
    let name = binary_name();
    if let Some(path) = path {
        candidates.extend(std::env::split_paths(path).map(|dir| dir.join(&name)));
    } else if let Some(path) = std::env::var_os("PATH") {
        candidates.extend(std::env::split_paths(&path).map(|dir| dir.join(&name)));
    }
    candidates.extend(fallback_candidates());
    candidates
}

/// 按优先级返回第一个存在、可执行且能运行 `--version` 的 yt-dlp。
/// `path` 为 None 时用进程继承的 PATH。
pub fn resolve_in(path: Option<&OsStr>) -> Option<PathBuf> {
    let mut seen = std::collections::BTreeSet::new();
    candidates_in(path)
        .into_iter()
        .filter(|candidate| seen.insert(candidate.clone()))
        .find(|candidate| is_exec(candidate) && runs(candidate))
}

/// 继承 PATH 的版本（CLI/终端调用）。
pub fn resolve() -> Option<PathBuf> {
    resolve_in(None)
}

/// 给 `Command::new` 的程序名：解析成功为绝对路径，否则退回裸名
///（spawn 错误由调用方现成的「无法启动 yt-dlp」上下文兜底，`require_cmd`
/// 会在更早处给出安装提示）。
pub fn program() -> PathBuf {
    resolve().unwrap_or_else(|| PathBuf::from(binary_name()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn make_exec(dir: &Path, name: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, b"#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn interpreter_dirs_filters_by_prefix() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("3.12/bin")).unwrap();
        std::fs::create_dir_all(root.path().join("2.7/bin")).unwrap();
        let dirs = interpreter_dirs(root.path(), "bin", "3.");
        assert_eq!(dirs, vec![root.path().join("3.12/bin")]);
    }

    #[cfg(unix)]
    #[test]
    fn is_exec_requires_exec_bit() {
        let dir = tempfile::tempdir().unwrap();
        let exec = make_exec(dir.path(), "yt-dlp");
        assert!(is_exec(&exec));
        let plain = dir.path().join("plain");
        std::fs::write(&plain, b"#!/bin/sh\n").unwrap();
        assert!(!is_exec(&plain));
        assert!(!is_exec(&dir.path().join("missing")));
    }
    #[cfg(unix)]
    #[test]
    fn broken_first_path_entry_falls_back_to_a_runnable_one() {
        use std::os::unix::fs::PermissionsExt;
        let broken = tempfile::tempdir().unwrap();
        let working = tempfile::tempdir().unwrap();
        let broken_path = broken.path().join("yt-dlp");
        std::fs::write(&broken_path, b"#!/bin/sh\nexit 1\n").unwrap();
        std::fs::set_permissions(&broken_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        let working_path = make_exec(working.path(), "yt-dlp");
        let path = std::env::join_paths([broken.path(), working.path()]).unwrap();
        assert_eq!(
            resolve_in(Some(path.as_os_str())),
            Some(working_path),
            "a broken bundled/earlier candidate must not hide a working installation"
        );
    }
}
