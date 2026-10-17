//! Logical folders never move or delete exported course files.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

const FILE: &str = ".course2md-library.json";
const TITLES: &str = ".course2md-titles.json";

#[derive(Clone)]
pub struct Recovery {
    pub has_backup: bool,
}

/// A registered root and a discovered note may use different filesystem aliases
/// (for example `/tmp` and `/private/tmp`). Folder and title records always keep
/// one relative key, regardless of which spelling the importer returned.
pub fn relative_key(root: &Path, course: &Path) -> Result<PathBuf> {
    let valid = |key: &Path| {
        !key.as_os_str().is_empty()
            && key
                .components()
                .all(|part| matches!(part, std::path::Component::Normal(_)))
    };
    if let Ok(key) = course.strip_prefix(root)
        && valid(key)
    {
        return Ok(key.to_path_buf());
    }
    let root = std::fs::canonicalize(root).context(course2md::tr!("笔记库文件夹暂时无法访问"))?;
    let course = std::fs::canonicalize(course).context(course2md::tr!("笔记位置暂时无法访问"))?;
    let key = course
        .strip_prefix(root)
        .context(course2md::tr!("笔记不在这个笔记库中"))?;
    ensure!(valid(key), course2md::tr!("笔记位置无效"));
    Ok(key.to_path_buf())
}

fn save_with_backup(path: &Path, bytes: &[u8]) -> Result<()> {
    let previous = std::fs::read(path);
    let backup = path.with_extension("json.bak");
    match previous {
        Ok(previous) => course2md::checkpoint::atomic_write(&backup, &previous)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            course2md::checkpoint::atomic_write(&backup, bytes)?
        }
        Err(error) => return Err(error).context(course2md::tr!("无法保留原记录")),
    }
    course2md::checkpoint::atomic_write(path, bytes)
}

/// Inspect only. A damaged folder record is never replaced during a read.
pub fn recovery(root: &Path) -> Result<Option<Recovery>> {
    metadata_recovery::<Library>(root, FILE)
}

fn metadata_recovery<T: serde::de::DeserializeOwned>(
    root: &Path,
    filename: &str,
) -> Result<Option<Recovery>> {
    let path = root.join(filename);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).context(course2md::tr!(
                "无法读取文件夹信息，请检查保存位置的访问权限"
            ));
        }
    };
    if serde_json::from_slice::<T>(&bytes).is_ok() {
        return Ok(None);
    }
    let has_backup = std::fs::read(path.with_extension("json.bak"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<T>(&bytes).ok())
        .is_some();
    Ok(Some(Recovery { has_backup }))
}

/// Preserve the damaged bytes before publishing a chosen recovery. Notes and title
/// aliases live in separate files and are never rewritten by classification recovery.
pub fn recover(root: &Path, rebuild: bool) -> Result<(Library, PathBuf)> {
    recover_metadata::<Library>(root, FILE, rebuild)
}

fn recover_metadata<T: serde::de::DeserializeOwned + Serialize + Default>(
    root: &Path,
    filename: &str,
    rebuild: bool,
) -> Result<(T, PathBuf)> {
    ensure!(
        root.is_dir(),
        course2md::tr!("笔记库文件夹暂时无法访问，请重新连接磁盘后再试")
    );
    let path = root.join(filename);
    let _lock = course2md::runtime::lock_file(&path.with_extension("lock"))?;
    let original =
        std::fs::read(&path).context(course2md::tr!("无法读取并保留原记录，尚未恢复"))?;
    ensure!(
        serde_json::from_slice::<T>(&original).is_err(),
        course2md::tr!("记录已恢复，请重新扫描笔记库")
    );
    let library = if rebuild {
        T::default()
    } else {
        serde_json::from_slice::<T>(
            &std::fs::read(path.with_extension("json.bak"))
                .context(course2md::tr!("最近备份无法读取，原记录已保留"))?,
        )
        .context(course2md::tr!("最近备份也无法读取，原记录已保留"))?
    };
    let preserved = root.join(format!(
        "{}-damaged-{}.json",
        filename.trim_end_matches(".json"),
        crate::workspace::new_id("record")
    ));
    course2md::checkpoint::atomic_write(&preserved, &original)?;
    course2md::checkpoint::atomic_write(&path, &serde_json::to_vec_pretty(&library)?)?;
    Ok((library, preserved))
}

#[derive(Default, Serialize, Deserialize)]
struct Titles {
    #[serde(default)]
    names: BTreeMap<PathBuf, String>,
}

