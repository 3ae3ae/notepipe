//! ASR checkpoint: each finished chunk is appended to asr.jsonl, and a rerun skips chunks by their exact time bounds.
//!
//! Rules:
//! - **Empty results count**: a chunk recognized as silence is recorded with `text: ""`;
//!   otherwise every resume would transcribe silent chunks again.
//! - **Only durable results count**: `record()` marks a chunk done only after it is written.
//! - **Corruption**: a half-written last line from a crash is tolerated and repaired;
//!   a corrupt line anywhere else is an error, because skipping it would silently lose text.

use crate::timeline::TranscriptEvent;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Schema version of the checkpoint identity. Bump it only when the checkpoint format or meaning changes:
/// it is independent of the package version, so upgrades do not discard ASR progress,
/// and every bump invalidates all existing checkpoints once.
const CHECKPOINT_SCHEMA_VERSION: u32 = 3;

/// What a checkpoint was produced with; it can be reused only by an identical identity.
/// Every field that changes the transcript must be part of it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AsrIdentity {
    pub schema_version: u32,
    pub mode: crate::settings::AsrMode,
    pub endpoint_or_command: String,
    pub model: String,
    pub language: String,
    pub max_speech: f32,
    pub vad_threshold: f32,
    pub min_silence: f32,
}

impl AsrIdentity {
    pub fn new(asr: &crate::settings::Asr) -> Self {
        let endpoint_or_command = if asr.command.is_empty() {
            let base = asr.base_url.trim();
            url::Url::parse(base)
                .map(|url| url.to_string())
                .unwrap_or_else(|_| base.to_owned())
                .trim_end_matches('/')
                .to_owned()
        } else {
            asr.command.join("\0")
        };
        Self {
            schema_version: CHECKPOINT_SCHEMA_VERSION,
            mode: asr.mode,
            endpoint_or_command,
            model: asr.model.clone(),
            language: asr.language.clone(),
            max_speech: asr.max_speech,
            vad_threshold: asr.vad_threshold,
            min_silence: asr.min_silence,
        }
    }
}

pub struct Checkpoint {
    path: PathBuf,
    file: Option<std::fs::File>,
    done: HashSet<(u64, u64)>,
    events: Vec<TranscriptEvent>,
}

/// Times as bit patterns for hashing; they come from the same JSON round trip, so equal times have equal bits.
fn key(start: f64, end: f64) -> (u64, u64) {
    (start.to_bits(), end.to_bits())
}

/// Parsed events plus what is needed to repair the file's tail before appending.
struct LoadedEvents {
    events: Vec<TranscriptEvent>,
    /// Length of the intact prefix: up to the last complete `\n`, or the whole file when the last line is valid but unterminated.
    /// A half-written last line is excluded.
    valid_len: u64,
    /// The last line is valid but has no trailing newline (a hand-edited file); add one before appending.
    needs_newline: bool,
}

impl Checkpoint {
    /// Opens the checkpoint and repairs an interrupted final record before appending.
    pub fn open(out_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(out_dir).with_context(|| {
            format!(
                "Failed to create checkpoint directory {}",
                out_dir.display()
            )
        })?;
        let path = out_dir.join("asr.jsonl");
        let mut cp = Checkpoint {
            path: path.clone(),
            file: None,
            done: HashSet::new(),
            events: vec![],
        };
        let loaded = Self::load_events(&path)?;
        if path.is_file() {
            tracing::info!(n = loaded.events.len(), "asr checkpoint resumed (partial)");
            cp.done = loaded.events.iter().map(|e| key(e.start, e.end)).collect();
            cp.events = loaded
                .events
                .into_iter()
                .filter(|e| !e.text.trim().is_empty())
                .collect();
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .read(true)
                .write(true)
                // Partial resume: keep the existing content.
                .truncate(false)
                .open(&path)
                .with_context(|| format!("Failed to open checkpoint {}", path.display()))?;
            let file_len = f.metadata().map(|m| m.len()).unwrap_or(0);
            if loaded.valid_len < file_len {
                // Truncate to the last complete line before opening the writer; otherwise a half-written line
                // from a crash and the next record would form a corrupt line in the middle of the file,
                // which the next resume would reject.
                // set_len cannot use an append-only handle: on Windows FILE_APPEND_DATA
                // does not include the right to change the length (Access denied).
                f.set_len(loaded.valid_len).with_context(|| {
                    format!(
                        "Failed to truncate checkpoint partial line {}",
                        path.display()
                    )
                })?;
            } else if loaded.needs_newline {
                // Valid last line without a trailing newline (hand-edited): add the newline before appending.
                use std::io::Seek as _;
                f.seek(std::io::SeekFrom::End(0))?;
                f.write_all(b"\n")
                    .with_context(|| format!("Failed to append newline to {}", path.display()))?;
            }
            // Not an append handle: seek to the end once; this process is the only writer.
            use std::io::Seek as _;
            f.seek(std::io::SeekFrom::End(0))
                .with_context(|| format!("seek checkpoint {}", path.display()))?;
            cp.file = Some(f);
        }
        Ok(cp)
    }

