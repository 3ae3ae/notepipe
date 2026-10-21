//! Source adapters resolve URLs into local media and subtitle files.

use crate::{runtime, settings::Source, subtitle};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};
use tokio::io::{AsyncBufReadExt, AsyncReadExt};

pub const ADAPTER_HINT: &str =
    "URLs need a source adapter; see the README, or set [source].command";
const MAX_LINE_BYTES: u64 = 1024 * 1024;

/// Public, vendor-neutral note metadata.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MediaMeta {
    pub title: String,
    pub author: String,
    pub duration: f64,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seek_url: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Output {
    media: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    author: String,
    #[serde(default)]
    duration: f64,
    language: Option<String>,
    url: Option<String>,
    seek_url: Option<String>,
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
    pub meta: MediaMeta,
}

fn output_path(root: &Path, relative: &str) -> Result<PathBuf> {
    let path = crate::artifact::safe_asset_path(root, relative)
        .context("Invalid source adapter path; paths must stay inside its output directory")?;
    ensure!(path.is_file(), "Source adapter path is not a file");
    Ok(path.canonicalize()?)
}

impl Output {
    pub(crate) fn read(cache: &Path) -> Result<Option<Self>> {
        let path = cache.join("complete.json");
        if !path.is_file() {
            return Ok(None);
        }
        Ok(Some(
            serde_json::from_slice(&std::fs::read(path)?)
                .context("Invalid source adapter cache")?,
        ))
    }

    pub(crate) fn metadata(&self, source: &str) -> Result<MediaMeta> {
        ensure!(
            self.duration.is_finite() && self.duration >= 0.,
            "Invalid source adapter duration"
        );
        if let Some(url) = self.url.as_deref().filter(|url| !url.is_empty()) {
            crate::config::ensure_http_url(url).context("Invalid source adapter URL")?;
        }
        if let Some(template) = &self.seek_url {
            crate::config::ensure_http_url(&template.replace("{t}", "0"))
                .context("Invalid source adapter seek URL")?;
        }
        Ok(MediaMeta {
            title: if self.title.trim().is_empty() {
                Path::new(&self.media)
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            } else {
                self.title.clone()
            },
            author: self.author.clone(),
            duration: self.duration,
            source: source.into(),
            seek_url: self.seek_url.clone(),
        })
    }

    pub(crate) fn tracks(&self, root: &Path) -> Result<Vec<subtitle::SubtitleTrack>> {
        self.subtitles
            .iter()
            .enumerate()
            .map(|(index, caption)| {
                Ok(subtitle::SubtitleTrack {
                    path: output_path(root, &caption.path)?,
                    language: caption.language.clone(),
                    source_order: index,
                    kind: if caption.automatic {
                        subtitle::SubtitleKind::Automatic
                    } else {
                        subtitle::SubtitleKind::Manual
                    },
                })
            })
            .collect()
    }

    pub(crate) fn media_path(&self, cache: &Path) -> Result<PathBuf> {
        output_path(&cache.join("files"), &self.media)
    }

    pub(crate) fn preferred_subtitle(
        &self,
        cache: &Path,
        language: &str,
    ) -> Result<Option<PathBuf>> {
        let mut tracks = self.tracks(&cache.join("files"))?;
        subtitle::sort_tracks(&mut tracks, language, self.language.as_deref());
        Ok(tracks.into_iter().next().map(|track| track.path))
    }

    fn resolve(&self, root: &Path, source: &str) -> Result<Resolved> {
        // Validate caption paths even though callers select captions from the cached output.
        self.tracks(root)?;
        Ok(Resolved {
            media: output_path(root, &self.media)?,
            meta: self.metadata(source)?,
        })
    }
}

/// An explicit command wins; otherwise discover exactly one adapter, preferring sibling executables.
pub fn command(source: &Source) -> Result<(PathBuf, &[String])> {
    if let Some((program, args)) = source.command.split_first() {
        ensure!(
            !program.trim().is_empty(),
            "The source adapter command must name an executable"
        );
        let path = crate::adapter::resolve_executable(program).context(ADAPTER_HINT)?;
        ensure!(path.is_file(), "{ADAPTER_HINT}");
        return Ok((path, args));
    }
    let executable = std::env::current_exe()?.canonicalize()?;
    let search_path = std::env::var_os("PATH");
    let directories = std::iter::once(
        executable
            .parent()
            .context("Missing executable directory")?
            .to_path_buf(),
    )
    .chain(search_path.iter().flat_map(std::env::split_paths));
    Ok((discover(directories)?, &[]))
}

