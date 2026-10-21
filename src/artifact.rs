//! Build a complete note folder before replacing the previous publication.
use crate::{
    checkpoint::atomic_write_shared, render, runtime, source::MediaMeta, timeline::Section,
};
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
        match path.symlink_metadata() {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(path),
            Err(error) => return Err(error.into()),
            Ok(_) if matches_source(&path, &meta.source)? => return Ok(path),
            Ok(_) => {}
        }
    }
    unreachable!()
}

const PREVIOUS_SUFFIX: &str = ".notepipe-previous";

fn previous_path(destination: &Path) -> PathBuf {
    destination.with_file_name(format!(
        ".{}{PREVIOUS_SUFFIX}",
        destination.file_name().unwrap().to_string_lossy()
    ))
}

fn recover(root: &Path) -> Result<()> {
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name
            .to_str()
            .and_then(|name| name.strip_prefix('.'))
            .and_then(|name| name.strip_suffix(PREVIOUS_SUFFIX))
        else {
            continue;
        };
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let destination = root.join(name);
        match destination.symlink_metadata() {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::rename(entry.path(), destination)
                    .context("Could not recover the previous note")?;
            }
            Err(error) => return Err(error.into()),
            Ok(_) => {}
        }
    }
    Ok(())
}

#[cfg(unix)]
fn restrict_permissions(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if path.is_dir() {
        for entry in std::fs::read_dir(path)? {
            restrict_permissions(&entry?.path(), mode)?;
        }
    }
    let current = path.metadata()?.permissions().mode();
    std::fs::set_permissions(
        path,
        std::fs::Permissions::from_mode(current & mode & 0o777),
    )?;
    Ok(())
}

fn remove_previous(path: &Path) -> Result<()> {
    #[cfg(unix)]
    if path.symlink_metadata()?.is_dir() {
        use std::os::unix::fs::PermissionsExt;
        let mode = path.metadata()?.permissions().mode();
        // Only the obsolete backup needs owner access for deletion.
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode | 0o700))?;
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                remove_previous(&entry.path())?;
            }
        }
    }
    std::fs::remove_dir_all(path)?;
    Ok(())
}

fn swap(staging: &Path, destination: &Path) -> Result<()> {
    if !destination.exists() {
        return std::fs::rename(staging, destination).context("Could not publish note folder");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        restrict_permissions(staging, destination.metadata()?.permissions().mode())?;
    }
    let previous = previous_path(destination);
    if previous.try_exists()? {
        remove_previous(&previous).context("Could not remove the previous note backup")?;
    }
    std::fs::rename(destination, &previous).context("Could not preserve the previous note")?;
    if let Err(error) = std::fs::rename(staging, destination) {
        if let Err(restore) = std::fs::rename(&previous, destination) {
            anyhow::bail!(
                "Could not publish note: {error}; could not restore it: {restore}; previous note retained at {}",
                previous.display()
            );
        }
        return Err(error).context("Could not publish note; previous note restored");
    }
    remove_previous(&previous)
        .context("Note published; could not remove the previous note backup")?;
    Ok(())
}

