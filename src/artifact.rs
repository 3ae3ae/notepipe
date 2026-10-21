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

pub fn recover(root: &Path) -> Result<()> {
    std::fs::create_dir_all(root)?;
    let _lock = runtime::lock_file(&root.join(".notepipe.lock"))?;
    for entry in std::fs::read_dir(root)?.collect::<std::io::Result<Vec<_>>>()? {
        let name = entry.file_name();
        let Some(name) = name.to_str().and_then(|name| name.strip_prefix('.')) else {
            continue;
        };
        if let Some(name) = name.strip_suffix(".notepipe-old") {
            let destination = root.join(name);
            match destination.symlink_metadata() {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    std::fs::rename(entry.path(), &destination)
                }
                Err(error) => Err(error),
                Ok(_) => std::fs::remove_dir_all(entry.path()),
            }
            .with_context(|| format!("Could not recover note folder {}", destination.display()))?;
        } else if name.ends_with(".notepipe-new") {
            std::fs::remove_dir_all(entry.path())
                .context("Could not remove interrupted note staging")?;
        }
    }
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
    let destination = destination(&root, &document.meta)?;
    let name = destination.file_name().unwrap().to_string_lossy();
    let staging = root.join(format!(".{name}.notepipe-new"));
    let previous = root.join(format!(".{name}.notepipe-old"));
    std::fs::create_dir(&staging)?;
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
            std::fs::create_dir_all(staging.join("images"))?;
            std::io::copy(
                &mut std::fs::File::open(image)?,
                &mut std::fs::File::create(staging.join(&name))?,
            )?;
            images.insert(section.image.clone(), name.clone());
            name
        };
        section.image = name;
    }
    let markdown = render::render_markdown(&document.meta, &document.sections, &document.top)?;
    std::fs::write(staging.join("note.md"), markdown)?;
    std::fs::write(
        staging.join("note.json"),
        serde_json::to_vec_pretty(&document)?,
    )?;
    runtime::check_cancelled()?;
    if destination.exists() {
        std::fs::rename(&destination, &previous).context("Could not preserve the previous note")?;
    }
    std::fs::rename(staging, &destination).context("Could not publish note folder")?;
    if previous.exists()
        && let Err(error) = std::fs::remove_dir_all(&previous)
    {
        document.warnings.push(format!(
            "Note published; could not remove the previous note backup {}: {error}",
            previous.display()
        ));
    }
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
        recover(root)?;
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
        let previous = root.join(".Shared-title.notepipe-old");
        assert!(previous.exists());
        std::fs::set_permissions(&previous, std::fs::Permissions::from_mode(0o700)).unwrap();
        recover(&root).unwrap();
        assert!(!previous.exists());
        assert!(note.join("note.json").is_file());
    }
}
