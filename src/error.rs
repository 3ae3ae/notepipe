use anyhow::Result;

/// 取多行文本的最后 n 行（保持原有先后顺序），供错误摘要附带日志尾部。
pub fn tail_lines(s: &str, n: usize) -> String {
    s.lines()
        .rev()
        .take(n)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n")
}

/// 子进程失败时附带 stderr 摘要的便捷构造。
pub fn cmd_error(program: &str, code: Option<i32>, stderr: &str) -> anyhow::Error {
    let tail = tail_lines(stderr, 5);
    anyhow::anyhow!("{program} 运行失败 / failed (code={code:?}):\n{tail}")
}

/// 校验外部工具存在。yt-dlp 走共享搜索清单（随包自带/PATH/常见安装目录），
/// 其余工具仍只查 PATH。
pub fn require_cmd(cmd: &str) -> Result<()> {
    let missing = if cmd == "yt-dlp" {
        crate::ytdlp::resolve().is_none()
    } else {
        crate::runtime::which(cmd).is_none()
    };
    if missing {
        anyhow::bail!(
            "未找到 {cmd}，请先安装 / {cmd} is not installed. {}",
            install_hint(cmd)
        );
    }
    Ok(())
}

/// 按平台附一行安装命令（常见包管理器；ffprobe 随 ffmpeg 包提供）。
fn install_hint(cmd: &str) -> String {
    let pkg = if cmd == "ffprobe" { "ffmpeg" } else { cmd };
    let media_tool = matches!(cmd, "ffmpeg" | "ffprobe" | "yt-dlp");
    if cfg!(target_os = "macos") {
        if media_tool {
            "macOS: brew install ffmpeg yt-dlp".into()
        } else {
            format!("macOS: brew install {pkg}")
        }
    } else if cfg!(target_os = "windows") {
        format!("Windows: winget install {pkg}")
    } else {
        format!("Debian/Ubuntu: sudo apt install {pkg}；Arch: sudo pacman -S {pkg}")
    }
}