    /// Reads the event lines. A half-written last line is tolerated; corruption elsewhere is an error.
    /// Only the first record for each (start, end) is kept, in case an old file has duplicates.
    fn load_events(path: &Path) -> Result<LoadedEvents> {
        if !path.is_file() {
            return Ok(LoadedEvents {
                events: vec![],
                valid_len: 0,
                needs_newline: false,
            });
        }
        let mut bytes =
            std::fs::read(path).with_context(|| format!("Failed to read {}", path.display()))?;
        if std::str::from_utf8(&bytes).is_err_and(|e| e.error_len().is_none()) {
            // A crash may cut a UTF-8 character in the final unterminated record.
            let intact_len = bytes.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
            bytes.truncate(intact_len);
        }
        let content = std::str::from_utf8(&bytes)
            .with_context(|| format!("Failed to read {}", path.display()))?;
        let lines: Vec<&str> = content.lines().filter(|l| !l.trim().is_empty()).collect();
        let last_idx = lines.len().saturating_sub(1);
        let mut out: Vec<TranscriptEvent> = vec![];
        let mut seen: HashSet<(u64, u64)> = HashSet::new();
        for (i, line) in lines.iter().enumerate() {
            match serde_json::from_str::<TranscriptEvent>(line) {
                Ok(ev) => {
                    let k = key(ev.start, ev.end);
                    if seen.insert(k) {
                        out.push(ev);
                    } else if i != last_idx {
                        tracing::warn!(line = i + 1, "Ignored duplicate transcription checkpoint");
                    }
                }
                Err(e) => {
                    if i == last_idx && !content.ends_with('\n') {
                        // A crash can leave the last line half-written.
                        tracing::debug!(
                            "{}",
                            format!("Ignored an incomplete checkpoint tail: {}", e)
                        );
                    } else {
                        anyhow::bail!(
                            "Line {1} of the transcription progress file is damaged; transcribe again ({0}): {e}",
                            path.display(),
                            i + 1
                        );
                    }
                }
            }
        }
        // How the tail must be repaired before appending.
        let last_nl = content.rfind('\n').map(|i| i + 1).unwrap_or(0);
        let tail = &content[last_nl..];
        let (valid_len, needs_newline) = if tail.trim().is_empty() {
            (content.len() as u64, false) // ends with a newline, or empty: append as is
        } else if serde_json::from_str::<TranscriptEvent>(tail.trim()).is_ok() {
            (content.len() as u64, true) // valid last line without a newline
        } else {
            (last_nl as u64, false) // half-written line: truncate to the last complete line
        };
        Ok(LoadedEvents {
            events: out,
            valid_len,
            needs_newline,
        })
    }

    /// Whether the chunk is done and can be skipped; silent chunks with empty text count as done.
    pub fn is_done(&self, start: f64, end: f64) -> bool {
        self.done.contains(&key(start, end))
    }

