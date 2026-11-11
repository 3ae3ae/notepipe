//! Build complete note folders and leave every previous publication untouched.
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
            document
                .meta
                .duration
                .is_none_or(|duration| duration.is_finite() && duration >= 0.),
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

#[derive(Serialize, Deserialize)]
struct Record {
    source: String,
    folder: PathBuf,
    files: BTreeMap<PathBuf, String>,
}

#[derive(Debug, Default)]
pub struct Published {
    pub note_dir: Option<PathBuf>,
    pub files: BTreeMap<String, String>,
    pub unchanged: bool,
}

fn file_digests(root: &Path) -> Result<BTreeMap<PathBuf, String>> {
    let mut files = BTreeMap::new();
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            for (name, digest) in file_digests(&entry.path())? {
                files.insert(Path::new(&entry.file_name()).join(name), digest);
            }
        } else {
            files.insert(
                entry.file_name().into(),
                runtime::file_digest(&entry.path())?,
            );
        }
    }
    Ok(files)
}

fn existing_note(
    root: &Path,
    source: &str,
    record: &Record,
    staging: &Path,
) -> Result<Option<PathBuf>> {
    let mut latest = None;
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        if entry.path() == staging || !matches_source(&entry.path(), source)? {
            continue;
        }
        if entry.file_name() == record.folder {
            return Ok(Some(entry.path()));
        }
        let modified = entry.metadata()?.modified()?;
        if latest.as_ref().is_none_or(|(_, time)| modified > *time) {
            latest = Some((entry.path(), modified));
        }
    }
    Ok(latest.map(|(path, _)| path))
}

#[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
fn rename_new(source: &Path, target: &Path) -> std::io::Result<()> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let source = CString::new(source.as_os_str().as_bytes())?;
    let target = CString::new(target.as_os_str().as_bytes())?;
    // Ordinary rename can replace an existing empty directory.
    // The system call itself: Android's libc has no renameat2 function before Android 11.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            source.as_ptr(),
            libc::AT_FDCWD,
            target.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    #[cfg(target_os = "macos")]
    let result = unsafe { libc::renamex_np(source.as_ptr(), target.as_ptr(), libc::RENAME_EXCL) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(windows)]
fn rename_new(source: &Path, target: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let target: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
    let result = unsafe {
        windows_sys::Win32::Storage::FileSystem::MoveFileExW(source.as_ptr(), target.as_ptr(), 0)
    };
    if result != 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    windows
)))]
fn rename_new(_source: &Path, _target: &Path) -> std::io::Result<()> {
    Err(std::io::ErrorKind::Unsupported.into())
}

