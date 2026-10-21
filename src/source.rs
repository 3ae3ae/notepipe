//! Source adapters resolve URLs into local media and subtitle files.

use crate::{runtime, settings::Source, subtitle};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    io::{BufRead, Read},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

pub const INSTALL_HINT: &str = "This URL needs a source adapter. Install it with: uv tool install notepipe --with notepipe-source-ytdlp";
const MAX_LINE_BYTES: u64 = 1024 * 1024;

/// Note metadata; field names are retained for existing note documents.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VideoMeta {
    pub title: String,
    #[serde(default)]
    pub uploader: String,
    #[serde(default)]
    pub duration: f64,
    pub webpage_url: String,
    #[serde(default)]
    pub extractor: String,
    #[serde(default)]
    pub id: String,
}

impl VideoMeta {
    pub fn is_local(&self) -> bool {
        self.extractor == "local"
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        crate::checkpoint::atomic_write_shared(path, serde_json::to_string_pretty(self)?.as_bytes())
    }
}

#[derive(Serialize, Deserialize)]
struct Output {
    media: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    author: String,
    #[serde(default)]
    duration: f64,
    language: Option<String>,
    url: Option<String>,
    #[serde(default)]
    subtitles: Vec<Caption>,
}

#[derive(Serialize, Deserialize)]
struct Caption {
    path: String,
    language: Option<String>,
    #[serde(default)]
    automatic: bool,
}

pub struct Resolved {
    pub media: PathBuf,
    pub meta: VideoMeta,
    pub language: Option<String>,
    pub subtitles: Vec<subtitle::SubtitleTrack>,
}

fn output_path(root: &Path, relative: &str) -> Result<PathBuf> {
    let path = crate::artifact::safe_asset_path(root, relative)
        .context("Invalid source adapter path; paths must stay inside its output directory")?;
    ensure!(path.is_file(), "Source adapter path is not a file");
    Ok(path.canonicalize()?)
}

