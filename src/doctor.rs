//! 环境体检：一次性报告依赖工具、平台后端可用性、配置与模型缓存状态。
//! 用户报 issue 时附上 `notepipe doctor` 输出即可定位大多数环境问题。

use anyhow::Result;
use std::process::Command;
use std::time::Duration;

/// 版本首行展示的最大字符数（防止某些工具输出超长首行撑爆版面）
const VERSION_LINE_MAX_CHARS: usize = 72;

/// 单个工具的版本探测上限：坏掉的二进制不能让 doctor 卡住。
const VERSION_TIMEOUT: Duration = Duration::from_secs(10);

fn tool_version(cmd: impl AsRef<std::ffi::OsStr>, args: &[&str]) -> Option<String> {
    let mut command = Command::new(cmd);
    command.args(args);
    let out = crate::runtime::run_bounded("doctor", &mut command, VERSION_TIMEOUT).ok()?;
    if !out.status.success() {
        return None;
    }
    Some(
        out.stdout
            .lines()
            .next()
            .unwrap_or("")
            .trim()
            .chars()
            .take(VERSION_LINE_MAX_CHARS)
            .collect(),
    )
}

fn check(line: &mut Vec<String>, ok: bool, name: &str, detail: &str) {
    let mark = if ok { "✓" } else { "✗" };
    line.push(format!("{mark} {name}{detail}"));
}

pub fn run() -> Result<()> {
    let mut out = vec![format!(
        "notepipe {} ({}/{})",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    )];
    let mut failures = 0;
    out.push("\nRequired tools".into());
    for tool in ["ffmpeg", "ffprobe"] {
        match tool_version(tool, &["-version", "-hide_banner"]) {
            Some(version) => check(&mut out, true, tool, &format!("  {version}")),
            None => {
                failures += 1;
                check(
                    &mut out,
                    false,
                    tool,
                    &format!("  Missing: {}", crate::error::install_hint(tool)),
                );
            }
        }
    }
    out.push("\nOptional tools by use".into());
    match crate::ytdlp::resolve() {
        Some(path) => match tool_version(&path, &["--version"]) {
            Some(version) => check(
                &mut out,
                true,
                "yt-dlp",
                &format!("  {version} · {}", path.display()),
            ),
            None => check(
                &mut out,
                false,
                "yt-dlp",
                &format!("  found but failed to run: {}", path.display()),
            ),
        },
        None => out.push(format!(
            "- yt-dlp  {}；{}",
            crate::error::install_hint("yt-dlp"),
            "Rerun install.sh for the bundled build"
        )),
    }
    out.push("\nConfiguration".into());
    let path = crate::settings::config_path();
    let loaded = crate::settings::load();
    match &loaded {
        Ok(cfg) => {
            if path.is_file() {
                check(&mut out, true, "config", &format!("  {}", path.display()));
            } else {
                out.push(format!(
                    "- config  Using built-in defaults: {}",
                    path.display()
                ));
                out.push("  Optional setup: notepipe config init".into());
            }
            // Validate the same merged defaults as conversion, without requiring a source or network.
            match crate::options::resolve(String::new(), &crate::cli::RunOpts::default(), cfg)
                .and_then(|cfg| cfg.validate())
            {
                Ok(()) => {}
                Err(error) => {
                    failures += 1;
                    check(&mut out, false, "Settings values", &format!("  {error:#}"));
                }
            }
            if cfg.asr.is_configured() {
                match crate::options::resolve(String::new(), &Default::default(), cfg)
                    .and_then(|cfg| cfg.validate_asr())
                {
                    Ok(()) => {
                        if let Some(program) = cfg.asr.command.first() {
                            match crate::adapter::resolve_executable(program) {
                                Ok(path) if path.is_file() => check(
                                    &mut out,
                                    true,
                                    "ASR adapter",
                                    &format!("  {}", path.display()),
                                ),
                                _ => {
                                    failures += 1;
                                    check(&mut out, false, "ASR adapter", program);
                                }
                            }
                        } else {
                            check(
                                &mut out,
                                true,
                                "ASR",
                                &format!("  {} ({})", cfg.asr.base_url, cfg.asr.mode),
                            );
                        }
                    }
                    Err(error) => {
                        failures += 1;
                        check(&mut out, false, "ASR", &format!("  {error:#}"));
                    }
                }
            } else {
                out.push("ASR is not configured; set [asr] in `notepipe config init` when transcription is needed.".into());
            }
        }
        Err(error) => {
            failures += 1;
            check(&mut out, false, "config", &format!("  {error:#}"));
        }
    }
    out.push("\n✓ available · - optional or unset · ! needs attention · ✗ must fix".into());
    for line in out {
        println!("{line}");
    }
    anyhow::ensure!(
        failures == 0,
        format!(
            "Fix the {} required-tool or configuration issues listed above",
            failures
        )
    );
    println!("Basic checks passed; optional services were not tested online.");
    Ok(())
}
