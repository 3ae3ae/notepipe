//! Build a complete note folder before replacing the previous publication.
use crate::{render, runtime, source::MediaMeta, timeline::Section};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
    pub schema: u32,
    pub meta: MediaMeta,
    pub sections: Vec<Section>,
    pub top: Vec<crate::timeline::StageText>,
    pub warnings: Vec<String>,
}

pub fn safe_asset_path(root: &Path, relative: &str) -> Result<PathBuf> {
    let path = Path::new(relative);
    anyhow::ensure!(
        !relative.is_empty() && path.components().all(|c| matches!(c, Component::Normal(_))),
        "Invalid note asset path"
    );
    let full = root.join(path);
    let canonical = full.canonicalize()?;
    anyhow::ensure!(
        canonical.starts_with(root.canonicalize()?),
        "Asset escapes note directory"
    );
    Ok(full)
}

pub fn has_readable_body(sections: &[Section]) -> bool {
    sections
        .iter()
        .flat_map(|section| &section.speech)
        .any(|event| !event.text.trim().is_empty())
}

fn matches_source(note: &Path, source: &str) -> Result<bool> {
    if note.symlink_metadata()?.file_type().is_symlink() || !note.is_dir() {
        return Ok(false);
    }
    let path = note.join("note.md");
    if !path.is_file() {
        return Ok(false);
    }
    let text = std::fs::read_to_string(path)?;
    let mut lines = text.lines();
    if lines.next() != Some("---") {
        return Ok(false);
    }
    let mut found = None;
    // shortcut: ownership reads our JSON-quoted YAML scalar, add a YAML parser if other scalar styles must be supported.
    for line in lines {
        if line == "---" {
            return Ok(found.as_deref() == Some(source));
        }
        if let Some(value) = line.strip_prefix("source: ") {
            found = serde_json::from_str::<String>(value).ok();
        }
    }
    Ok(false)
}