    /// Finished events that have text, both loaded on resume and recorded in this run.
    /// Empty-text chunks only mark silence and are not part of the timeline.
    #[cfg(test)]
    pub fn events(&self) -> &[TranscriptEvent] {
        &self.events
    }

    /// `events()` sorted by start time; loaded and new events are otherwise unordered.
    pub fn sorted_events(&self) -> Vec<TranscriptEvent> {
        let mut all = self.events.clone();
        all.sort_by(|a, b| a.start.total_cmp(&b.start));
        all
    }

    /// Records a finished chunk (append and flush). If writing fails it returns the error and does not mark the chunk done.
    /// An empty `text` means the chunk was recognized and contains no speech.
    pub fn record(&mut self, start: f64, end: f64, text: &str) -> Result<()> {
        let ev = TranscriptEvent {
            start,
            end,
            text: text.to_string(),
            raw: None,
            below: Vec::new(),
        };
        if self.file.is_none() {
            if let Some(dir) = self.path.parent() {
                std::fs::create_dir_all(dir).with_context(|| {
                    format!("Failed to create checkpoint directory {}", dir.display())
                })?;
            }
            self.file = Some(
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&self.path)
                    .with_context(|| {
                        format!("Failed to open checkpoint {}", self.path.display())
                    })?,
            );
        }
        let line = serde_json::to_string(&ev)?;
        if let Some(f) = &mut self.file {
            writeln!(f, "{line}")
                .with_context(|| format!("Failed to write checkpoint {}", self.path.display()))?;
            f.flush()
                .with_context(|| format!("flush checkpoint {}", self.path.display()))?;
        }
        // Update the in-memory state only after the write succeeded.
        self.done.insert(key(start, end));
        if !text.trim().is_empty() {
            self.events.push(ev);
        }
        Ok(())
    }
}

