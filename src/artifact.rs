//! Build a complete note folder before replacing the previous publication.
use crate::{runtime, source::MediaMeta, timeline::Section};
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
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

pub struct Publication {
    pub stage: String,
    pub output: String,
    pub name: String,
    pub images: Vec<String>,
}

impl Document {
    pub fn read(path: &Path, work: &Path) -> Result<Self> {
        let document: Self = serde_json::from_slice(&std::fs::read(path)?)?;
        anyhow::ensure!(document.schema == 3, "Invalid document schema; expected 3");
        anyhow::ensure!(
            document.meta.duration.is_finite() && document.meta.duration >= 0.,
            "Invalid document duration"
        );
        for section in &document.sections {
            anyhow::ensure!(
                section.t.is_finite()
                    && section.t >= 0.
                    && section.end.is_finite()
                    && section.end >= section.t,
                "Invalid document section times"
            );
            anyhow::ensure!(
                section.speech.iter().all(|e| e.start.is_finite()
                    && e.end.is_finite()
                    && e.start >= 0.
                    && e.end >= e.start),
                "Invalid subtitle timestamps"
            );
            if !section.image.is_empty() {
                anyhow::ensure!(
                    safe_asset_path(work, &section.image)?.is_file(),
                    "Invalid note asset path"
                );
            }
        }
        Ok(document)
    }
    pub fn images(&self) -> Vec<String> {
        self.sections
            .iter()
            .map(|s| s.image.clone())
            .filter(|s| !s.is_empty())
            .collect()
    }
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
    Ok(canonical)
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
    let path = note.join(".notepipe.json");
    if !path.is_file() {
        return Ok(false);
    }
    if path.symlink_metadata()?.is_symlink() {
        return Ok(false);
    }
    let marker = serde_json::from_slice::<serde_json::Value>(&std::fs::read(path)?).ok();
    Ok(marker.as_ref().and_then(|m| m["source"].as_str()) == Some(source))
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
        let names = [&self.name, &self.staging, &self.old].map(|name| name.to_lowercase());
        anyhow::ensure!(
            names[0] != names[1] && names[0] != names[2] && names[1] != names[2],
            "Note transaction paths must be distinct"
        );
        anyhow::ensure!(
            self.staging
                .starts_with(&format!(".{}.notepipe-new-", self.name))
                && self
                    .old
                    .starts_with(&format!(".{}.notepipe-old-", self.name)),
            "Note transaction paths do not match the note name"
        );
        Ok(())
    }

    fn write(&self, root: &Path) -> Result<()> {
        self.validate()?;
        crate::checkpoint::atomic_write(&root.join(TRANSACTION_FILE), &serde_json::to_vec(self)?)
    }
}

fn warn(warnings: &mut Vec<String>, message: String) {
    tracing::warn!("{message}");
    warnings.push(message);
}

fn remove_dir_if_exists(path: &Path, kind: &str, warnings: &mut Vec<String>) {
    if let Err(error) = std::fs::remove_dir_all(path)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        warn(
            warnings,
            format!(
                "Could not remove {kind} {}: {error}. Remove it manually.",
                path.display()
            ),
        );
    }
}

fn discard_record(root: &Path, warnings: &mut Vec<String>) {
    if let Err(error) = std::fs::remove_file(root.join(TRANSACTION_FILE))
        && error.kind() != std::io::ErrorKind::NotFound
    {
        warn(
            warnings,
            format!(
                "Could not remove note transaction record {}: {error}",
                root.join(TRANSACTION_FILE).display()
            ),
        );
    }
}

fn restore(previous: &Path, destination: &Path) -> Result<()> {
    let metadata = previous.symlink_metadata().with_context(|| {
        format!(
            "Could not inspect previous note backup {}",
            previous.display()
        )
    })?;
    anyhow::ensure!(
        metadata.is_dir(),
        "Previous note backup {} must be a real directory",
        previous.display()
    );
    std::fs::rename(previous, destination).context("Could not restore the previous note")
}