fn discover(directories: impl IntoIterator<Item = PathBuf>) -> Result<PathBuf> {
    let mut found = std::collections::BTreeMap::new();
    let cwd = std::env::current_dir()?;
    let pathext = std::env::var("PATHEXT").ok();
    for directory in directories
        .into_iter()
        .filter(|path| !path.as_os_str().is_empty())
    {
        let directory = cwd.join(directory);
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if cfg!(windows) && !windows_executable(name, pathext.as_deref()) {
                continue;
            }
            let key = if cfg!(windows) {
                Path::new(name)
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_ascii_lowercase()
            } else {
                name.into()
            };
            if key.starts_with("notepipe-source-")
                && let Ok(path) = which::which_in(name, Some(directory.as_os_str()), &directory)
            {
                found.entry(key).or_insert(path);
            }
        }
    }
    ensure!(!found.is_empty(), "{ADAPTER_HINT}");
    ensure!(
        found.len() == 1,
        "Multiple source adapters found: {}. Set [source].command.",
        found.keys().cloned().collect::<Vec<_>>().join(", ")
    );
    Ok(found.into_values().next().unwrap())
}

fn windows_executable(name: &str, pathext: Option<&str>) -> bool {
    let Some(extension) = Path::new(name).extension().and_then(|value| value.to_str()) else {
        return false;
    };
    pathext
        .unwrap_or(".COM;.EXE;.BAT;.CMD")
        .split(';')
        .any(|value| {
            value
                .strip_prefix('.')
                .is_some_and(|value| value.eq_ignore_ascii_case(extension))
        })
}

fn accept_line(line: &[u8], last: &mut Option<Output>) -> Result<()> {
    ensure!(
        line.len() as u64 <= MAX_LINE_BYTES,
        "Source adapter output line exceeds 1 MiB"
    );
    let value: serde_json::Value =
        serde_json::from_slice(line).context("Invalid source adapter JSON")?;
    if let Some(progress) = value.get("progress") {
        let progress = progress
            .as_f64()
            .filter(|value| (0.0..=1.0).contains(value))
            .context("Invalid source adapter progress")?;
        crate::progress::download_progress((progress * 10000.).round() as u64, 10000);
        *last = None;
    } else {
        *last = Some(serde_json::from_value(value).context("Invalid source adapter result")?);
    }
    Ok(())
}