fn commit(root: &Path, staging: &Path, title: &str) -> Result<PathBuf> {
    let slug = crate::config::sanitize_filename_with_fallback(title, "untitled");
    for number in 1.. {
        runtime::check_cancelled()?;
        let name = if number == 1 {
            slug.clone()
        } else {
            format!("{slug}-{number}")
        };
        let target = root.join(name);
        match target.symlink_metadata() {
            Ok(_) => continue,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        match rename_new(staging, &target) {
            Ok(()) => return Ok(target),
            Err(error)
                if error.kind() == std::io::ErrorKind::AlreadyExists
                    || target.symlink_metadata().is_ok() => {}
            Err(error) => return Err(error).context("Could not publish note folder"),
        }
    }
    unreachable!()
}

pub fn publish(
    root: &Path,
    work: &Path,
    identity: &str,
    meta: &MediaMeta,
    outputs: &[Publication],
    warnings: &mut Vec<String>,
) -> Result<Published> {
    runtime::check_cancelled()?;
    let mut published = Published::default();
    let folder_outputs = outputs.iter().filter(|o| o.name != "-").collect::<Vec<_>>();
    published.note_dir = if folder_outputs.is_empty() {
        None
    } else {
        std::fs::create_dir_all(root)?;
        let root = root.canonicalize()?;
        let record_path = work.join(".cache/published.json");
        let mut records: BTreeMap<PathBuf, Record> = match std::fs::read(&record_path) {
            Ok(bytes) => serde_json::from_slice(&bytes).context("Invalid publication cache")?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(error) => return Err(error.into()),
        };
        let staging = tempfile::Builder::new()
            .prefix(".notepipe-new-")
            .tempdir_in(&root)?;
        for (index, output) in folder_outputs.iter().enumerate() {
            runtime::check_cancelled()?;
            let target = staging.path().join(&output.name);
            for previous in &folder_outputs[..index] {
                anyhow::ensure!(
                    !same_file(&target, &staging.path().join(&previous.name))?,
                    "Stages {} and {} publish the same file",
                    previous.stage,
                    output.stage
                );
            }
            copy_path(&work.join(&output.output), &target)?;
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
            serde_json::to_vec(&serde_json::json!({"source":identity}))?,
        )?;
        runtime::check_cancelled()?;
        let digests = file_digests(staging.path())?;
        let existing = match records.get(&root) {
            Some(record) if record.source == identity && record.files == digests => {
                existing_note(&root, identity, record, staging.path())?
            }
            _ => None,
        };
        let destination = if let Some(note) = existing {
            published.unchanged = true;
            note
        } else {
            let destination = commit(&root, staging.path(), &meta.title)?;
            records.insert(
                root.clone(),
                Record {
                    source: identity.into(),
                    folder: destination.file_name().unwrap().into(),
                    files: digests,
                },
            );
            if let Err(error) =
                crate::checkpoint::atomic_write(&record_path, &serde_json::to_vec(&records)?)
            {
                warnings.push(format!(
                    "Note published; could not update publication cache {}: {error:#}",
                    record_path.display()
                ));
            }
            destination
        };
        for output in &folder_outputs {
            published.files.insert(
                output.stage.clone(),
                destination.join(&output.name).display().to_string(),
            );
        }
        Some(destination)
    };
    for output in outputs.iter().filter(|o| o.name == "-") {
        published.files.insert(output.stage.clone(), "-".into());
    }
    Ok(published)
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
            &mut std::fs::File::create_new(target)?,
        )?;
    }
    Ok(())
}

pub(crate) fn same_file(a: &Path, b: &Path) -> Result<bool> {
    match (file_identity(a), file_identity(b)) {
        (Ok(a), Ok(b)) => Ok(a == b),
        (Err(error), _) | (_, Err(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(false)
        }
        (Err(error), _) | (_, Err(error)) => Err(error.into()),
    }
}

#[cfg(unix)]
fn file_identity(path: &Path) -> std::io::Result<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let metadata = path.metadata()?;
    Ok((metadata.dev(), metadata.ino()))
}