impl Output {
    fn resolve(&self, root: &Path, url: &str) -> Result<Resolved> {
        ensure!(
            self.duration.is_finite() && self.duration >= 0.,
            "Invalid source adapter duration"
        );
        if let Some(url) = self.url.as_deref().filter(|url| !url.is_empty()) {
            crate::config::ensure_http_url(url).context("Invalid source adapter URL")?;
        }
        let media = output_path(root, &self.media)?;
        let subtitles = self
            .subtitles
            .iter()
            .enumerate()
            .map(|(index, caption)| {
                let mut track = subtitle::file_track(
                    output_path(root, &caption.path)?,
                    caption.language.clone(),
                );
                track.source_order = index;
                track.kind = if caption.automatic {
                    subtitle::SubtitleKind::Automatic
                } else {
                    subtitle::SubtitleKind::Manual
                };
                Ok(track)
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Resolved {
            meta: VideoMeta {
                title: if self.title.trim().is_empty() {
                    media
                        .file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                } else {
                    self.title.clone()
                },
                uploader: self.author.clone(),
                duration: self.duration,
                webpage_url: self
                    .url
                    .as_deref()
                    .filter(|value| !value.is_empty())
                    .unwrap_or(url)
                    .into(),
                extractor: "web".into(),
                id: crate::execution::digest(url.as_bytes()),
            },
            media,
            language: self.language.clone(),
            subtitles,
        })
    }
}

/// Async callers can drop this future; the existing process guard kills and reaps the child.
pub async fn resolve(
    url: &str,
    source: &Source,
    cache: &Path,
    no_download: bool,
) -> Result<Resolved> {
    let cancelled = Arc::new(AtomicBool::new(false));
    let _cancel_on_drop = runtime::CancelOnDrop(cancelled.clone());
    let (url, source, cache) = (url.to_owned(), source.clone(), cache.to_path_buf());
    tokio::task::spawn_blocking(move || {
        resolve_blocking(&url, &source, &cache, no_download, &cancelled)
    })
    .await
    .context("Source adapter task interrupted")?
}

fn resolve_blocking(
    url: &str,
    source: &Source,
    cache: &Path,
    no_download: bool,
    cancelled: &AtomicBool,
) -> Result<Resolved> {
    crate::config::ensure_http_url(url)?;
    std::fs::create_dir_all(cache)?;
    let _lock = runtime::lock_file(&cache.join(".lock"))?;
    let directory = cache.join("files");
    let marker = cache.join("complete.json");
    if marker.is_file() {
        let output: Output = serde_json::from_slice(&std::fs::read(&marker)?)
            .context("Invalid source adapter cache")?;
        return output.resolve(&directory, url);
    }
    ensure!(
        !no_download,
        "Cached source media is missing; allow downloading it again"
    );
    let program = source
        .command
        .first()
        .filter(|program| !program.trim().is_empty())
        .context("The source adapter command must name an executable")?;
    let executable = crate::adapter::resolve_executable(program).with_context(|| INSTALL_HINT)?;
    ensure!(executable.is_file(), "{INSTALL_HINT}");
    // Only incomplete output owned by this cache is replaced on a retry.
    if directory.exists() {
        std::fs::remove_dir_all(&directory)?;
    }
    std::fs::create_dir(&directory)?;
    let directory = directory.canonicalize()?;
    let mut command = Command::new(executable);
    command
        .args(&source.command[1..])
        .arg(url)
        .arg("--out")
        .arg(&directory)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = runtime::ManagedChild::spawn_adapter("source adapter", &mut command)?;
    let stderr = runtime::StderrTail::default();
    let drain = stderr.drain(
        child
            .take_stderr()
            .context("Missing source adapter stderr pipe")?,
        "source adapter",
    );
    let stdout = child
        .take_stdout()
        .context("Missing source adapter stdout pipe")?;
    let (sender, receiver) = mpsc::sync_channel(1);
    let reader = std::thread::spawn(move || {
        let mut reader = std::io::BufReader::new(stdout);
        loop {
            let mut line = Vec::new();
            let read = reader
                .by_ref()
                .take(MAX_LINE_BYTES + 1)
                .read_until(b'\n', &mut line);
            if matches!(read, Ok(0)) {
                break;
            }
            let line = read.map_err(anyhow::Error::from).and_then(|_| {
                ensure!(
                    line.len() as u64 <= MAX_LINE_BYTES,
                    "Source adapter output line exceeds 1 MiB"
                );
                Ok(line)
            });
            let failed = line.is_err();
            if sender.send(line).is_err() || failed {
                break;
            }
        }
    });
    crate::progress::stage("download", "start");
    let result = (|| {
        let mut last = None;
        loop {
            ensure!(
                !cancelled.load(Ordering::Relaxed),
                "Source adapter cancelled"
            );
            crate::dispatch::check_control()?;
            match receiver.recv_timeout(Duration::from_millis(50)) {
                Ok(line) => {
                    let value: serde_json::Value =
                        serde_json::from_slice(&line?).context("Invalid source adapter JSON")?;
                    if let Some(progress) = value.get("progress") {
                        let progress = progress
                            .as_f64()
                            .filter(|value| (0.0..=1.0).contains(value))
                            .context("Invalid source adapter progress")?;
                        crate::progress::download_progress(
                            (progress * 10000.).round() as u64,
                            10000,
                        );
                        last = None;
                    } else {
                        last = Some(
                            serde_json::from_value::<Output>(value)
                                .context("Invalid source adapter result")?,
                        );
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    if let Some(status) = child.try_wait() {
                        ensure!(status.success(), "Source adapter failed ({status})");
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        }
        let output = last.context("Source adapter returned no result")?;
        let resolved = output.resolve(&directory, url)?;
        crate::checkpoint::atomic_write(&marker, &serde_json::to_vec(&output)?)?;
        Ok(resolved)
    })();
    child.stop();
    drop(receiver);
    let _ = reader.join();
    let _ = drain.join();
    let result = result.map_err(|error| stderr.attach(error, "Source adapter diagnostics"))?;
    crate::progress::stage("download", "done");
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(record: &Path, behavior: &str) -> Option<Source> {
        let python = runtime::which("python3").or_else(|| runtime::which("python"))?;
        Some(Source {
            command: vec![
                python.display().to_string(),
                concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tests/fixtures/source_adapter.py"
                )
                .into(),
                "--record".into(),
                record.display().to_string(),
                "--behavior".into(),
                behavior.into(),
            ],
        })
    }

    #[test]
    fn only_media_is_required_and_all_output_paths_stay_inside_the_directory() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("media.wav"), b"audio").unwrap();
        let mut output: Output = serde_json::from_str(r#"{"media":"media.wav"}"#).unwrap();
        let resolved = output
            .resolve(root.path(), "https://example.test/media")
            .unwrap();
        assert_eq!(resolved.meta.title, "media");
        assert_eq!(resolved.meta.webpage_url, "https://example.test/media");
        for path in [
            "",
            ".",
            "..",
            "../media.wav",
            "/media.wav",
            "sub/../../media.wav",
        ] {
            output.media = path.into();
            assert!(
                output
                    .resolve(root.path(), "https://example.test/media")
                    .is_err(),
                "{path}"
            );
        }
        output.media = "media.wav".into();
        output.subtitles.push(Caption {
            path: "../captions.srt".into(),
            language: None,
            automatic: false,
        });
        assert!(
            output
                .resolve(root.path(), "https://example.test/media")
                .is_err()
        );
        #[cfg(unix)]
        {
            let outside = tempfile::NamedTempFile::new().unwrap();
            std::os::unix::fs::symlink(outside.path(), root.path().join("escaped.wav")).unwrap();
            assert!(output_path(root.path(), "escaped.wav").is_err());
        }
    }

    #[test]
    fn retries_replace_incomplete_output_and_completed_output_is_reused() {
        let _dispatch = runtime::lock_recover(&crate::dispatch::TEST_LOCK);
        let root = tempfile::tempdir().unwrap();
        let record = root.path().join("record");
        let Some(broken) = command(&record, "malformed") else {
            return;
        };
        runtime::block_on(async {
            let cache = root.path().join("cache");
            assert!(
                resolve("https://example.test/media", &broken, &cache, false)
                    .await
                    .is_err()
            );
            assert!(!cache.join("complete.json").exists());
            let source = command(&record, "files").unwrap();
            let first = resolve("https://example.test/media", &source, &cache, false)
                .await
                .unwrap();
            let again = resolve("https://example.test/media", &source, &cache, true)
                .await
                .unwrap();
            assert_eq!(first.media, again.media);
            assert!(again.media.is_file());
            assert_eq!(
                std::fs::read_to_string(record.join("calls"))
                    .unwrap()
                    .lines()
                    .count(),
                2
            );
        })
        .unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn dropping_resolution_stops_the_adapter_and_its_children() {
        let _dispatch = runtime::lock_recover(&crate::dispatch::TEST_LOCK);
        let root = tempfile::tempdir().unwrap();
        let record = root.path().join("record");
        let Some(source) = command(&record, "hang") else {
            return;
        };
        runtime::block_on(async {
            let cache = root.path().join("cache");
            let task = tokio::spawn(async move {
                resolve("https://example.test/media", &source, &cache, false).await
            });
            let start = std::time::Instant::now();
            while !record.join("child-ready").exists() {
                assert!(start.elapsed() < Duration::from_secs(10));
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            let pid: i32 = std::fs::read_to_string(record.join("pid"))
                .unwrap()
                .parse()
                .unwrap();
            task.abort();
            assert!(task.await.err().unwrap().is_cancelled());
            let start = std::time::Instant::now();
            while unsafe { libc::kill(pid, 0) } == 0 {
                assert!(start.elapsed() < Duration::from_secs(5));
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert!(record.join("child-stopped").is_file());
            assert!(!root.path().join("cache/complete.json").exists());
        })
        .unwrap();
    }
}