/// Tokio reads stdout while the shared process guard retains ownership of the whole tree.
pub async fn resolve(url: &str, source: &Source, cache: &Path) -> Result<Resolved> {
    runtime::check_cancelled()?;
    crate::config::ensure_http_url(url)?;
    std::fs::create_dir_all(cache)?;
    let _lock = runtime::lock_file(&cache.join(".lock"))?;
    let directory = cache.join("files");
    let marker = cache.join("complete.json");
    if marker.is_file() {
        let output: Output = serde_json::from_slice(&std::fs::read(&marker)?)
            .context("Invalid source adapter cache")?;
        match output.resolve(&directory, url) {
            Ok(resolved) => return Ok(resolved),
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) => {}
            Err(error) => return Err(error),
        }
    }
    let (executable, args) = command(source)?;
    runtime::check_cancelled()?;
    if marker.exists() {
        std::fs::remove_file(&marker)?;
    }
    // Only incomplete output owned by this cache is replaced on a retry.
    if directory.exists() {
        std::fs::remove_dir_all(&directory)?;
    }
    std::fs::create_dir(&directory)?;
    let directory = directory.canonicalize()?;
    let mut command = Command::new(executable);
    command
        .args(args)
        .arg(url)
        .arg("--out")
        .arg(&directory)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    runtime::check_cancelled()?;
    let mut child = runtime::ManagedChild::spawn_tree("source adapter", &mut command)?;
    let stderr = runtime::StderrTail::default();
    let drain = stderr.drain(
        child
            .take_stderr()
            .context("Missing source adapter stderr pipe")?,
        "source adapter",
    );
    let stdout = tokio::process::ChildStdout::from_std(
        child
            .take_stdout()
            .context("Missing source adapter stdout pipe")?,
    )?;
    let mut stdout = tokio::io::BufReader::new(stdout);
    let mut line = Vec::new();
    let mut last = None;
    let mut eof = false;
    let poll = Duration::from_millis(50);
    let mut exited = tokio::time::interval_at(tokio::time::Instant::now() + poll, poll);
    crate::progress::stage("download", "start");
    let result = async {
        let status = loop {
            let mut bounded = (&mut stdout).take(MAX_LINE_BYTES + 1 - line.len() as u64);
            tokio::select! {
                biased;
                error = runtime::watch_cancelled() => return Err(error),
                count = bounded.read_until(b'\n', &mut line), if !eof => {
                    if count? == 0 { eof = true; }
                    else { accept_line(&line, &mut last)?; line.clear(); }
                },
                _ = exited.tick() => if let Some(status) = child.try_wait() {
                    break status;
                },
            }
        };
        child.stop();
        // An exited child can still have a final result buffered in its pipe.
        while !eof {
            runtime::check_cancelled()?;
            let mut bounded = (&mut stdout).take(MAX_LINE_BYTES + 1 - line.len() as u64);
            if bounded.read_until(b'\n', &mut line).await? == 0 {
                eof = true;
            } else {
                accept_line(&line, &mut last)?;
                line.clear();
            }
        }
        if !line.is_empty() {
            accept_line(&line, &mut last)?;
        }
        Ok(status)
    }
    .await;
    child.stop();
    let _ = drain.join();
    let result = result
        .and_then(|status| {
            ensure!(status.success(), "Source adapter failed ({status})");
            let output = last.context("Source adapter returned no result")?;
            let resolved = output.resolve(&directory, url)?;
            runtime::check_cancelled()?;
            crate::checkpoint::atomic_write(&marker, &serde_json::to_vec(&output)?)?;
            Ok(resolved)
        })
        .map_err(|error| stderr.attach(error, "Source adapter diagnostics"))?;
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
        assert_eq!(resolved.meta.source, "https://example.test/media");
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
    fn seek_templates_are_optional_and_must_use_http() {
        let mut output: Output = serde_json::from_str(r#"{"media":"media.wav"}"#).unwrap();
        let source = "https://example.test/media?x=1#part";
        assert!(output.metadata(source).unwrap().seek_url.is_none());
        let template = "https://example.test/play?at={t}#part";
        output.seek_url = Some(template.into());
        let meta = output.metadata(source).unwrap();
        assert_eq!(meta.source, source);
        assert_eq!(meta.seek_url.as_deref(), Some(template));
        output.seek_url = Some("javascript:alert({t})".into());
        assert!(output.metadata(source).is_err());
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
                resolve("https://example.test/media", &broken, &cache)
                    .await
                    .is_err()
            );
            assert!(!cache.join("complete.json").exists());
            let source = command(&record, "files").unwrap();
            let first = resolve("https://example.test/media", &source, &cache)
                .await
                .unwrap();
            let again = resolve("https://example.test/media", &source, &cache)
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

    #[test]
    fn missing_media_or_caption_files_make_a_completed_cache_a_miss() {
        let root = tempfile::tempdir().unwrap();
        let record = root.path().join("record");
        let Some(source) = command(&record, "files") else {
            return;
        };
        runtime::block_on(async {
            let cache = root.path().join("cache");
            let first = resolve("https://example.test/media", &source, &cache)
                .await
                .unwrap();
            let tracks = Output::read(&cache)
                .unwrap()
                .unwrap()
                .tracks(&cache.join("files"))
                .unwrap();
            for path in [&first.media, &tracks[0].path] {
                std::fs::remove_file(path).unwrap();
                let again = resolve("https://example.test/media", &source, &cache)
                    .await
                    .unwrap();
                assert!(again.media.is_file());
                assert!(
                    Output::read(&cache)
                        .unwrap()
                        .unwrap()
                        .tracks(&cache.join("files"))
                        .unwrap()
                        .iter()
                        .all(|track| track.path.is_file())
                );
            }
            assert_eq!(
                std::fs::read_to_string(record.join("calls"))
                    .unwrap()
                    .lines()
                    .count(),
                3
            );
        })
        .unwrap();
    }

    #[test]
    fn async_reads_preserve_partial_lines_and_reject_oversized_lines() {
        let root = tempfile::tempdir().unwrap();
        let Some(source) = command(&root.path().join("fragmented"), "fragmented") else {
            return;
        };
        runtime::block_on(async {
            assert!(
                resolve(
                    "https://example.test/media",
                    &source,
                    &root.path().join("cache"),
                )
                .await
                .is_ok()
            );
            let source = command(&root.path().join("oversized"), "oversized").unwrap();
            let error = resolve(
                "https://example.test/media",
                &source,
                &root.path().join("oversized-cache"),
            )
            .await
            .err()
            .unwrap();
            assert!(format!("{error:#}").contains("exceeds 1 MiB"));
        })
        .unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn an_exited_adapter_does_not_wait_for_a_helper_holding_stdout() {
        let root = tempfile::tempdir().unwrap();
        let Some(source) = command(&root.path().join("record"), "orphan") else {
            return;
        };
        runtime::block_on(async {
            let resolved = tokio::time::timeout(
                Duration::from_secs(3),
                resolve(
                    "https://example.test/media",
                    &source,
                    &root.path().join("cache"),
                ),
            )
            .await
            .unwrap()
            .unwrap();
            assert!(resolved.media.is_file());
        })
        .unwrap();
    }

    #[test]
    fn an_immediate_exit_preserves_the_complete_final_unterminated_line() {
        let root = tempfile::tempdir().unwrap();
        let Some(source) = command(&root.path().join("record"), "instant") else {
            return;
        };
        runtime::block_on(async {
            for number in 0..3 {
                let cache = root.path().join(format!("cache-{number}"));
                let resolved = resolve("https://example.test/media", &source, &cache)
                    .await
                    .unwrap();
                assert_eq!(resolved.meta.title.len(), 131072);
                assert!(resolved.media.is_file());
                assert_eq!(
                    Output::read(&cache)
                        .unwrap()
                        .unwrap()
                        .tracks(&cache.join("files"))
                        .unwrap()
                        .len(),
                    3
                );
                assert!(cache.join("complete.json").is_file());
            }
        })
        .unwrap();
    }

    #[test]
    fn windows_discovery_only_accepts_executable_extensions() {
        for extension in ["com", "EXE", "Bat", "cMd"] {
            assert!(windows_executable(
                &format!("notepipe-source-test.{extension}"),
                None
            ));
        }
        for name in [
            "notepipe-source-test",
            "notepipe-source-test.txt",
            "notepipe-source-test.pdb",
            "notepipe-source-test.exe.manifest",
        ] {
            assert!(!windows_executable(name, None), "{name}");
        }
        assert!(windows_executable(
            "notepipe-source-test.PY",
            Some(".Py;.EXE")
        ));
        assert!(!windows_executable(
            "notepipe-source-test.cmd",
            Some(".Py;.EXE")
        ));
        assert!(!windows_executable("notepipe-source-test.exe", Some("")));
    }

    #[test]
    fn discovery_handles_none_one_and_multiple_adapters_and_prefers_siblings() {
        let root = tempfile::tempdir_in(".").unwrap();
        let sibling = root.path().join("sibling");
        let path = root.path().join("path");
        std::fs::create_dir(&sibling).unwrap();
        std::fs::create_dir(&path).unwrap();
        let directories = [sibling.clone(), path.clone()];
        assert!(
            discover(directories.clone())
                .unwrap_err()
                .to_string()
                .contains(ADAPTER_HINT)
        );
        let executable = |directory: &Path, name: &str| {
            let path = directory.join(if cfg!(windows) {
                format!("{name}.cmd")
            } else {
                name.into()
            });
            std::fs::write(&path, "fixture").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            std::env::current_dir().unwrap().join(path)
        };
        let first = executable(&sibling, "notepipe-source-first");
        assert_eq!(discover(directories.clone()).unwrap(), first);
        executable(&path, "notepipe-source-first");
        assert_eq!(discover(directories.clone()).unwrap(), first);
        executable(&path, "notepipe-source-second");
        let error = discover(directories).unwrap_err().to_string();
        for text in [
            "notepipe-source-first",
            "notepipe-source-second",
            "[source].command",
        ] {
            assert!(error.contains(text), "{error}");
        }
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
                resolve("https://example.test/media", &source, &cache).await
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
