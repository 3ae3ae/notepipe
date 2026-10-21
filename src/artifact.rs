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
    pub summary: Option<String>,
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

fn swap(staging: &Path, destination: &Path) -> Result<()> {
    if !destination.exists() {
        return std::fs::rename(staging, destination).context("Could not publish note folder");
    }
    let backup = tempfile::Builder::new()
        .prefix(".notepipe-old-")
        .tempdir_in(destination.parent().unwrap())?;
    let previous = backup.path().join("note");
    std::fs::rename(destination, &previous).context("Could not preserve the previous note")?;
    if let Err(error) = std::fs::rename(staging, destination) {
        if let Err(restore) = std::fs::rename(&previous, destination) {
            let retained = backup.keep().join("note");
            anyhow::bail!(
                "Could not publish note: {error}; could not restore it: {restore}; previous note retained at {}",
                retained.display()
            );
        }
        return Err(error).context("Could not publish note; previous note restored");
    }
    Ok(())
}

pub fn publish(
    root: &Path,
    cache_root: &Path,
    frames_root: &Path,
    document: &Document,
) -> Result<PathBuf> {
    runtime::check_cancelled()?;
    anyhow::ensure!(
        has_readable_body(&document.sections),
        "No readable transcript"
    );
    std::fs::create_dir_all(root)?;
    let root = root.canonicalize()?;
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
    let markdown = render::render_markdown(
        &document.meta,
        &document.sections,
        document.summary.as_deref(),
    )?;
    atomic_write_shared(&staging.path().join("note.md"), markdown.as_bytes())?;
    atomic_write_shared(
        &staging.path().join("note.json"),
        &serde_json::to_vec_pretty(&document)?,
    )?;
    // Different sources can have the same title; serialize their final name selection and swap.
    let locks = cache_root.join("outputs");
    std::fs::create_dir_all(&locks)?;
    let _lock = runtime::lock_file(&locks.join(format!(
        "{}.lock",
        runtime::digest(root.to_string_lossy().as_bytes())
    )))?;
    let destination = destination(&root, &document.meta)?;
    runtime::check_cancelled()?;
    swap(staging.path(), &destination)?;
    Ok(destination)
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
                    translation: None,
                }],
            }],
            summary: None,
            warnings: vec![],
        }
    }
    #[test]
    fn publishing_replaces_only_its_source_and_preserves_old_notes_on_failure() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("notes");
        let cache = temp.path().join("cache");
        let mut first = document("https://example.test/one");
        let one = publish(&root, &cache, temp.path(), &first).unwrap();
        assert_eq!(one.file_name().unwrap(), "Shared-title");
        let two = publish(
            &root,
            &cache,
            temp.path(),
            &document("https://example.test/two"),
        )
        .unwrap();
        assert_eq!(two.file_name().unwrap(), "Shared-title-2");
        first.summary = Some("Updated summary".into());
        assert_eq!(publish(&root, &cache, temp.path(), &first).unwrap(), one);
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
        assert!(swap(&temp.path().join("missing"), &note).is_err());
        assert_eq!(
            std::fs::read_to_string(note.join("note.md")).unwrap(),
            "previous"
        );
    }
}
