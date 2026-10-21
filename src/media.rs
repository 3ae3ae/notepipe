//! ffmpeg helpers: audio extraction (16 kHz mono PCM WAV) and media probing.

use anyhow::{Context, Result};
use std::path::Path;
use std::time::Duration;
use tokio::process::Command;

/// Time limit for ffprobe, so a broken input cannot hang the run.
const PROBE_TIMEOUT: Duration = Duration::from_secs(60);

/// Runs ffprobe with a time limit; returns None on timeout or failure (probing is never fatal).
async fn probe_output(cmd: &mut Command) -> Option<std::process::Output> {
    let output = cmd
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null())
        .output();
    tokio::time::timeout(PROBE_TIMEOUT, output).await.ok()?.ok()
}

/// Runs a command and returns an error with the end of stderr if it fails.
pub(crate) async fn run_cmd(cmd: &mut Command, what: &str) -> Result<std::process::Output> {
    let out = cmd
        .kill_on_drop(true)
        .output()
        .await
        .with_context(|| format!("Could not start {}", what))?;
    if !out.status.success() {
        return Err(crate::error::cmd_error(
            what,
            out.status.code(),
            &String::from_utf8_lossy(&out.stderr),
        ));
    }
    Ok(out)
}

/// Extracts 16 kHz mono 16-bit WAV audio, unless it already exists.
pub async fn extract_audio(media: &Path, dest: &Path) -> Result<()> {
    if dest.is_file() {
        tracing::info!(path = %dest.display(), "audio exists, skip extract");
        return Ok(());
    }
    if let Some(p) = dest.parent() {
        tokio::fs::create_dir_all(p).await?;
    }
    let temporary = tempfile::Builder::new()
        .suffix(".wav")
        .tempfile_in(dest.parent().unwrap_or(Path::new(".")))?;
    run_cmd(
        Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-y"])
            .arg("-i")
            .arg(media)
            .args(["-vn", "-ac", "1", "-ar", "16000", "-c:a", "pcm_s16le"])
            .arg(temporary.path()),
        "ffmpeg",
    )
    .await?;
    temporary.persist(dest)?;
    Ok(())
}

#[derive(Debug, Clone, Copy)]
pub struct VideoInfo {
    pub width: u32,
    pub height: u32,
    pub duration: f64,
}

/// The fields used from ffprobe's JSON output.
#[derive(serde::Deserialize)]
struct ProbeJson {
    #[serde(default)]
    streams: Vec<ProbeStream>,
    format: Option<ProbeFormat>,
}

#[derive(serde::Deserialize)]
struct ProbeStream {
    width: Option<u32>,
    height: Option<u32>,
}

#[derive(serde::Deserialize)]
struct ProbeFormat {
    /// ffprobe reports duration as a string, for example "95.040000".
    duration: Option<String>,
}

/// Video size and duration, from ffprobe's JSON output.
pub async fn probe_video(media: &Path) -> Result<Option<VideoInfo>> {
    let out = probe_output(
        Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-select_streams",
                "V:0",
                "-show_entries",
                "stream=width,height:format=duration",
                "-of",
                "json",
            ])
            .arg(media),
    )
    .await
    .context("Could not run ffprobe for video streams")?;
    if !out.status.success() {
        return Err(crate::error::cmd_error(
            "ffprobe",
            out.status.code(),
            &String::from_utf8_lossy(&out.stderr),
        ));
    }
    let probe: ProbeJson =
        serde_json::from_slice(&out.stdout).context("Invalid ffprobe response")?;
    // Uppercase V excludes attached pictures such as album covers.
    let Some(stream) = probe.streams.first() else {
        return Ok(None);
    };
    let duration = probe
        .format
        .and_then(|f| f.duration)
        .and_then(|d| d.parse::<f64>().ok())
        .context("Could not read video duration")?;
    Ok(Some(VideoInfo {
        width: stream
            .width
            .filter(|&width| width > 0)
            .context("Could not read video width")?,
        height: stream
            .height
            .filter(|&height| height > 0)
            .context("Could not read video height")?,
        duration,
    }))
}

/// ffprobe arguments that print only the duration, shared by both probe_duration variants.
fn duration_args() -> [&'static str; 6] {
    [
        "-v",
        "error",
        "-show_entries",
        "format=duration",
        "-of",
        "default=noprint_wrappers=1:nokey=1",
    ]
}

/// Duration in seconds from ffprobe output; None when ffprobe failed or printed nonsense.
fn parse_duration_text(text: &str) -> Option<f64> {
    text.trim().parse::<f64>().ok()
}

fn parse_duration(out: &std::process::Output) -> Option<f64> {
    out.status
        .success()
        .then(|| parse_duration_text(&String::from_utf8_lossy(&out.stdout)))
        .flatten()
}

/// Duration in seconds via ffprobe; None on failure.
pub async fn probe_duration(media: &Path) -> Option<f64> {
    let out = probe_output(Command::new("ffprobe").args(duration_args()).arg(media)).await?;
    parse_duration(&out)
}