fn recover_locked(root: &Path) -> Result<Vec<String>> {
    let record = root.join(TRANSACTION_FILE);
    let metadata = match record.symlink_metadata() {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        result => result?,
    };
    anyhow::ensure!(
        metadata.is_file(),
        "Note transaction record must be a regular file: {}",
        record.display()
    );
    let transaction: Transaction = serde_json::from_slice(&std::fs::read(&record)?)
        .context("Invalid note transaction record")?;
    transaction.validate()?;
    let destination = root.join(&transaction.name);
    let staging = root.join(&transaction.staging);
    let previous = root.join(&transaction.old);
    let mut warnings = Vec::new();
    match transaction.phase {
        Phase::Staged => {
            remove_dir_if_exists(&staging, "note staging folder", &mut warnings);
        }
        Phase::OldMoved => match destination.symlink_metadata() {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                restore(&previous, &destination)?;
                remove_dir_if_exists(&staging, "note staging folder", &mut warnings);
            }
            Err(error) => return Err(error.into()),
            Ok(_) => warn(
                &mut warnings,
                format!(
                    "Note recovery conflict: {} exists; previous note {} and staging {} left untouched.",
                    destination.display(),
                    previous.display(),
                    staging.display()
                ),
            ),
        },
        Phase::Published => {
            remove_dir_if_exists(&previous, "the previous note backup", &mut warnings)
        }
    }
    discard_record(root, &mut warnings);
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
        if let Err(error) = transaction.write(root).and_then(|()| {
            std::fs::rename(&destination, &previous).context("Could not preserve the previous note")
        }) {
            remove_dir_if_exists(&staging, "note staging folder", warnings);
            discard_record(root, warnings);
            return Err(error);
        }
    }
    if let Err(error) = std::fs::rename(&staging, &destination) {
        if moved {
            restore(&previous, &destination).with_context(|| format!("Could not publish note: {error}; could not restore the previous note retained at {}", previous.display()))?;
        }
        remove_dir_if_exists(&staging, "note staging folder", warnings);
        discard_record(root, warnings);
        return Err(error).context("Could not publish note folder");
    }
    transaction.phase = Phase::Published;
    if let Err(error) = transaction.write(root) {
        warn(
            warnings,
            format!(
                "Note published; could not update note transaction record {}: {error:#}",
                root.join(TRANSACTION_FILE).display()
            ),
        );
    }
    remove_dir_if_exists(&previous, "the previous note backup", warnings);
    discard_record(root, warnings);
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
    work: &Path,
    meta: &MediaMeta,
    outputs: &[Publication],
    warnings: &mut Vec<String>,
) -> Result<(Option<PathBuf>, BTreeMap<String, String>)> {
    runtime::check_cancelled()?;
    let mut files = BTreeMap::new();
    let folder_outputs = outputs.iter().filter(|o| o.name != "-").collect::<Vec<_>>();
    let destination = if folder_outputs.is_empty() {
        None
    } else {
        std::fs::create_dir_all(root)?;
        let root = root.canonicalize()?;
        let _lock = runtime::lock_file(&root.join(".notepipe.lock"))?;
        warnings.extend(recover_locked(&root)?);
        anyhow::ensure!(
            !root.join(TRANSACTION_FILE).try_exists()?,
            "Unfinished note transaction remains in {}",
            root.display()
        );
        let destination = destination(&root, meta)?;
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
        for output in &folder_outputs {
            runtime::check_cancelled()?;
            copy_path(
                &work.join(&output.output),
                &staging.path().join(&output.name),
            )?;
            files.insert(
                output.stage.clone(),
                destination.join(&output.name).display().to_string(),
            );
        }
        let images: std::collections::BTreeSet<_> =
            outputs.iter().flat_map(|o| &o.images).collect();
        for image in images {
            let source = safe_asset_path(work, image)?;
            let target = staging.path().join(image);
            if target.exists() {
                anyhow::ensure!(
                    target.is_file()
                        && runtime::file_digest(&source)? == runtime::file_digest(&target)?,
                    "Published output conflicts with image {image}"
                );
            } else {
                copy_path(&source, &target)?;
            }
        }
        std::fs::write(
            staging.path().join(".notepipe.json"),
            serde_json::to_vec(&serde_json::json!({"source":meta.source}))?,
        )?;
        runtime::check_cancelled()?;
        transaction.write(&root)?;
        let _ = staging.keep();
        commit(&root, &mut transaction, warnings)?;
        Some(destination)
    };
    for output in outputs.iter().filter(|o| o.name == "-") {
        files.insert(output.stage.clone(), "-".into());
    }
    Ok((destination, files))
}

