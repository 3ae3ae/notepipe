//! ffmpeg helpers: audio extraction (16 kHz mono PCM WAV) and media probing.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::process::Command;

pub(crate) struct Recording {
    pub file: PathBuf,
    pub digest: String,
}
impl Recording {
    pub fn resolve(file: PathBuf, digest: Option<&str>) -> Result<Self> {
        let digest = match digest {
            Some(digest) => digest.to_owned(),
            None => crate::runtime::file_digest(&file)?,
        };
        Ok(Self { file, digest })
    }
}

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

/// Extracts 16 kHz mono 16-bit WAV audio.
pub async fn extract_audio(media: &Path, dest: &Path) -> Result<()> {
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
    pub duration: Option<f64>,
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
        .and_then(|duration| parse_duration_text(&duration).ok());
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

/// Duration in seconds from ffprobe output.
fn parse_duration_text(text: &str) -> Result<f64> {
    text.trim()
        .parse::<f64>()
        .ok()
        .filter(|duration| duration.is_finite() && *duration >= 0.)
        .context("ffprobe returned no valid duration")
}

/// Duration in seconds via ffprobe; callers report failures as warnings.
pub async fn probe_duration(media: &Path) -> Result<f64> {
    let out = probe_output(Command::new("ffprobe").args(duration_args()).arg(media))
        .await
        .context("Could not run ffprobe for duration")?;
    if !out.status.success() {
        return Err(crate::error::cmd_error(
            "ffprobe",
            out.status.code(),
            &String::from_utf8_lossy(&out.stderr),
        ));
    }
    parse_duration_text(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recording_resolves_new_bytes_and_reuses_a_known_digest_without_the_file() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("recording");
        std::fs::write(&file, "first media").unwrap();
        let first = Recording::resolve(file.clone(), None).unwrap();
        assert_eq!(first.file, file);
        assert_eq!(first.digest, crate::runtime::digest(b"first media"));
        std::fs::write(&file, "other media").unwrap();
        let changed = Recording::resolve(file.clone(), None).unwrap();
        assert_ne!(first.digest, changed.digest);
        std::fs::remove_file(&file).unwrap();
        let cached = Recording::resolve(file, Some(&first.digest)).unwrap();
        assert_eq!(cached.digest, first.digest);
    }

    #[test]
    fn duration_requires_a_finite_nonnegative_number() {
        for text in ["", "N/A", "NaN", "inf", "-1"] {
            assert!(parse_duration_text(text).is_err(), "{text}");
        }
        assert_eq!(parse_duration_text("0\n").unwrap(), 0.);
        assert_eq!(parse_duration_text("12.5\n").unwrap(), 12.5);
    }
}
