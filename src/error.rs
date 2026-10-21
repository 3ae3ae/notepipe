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
    anyhow::anyhow!("{program} failed (code={code:?}):\n{tail}")
}

/// Check whether an external tool is available on PATH.
pub fn require_cmd(cmd: &str) -> Result<()> {
    if crate::runtime::which(cmd).is_none() {
        return Err(missing_tool(cmd));
    }
    Ok(())
}

fn missing_tool(cmd: &str) -> anyhow::Error {
    anyhow::anyhow!("{cmd} is not installed. {FFMPEG_INSTALL_HINT}")
}

pub const FFMPEG_INSTALL_HINT: &str =
    "Install FFmpeg from https://ffmpeg.org/download.html and add its executables to PATH.";