pub fn copy_path(source: &Path, target: &Path) -> Result<()> {
    anyhow::ensure!(
        !source.symlink_metadata()?.is_symlink(),
        "Invalid note asset path"
    );
    if source.is_dir() {
        std::fs::create_dir_all(target)?;
        for entry in std::fs::read_dir(source)? {
            let entry = entry?;
            copy_path(&entry.path(), &target.join(entry.file_name()))?;
        }
    } else {
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::io::copy(
            &mut std::fs::File::open(source)?,
            &mut std::fs::File::create(target)?,
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn document(source: &str) -> Document {
        Document {
            schema: 3,
            meta: MediaMeta {
                extra: Default::default(),
                title: "Shared title".into(),
                source: source.into(),
                author: String::new(),
                duration: 1.,
                seek_url: None,
            },
            sections: vec![Section {
                extra: Default::default(),
                t: 0.,
                end: 1.,
                image: String::new(),
                speech: vec![crate::timeline::TranscriptEvent {
                    extra: Default::default(),
                    start: 0.,
                    end: 1.,
                    text: "Original body".into(),
                    raw: None,
                }],
            }],
            extra: Default::default(),
        }
    }
    fn publish(root: &Path, work: &Path, document: &Document) -> Result<(PathBuf, Vec<String>)> {
        std::fs::write(work.join("test.json"), serde_json::to_vec(document)?)?;
        std::fs::write(
            work.join("test.md"),
            crate::render::markdown(&[crate::render::document(&document.sections)]),
        )?;
        let mut warnings = Vec::new();
        let outputs = vec![
            Publication {
                stage: "document".into(),
                output: "test.json".into(),
                name: "note.json".into(),
                images: document.images(),
            },
            Publication {
                stage: "markdown".into(),
                output: "test.md".into(),
                name: "note.md".into(),
                images: document.images(),
            },
        ];
        let (note, _) = super::publish(root, work, &document.meta, &outputs, &mut warnings)?;
        Ok((note.unwrap(), warnings))
    }

    fn transaction(name: &str, phase: Phase) -> Transaction {
        Transaction {
            name: name.into(),
            staging: format!(".{name}.notepipe-new-recorded"),
            old: format!(".{name}.notepipe-old-recorded"),
            phase,
        }
    }
    #[test]
    fn publishing_replaces_only_its_source_and_preserves_old_notes_on_failure() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("notes");
        let mut first = document("https://example.test/one");
        std::fs::write(temp.path().join("frame.jpg"), "image").unwrap();
        first.sections[0].image = "frame.jpg".into();
        let (one, _) = publish(&root, temp.path(), &first).unwrap();
        assert_eq!(one.file_name().unwrap(), "Shared-title");
        let published: Document =
            serde_json::from_slice(&std::fs::read(one.join("note.json")).unwrap()).unwrap();
        assert_eq!(published.sections[0].image, "frame.jpg");
        assert!(one.join(&published.sections[0].image).is_file());
        let (two, _) = publish(&root, temp.path(), &document("https://example.test/two")).unwrap();
        assert_eq!(two.file_name().unwrap(), "Shared-title-2");
        assert_eq!(publish(&root, temp.path(), &first).unwrap().0, one);
        let previous = std::fs::read(one.join("note.md")).unwrap();
        let previous_json = std::fs::read(one.join("note.json")).unwrap();
        first.sections[0].image = "../missing.jpg".into();
        assert!(publish(&root, temp.path(), &first).is_err());
        assert_eq!(std::fs::read(one.join("note.md")).unwrap(), previous);
        assert_eq!(std::fs::read(one.join("note.json")).unwrap(), previous_json);
        assert!(matches_source(&two, "https://example.test/two").unwrap());
    }
    #[test]
    fn folders_without_a_marker_are_not_claimed() {
        let temp = tempfile::tempdir().unwrap();
        let note = temp.path().join("Shared-title");
        std::fs::create_dir(&note).unwrap();
        std::fs::write(note.join("note.md"), "---\nsource: \"one\"\n---\nUser text").unwrap();
        assert_eq!(
            publish(temp.path(), temp.path(), &document("one"))
                .unwrap()
                .0
                .file_name()
                .unwrap(),
            "Shared-title-2"
        );
        assert!(note.join("note.md").is_file());
    }

    #[test]
    fn output_lock_protects_recovery_and_publication() {
        let root = tempfile::tempdir().unwrap();
        let lock = runtime::lock_file(&root.path().join(".notepipe.lock")).unwrap();
        assert!(recover(root.path()).is_err());
        assert!(publish(root.path(), root.path(), &document("one")).is_err());
        drop(lock);
        recover(root.path()).unwrap();
        publish(root.path(), root.path(), &document("one")).unwrap();
    }

    #[test]
    fn recovery_conflict_preserves_every_folder_and_discards_the_record() {
        let root = tempfile::tempdir().unwrap();
        let transaction = transaction("note", Phase::OldMoved);
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
        let mut transaction =
            transaction(note.file_name().unwrap().to_str().unwrap(), Phase::Staged);
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
        let names = [
            "note",
            ".note.notepipe-new-recorded",
            ".note.notepipe-old-recorded",
            "unrelated",
        ];
        for name in names {
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
            ("staging", "NOTE"),
            ("old", ".note.notepipe-new-recorded"),
            ("old", ".NOTE.NOTEPIPE-NEW-RECORDED"),
            ("staging", "unrelated"),
            ("old", "unrelated"),
            ("staging", ".other.notepipe-new-recorded"),
            ("old", ".other.notepipe-old-recorded"),
            ("phase", "unknown"),
            ("unknown", "field"),
        ] {
            let mut record = serde_json::to_value(transaction("note", Phase::Staged)).unwrap();
            record[key] = value.into();
            let bytes = serde_json::to_vec(&record).unwrap();
            let path = root.path().join(TRANSACTION_FILE);
            let lock = runtime::lock_file(&root.path().join(".notepipe.lock")).unwrap();
            crate::checkpoint::atomic_write(&path, &bytes).unwrap();
            drop(lock);
            let error = recover(root.path()).unwrap_err();
            if matches!(value, "NOTE" | ".NOTE.NOTEPIPE-NEW-RECORDED") {
                assert!(error.to_string().contains("must be distinct"));
            }
            assert_eq!(std::fs::read(path).unwrap(), bytes);
            for name in names {
                assert_eq!(
                    std::fs::read_to_string(root.path().join(name).join("content")).unwrap(),
                    name
                );
            }
        }
    }

    #[test]
    fn recovery_requires_a_directory_backup_and_retains_an_unresolved_record() {
        let root = tempfile::tempdir().unwrap();
        let transaction = transaction("note", Phase::OldMoved);
        let previous = root.path().join(&transaction.old);
        std::fs::write(&previous, "not a directory").unwrap();
        let staging = root.path().join(&transaction.staging);
        std::fs::create_dir(&staging).unwrap();
        std::fs::write(staging.join("content"), "Staged note").unwrap();
        let lock = runtime::lock_file(&root.path().join(".notepipe.lock")).unwrap();
        transaction.write(root.path()).unwrap();
        drop(lock);
        assert!(
            recover(root.path())
                .unwrap_err()
                .to_string()
                .contains("must be a real directory")
        );
        assert!(!root.path().join(&transaction.name).exists());
        assert_eq!(
            std::fs::read_to_string(previous).unwrap(),
            "not a directory"
        );
        assert_eq!(
            std::fs::read_to_string(staging.join("content")).unwrap(),
            "Staged note"
        );
        assert!(root.path().join(TRANSACTION_FILE).is_file());
    }

    #[cfg(unix)]
    #[test]
    fn recovery_rejects_symlink_records_and_backups_without_touching_their_targets() {
        use std::os::unix::fs::symlink;
        for link_record in [true, false] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("notes");
            std::fs::create_dir(&root).unwrap();
            let transaction = transaction("note", Phase::OldMoved);
            let previous = root.join(&transaction.old);
            let staging = root.join(&transaction.staging);
            std::fs::create_dir(&staging).unwrap();
            std::fs::write(staging.join("content"), "Staged note").unwrap();
            let record = root.join(TRANSACTION_FILE);
            let lock = runtime::lock_file(&root.join(".notepipe.lock")).unwrap();
            transaction.write(&root).unwrap();
            let target = temp.path().join("target");
            if link_record {
                std::fs::rename(&record, &target).unwrap();
                symlink(&target, &record).unwrap();
                std::fs::create_dir(&previous).unwrap();
                std::fs::write(previous.join("content"), "Previous note").unwrap();
            } else {
                std::fs::create_dir(&target).unwrap();
                std::fs::write(target.join("content"), "Previous note").unwrap();
                symlink(&target, &previous).unwrap();
            }
            drop(lock);
            let error = recover(&root).unwrap_err().to_string();
            assert!(
                error.contains(if link_record {
                    "must be a regular file"
                } else {
                    "must be a real directory"
                }),
                "{error}"
            );
            assert_eq!(
                std::fs::read(&record).unwrap(),
                serde_json::to_vec(&transaction).unwrap()
            );
            assert_eq!(
                std::fs::read_to_string(previous.join("content")).unwrap(),
                "Previous note"
            );
            assert_eq!(
                std::fs::read_to_string(staging.join("content")).unwrap(),
                "Staged note"
            );
            assert!(!root.join(&transaction.name).exists());
            assert!(
                if link_record { record } else { previous }
                    .symlink_metadata()
                    .unwrap()
                    .is_symlink()
            );
        }
    }

    #[test]
    fn completed_publication_survives_record_write_and_removal_errors() {
        let root = tempfile::tempdir().unwrap();
        let (note, _) = publish(root.path(), root.path(), &document("one")).unwrap();
        let markdown = std::fs::read(note.join("note.md")).unwrap();
        let json = std::fs::read(note.join("note.json")).unwrap();
        let mut transaction =
            transaction(note.file_name().unwrap().to_str().unwrap(), Phase::Staged);
        std::fs::rename(&note, root.path().join(&transaction.staging)).unwrap();
        let lock = runtime::lock_file(&root.path().join(".notepipe.lock")).unwrap();
        let record = root.path().join(TRANSACTION_FILE);
        std::fs::create_dir(&record).unwrap();
        std::fs::write(record.join("content"), "Record path conflict").unwrap();
        let mut warnings = Vec::new();
        commit(root.path(), &mut transaction, &mut warnings).unwrap();
        assert_eq!(warnings.len(), 2);
        assert!(warnings[0].contains("could not update note transaction record"));
        assert!(warnings[1].contains("Could not remove note transaction record"));
        assert_eq!(std::fs::read(note.join("note.md")).unwrap(), markdown);
        assert_eq!(std::fs::read(note.join("note.json")).unwrap(), json);
        assert_eq!(
            std::fs::read_to_string(record.join("content")).unwrap(),
            "Record path conflict"
        );
        drop(lock);
        assert!(
            recover(root.path())
                .unwrap_err()
                .to_string()
                .contains("must be a regular file")
        );
    }

    #[cfg(unix)]
    #[test]
    fn staging_cleanup_failure_does_not_block_recovery_or_a_later_publication() {
        use std::os::unix::fs::PermissionsExt;
        for phase in [Phase::Staged, Phase::OldMoved] {
            let root = tempfile::tempdir().unwrap();
            let doc = document("one");
            let (note, _) = publish(root.path(), root.path(), &doc).unwrap();
            let transaction = transaction(note.file_name().unwrap().to_str().unwrap(), phase);
            let staging = root.path().join(&transaction.staging);
            std::fs::create_dir(&staging).unwrap();
            std::fs::write(staging.join("content"), "Staged note").unwrap();
            std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o500)).unwrap();
            if matches!(&transaction.phase, Phase::OldMoved) {
                std::fs::rename(&note, root.path().join(&transaction.old)).unwrap();
            }
            let lock = runtime::lock_file(&root.path().join(".notepipe.lock")).unwrap();
            transaction.write(root.path()).unwrap();
            drop(lock);
            let warnings = recover(root.path()).unwrap();
            std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o700)).unwrap();
            assert_eq!(warnings.len(), 1);
            assert!(warnings[0].contains("Could not remove note staging folder"));
            assert!(warnings[0].contains(&staging.display().to_string()));
            assert!(!root.path().join(TRANSACTION_FILE).exists());
            assert!(note.join("note.md").is_file() && note.join("note.json").is_file());
            let (published, warnings) = publish(root.path(), root.path(), &doc).unwrap();
            assert_eq!(published, note);
            assert!(warnings.is_empty());
            assert_eq!(
                std::fs::read_to_string(staging.join("content")).unwrap(),
                "Staged note"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn failed_publication_keeps_its_original_error_when_staging_cleanup_fails() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let (note, _) = publish(root.path(), root.path(), &document("one")).unwrap();
        let markdown = std::fs::read(note.join("note.md")).unwrap();
        let json = std::fs::read(note.join("note.json")).unwrap();
        let mut transaction =
            transaction(note.file_name().unwrap().to_str().unwrap(), Phase::Staged);
        let previous = root.path().join(&transaction.old);
        std::fs::create_dir(&previous).unwrap();
        std::fs::write(previous.join("content"), "Conflicting folder").unwrap();
        let staging = root.path().join(&transaction.staging);
        std::fs::create_dir(&staging).unwrap();
        std::fs::write(staging.join("content"), "Staged note").unwrap();
        std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o500)).unwrap();
        let _lock = runtime::lock_file(&root.path().join(".notepipe.lock")).unwrap();
        transaction.write(root.path()).unwrap();
        let mut warnings = Vec::new();
        let error = commit(root.path(), &mut transaction, &mut warnings).unwrap_err();
        std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(
            error
                .to_string()
                .contains("Could not preserve the previous note")
        );
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("Could not remove note staging folder"));
        assert!(!root.path().join(TRANSACTION_FILE).exists());
        assert_eq!(std::fs::read(note.join("note.md")).unwrap(), markdown);
        assert_eq!(std::fs::read(note.join("note.json")).unwrap(), json);
        assert_eq!(
            std::fs::read_to_string(previous.join("content")).unwrap(),
            "Conflicting folder"
        );
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
        assert!(warnings[0].contains("Could not remove the previous note backup"));
        assert!(
            std::fs::read_to_string(note.join("note.md"))
                .unwrap()
                .contains("Updated body")
        );
        let saved: Document =
            serde_json::from_slice(&std::fs::read(note.join("note.json")).unwrap()).unwrap();
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