pub fn publish(
    root: &Path,
    cache_root: &Path,
    frames_root: &Path,
    document: &Document,
) -> Result<(PathBuf, Document)> {
    runtime::check_cancelled()?;
    anyhow::ensure!(
        has_readable_body(&document.sections),
        "No readable transcript"
    );
    std::fs::create_dir_all(root)?;
    let root = root.canonicalize()?;
    // Recover interrupted swaps before staging or choosing a name for this source.
    let locks = cache_root.join("outputs");
    std::fs::create_dir_all(&locks)?;
    let _lock = runtime::lock_file(&locks.join(format!(
        "{}.lock",
        runtime::digest(root.to_string_lossy().as_bytes())
    )))?;
    recover(&root)?;
    let destination = destination(&root, &document.meta)?;
    let staging = tempfile::Builder::new()
        .prefix(".notepipe-new-")
        .tempdir_in(&root)?;
    let mut document = document.clone();
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
            std::fs::copy(image, staging.path().join(&name))?;
            images.insert(section.image.clone(), name.clone());
            name
        };
        section.image = name;
    }
    let markdown = render::render_markdown(&document.meta, &document.sections, &document.top)?;
    atomic_write_shared(&staging.path().join("note.md"), markdown.as_bytes())?;
    atomic_write_shared(
        &staging.path().join("note.json"),
        &serde_json::to_vec_pretty(&document)?,
    )?;
    runtime::check_cancelled()?;
    swap(staging.path(), &destination)?;
    Ok((destination, document))
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
    #[test]
    fn publishing_replaces_only_its_source_and_preserves_old_notes_on_failure() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("notes");
        let cache = temp.path().join("cache");
        let mut first = document("https://example.test/one");
        let (one, _) = publish(&root, &cache, temp.path(), &first).unwrap();
        assert_eq!(one.file_name().unwrap(), "Shared-title");
        let (two, _) = publish(
            &root,
            &cache,
            temp.path(),
            &document("https://example.test/two"),
        )
        .unwrap();
        assert_eq!(two.file_name().unwrap(), "Shared-title-2");
        first.top.push(crate::timeline::StageText {
            stage: "summary".into(),
            text: "Updated summary".into(),
        });
        assert_eq!(publish(&root, &cache, temp.path(), &first).unwrap().0, one);
        let previous = std::fs::read(one.join("note.md")).unwrap();
        first.sections[0].image = "../missing.jpg".into();
        assert!(publish(&root, &cache, temp.path(), &first).is_err());
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
        std::fs::write(note.join("note.md"), "previous").unwrap();
        let staging = note.join("staging");
        std::fs::create_dir(&staging).unwrap();
        assert!(swap(&staging, &note).is_err());
        assert_eq!(
            std::fs::read_to_string(note.join("note.md")).unwrap(),
            "previous"
        );
    }

    #[cfg(unix)]
    #[test]
    fn replacement_applies_folder_restrictions_to_all_published_files() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("notes");
        let cache = temp.path().join("cache");
        let mut doc = document("same source");
        std::fs::write(temp.path().join("frame.jpg"), "image").unwrap();
        doc.sections[0].image = "frame.jpg".into();
        let (note, _) = publish(&root, &cache, temp.path(), &doc).unwrap();
        for mode in [0o500, 0o700, 0o750] {
            std::fs::set_permissions(&note, std::fs::Permissions::from_mode(mode)).unwrap();
            publish(&root, &cache, temp.path(), &doc).unwrap();
            for path in [
                note.clone(),
                note.join("images"),
                note.join("note.md"),
                note.join("note.json"),
                note.join("images/0001.jpg"),
            ] {
                let permissions = path.metadata().unwrap().permissions().mode() & 0o777;
                assert_eq!(permissions & !mode, 0, "{}", path.display());
                if path.is_file() {
                    assert_eq!(permissions, 0o644 & mode);
                }
            }
            assert!(!previous_path(&note).exists());
        }
    }

    #[test]
    fn interrupted_swaps_are_recovered_before_staging_even_for_colliding_titles() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("notes");
        let cache = temp.path().join("cache");
        let first = document("one");
        let mut second = document("two");
        let (one, _) = publish(&root, &cache, temp.path(), &first).unwrap();
        let (two, _) = publish(&root, &cache, temp.path(), &second).unwrap();
        let original = std::fs::read(two.join("note.md")).unwrap();
        for note in [&one, &two] {
            std::fs::rename(note, previous_path(note)).unwrap();
        }
        let mut unrelated = document("three");
        unrelated.meta.title = "Other title".into();
        unrelated.sections[0].image = "missing.jpg".into();
        assert!(publish(&root, &cache, temp.path(), &unrelated).is_err());
        assert!(matches_source(&one, "one").unwrap());
        assert_eq!(std::fs::read(two.join("note.md")).unwrap(), original);
        // A crash after the second rename leaves both the new note and its backup.
        std::fs::create_dir(previous_path(&two)).unwrap();
        std::fs::write(previous_path(&two).join("note.md"), "previous").unwrap();
        second.sections[0].image.clear();
        assert_eq!(publish(&root, &cache, temp.path(), &second).unwrap().0, two);
        assert!(!previous_path(&two).exists());
    }
}