pub fn title_aliases(root: &Path) -> Result<BTreeMap<PathBuf, String>> {
    match std::fs::read(root.join(TITLES)) {
        Ok(bytes) => Ok(serde_json::from_slice::<Titles>(&bytes)
            .context(course2md::tr!("笔记名称记录无法读取，原文件已保留"))?
            .names),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(error) => Err(error).context(course2md::tr!("笔记名称记录暂时无法读取")),
    }
}

pub fn title_recovery(root: &Path) -> Result<Option<Recovery>> {
    metadata_recovery::<Titles>(root, TITLES)
}

pub fn recover_titles(root: &Path, reset: bool) -> Result<PathBuf> {
    recover_metadata::<Titles>(root, TITLES, reset).map(|(_, preserved)| preserved)
}

#[cfg(test)]
pub fn title_alias(root: &Path, course: &Path) -> Result<Option<String>> {
    Ok(title_aliases(root)?
        .get(&relative_key(root, course)?)
        .cloned())
}

pub fn rename_course(root: &Path, course: &Path, name: &str) -> Result<()> {
    let name = name.trim();
    ensure!(!name.is_empty(), course2md::tr!("请输入笔记名称"));
    ensure!(
        name.chars().count() <= 240,
        course2md::tr!("笔记名称最多 240 个字符")
    );
    ensure!(
        root.is_dir() && course.is_dir(),
        course2md::tr!("这份笔记的位置暂时无法访问")
    );
    let key = relative_key(root, course)?;
    let _lock = course2md::runtime::lock_file(&root.join(".course2md-titles.lock"))?;
    let mut names = title_aliases(root)?;
    names.insert(key, name.to_owned());
    save_with_backup(
        &root.join(TITLES),
        &serde_json::to_vec_pretty(&Titles { names })?,
    )
}