#[cfg(windows)]
fn file_identity(path: &Path) -> std::io::Result<(u64, u64)> {
    use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS, GetFileInformationByHandle,
    };
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)?;
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok((
        u64::from(info.dwVolumeSerialNumber),
        (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn copying_never_overwrites_an_existing_file() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        let target = root.path().join("target");
        std::fs::write(&source, "New").unwrap();
        std::fs::write(&target, "Existing").unwrap();
        assert!(copy_path(&source, &target).is_err());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "Existing");
    }
    #[test]
    fn colliding_publications_preserve_the_previous_note() {
        let root = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        std::fs::write(work.path().join("a"), "First").unwrap();
        std::fs::write(work.path().join("b"), "Second").unwrap();
        let mut outputs = vec![Publication {
            stage: "a".into(),
            output: "a".into(),
            name: "note.md".into(),
            images: vec![],
        }];
        let meta = document("source").meta;
        let mut warnings = vec![];
        let note = super::publish(
            root.path(),
            work.path(),
            &meta.source,
            &meta,
            &outputs,
            &mut warnings,
        )
        .unwrap()
        .note_dir
        .unwrap();
        outputs.push(Publication {
            stage: "b".into(),
            output: "b".into(),
            name: "note.md".into(),
            images: vec![],
        });
        assert_eq!(
            super::publish(
                root.path(),
                work.path(),
                &meta.source,
                &meta,
                &outputs,
                &mut warnings
            )
            .unwrap_err()
            .to_string(),
            "Stages a and b publish the same file"
        );
        assert_eq!(
            std::fs::read_to_string(note.join("note.md")).unwrap(),
            "First"
        );
    }
    fn document(source: &str) -> Document {
        Document {
            schema: 3,
            meta: MediaMeta {
                extra: Default::default(),
                title: "Shared title".into(),
                source: source.into(),
                author: String::new(),
                duration: Some(1.),
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
        let published = super::publish(
            root,
            work,
            &document.meta.source,
            &document.meta,
            &outputs,
            &mut warnings,
        )?;
        Ok((published.note_dir.unwrap(), warnings))
    }

    #[test]
    fn publishing_reuses_identical_files_and_preserves_old_notes_on_failure() {
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
        let other_work = tempfile::tempdir().unwrap();
        let (two, _) = publish(
            &root,
            other_work.path(),
            &document("https://example.test/two"),
        )
        .unwrap();
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
    fn changed_images_and_file_sets_get_new_folders_with_complete_digest_records() {
        let root = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let mut doc = document("source");
        doc.sections[0].image = "images/frame.jpg".into();
        std::fs::create_dir(work.path().join("images")).unwrap();
        std::fs::write(work.path().join("images/frame.jpg"), "First image").unwrap();
        let (first, _) = publish(root.path(), work.path(), &doc).unwrap();
        let original = file_digests(&first).unwrap();
        let records: BTreeMap<PathBuf, Record> = serde_json::from_slice(
            &std::fs::read(work.path().join(".cache/published.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            records[&root.path().canonicalize().unwrap()].files,
            original
        );
        assert_eq!(original.len(), 4);
        assert_eq!(
            original[Path::new("images/frame.jpg")],
            runtime::digest(b"First image")
        );
        std::fs::write(work.path().join("images/frame.jpg"), "Changed image").unwrap();
        let (second, _) = publish(root.path(), work.path(), &doc).unwrap();
        assert_eq!(second.file_name().unwrap(), "Shared-title-2");
        assert_eq!(file_digests(&first).unwrap(), original);
        assert_eq!(
            std::fs::read_to_string(second.join("images/frame.jpg")).unwrap(),
            "Changed image"
        );
        doc.sections[0].image.clear();
        let (third, _) = publish(root.path(), work.path(), &doc).unwrap();
        assert_eq!(third.file_name().unwrap(), "Shared-title-3");
        assert!(!third.join("images").exists());
        assert_eq!(file_digests(&first).unwrap(), original);
    }

    #[test]
    fn records_are_separate_per_canonical_root_and_prefer_the_recorded_folder() {
        let root = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let doc = document("source");
        let first_root = root.path().join("first");
        let second_root = root.path().join("second");
        let (first, _) = publish(&first_root, work.path(), &doc).unwrap();
        let (second, _) = publish(&second_root, work.path(), &doc).unwrap();
        assert_eq!(
            publish(&first_root.join("."), work.path(), &doc).unwrap().0,
            first
        );
        assert_eq!(publish(&second_root, work.path(), &doc).unwrap().0, second);
        let duplicate = first_root.join("newer duplicate");
        copy_path(&first, &duplicate).unwrap();
        assert_eq!(publish(&first_root, work.path(), &doc).unwrap().0, first);
        let renamed = first_root.join("renamed");
        std::fs::rename(&first, &renamed).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let newest = first_root.join(".notepipe-new-renamed");
        copy_path(&duplicate, &newest).unwrap();
        assert!(
            newest.metadata().unwrap().modified().unwrap()
                > renamed.metadata().unwrap().modified().unwrap()
        );
        assert_eq!(
            publish(&first_root, work.path(), &doc).unwrap().0,
            newest.canonicalize().unwrap()
        );
        assert!(renamed.join("note.md").exists());
    }

    #[test]
    fn concurrent_publications_get_different_complete_folders() {
        let root = tempfile::tempdir().unwrap();
        let barrier = std::sync::Barrier::new(2);
        let mut notes = std::thread::scope(|scope| {
            let jobs = ["one", "two"].map(|source| {
                let barrier = &barrier;
                let root = root.path();
                scope.spawn(move || {
                    let work = tempfile::tempdir().unwrap();
                    barrier.wait();
                    publish(root, work.path(), &document(source)).unwrap().0
                })
            });
            jobs.map(|job| job.join().unwrap())
        });
        notes.sort();
        assert_ne!(notes[0], notes[1]);
        for note in notes {
            assert!(note.join("note.md").is_file());
            assert!(note.join("note.json").is_file());
            assert!(note.join(".notepipe.json").is_file());
        }
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 2);
    }

    #[test]
    fn atomic_rename_never_replaces_an_existing_entry_and_racing_commits_retry() {
        let root = tempfile::tempdir().unwrap();
        let staging = tempfile::Builder::new()
            .prefix(".notepipe-new-")
            .tempdir_in(root.path())
            .unwrap();
        std::fs::write(staging.path().join("note.md"), "New note").unwrap();
        let existing = root.path().join("existing");
        std::fs::create_dir(&existing).unwrap();
        let identity = file_identity(&existing).unwrap();
        assert!(rename_new(staging.path(), &existing).is_err());
        assert_eq!(file_identity(&existing).unwrap(), identity);
        assert_eq!(std::fs::read_dir(&existing).unwrap().count(), 0);
        std::fs::remove_dir(&existing).unwrap();
        std::fs::write(&existing, "User file").unwrap();
        assert!(rename_new(staging.path(), &existing).is_err());
        assert_eq!(std::fs::read_to_string(&existing).unwrap(), "User file");
        let barrier = std::sync::Barrier::new(2);
        let destinations = std::thread::scope(|scope| {
            let jobs = ["First", "Second"].map(|body| {
                let root = root.path();
                let barrier = &barrier;
                scope.spawn(move || {
                    let staging = tempfile::Builder::new()
                        .prefix(".notepipe-new-")
                        .tempdir_in(root)
                        .unwrap();
                    std::fs::write(staging.path().join("note.md"), body).unwrap();
                    barrier.wait();
                    commit(root, staging.path(), "note").unwrap()
                })
            });
            jobs.map(|job| job.join().unwrap())
        });
        assert_ne!(destinations[0], destinations[1]);
        assert!(root.path().join("note/note.md").is_file());
        assert!(root.path().join("note-2/note.md").is_file());
    }

    #[test]
    fn failed_publication_removes_its_staging_folder() {
        let root = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let mut doc = document("source");
        let (note, _) = publish(root.path(), work.path(), &doc).unwrap();
        let original = file_digests(&note).unwrap();
        doc.sections[0].image = "missing.jpg".into();
        assert!(publish(root.path(), work.path(), &doc).is_err());
        assert_eq!(file_digests(&note).unwrap(), original);
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn publication_cache_write_failures_warn_and_keep_the_new_note() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let mut doc = document("source");
        publish(root.path(), work.path(), &doc).unwrap();
        let cache = work.path().join(".cache");
        std::fs::set_permissions(&cache, std::fs::Permissions::from_mode(0o500)).unwrap();
        doc.sections[0].speech[0].text = "Changed".into();
        let result = publish(root.path(), work.path(), &doc);
        std::fs::set_permissions(cache, std::fs::Permissions::from_mode(0o700)).unwrap();
        let (note, warnings) = result.unwrap();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].starts_with("Note published; could not update publication cache "));
        assert_eq!(
            std::fs::read_to_string(note.join("note.md")).unwrap(),
            "Changed\n"
        );
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 2);
    }
    #[test]
    fn damaged_publication_records_fail_without_touching_the_previous_note() {
        let root = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let doc = document("source");
        let (note, _) = publish(root.path(), work.path(), &doc).unwrap();
        let original = file_digests(&note).unwrap();
        std::fs::write(work.path().join(".cache/published.json"), "invalid").unwrap();
        assert_eq!(
            publish(root.path(), work.path(), &doc)
                .unwrap_err()
                .to_string(),
            "Invalid publication cache"
        );
        assert_eq!(file_digests(&note).unwrap(), original);
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    }

    #[test]
    fn stdout_publication_writes_no_folder_or_publication_record() {
        let root = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let outputs = [Publication {
            stage: "markdown".into(),
            output: "note.md".into(),
            name: "-".into(),
            images: vec![],
        }];
        let published = super::publish(
            root.path(),
            work.path(),
            "source",
            &document("source").meta,
            &outputs,
            &mut Vec::new(),
        )
        .unwrap();
        assert!(published.note_dir.is_none());
        assert!(!published.unchanged);
        assert_eq!(published.files["markdown"], "-");
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
        assert!(!work.path().join(".cache/published.json").exists());
    }
}
