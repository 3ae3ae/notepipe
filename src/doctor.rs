//! `notepipe doctor`: reports tools, adapters and settings in one place.
//! Its output explains most setup problems in bug reports.

use anyhow::Result;
use std::process::Command;
use std::time::Duration;

/// Longest version line shown, since some tools print very long ones.
const VERSION_LINE_MAX_CHARS: usize = 72;

/// Time limit for one tool's version check, so a broken binary cannot hang doctor.
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
                    &format!("  Missing: {}", crate::error::FFMPEG_INSTALL_HINT),
                );
            }
        }
    }
    out.push("\nOptional tools by use".into());
    let path = crate::settings::config_path();
    let loaded = crate::settings::load();
    let default_source = crate::settings::Source::default();
    let source = loaded.as_ref().map_or(&default_source, |cfg| &cfg.download);
    match crate::source::command(source) {
        Ok((path, _)) => check(
            &mut out,
            true,
            "Download adapter",
            &format!("  {}", path.display()),
        ),
        Err(error) => out.push(format!("- Download adapter unavailable. {error:#}")),
    }
    out.push("\nConfiguration".into());
    match &loaded {
        Ok(cfg) => {
            if path.is_file() {
                check(&mut out, true, "config", &format!("  {}", path.display()));
            } else {
                out.push(format!("- config  Not created yet: {}", path.display()));
                out.push("  Create it with: notepipe config init".into());
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
            if cfg.speech.is_configured() {
                match crate::options::resolve(String::new(), &Default::default(), cfg)
                    .and_then(|cfg| cfg.validate_asr())
                {
                    Ok(()) => {
                        if let Some(program) = cfg.speech.command.first() {
                            match crate::adapter::resolve_executable(program) {
                                Ok(path) if path.is_file() => check(
                                    &mut out,
                                    true,
                                    "Speech adapter",
                                    &format!("  {}", path.display()),
                                ),
                                _ => {
                                    failures += 1;
                                    check(&mut out, false, "Speech adapter", program);
                                }
                            }
                        } else {
                            check(
                                &mut out,
                                true,
                                "Speech service",
                                &format!("  {} ({})", cfg.speech.base_url, cfg.speech.mode),
                            );
                        }
                    }
                    Err(error) => {
                        failures += 1;
                        check(&mut out, false, "Speech service", &format!("  {error:#}"));
                    }
                }
            } else {
                out.push("- Speech recognition is not set up; it is needed only when there are no subtitles. Set [speech] in the configuration file.".into());
            }
            let mut checked = std::collections::HashSet::new();
            for program in std::iter::once(&cfg.ai.adapter)
                .chain(cfg.stages.iter().map(|stage| &stage.adapter))
                .filter_map(|argv| argv.first())
                .filter(|program| checked.insert(*program))
            {
                match crate::adapter::resolve_executable(program) {
                    Ok(path) if path.is_file() => check(
                        &mut out,
                        true,
                        "AI adapter",
                        &format!("  {}", path.display()),
                    ),
                    _ => {
                        failures += 1;
                        check(&mut out, false, "AI adapter", &format!(" {program}"));
                    }
                }
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