/// Atomic write for small files: temporary file, fsync, rename, so a crash never leaves a partial file.
/// Crash-safe because the file is synced before the rename; the parent directory is not synced,
/// so after a power loss the directory entry itself may be missing.
/// Files are readable only by the owner (0600), because configuration and credentials use this path.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)
        .with_context(|| format!("Failed to create directory {}", dir.display()))?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.write_all(bytes)
        .with_context(|| format!("Failed to write {}", path.display()))?;
    tmp.as_file().sync_all()?;
    tmp.persist(path)
        .with_context(|| format!("Failed to replace {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("notepipe-cp-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn complete_corrupt_line_is_not_treated_as_truncation() {
        let dir = tempfile::tempdir().unwrap();
        Checkpoint::open(dir.path()).unwrap();
        std::fs::write(dir.path().join("asr.jsonl"), b"garbage\n").unwrap();
        assert!(Checkpoint::open(dir.path()).is_err());
    }

    #[test]
    fn atomic_write_replaces_existing_file_without_touching_sibling_tmp() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(path.with_extension("tmp"), b"unrelated").unwrap();
        atomic_write(&path, b"first").unwrap();
        atomic_write(&path, b"second").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"second");
        assert_eq!(
            std::fs::read(path.with_extension("tmp")).unwrap(),
            b"unrelated"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(
                mode(&path),
                0o600,
                "configuration and credentials are private"
            );
        }
    }

    #[test]
    fn empty_chunk_is_recorded_and_skipped_on_resume() {
        let d = tmpdir("empty");
        let mut cp = Checkpoint::open(&d).unwrap();
        cp.record(0.0, 2.0, "hello").unwrap();
        cp.record(2.0, 4.0, "").unwrap(); // silent chunk
        drop(cp);

        let cp = Checkpoint::open(&d).unwrap();
        assert!(cp.is_done(0.0, 2.0));
        assert!(cp.is_done(2.0, 4.0), "an empty chunk is done");
        assert_eq!(cp.events().len(), 1, "empty text is not an event");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn middle_corruption_is_hard_error() {
        let ev = |s: f64, t: &str| {
            serde_json::to_string(&TranscriptEvent {
                start: s,
                end: s + 1.0,
                text: t.into(),
                raw: None,
                below: Vec::new(),
            })
            .unwrap()
        };
        let d = tmpdir("mid");
        Checkpoint::open(&d).unwrap();
        std::fs::write(
            d.join("asr.jsonl"),
            format!("{}\n garbage \n{}\n", ev(0.0, "a"), ev(1.0, "b")),
        )
        .unwrap();
        // Corruption in the middle is an error (a half-written last line is recoverable; see the truncation test).
        assert!(Checkpoint::open(&d).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn truncated_tail_is_cut_before_resume_appends() {
        // Half-written last line: records after a resume must not be appended to the fragment,
        // which would create a corrupt middle line that the next resume rejects.
        let d = tmpdir("tailwrite");
        let ev = |s: f64, t: &str| {
            serde_json::to_string(&TranscriptEvent {
                start: s,
                end: s + 1.0,
                text: t.into(),
                raw: None,
                below: Vec::new(),
            })
            .unwrap()
        };
        std::fs::write(
            d.join("asr.jsonl"),
            format!("{}\n{{\"start\":1.", ev(0.0, "a")),
        )
        .unwrap();

        let mut cp = Checkpoint::open(&d).unwrap();
        assert_eq!(cp.events().len(), 1);
        assert!(!cp.is_done(1.0, 2.0), "a half-written line is not done");
        cp.record(2.0, 3.0, "new record").unwrap();
        drop(cp);

        // Reopen: both records read back and no corrupt middle line.
        let cp = Checkpoint::open(&d).unwrap();
        assert_eq!(
            cp.events().len(),
            2,
            "old and new records survive truncation"
        );
        assert_eq!(cp.events()[0].text, "a");
        assert_eq!(cp.events()[1].text, "new record");
        assert!(cp.is_done(2.0, 3.0));
        let _ = std::fs::remove_dir_all(&d);

        // Variant: the fragment is the only line; it is truncated before appending too.
        let d = tmpdir("tailfirst");
        std::fs::write(d.join("asr.jsonl"), b"{\"start\":").unwrap();
        let mut cp = Checkpoint::open(&d).unwrap();
        assert!(cp.events().is_empty());
        cp.record(1.0, 2.0, "recovered").unwrap();
        drop(cp);
        let cp = Checkpoint::open(&d).unwrap();
        assert_eq!(cp.events().len(), 1);
        assert_eq!(cp.events()[0].text, "recovered");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn truncated_utf8_tail_is_cut_before_resume_appends() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("asr.jsonl");
        let mut cp = Checkpoint::open(dir.path()).unwrap();
        cp.record(0.0, 1.0, "완료").unwrap();
        drop(cp);
        let intact = std::fs::read(&path).unwrap();
        let partial = "{\"start\":1,\"end\":2,\"text\":\"한국어\"}";
        let cut = partial.find('한').unwrap() + 1;
        let mut bytes = intact.clone();
        bytes.extend_from_slice(&partial.as_bytes()[..cut]);
        std::fs::write(&path, bytes).unwrap();

        let mut cp = Checkpoint::open(dir.path()).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), intact);
        assert!(cp.is_done(0.0, 1.0));
        assert!(!cp.is_done(1.0, 2.0));
        cp.record(1.0, 2.0, "복구").unwrap();
        drop(cp);

        let cp = Checkpoint::open(dir.path()).unwrap();
        assert_eq!(cp.events().len(), 2);
        assert_eq!(cp.events()[0].text, "완료");
        assert_eq!(cp.events()[1].text, "복구");
    }

    #[test]
    fn valid_tail_without_newline_gets_separator_before_append() {
        // Hand-edited file: a valid last line without a newline gets one before appending.
        let d = tmpdir("nonl");
        let l = serde_json::to_string(&TranscriptEvent {
            start: 0.0,
            end: 1.0,
            text: "hand-edited".into(),
            raw: None,
            below: Vec::new(),
        })
        .unwrap();
        std::fs::write(d.join("asr.jsonl"), &l).unwrap(); // no trailing newline

        let mut cp = Checkpoint::open(&d).unwrap();
        assert_eq!(cp.events().len(), 1, "a valid last line is kept");
        cp.record(2.0, 3.0, "appended").unwrap();
        drop(cp);

        let cp = Checkpoint::open(&d).unwrap();
        assert_eq!(cp.events().len(), 2);
        assert_eq!(cp.events()[1].text, "appended");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn unwritable_out_dir_fails_loudly() {
        // out_dir is a regular file, so the checkpoint is unusable and open must fail.
        let f = std::env::temp_dir().join(format!("notepipe-cp-file-{}", std::process::id()));
        std::fs::write(&f, b"x").unwrap();
        assert!(Checkpoint::open(&f).is_err());
        let _ = std::fs::remove_file(&f);
    }

    #[test]
    fn record_write_failure_does_not_mark_done() {
        // Open normally, then break the write path: replace the directory with a file,
        // and check that a failed record does not mark the chunk done.
        let d = tmpdir("wfail");
        let mut cp = Checkpoint::open(&d).unwrap();
        std::fs::remove_dir_all(&d).unwrap();
        std::fs::write(&d, b"x").unwrap(); // the directory becomes a file
        assert!(
            cp.record(0.0, 1.0, "t").is_err(),
            "a failed write is an error"
        );
        assert!(!cp.is_done(0.0, 1.0), "a failed write is not done");
        let _ = std::fs::remove_file(&d);
    }

    #[test]
    fn duplicate_lines_keep_first() {
        let d = tmpdir("dup");
        let l = serde_json::to_string(&TranscriptEvent {
            start: 0.0,
            end: 1.0,
            text: "first".into(),
            raw: None,
            below: Vec::new(),
        })
        .unwrap();
        let l2 = serde_json::to_string(&TranscriptEvent {
            start: 0.0,
            end: 1.0,
            text: "second".into(),
            raw: None,
            below: Vec::new(),
        })
        .unwrap();
        std::fs::write(d.join("asr.jsonl"), format!("{l}\n{l2}\n")).unwrap();
        let cp = Checkpoint::open(&d).unwrap();
        assert_eq!(cp.events().len(), 1);
        assert_eq!(cp.events()[0].text, "first");
        let _ = std::fs::remove_dir_all(&d);
    }
}

#[cfg(test)]
mod identity_tests {
    use super::*;
    use crate::settings::{Asr, AsrMode};

    #[test]
    fn identity_tracks_semantic_settings_and_ignores_keys_and_concurrency() {
        let asr = Asr {
            base_url: " https://EXAMPLE.test/v1/ ".into(),
            model: "model".into(),
            ..Default::default()
        };
        let identity = AsrIdentity::new(&asr);
        assert_eq!(identity.schema_version, 3);
        assert_eq!(identity.endpoint_or_command, "https://example.test/v1");
        let changes: &[fn(&mut Asr)] = &[
            |a| a.mode = AsrMode::Chat,
            |a| a.base_url.push_str("other"),
            |a| a.model.push('2'),
            |a| a.language = "ko".into(),
            |a| a.max_speech = 30.0,
            |a| a.vad_threshold = 0.6,
            |a| a.min_silence = 0.5,
            |a| a.command = vec!["adapter".into()],
        ];
        for change in changes {
            let mut changed = asr.clone();
            change(&mut changed);
            assert_ne!(identity, AsrIdentity::new(&changed));
        }
        let mut changed = asr.clone();
        changed.api_key = "secret".into();
        changed.concurrency = 1;
        assert_eq!(identity, AsrIdentity::new(&changed));
        let mut managed = asr.clone();
        managed.command = vec!["adapter".into(), "--model".into(), "some model".into()];
        let managed_id = AsrIdentity::new(&managed);
        managed.base_url = "ignored".into();
        assert_eq!(managed_id, AsrIdentity::new(&managed));
        managed.command = vec!["adapter --model".into(), "some model".into()];
        assert_ne!(managed_id, AsrIdentity::new(&managed));
    }
}