const TRANSACTION_FILE: &str = ".notepipe-transaction.json";

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Phase {
    Staged,
    OldMoved,
    Published,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Transaction {
    name: String,
    staging: String,
    old: String,
    phase: Phase,
}
impl Transaction {
    fn validate(&self) -> Result<()> {
        for name in [&self.name, &self.staging, &self.old] {
            let mut components = Path::new(name).components();
            anyhow::ensure!(
                !name.contains(['/', '\\'])
                    && matches!(components.next(), Some(Component::Normal(_)))
                    && components.next().is_none(),
                "Invalid note transaction path"
            );
        }
        anyhow::ensure!(
            self.name != self.staging && self.name != self.old && self.staging != self.old,
            "Note transaction paths must be distinct"
        );
        Ok(())
    }

    fn write(&self, root: &Path) -> Result<()> {
        self.validate()?;
        crate::checkpoint::atomic_write(&root.join(TRANSACTION_FILE), &serde_json::to_vec(self)?)
    }
}

fn remove_dir_if_exists(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_dir_all(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

fn finish(root: &Path, previous: &Path, warnings: &mut Vec<String>) {
    if let Err(error) = remove_dir_if_exists(previous) {
        warnings.push(format!("Note published; could not remove the previous note backup {}: {error}. Remove it manually.", previous.display()));
    }
    if let Err(error) = std::fs::remove_file(root.join(TRANSACTION_FILE)) {
        warnings.push(format!(
            "Could not remove note transaction record {}: {error}",
            root.join(TRANSACTION_FILE).display()
        ));
    }
}

fn recover_locked(root: &Path) -> Result<Vec<String>> {
    let record = root.join(TRANSACTION_FILE);
    let bytes = match std::fs::read(&record) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        result => result?,
    };
    let transaction: Transaction =
        serde_json::from_slice(&bytes).context("Invalid note transaction record")?;
    transaction.validate()?;
    let destination = root.join(&transaction.name);
    let staging = root.join(&transaction.staging);
    let previous = root.join(&transaction.old);
    let mut warnings = Vec::new();
    match transaction.phase {
        Phase::Staged => {
            remove_dir_if_exists(&staging)?;
            std::fs::remove_file(record)?;
        }
        Phase::OldMoved => {
            match destination.symlink_metadata() {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    std::fs::rename(&previous, &destination).context("Could not restore the previous note")?;
                    remove_dir_if_exists(&staging)?;
                }
                Err(error) => return Err(error.into()),
                Ok(_) => warnings.push(format!("Note recovery conflict: {} exists; previous note {} and staging {} left untouched.", destination.display(), previous.display(), staging.display())),
            }
            std::fs::remove_file(record)?;
        }
        Phase::Published => finish(root, &previous, &mut warnings),
    }
    for warning in &warnings {
        tracing::warn!("{warning}");
    }
    Ok(warnings)
}

pub fn recover(root: &Path) -> Result<Vec<String>> {
    std::fs::create_dir_all(root)?;
    let _lock = runtime::lock_file(&root.join(".notepipe.lock"))?;
    recover_locked(root)
}

fn commit(root: &Path, transaction: &mut Transaction, warnings: &mut Vec<String>) -> Result<()> {
    let destination = root.join(&transaction.name);
    let staging = root.join(&transaction.staging);
    let previous = root.join(&transaction.old);
    let moved = destination.try_exists()?;
    if moved {
        // Record the intent first so interruption after the rename is always recoverable.
        transaction.phase = Phase::OldMoved;
        transaction.write(root)?;
        std::fs::rename(&destination, &previous).context("Could not preserve the previous note")?;
    }
    if let Err(error) = std::fs::rename(&staging, &destination) {
        if moved {
            std::fs::rename(&previous, &destination).with_context(|| format!("Could not publish note: {error}; could not restore the previous note retained at {}", previous.display()))?;
            remove_dir_if_exists(&staging)?;
            std::fs::remove_file(root.join(TRANSACTION_FILE))?;
        }
        return Err(error).context("Could not publish note folder");
    }
    transaction.phase = Phase::Published;
    transaction.write(root)?;
    finish(root, &previous, warnings);
    Ok(())
}

fn destination(root: &Path, meta: &MediaMeta) -> Result<PathBuf> {
    let slug = crate::config::sanitize_filename_with_fallback(&meta.title, "untitled");
    for number in 1.. {
        let name = if number == 1 {
            slug.clone()
        } else {
            format!("{slug}-{number}")
        };
        let path = root.join(name);
        match path.symlink_metadata() {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(path),
            Err(error) => return Err(error.into()),
            Ok(_) if matches_source(&path, &meta.source)? => return Ok(path),
            Ok(_) => {}
        }
    }
    unreachable!()
}

pub fn publish(
    root: &Path,
    frames_root: &Path,
    mut document: Document,
) -> Result<(PathBuf, Vec<String>)> {
    runtime::check_cancelled()?;
    anyhow::ensure!(
        has_readable_body(&document.sections),
        "No readable transcript"
    );
    std::fs::create_dir_all(root)?;
    let root = root.canonicalize()?;
    let _lock = runtime::lock_file(&root.join(".notepipe.lock"))?;
    document.warnings.extend(recover_locked(&root)?);
    anyhow::ensure!(
        !root.join(TRANSACTION_FILE).try_exists()?,
        "Unfinished note transaction remains in {}",
        root.display()
    );
    let destination = destination(&root, &document.meta)?;
    let name = destination.file_name().unwrap().to_string_lossy();
    let staging = tempfile::Builder::new()
        .prefix(&format!(".{name}.notepipe-new-"))
        .tempdir_in(&root)?;
    let previous = tempfile::Builder::new()
        .prefix(&format!(".{name}.notepipe-old-"))
        .tempdir_in(&root)?
        .keep();
    std::fs::remove_dir(&previous)?;
    let mut transaction = Transaction {
        name: name.into_owned(),
        staging: staging
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned(),
        old: previous.file_name().unwrap().to_string_lossy().into_owned(),
        phase: Phase::Staged,
    };
    let mut images: BTreeMap<String, String> = BTreeMap::new();
    for section in &mut document.sections {
        runtime::check_cancelled()?;
        if section.image.is_empty() {
            continue;
        }
        let number = images.len() + 1;
        let name = if let Some(name) = images.get(&section.image) {
            name.clone()
        } else {
            let image = safe_asset_path(frames_root, &section.image)?;
            let name = format!("images/{number:04}.jpg");
            std::fs::create_dir_all(staging.path().join("images"))?;
            std::io::copy(
                &mut std::fs::File::open(image)?,
                &mut std::fs::File::create(staging.path().join(&name))?,
            )?;
            images.insert(section.image.clone(), name.clone());
            name
        };
        section.image = name;
    }
    let markdown = render::render_markdown(&document.meta, &document.sections, &document.top)?;
    std::fs::write(staging.path().join("note.md"), markdown)?;
    std::fs::write(
        staging.path().join("note.json"),
        serde_json::to_vec_pretty(&document)?,
    )?;
    runtime::check_cancelled()?;
    transaction.write(&root)?;
    let _ = staging.keep();
    commit(&root, &mut transaction, &mut document.warnings)?;
    Ok((destination, document.warnings))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn document(source: &str) -> Document {
        Document {
            schema: 1,
            meta: MediaMeta {
                title: "Shared title".into(),
                source: source.into(),
                author: String::new(),
                duration: 1.,
                seek_url: None,
            },
            sections: vec![Section {
                t: 0.,
                end: 1.,
                image: String::new(),
                speech: vec![crate::timeline::TranscriptEvent {
                    start: 0.,
                    end: 1.,
                    text: "Original body".into(),
                    raw: None,
                    below: Vec::new(),
                }],
            }],
            top: Vec::new(),
            warnings: vec![],
        }
    }
    fn publish(root: &Path, frames: &Path, document: &Document) -> Result<(PathBuf, Vec<String>)> {
        super::publish(root, frames, document.clone())
    }
    #[test]
    fn publishing_replaces_only_its_source_and_preserves_old_notes_on_failure() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("notes");
        let mut first = document("https://example.test/one");
        std::fs::write(temp.path().join("frame.jpg"), "image").unwrap();
        first.sections[0].image = "frame.jpg".into();
        first.warnings.push("Known warning".into());
        let (one, _) = publish(&root, temp.path(), &first).unwrap();
        assert_eq!(one.file_name().unwrap(), "Shared-title");
        let published: Document =
            serde_json::from_slice(&std::fs::read(one.join("note.json")).unwrap()).unwrap();
        assert_eq!(published.sections[0].image, "images/0001.jpg");
        assert!(one.join(&published.sections[0].image).is_file());
        assert_eq!(published.warnings, first.warnings);
        let (two, _) = publish(&root, temp.path(), &document("https://example.test/two")).unwrap();
        assert_eq!(two.file_name().unwrap(), "Shared-title-2");
        first.top.push(crate::timeline::StageText {
            stage: "summary".into(),
            text: "Updated summary".into(),
        });
        assert_eq!(publish(&root, temp.path(), &first).unwrap().0, one);
        let previous = std::fs::read(one.join("note.md")).unwrap();
        let previous_json = std::fs::read(one.join("note.json")).unwrap();
        first.sections[0].image = "../missing.jpg".into();
        assert!(publish(&root, temp.path(), &first).is_err());
        assert_eq!(std::fs::read(one.join("note.md")).unwrap(), previous);
        assert_eq!(std::fs::read(one.join("note.json")).unwrap(), previous_json);
        assert!(
            std::fs::read_to_string(two.join("note.md"))
                .unwrap()
                .contains("https://example.test/two")
        );
    }
    #[test]
    fn incomplete_front_matter_does_not_claim_ownership() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("note.md");
        std::fs::write(&path, "---\nsource: \"same\"\nUser text").unwrap();
        assert!(!matches_source(temp.path(), "same").unwrap());
        std::fs::write(&path, "---\r\nsource: \"same\"\r\n---\r\nUser text").unwrap();
        assert!(matches_source(temp.path(), "same").unwrap());
    }

    #[test]
    fn output_lock_protects_recovery_and_publication() {
        let root = tempfile::tempdir().unwrap();
        let lock = runtime::lock_file(&root.path().join(".notepipe.lock")).unwrap();
        assert!(recover(root.path()).is_err());
        assert!(super::publish(root.path(), root.path(), document("one")).is_err());
        drop(lock);
        recover(root.path()).unwrap();
        publish(root.path(), root.path(), &document("one")).unwrap();
    }

    #[test]
    fn recovery_conflict_preserves_every_folder_and_discards_the_record() {
        let root = tempfile::tempdir().unwrap();
        let transaction = Transaction {
            name: "note".into(),
            staging: ".note.notepipe-new-recorded".into(),
            old: ".note.notepipe-old-recorded".into(),
            phase: Phase::OldMoved,
        };
        for name in [&transaction.name, &transaction.staging, &transaction.old] {
            let path = root.path().join(name);
            std::fs::create_dir(&path).unwrap();
            std::fs::write(path.join("content"), name).unwrap();
        }
        let lock = runtime::lock_file(&root.path().join(".notepipe.lock")).unwrap();
        transaction.write(root.path()).unwrap();
        drop(lock);
        let warnings = recover(root.path()).unwrap();
        assert_eq!(warnings.len(), 1);
        for name in [&transaction.name, &transaction.staging, &transaction.old] {
            let path = root.path().join(name);
            assert!(warnings[0].contains(&path.display().to_string()));
            assert_eq!(
                std::fs::read_to_string(path.join("content")).unwrap(),
                *name
            );
        }
        assert!(!root.path().join(TRANSACTION_FILE).exists());
        assert!(recover(root.path()).unwrap().is_empty());
    }

    #[test]
    fn failed_second_rename_restores_the_previous_note_immediately() {
        let root = tempfile::tempdir().unwrap();
        let (note, _) = publish(root.path(), root.path(), &document("one")).unwrap();
        let markdown = std::fs::read(note.join("note.md")).unwrap();
        let json = std::fs::read(note.join("note.json")).unwrap();
        let mut transaction = Transaction {
            name: note.file_name().unwrap().to_str().unwrap().into(),
            staging: ".note.notepipe-new-missing".into(),
            old: ".note.notepipe-old-recorded".into(),
            phase: Phase::Staged,
        };
        let _lock = runtime::lock_file(&root.path().join(".notepipe.lock")).unwrap();
        transaction.write(root.path()).unwrap();
        assert!(commit(root.path(), &mut transaction, &mut Vec::new()).is_err());
        assert_eq!(std::fs::read(note.join("note.md")).unwrap(), markdown);
        assert_eq!(std::fs::read(note.join("note.json")).unwrap(), json);
        assert!(!root.path().join(&transaction.old).exists());
        assert!(!root.path().join(TRANSACTION_FILE).exists());
    }

    #[test]
    fn invalid_transaction_records_leave_every_path_untouched() {
        let root = tempfile::tempdir().unwrap();
        for name in ["note", "staging", "old"] {
            let path = root.path().join(name);
            std::fs::create_dir(&path).unwrap();
            std::fs::write(path.join("content"), name).unwrap();
        }
        for (key, value) in [
            ("name", ""),
            ("name", "."),
            ("name", ".."),
            ("name", "/absolute"),
            ("name", "C:\\absolute"),
            ("staging", "../outside"),
            ("staging", "nested/path"),
            ("staging", "nested\\path"),
            ("staging", "note"),
            ("old", "staging"),
            ("phase", "unknown"),
            ("unknown", "field"),
        ] {
            let mut record = serde_json::json!({
                "name":"note", "staging":"staging", "old":"old", "phase":"staged"
            });
            record[key] = value.into();
            let bytes = serde_json::to_vec(&record).unwrap();
            let path = root.path().join(TRANSACTION_FILE);
            let lock = runtime::lock_file(&root.path().join(".notepipe.lock")).unwrap();
            crate::checkpoint::atomic_write(&path, &bytes).unwrap();
            drop(lock);
            assert!(recover(root.path()).is_err(), "{record}");
            assert_eq!(std::fs::read(path).unwrap(), bytes);
            for name in ["note", "staging", "old"] {
                assert_eq!(
                    std::fs::read_to_string(root.path().join(name).join("content")).unwrap(),
                    name
                );
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn backup_cleanup_failure_keeps_the_new_note_and_reports_a_warning() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("notes");
        let mut doc = document("one");
        let (note, _) = publish(&root, temp.path(), &doc).unwrap();
        std::fs::set_permissions(&note, std::fs::Permissions::from_mode(0o500)).unwrap();
        doc.sections[0].speech[0].text = "Updated body".into();
        let (note, warnings) = publish(&root, temp.path(), &doc).unwrap();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("could not remove the previous note backup"));
        assert!(
            std::fs::read_to_string(note.join("note.md"))
                .unwrap()
                .contains("Updated body")
        );
        let saved: Document =
            serde_json::from_slice(&std::fs::read(note.join("note.json")).unwrap()).unwrap();
        assert_eq!(saved.warnings, doc.warnings);
        assert_eq!(saved.sections[0].speech[0].text, "Updated body");
        let previous = std::fs::read_dir(&root)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(".Shared-title.notepipe-old-")
            })
            .unwrap();
        assert!(warnings[0].contains(&previous.display().to_string()));
        assert!(!root.join(TRANSACTION_FILE).exists());
        std::fs::set_permissions(&previous, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(recover(&root).unwrap().is_empty());
        assert!(
            previous.exists(),
            "unrecorded leftovers must never be retried"
        );
        let (_, warnings) = publish(&root, temp.path(), &doc).unwrap();
        assert!(warnings.is_empty());
        assert!(previous.exists(), "a later transaction must use new paths");
        assert!(!root.join(TRANSACTION_FILE).exists());
        assert!(note.join("note.json").is_file());
    }
}
