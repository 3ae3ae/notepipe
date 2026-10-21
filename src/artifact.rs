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

fn destination(root: &Path, meta: &MediaMeta) -> Result<PathBuf> {
    let slug = crate::config::sanitize_filename_with_fallback(&meta.title, "untitled");
    for number in 1.. {
        let name = if number == 1 {
            slug.clone()
        } else {
            format!("{slug}-{number}")
        };
        let path = root.join(name);
        let previous = previous_path(&path);
        let backup = match previous.symlink_metadata() {
            Ok(_) => {
                if !matches_source(&previous, &meta.source)? {
                    continue;
                }
                true
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => return Err(error.into()),
        };
        match path.symlink_metadata() {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if backup {
                    std::fs::rename(previous, &path)
                        .context("Could not recover the previous note")?;
                }
                return Ok(path);
            }
            Err(error) => return Err(error.into()),
            Ok(_) if matches_source(&path, &meta.source)? => return Ok(path),
            Ok(_) => {}
        }
    }
    unreachable!()
}

fn previous_path(destination: &Path) -> PathBuf {
    destination.with_file_name(format!(
        ".{}.notepipe-previous",
        destination.file_name().unwrap().to_string_lossy()
    ))
}

fn swap(
    staging: &Path,
    destination: &Path,
    source: &str,
    warnings: &mut Vec<String>,
) -> Result<()> {
    let previous = previous_path(destination);
    if previous.try_exists()? {
        anyhow::ensure!(
            matches_source(&previous, source)?,
            "Previous note backup belongs to another source"
        );
        std::fs::remove_dir_all(&previous).context("Could not remove the previous note backup")?;
    }
    if destination.exists() {
        anyhow::ensure!(
            matches_source(destination, source)?,
            "Note folder belongs to another source"
        );
        std::fs::rename(destination, &previous).context("Could not preserve the previous note")?;
    }
    if let Err(error) = std::fs::rename(staging, destination) {
        if previous.exists() {
            std::fs::rename(&previous, destination).with_context(|| {
                format!(
                    "Could not publish note: {error}; previous note retained at {}",
                    previous.display()
                )
            })?;
        }
        return Err(error).context("Could not publish note folder");
    }
    if previous.exists()
        && let Err(error) = std::fs::remove_dir_all(&previous)
    {
        warnings.push(format!(
            "Note published; could not remove the previous note backup {}: {error}",
            previous.display()
        ));
    }
    Ok(())
}

pub struct Publication {
    destination: PathBuf,
    _lock: runtime::FileLock,
}
impl Publication {
    pub fn prepare(root: &Path, meta: &MediaMeta) -> Result<Self> {
        std::fs::create_dir_all(root)?;
        let root = root.canonicalize()?;
        let lock = runtime::lock_file(&root.join(".notepipe.lock"))?;
        Ok(Self {
            destination: destination(&root, meta)?,
            _lock: lock,
        })
    }

