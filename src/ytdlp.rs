//! yt-dlp 二进制定位：`COURSE2MD_YTDLP` 覆盖 → 可执行文件旁（随包自带）→
//! PATH → 常见安装目录兜底。CLI（fetch/pipeline/doctor）与桌面端（环境检测、
//! 在线读取）共用同一份清单，保证「检测到可用」与「实际能启动」不互相矛盾。
//!
//! 各档位的来由：
//! - `COURSE2MD_YTDLP`：显式指向某个二进制（与 COURSE2MD_BIN 同一约定），
//!   打包测试、临时换版本时用；
//! - 可执行文件同目录：install.sh 与桌面安装包把 yt-dlp 放在 course2md 旁；
//!   `.app` 布局额外检查 `../Resources`（codesign 把 Contents/MacOS 当代码区，
//!   数据文件属于资源区）；Homebrew/pipx 装的 yt-dlp 也常与 course2md 同在 bin/；
//! - PATH：终端安装的标准位置；桌面端传入 tool_path() 的扩展版本；
//! - 常见安装目录兜底：Finder/桌面会话启动的进程拿不到登录 shell 的 PATH，
//!   mise/asdf/conda/Nix/Scoop/Python user-base 等位置只能逐个枚举。

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

/// 显式指定 yt-dlp 二进制的环境变量；指向不可执行文件时忽略并继续按清单搜索。
pub const ENV_OVERRIDE: &str = "COURSE2MD_YTDLP";

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

/// 随包自带/同目录候选：exe 旁、`.app` 的 Contents/Resources[/bin]。
fn bundled_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        for base in [dir.to_path_buf(), dir.join("../Resources"), dir.join("../Resources/bin")] {
            candidates.push(base.join(binary_name()));
        }
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

/// PATH 之外的常见安装位置：覆盖「终端里有、桌面会话里没有」的安装方式。
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

/// 按优先级返回第一个可执行的 yt-dlp。
/// `path` 为 None 时用进程继承的 PATH；桌面端传入 `tool_path()` 的扩展版本。
pub fn resolve_in(path: Option<&OsStr>) -> Option<PathBuf> {
    if let Some(custom) = std::env::var_os(ENV_OVERRIDE).filter(|v| !v.is_empty()) {
        let p = PathBuf::from(custom);
        if is_exec(&p) {
            return Some(p);
        }
    }
    if let Some(found) = bundled_candidates().into_iter().find(|p| is_exec(p)) {
        return Some(found);
    }
    let name = OsString::from(binary_name());
    let on_path = match path {
        Some(p) => which::which_in(&name, Some(p), ".").ok(),
        None => which::which(&name).ok(),
    };
    if on_path.is_some() {
        return on_path;
    }
    fallback_candidates().into_iter().find(|p| is_exec(p))
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
}