#[derive(Clone, Default, Deserialize, Serialize)]
pub struct Library {
    pub folders: BTreeMap<u64, String>,
    pub courses: BTreeMap<PathBuf, u64>,
    #[serde(default)]
    next_id: u64,
}
impl Library {
    pub fn load(root: &Path) -> Result<Self> {
        ensure!(
            root.is_dir(),
            course2md::tr!("笔记库文件夹暂时无法访问，请重新连接磁盘后再试")
        );
        match std::fs::read(root.join(FILE)) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .context(course2md::tr!("文件夹信息损坏；原文件已保留，请修复后重试")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }
    pub fn edit(root: &Path, change: impl FnOnce(&mut Self) -> Result<()>) -> Result<Self> {
        ensure!(
            root.is_dir(),
            course2md::tr!("笔记库文件夹暂时无法访问，请重新连接磁盘后再试")
        );
        let _lock = course2md::runtime::lock_file(&root.join(".course2md-library.lock"))?;
        let mut library = Self::load(root)?;
        change(&mut library)?;
        save_with_backup(&root.join(FILE), &serde_json::to_vec_pretty(&library)?)?;
        Ok(library)
    }
    pub fn rename(&mut self, id: Option<u64>, name: &str) -> Result<u64> {
        let name = name.trim();
        ensure!(!name.is_empty(), course2md::tr!("请输入文件夹名称"));
        ensure!(
            name.chars().count() <= 60,
            course2md::tr!("文件夹名称最多 60 个字符")
        );
        ensure!(
            name != course2md::tr!("未分类"),
            course2md::tr!("「未分类」已被使用，请换一个文件夹名称")
        );
        ensure!(!self.folders.iter().any(|(key, value)| Some(*key) != id && value.to_lowercase() == name.to_lowercase()), course2md::tr!("已有同名文件夹"));
        let id = match id {
            Some(id) => {
                ensure!(
                    self.folders.contains_key(&id),
                    course2md::tr!("文件夹已不存在，请重新扫描笔记库")
                );
                id
            }
            None => {
                self.next_id = self
                    .next_id
                    .max(self.folders.keys().copied().max().unwrap_or(0))
                    .checked_add(1)
                    .context(course2md::tr!("文件夹编号已耗尽"))?;
                self.next_id
            }
        };
        self.folders.insert(id, name.to_owned());
        Ok(id)
    }
    pub fn remove(&mut self, id: u64) {
        self.folders.remove(&id);
        self.courses.retain(|_, folder| *folder != id);
    }
    #[cfg(test)]
    pub fn folder(&self, root: &Path, course: &Path) -> Option<u64> {
        self.folder_key(&relative_key(root, course).ok()?)
    }
    /// Query a relative key already validated by the background library scan.
    pub fn folder_key(&self, key: &Path) -> Option<u64> {
        self.courses
            .get(key)
            .copied()
            .filter(|id| self.folders.contains_key(id))
    }
    pub fn assign(&mut self, root: &Path, course: &Path, folder: Option<u64>) -> Result<()> {
        let key = relative_key(root, course)?;
        if let Some(id) = folder {
            ensure!(
                self.folders.contains_key(&id),
                course2md::tr!("文件夹已不存在，请选择其他文件夹")
            );
            self.courses.insert(key, id);
        } else {
            self.courses.remove(&key);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(unix)]
    fn canonical_imports_keep_the_registered_library_folders_and_names() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("library");
        let course = root.join("old-lecture");
        std::fs::create_dir_all(&course).unwrap();
        let alias = temp.path().join("registered-library");
        std::os::unix::fs::symlink(&root, &alias).unwrap();
        let imported = std::fs::canonicalize(&course).unwrap();
        assert_eq!(
            relative_key(&alias, &imported).unwrap(),
            Path::new("old-lecture")
        );
        let library = Library::edit(&alias, |library| {
            let folder = library.rename(None, "金融学")?;
            library.assign(&alias, &imported, Some(folder))
        })
        .unwrap();
        assert_eq!(library.folder(&alias, &imported), Some(1));
        assert_eq!(library.folder(&root, &alias.join("old-lecture")), Some(1));
        rename_course(&alias, &imported, "旧课程的新名称").unwrap();
        assert_eq!(
            title_alias(&root, &imported).unwrap().as_deref(),
            Some("旧课程的新名称")
        );
        assert_eq!(
            title_alias(&alias, &alias.join("old-lecture"))
                .unwrap()
                .as_deref(),
            Some("旧课程的新名称")
        );
        let outside = temp.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        assert!(relative_key(&alias, &outside).is_err());
        assert!(relative_key(&alias, &alias.join("../outside")).is_err());
    }

    #[test]
    fn damaged_classification_recovers_last_good_backup_without_touching_notes_or_titles() {
        let root = tempfile::tempdir().unwrap();
        let course = root.path().join("course");
        std::fs::create_dir(&course).unwrap();
        std::fs::write(course.join("course.md"), "hand edited text").unwrap();
        rename_course(root.path(), &course, "My name").unwrap();
        Library::edit(root.path(), |library| {
            let id = library.rename(None, "Math")?;
            library.assign(root.path(), &course, Some(id))
        })
        .unwrap();
        Library::edit(root.path(), |library| {
            library.rename(None, "Physics")?;
            Ok(())
        })
        .unwrap();
        std::fs::write(root.path().join(FILE), "damaged bytes").unwrap();
        assert!(Library::load(root.path()).is_err());
        assert!(recovery(root.path()).unwrap().unwrap().has_backup);
        let (restored, preserved) = recover(root.path(), false).unwrap();
        assert_eq!(restored.folders.len(), 1);
        assert_eq!(restored.folder(root.path(), &course), Some(1));
        assert_eq!(std::fs::read_to_string(preserved).unwrap(), "damaged bytes");
        assert_eq!(
            title_alias(root.path(), &course).unwrap().as_deref(),
            Some("My name")
        );
        assert_eq!(
            std::fs::read_to_string(course.join("course.md")).unwrap(),
            "hand edited text"
        );
    }

    #[test]
    fn rebuild_only_resets_classification_and_names_are_shared_across_versions() {
        let root = tempfile::tempdir().unwrap();
        let course = root.path().join("course");
        std::fs::create_dir_all(course.join("versions/a")).unwrap();
        std::fs::create_dir_all(course.join("versions/b")).unwrap();
        std::fs::write(course.join("versions/a/course.md"), "old content").unwrap();
        std::fs::write(root.path().join(FILE), "damaged without backup").unwrap();
        rename_course(root.path(), &course, " Shared name ").unwrap();
        assert!(!recovery(root.path()).unwrap().unwrap().has_backup);
        let (rebuilt, _) = recover(root.path(), true).unwrap();
        assert!(rebuilt.folders.is_empty());
        assert_eq!(
            title_alias(root.path(), &course).unwrap().as_deref(),
            Some("Shared name")
        );
        assert_eq!(
            std::fs::read_to_string(course.join("versions/a/course.md")).unwrap(),
            "old content"
        );
        assert!(rename_course(root.path(), &course, " ").is_err());
        assert_eq!(
            title_alias(root.path(), &course).unwrap().as_deref(),
            Some("Shared name")
        );
    }

    #[test]
    fn title_recovery_is_independent_and_unavailable_roots_are_not_recreated() {
        let root = tempfile::tempdir().unwrap();
        let course = root.path().join("course");
        std::fs::create_dir(&course).unwrap();
        Library::edit(root.path(), |library| {
            library.rename(None, "Keep folder")?;
            Ok(())
        })
        .unwrap();
        let classification = std::fs::read(root.path().join(FILE)).unwrap();
        rename_course(root.path(), &course, "First name").unwrap();
        rename_course(root.path(), &course, "Second name").unwrap();
        std::fs::write(root.path().join(TITLES), "damaged names").unwrap();
        assert!(title_recovery(root.path()).unwrap().unwrap().has_backup);
        let preserved = recover_titles(root.path(), false).unwrap();
        assert_eq!(std::fs::read_to_string(preserved).unwrap(), "damaged names");
        assert_eq!(
            title_alias(root.path(), &course).unwrap().as_deref(),
            Some("First name")
        );
        assert_eq!(
            std::fs::read(root.path().join(FILE)).unwrap(),
            classification
        );
        let missing = root.path().join("disconnected");
        assert!(Library::load(&missing).is_err());
        assert!(Library::edit(&missing, |_| Ok(())).is_err());
        assert!(!missing.exists());
    }
    #[test]
    fn reserved_name_edits_leave_saved_folders_and_assignments_unchanged() {
        let root = tempfile::tempdir().unwrap();
        let course = root.path().join("lecture");
        let mut id = 0;
        Library::edit(root.path(), |library| {
            id = library.rename(None, "数学")?;
            library.assign(root.path(), &course, Some(id))
        })
        .unwrap();
        let path = root.path().join(".course2md-library.json");
        let before = std::fs::read(&path).unwrap();
        for target in [None, Some(id)] {
            assert!(
                Library::edit(root.path(), |library| {
                    library.rename(target, "  未分类  ")?;
                    Ok(())
                })
                .is_err(),
                "the unfiled name is reserved"
            );
            assert_eq!(std::fs::read(&path).unwrap(), before);
        }
        let reopened = Library::load(root.path()).unwrap();
        assert_eq!(reopened.folders[&id], "数学");
        assert_eq!(reopened.folder(root.path(), &course), Some(id));
    }

    #[test]
    fn folders_survive_rename_restart_and_deletion_keeps_notes() {
        let root = tempfile::tempdir().unwrap();
        let course = root.path().join("local/lecture");
        std::fs::create_dir_all(&course).unwrap();
        std::fs::write(course.join("course.md"), "valuable notes").unwrap();
        let mut id = 0;
        Library::edit(root.path(), |library| {
            id = library.rename(None, "  数学  ")?;
            library.assign(root.path(), &course, Some(id))
        })
        .unwrap();
        Library::edit(root.path(), |library| {
            library.rename(Some(id), "线性代数")?;
            Ok(())
        })
        .unwrap();
        let reopened = Library::load(root.path()).unwrap();
        assert_eq!(reopened.folders[&id], "线性代数");
        assert_eq!(reopened.folder(root.path(), &course), Some(id));
        Library::edit(root.path(), |library| {
            library.remove(id);
            Ok(())
        })
        .unwrap();
        assert_eq!(
            Library::load(root.path())
                .unwrap()
                .folder(root.path(), &course),
            None
        );
        assert_eq!(
            std::fs::read_to_string(course.join("course.md")).unwrap(),
            "valuable notes"
        );
    }
    #[test]
    fn stale_writers_merge_latest_state_and_failed_edits_do_not_write() {
        let root = tempfile::tempdir().unwrap();
        Library::edit(root.path(), |library| {
            library.rename(None, "First")?;
            Ok(())
        })
        .unwrap();
        Library::edit(root.path(), |library| {
            library.rename(None, "Second")?;
            Ok(())
        })
        .unwrap();
        assert_eq!(Library::load(root.path()).unwrap().folders.len(), 2);
        assert!(
            Library::edit(root.path(), |library| {
                library.rename(None, "FIRST")?;
                Ok(())
            })
            .is_err()
        );
        let path = root.path().join(".course2md-library.json");
        std::fs::write(&path, "broken metadata").unwrap();
        assert!(
            Library::edit(root.path(), |library| {
                library.rename(None, "Replacement")?;
                Ok(())
            })
            .is_err()
        );
        assert_eq!(std::fs::read_to_string(path).unwrap(), "broken metadata");
    }
}