    pub fn publish(
        self,
        frames_root: &Path,
        mut document: Document,
    ) -> Result<(PathBuf, Vec<String>)> {
        runtime::check_cancelled()?;
        anyhow::ensure!(
            has_readable_body(&document.sections),
            "No readable transcript"
        );
        let staging = tempfile::Builder::new()
            .prefix(".notepipe-new-")
            .tempdir_in(self.destination.parent().unwrap())?;
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
        runtime::check_cancelled()?;
        swap(
            staging.path(),
            &self.destination,
            &document.meta.source,
            &mut document.warnings,
        )?;
        std::fs::write(
            self.destination.join("note.json"),
            serde_json::to_vec_pretty(&document)?,
        )?;
        Ok((self.destination, document.warnings))
    }
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
        Publication::prepare(root, &document.meta)?.publish(frames, document.clone())
    }
    #[test]
    fn publishing_replaces_only_its_source_and_preserves_old_notes_on_failure() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("notes");
        let mut first = document("https://example.test/one");
        let (one, _) = publish(&root, temp.path(), &first).unwrap();
        assert_eq!(one.file_name().unwrap(), "Shared-title");
        let (two, _) = publish(&root, temp.path(), &document("https://example.test/two")).unwrap();
        assert_eq!(two.file_name().unwrap(), "Shared-title-2");
        first.top.push(crate::timeline::StageText {
            stage: "summary".into(),
            text: "Updated summary".into(),
        });
        assert_eq!(publish(&root, temp.path(), &first).unwrap().0, one);
        let previous = std::fs::read(one.join("note.md")).unwrap();
        first.sections[0].image = "../missing.jpg".into();
        assert!(publish(&root, temp.path(), &first).is_err());
        assert_eq!(std::fs::read(one.join("note.md")).unwrap(), previous);
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
    fn failed_swap_restores_the_previous_directory() {
        let temp = tempfile::tempdir().unwrap();
        let note = temp.path().join("note");
        std::fs::create_dir(&note).unwrap();
        let previous = "---\nsource: \"same\"\n---\nprevious";
        std::fs::write(note.join("note.md"), previous).unwrap();
        let staging = note.join("staging");
        std::fs::create_dir(&staging).unwrap();
        assert!(swap(&staging, &note, "same", &mut Vec::new()).is_err());
        assert_eq!(
            std::fs::read_to_string(note.join("note.md")).unwrap(),
            previous
        );
    }

    #[test]
    fn preparation_recovers_only_owned_backups_and_holds_the_output_lock() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("notes");
        let first = document("one");
        let mut second = document("two");
        let (one, _) = publish(&root, temp.path(), &first).unwrap();
        let (two, _) = publish(&root, temp.path(), &second).unwrap();
        let original = std::fs::read(two.join("note.md")).unwrap();
        for note in [&one, &two] {
            std::fs::rename(note, previous_path(note)).unwrap();
        }
        let publication = Publication::prepare(&root, &second.meta).unwrap();
        assert!(!one.exists());
        assert!(previous_path(&one).exists());
        assert_eq!(std::fs::read(two.join("note.md")).unwrap(), original);
        assert!(Publication::prepare(&root, &first.meta).is_err());
        assert!(runtime::lock_file(&root.join(".notepipe.lock")).is_err());
        second.sections[0].image = "missing.jpg".into();
        assert!(publication.publish(temp.path(), second.clone()).is_err());
        assert_eq!(std::fs::read(two.join("note.md")).unwrap(), original);
        // A crash after the second rename leaves both the new note and its owned backup.
        std::fs::create_dir(previous_path(&two)).unwrap();
        std::fs::write(previous_path(&two).join("note.md"), &original).unwrap();
        second.sections[0].image.clear();
        assert_eq!(publish(&root, temp.path(), &second).unwrap().0, two);
        assert!(!previous_path(&two).exists());
        assert!(previous_path(&one).exists());
    }

    #[test]
    fn foreign_backups_reserve_names_without_being_restored_or_deleted() {
        for occupied in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("notes");
            std::fs::create_dir(&root).unwrap();
            let doc = document("one");
            let note = root.join("Shared-title");
            if occupied {
                publish(&root, temp.path(), &doc).unwrap();
            }
            let previous = previous_path(&note);
            std::fs::create_dir(&previous).unwrap();
            let foreign = render::render_markdown(&document("other").meta, &[], &[]).unwrap();
            std::fs::write(previous.join("note.md"), &foreign).unwrap();
            let (published, _) = publish(&root, temp.path(), &doc).unwrap();
            assert_eq!(published.file_name().unwrap(), "Shared-title-2");
            assert_eq!(note.exists(), occupied);
            assert_eq!(
                std::fs::read_to_string(previous.join("note.md")).unwrap(),
                foreign
            );
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
        assert_eq!(saved.warnings, warnings);
        let previous = previous_path(&note);
        assert!(previous.exists());
        std::fs::set_permissions(previous, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
}
