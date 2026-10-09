//! Controlled library migration and explicit cleanup of verified old-location backups.
use super::*;
use crate::{
    settings_ui::text,
    storage::{self, Journal, Phase, Progress},
    theme::*,
};
use anyhow::{Context as _, Result, ensure};
use gpui_component::button::*;
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Default)]
pub struct State {
    pub busy: bool,
    cancel: Option<Arc<AtomicBool>>,
    generation: u64,
    progress: Progress,
    error: Option<String>,
    pending: Vec<(PathBuf, Journal)>,
    resume_task: Option<String>,
    cleanup: bool,
    association: bool,
    relocation: Option<String>,
    location_checks: LocationChecks,
}

#[derive(Clone, Debug)]
pub(super) struct LocationCheck {
    pub available: bool,
    pub needs_reassociation: bool,
    pub problem: Option<String>,
}

#[derive(Default)]
struct LocationChecks {
    generation: u64,
    expected: BTreeMap<String, PathBuf>,
    results: BTreeMap<String, (PathBuf, LocationCheck)>,
}

fn location_paths(locations: &[workspace::LibraryLocation]) -> BTreeMap<String, PathBuf> {
    locations
        .iter()
        .map(|location| (location.id.clone(), location.root.clone()))
        .collect()
}

impl LocationChecks {
    fn begin(&mut self, generation: u64, locations: &[workspace::LibraryLocation]) {
        self.generation = generation;
        self.expected = location_paths(locations);
        // A refresh can keep useful facts about the same path. A new path must
        // remain unknown until its own check returns.
        self.results
            .retain(|id, (root, _)| self.expected.get(id) == Some(root));
    }

    fn get(&self, location: &workspace::LibraryLocation) -> Option<&LocationCheck> {
        self.results
            .get(&location.id)
            .filter(|(root, _)| root == &location.root)
            .map(|(_, check)| check)
    }

    fn finish(
        &mut self,
        generation: u64,
        current: &[workspace::LibraryLocation],
        checks: Vec<(workspace::LibraryLocation, LocationCheck)>,
    ) -> bool {
        if generation != self.generation
            || location_paths(current) != self.expected
            || checks.len() != self.expected.len()
            || checks
                .iter()
                .any(|(location, _)| self.expected.get(&location.id) != Some(&location.root))
        {
            return false;
        }
        self.results = checks
            .into_iter()
            .map(|(location, check)| (location.id, (location.root, check)))
            .collect();
        true
    }
}

fn inspect_location(location: &workspace::LibraryLocation) -> LocationCheck {
    let available = std::fs::read_dir(&location.root).is_ok();
    let mut check = LocationCheck {
        available,
        needs_reassociation: false,
        problem: None,
    };
    if !available || location.id.is_empty() {
        return check;
    }
    match std::fs::read_to_string(location.root.join(".course2md-library-id")) {
        Ok(identity) if identity.trim() == location.id => {}
        Ok(_) => {
            check.problem =
                Some("保存位置对应其他课程库，请恢复原位置或在存储设置中重新登记".into())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            check.needs_reassociation = true
        }
        Err(error) => check.problem = Some(format!("无法读取课程库关联记录：{error}")),
    }
    check
}

/// All filesystem probes and the caller's library scan run on the blocking
/// pool, even when this future is polled by the UI executor.
pub(super) async fn scan_locations<T, F>(
    locations: Vec<workspace::LibraryLocation>,
    scan: F,
) -> Vec<(workspace::LibraryLocation, LocationCheck, T)>
where
    T: Send + 'static,
    F: Fn(&workspace::LibraryLocation) -> T + Send + 'static,
{
    smol::unblock(move || {
        locations
            .into_iter()
            .map(|location| {
                let check = inspect_location(&location);
                let result = scan(&location);
                (location, check, result)
            })
            .collect()
    })
    .await
}

impl State {
    pub(super) fn begin_location_checks(
        &mut self,
        generation: u64,
        locations: &[workspace::LibraryLocation],
    ) {
        self.location_checks.begin(generation, locations);
    }

    pub(super) fn finish_location_checks(
        &mut self,
        generation: u64,
        current: &[workspace::LibraryLocation],
        checks: Vec<(workspace::LibraryLocation, LocationCheck)>,
    ) -> bool {
        self.location_checks.finish(generation, current, checks)
    }

    /// (title, cancel_label, cancel_message) for the active operation kind.
    fn operation_kind(&self) -> (&'static str, &'static str, &'static str) {
        if self.relocation.is_some() {
            (
                "重新定位课程库",
                "取消重新定位",
                "正在结束核对，原保存位置保持不变…",
            )
        } else if self.association {
            (
                "重新关联保存位置",
                "取消重新关联",
                "正在结束读取，现有文件会保留…",
            )
        } else if self.cleanup {
            ("清理旧位置备份", "取消清理", "正在结束清理操作…")
        } else {
            ("移动课程库", "取消迁移", "正在结束迁移，原课程库会保留…")
        }
    }

    fn title(&self) -> &'static str {
        self.operation_kind().0
    }
}

struct StorageDialog {
    desktop: Entity<Desktop>,
    _observation: Subscription,
}
impl Render for StorageDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.desktop.update(cx, |desktop, cx| {
            div()
                .id("storage-dialog-content")
                .role(Role::Dialog)
                .aria_label(desktop.storage_ui.title())
                .child(desktop.storage_operation_view(window, cx))
        })
    }
}

impl Desktop {
    pub(super) fn registered_storage_locations(&self) -> Vec<workspace::LibraryLocation> {
        self.workspace
            .as_ref()
            .map(|workspace| workspace.state.libraries.clone())
            .unwrap_or_else(|| {
                // Without a workspace record the root has no registered id; an
                // empty id skips the marker identity check for this placeholder.
                vec![workspace::LibraryLocation {
                    id: String::new(),
                    name: "课程库".into(),
                    root: self.library_root.clone(),
                    previous_roots: Vec::new(),
                }]
            })
    }

    pub(super) fn cached_location_check(
        &self,
        location: &workspace::LibraryLocation,
    ) -> Option<&LocationCheck> {
        self.storage_ui.location_checks.get(location)
    }

    /// None means that at least one current path has not been checked yet. It
    /// must be shown as checking, never as an offline or empty library.
    pub(super) fn cached_library_access(&self) -> Option<storage::LibraryAccess> {
        let locations = self.registered_storage_locations();
        let mut access = storage::LibraryAccess {
            available: Vec::new(),
            unavailable: Vec::new(),
        };
        for location in &locations {
            let check = self.cached_location_check(location)?;
            if access.available.contains(&location.root)
                || access.unavailable.contains(&location.root)
            {
                continue;
            }
            if check.available {
                access.available.push(location.root.clone());
            } else {
                access.unavailable.push(location.root.clone());
            }
        }
        Some(access)
    }
}

fn relocate_source(source: &mut source::Source, old: &Path, new: &Path) {
    if !source.online {
        let mut path = PathBuf::from(&source.input);
        storage::relocate_path(&mut path, old, new);
        source.input = path.display().to_string();
    }
    if let Some(path) = &mut source.cover {
        storage::relocate_path(path, old, new);
    }
    if let Some(subtitle) = &mut source.selected_subtitle {
        storage::relocate_path(&mut subtitle.path, old, new);
    }
    fn track(track: &mut course2md::subtitle::SubtitleTrack, old: &Path, new: &Path) {
        if let course2md::subtitle::SubtitleOrigin::File { path } = &mut track.origin {
            storage::relocate_path(path, old, new);
            // Track identity refers to the original confirmed choice. Its content
            // is stored in the task; relocating a file does not reselect a track.
        }
    }
    if let course2md::subtitle::SubtitleEvidence::Found { tracks, .. } = &mut source.subtitles {
        for item in tracks {
            track(item, old, new);
        }
    }
    if let Some(item) = &mut source.subtitle_request {
        track(item, old, new);
    }
}

fn relocate_config(config: &mut course2md::settings::ConfigFile, old: &Path, new: &Path) {
    if let Some(path) = &mut config.defaults.out {
        storage::relocate_path(path, old, new);
    }
    if let Some(path) = &mut config.defaults.model_dir {
        storage::relocate_path(path, old, new);
    }
}

fn relocate_preview(preview: &mut crate::notes::Preview, old: &Path, new: &Path) {
    storage::relocate_path(&mut preview.course.dir, old, new);
    if let Some(path) = &mut preview.course.thumbnail {
        storage::relocate_path(path, old, new);
    }
    for path in &mut preview.frames {
        storage::relocate_path(path, old, new);
    }
    for block in &mut preview.blocks {
        if let crate::notes::PreviewBlock::Image(path) = block {
            storage::relocate_path(path, old, new);
        }
    }
}

fn validate_registered_destination(
    state: &workspace::State,
    library_id: &str,
    destination: &Path,
) -> Result<()> {
    let destination = std::fs::canonicalize(destination).context("目标位置暂时无法访问")?;
    ensure!(
        !state
            .libraries
            .iter()
            .filter(|library| library.id != library_id)
            .any(|library| {
                let root =
                    std::fs::canonicalize(&library.root).unwrap_or_else(|_| library.root.clone());
                root.starts_with(&destination) || destination.starts_with(root)
            }),
        "目标与另一个已登记课程库重叠，请选择独立的课程库位置"
    );
    Ok(())
}

/// Apply only after the destination copy is verified. Keeping the library ID
/// makes folder IDs, reading positions and all service bindings stable.
fn publish_location(
    state: &mut workspace::State,
    journal: &Journal,
    journal_path: &Path,
) -> Result<()> {
    ensure!(
        journal.phase == Phase::Verified,
        "目标副本尚未核验，不能切换保存位置"
    );
    let location = state
        .libraries
        .iter()
        .find(|library| library.id == journal.library_id)
        .context("原课程库已不在已登记位置中")?;
    ensure!(
        std::fs::canonicalize(&location.root)? == journal.source,
        "课程库的位置在迁移期间已经变化，尚未切换"
    );
    validate_registered_destination(state, &journal.library_id, &journal.destination)?;
    let roots = [journal.source.clone(), location.root.clone()];
    rebind_location(state, &journal.library_id, &roots, &journal.destination)?;
    state.storage_backups.push(storage::BackupRecord {
        id: journal.id.clone(),
        library_id: journal.library_id.clone(),
        path: journal.source.clone(),
        current_root: journal.destination.clone(),
        created: journal.created,
        verified: true,
        journal_path: journal_path.to_owned(),
    });
    Ok(())
}

/// A library move and locating an already moved library share the same registry
/// update. Only a verified copy operation creates a backup record.
fn rebind_location(
    state: &mut workspace::State,
    library_id: &str,
    roots: &[PathBuf],
    destination: &Path,
) -> Result<()> {
    ensure!(
        state.library(library_id).is_some(),
        "原课程库已不在已登记位置中"
    );
    for old in roots {
        for path in state.reader_sources.values_mut() {
            storage::relocate_path(path, old, destination);
        }
        for draft in &mut state.drafts {
            if let Some(source) = &mut draft.source {
                let before = source.input.clone();
                relocate_source(source, old, destination);
                if !source.online && before != source.input {
                    draft.input = source.input.clone();
                }
            } else if !draft.online {
                let mut path = PathBuf::from(&draft.input);
                storage::relocate_path(&mut path, old, destination);
                draft.input = path.display().to_string();
            }
            if let Some(path) = &mut draft.subtitle {
                storage::relocate_path(path, old, destination);
            }
            if let Some(config) = &mut draft.base_config {
                relocate_config(config, old, destination);
            }
        }
        for task in &mut state.tasks {
            storage::relocate_path(&mut task.work_dir, old, destination);
            if let Some(path) = &mut task.artifact {
                storage::relocate_path(path, old, destination);
            }
            if let Some(path) = &mut task.plan.subtitle {
                storage::relocate_path(path, old, destination);
            }
            relocate_source(&mut task.plan.source, old, destination);
            relocate_config(&mut task.plan.config, old, destination);
            if let course2md::execution::Operation::Reprocess {
                base_version_dir,
                prior_work_dir,
                ..
            } = &mut task.plan.operation
            {
                storage::relocate_path(base_version_dir, old, destination);
                if let Some(path) = prior_work_dir {
                    storage::relocate_path(path, old, destination);
                }
            }
        }
    }
    let location = state
        .libraries
        .iter_mut()
        .find(|library| library.id == library_id)
        .unwrap();
    for old in roots {
        if !location.previous_roots.contains(old) {
            location.previous_roots.push(old.clone());
        }
    }
    location.root = destination.to_owned();
    for backup in &mut state.storage_backups {
        if backup.library_id == library_id {
            backup.current_root = destination.to_owned();
        }
    }
    Ok(())
}

struct LocatedLibrary {
    library_id: String,
    previous_root: PathBuf,
    destination: PathBuf,
    stamp: storage::DirectoryStamp,
}

/// Read only: finding a moved library never creates or repairs its identity.
fn inspect_relocation(
    state: &workspace::State,
    library_id: &str,
    destination: &Path,
) -> Result<LocatedLibrary> {
    let location = state
        .library(library_id)
        .context("课程库已不在已登记位置中")?;
    let destination = destination
        .canonicalize()
        .context("所选文件夹暂时无法访问")?;
    ensure!(
        destination
            != location
                .root
                .canonicalize()
                .unwrap_or_else(|_| location.root.clone()),
        "所选仍是当前登记位置，请选择移动后的课程库文件夹"
    );
    validate_registered_destination(state, library_id, &destination)?;
    let stamp = storage::directory_stamp(&destination)?;
    let identity = std::fs::read_to_string(destination.join(".course2md-library-id"))
        .context("所选文件夹没有可读取的课程库关联记录，请选择移动后的完整课程库文件夹")?;
    ensure!(
        identity.trim() == library_id,
        "所选文件夹属于其他课程库，请选择这个课程库移动后的文件夹"
    );
    ensure!(
        storage::directory_stamp(&destination)? == stamp,
        "所选文件夹已变化，请重新选择"
    );
    Ok(LocatedLibrary {
        library_id: library_id.to_owned(),
        previous_root: location.root.clone(),
        destination,
        stamp,
    })
}

fn publish_relocation(state: &mut workspace::State, located: &LocatedLibrary) -> Result<()> {
    ensure!(
        state
            .library(&located.library_id)
            .is_some_and(|location| location.root == located.previous_root),
        "这个课程库的登记位置已变化，请重新选择"
    );
    // Recheck the actual directory and identity immediately before publication.
    let current = inspect_relocation(state, &located.library_id, &located.destination)?;
    ensure!(
        current.stamp == located.stamp,
        "所选文件夹已被替换，请重新选择"
    );
    rebind_location(
        state,
        &located.library_id,
        std::slice::from_ref(&located.previous_root),
        &located.destination,
    )
}

impl Desktop {
    pub fn begin_library_relocation(
        &mut self,
        library_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.storage_ui.busy {
            return;
        }
        self.storage_ui.relocation = Some(library_id.clone());
        self.storage_ui.association = false;
        self.storage_ui.cleanup = false;
        self.storage_ui.error = None;
        self.storage_ui.progress = Progress::default();
        if self.job.is_some() || self.preview_workers > 0 {
            self.storage_ui.error = Some(
                "正在读取或生成内容，请等待结束或停止后再重新定位。原保存位置保持不变。".into(),
            );
            self.open_storage_dialog(window, cx);
            return;
        }
        if !self.save_current_draft(cx) {
            return;
        }
        let Some(state) = self
            .workspace
            .as_ref()
            .map(|workspace| workspace.state.clone())
        else {
            return;
        };
        self.storage_ui.busy = true;
        self.storage_ui.generation = self.storage_ui.generation.wrapping_add(1);
        let generation = self.storage_ui.generation;
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("选择这个课程库移动后的文件夹".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let answer = prompt.await;
            let destination = match answer {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                Ok(Ok(None)) => None,
                error => {
                    let message = match error {
                        Ok(Err(error)) => format!("无法选择课程库位置：{error:#}"),
                        Err(error) => format!("无法打开文件选择器：{error}"),
                        _ => unreachable!(),
                    };
                    let _ = this.update_in(cx, |this, window, cx| {
                        this.storage_ui.busy = false;
                        this.storage_ui.error = Some(message);
                        this.open_storage_dialog(window, cx);
                        cx.notify();
                    });
                    return;
                }
            };
            let Some(destination) = destination else {
                let _ = this.update_in(cx, |this, _, cx| {
                    this.storage_ui.busy = false;
                    this.storage_ui.relocation = None;
                    this.start_next_task(cx);
                    cx.notify();
                });
                return;
            };
            let cancel = Arc::new(AtomicBool::new(false));
            let _ = this.update_in(cx, |this, window, cx| {
                this.storage_ui.cancel = Some(cancel.clone());
                this.storage_ui.progress.message = "正在核对所选课程库…".into();
                this.open_storage_dialog(window, cx);
                cx.notify();
            });
            let work = crate::spawn_blocking_io(move || {
                inspect_relocation(&state, &library_id, &destination)
            });
            let result = work
                .recv()
                .await
                .unwrap_or_else(|_| Err(anyhow::anyhow!("核对课程库的工作线程意外结束")));
            let _ = this.update_in(cx, |this, window, cx| {
                if this.storage_ui.generation != generation {
                    return;
                }
                let result = result.and_then(|located| {
                    ensure!(!this.closing && !cancel.load(Ordering::Relaxed), "已取消重新定位，原保存位置保持不变");
                    ensure!(this.job.is_none() && this.preview_workers == 0, "内容仍在处理中，请停止后重新定位");
                    this.workspace.as_mut().context("课程库登记记录暂时不可用")?
                        .transaction(|state| publish_relocation(state, &located))?;
                    Ok(located)
                });
                this.storage_ui.busy = false;
                this.storage_ui.cancel = None;
                this.storage_ui.progress = Progress::default();
                match result {
                    Ok(located) => {
                        if this.library_root == located.previous_root {
                            this.library_root = located.destination.clone();
                        }
                        this.read_generation = this.read_generation.wrapping_add(1);
                        this.reading = false;
                        if let Some(preview) = &mut this.preview {
                            let was_inside = preview.course.dir.starts_with(&located.previous_root);
                            relocate_preview(preview, &located.previous_root, &located.destination);
                            if was_inside {
                                this.restore_reading_position(cx);
                            }
                        }
                        let settings = this.relocate_settings_paths(&located.previous_root, &located.destination, cx);
                        this.storage_ui.error = settings.err().map(|error| format!("课程库已重新定位，模型位置尚未保存。请在生成笔记设置中重试保存：{error:#}"));
                        this.storage_ui.progress = Progress {
                            message: format!("已找到课程库：{}。已有文件保留在所选位置。", located.destination.display()),
                            completed: 1,
                            total: 1,
                        };
                        this.workspace_error = None;
                        this.restore_draft(window, cx);
                        this.refresh_library(cx);
                        if this.storage_ui.error.is_none() {
                            window.close_dialog(cx);
                        }
                    }
                    Err(error) => {
                        this.storage_ui.error = Some(format!("尚未重新定位：{error:#}。原登记和文件保持不变。"));
                    }
                }
                this.start_next_task(cx);
                cx.notify();
            });
        }).detach();
        cx.notify();
    }

    pub fn begin_library_reassociation(
        &mut self,
        library_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.storage_ui.busy {
            return;
        }
        self.storage_ui.relocation = None;
        let Some(location) = self
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.state.library(&library_id))
            .cloned()
        else {
            return;
        };
        let courses = self
            .courses
            .iter()
            .filter(|course| {
                self.course_location(course)
                    .is_some_and(|library| library.id == library_id)
            })
            .cloned()
            .collect::<Vec<_>>();
        self.storage_ui.busy = true;
        self.storage_ui.association = true;
        self.storage_ui.cleanup = false;
        self.storage_ui.error = None;
        self.storage_ui.progress = Progress {
            message: "正在重新读取这个位置中的笔记…".into(),
            ..Default::default()
        };
        let cancel = Arc::new(AtomicBool::new(false));
        self.storage_ui.cancel = Some(cancel.clone());
        self.open_storage_dialog(window, cx);
        let root = location.root.clone();
        let worker_cancel = cancel.clone();
        // 扫描会同步读取磁盘上的全部笔记；见 crate::spawn_blocking_io 的说明
        let worker = crate::spawn_blocking_io(move || {
            let stamp = storage::directory_stamp(&root)?;
            let mut names = Vec::new();
            let mut unreadable = 0;
            for course in courses {
                ensure!(
                    !worker_cancel.load(Ordering::Relaxed),
                    "已取消重新关联，原文件保持完整。"
                );
                let title = course.title.clone();
                if crate::notes::read_preview(course).is_ok() {
                    names.push(title);
                } else {
                    unreadable += 1;
                }
            }
            ensure!(
                storage::directory_stamp(&root)? == stamp,
                "保存位置在读取期间已经变化，请重新检查。"
            );
            ensure!(
                !worker_cancel.load(Ordering::Relaxed),
                "已取消重新关联，原文件保持完整。"
            );
            Ok::<_, anyhow::Error>((stamp, names, unreadable))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = worker
                .recv()
                .await
                .unwrap_or_else(|_| Err(anyhow::anyhow!("重新关联的工作线程意外结束")));
            let Ok((stamp, names, unreadable)) = result else {
                let message = result.unwrap_err().to_string();
                let _ = this.update_in(cx, |this, _, cx| {
                    this.storage_ui.busy = false;
                    this.storage_ui.cancel = None;
                    this.storage_ui.progress = Progress::default();
                    this.storage_ui.error = Some(message);
                    this.start_next_task(cx);
                    cx.notify();
                });
                return;
            };
            let mut detail = format!("保存位置：{}\n\n", location.root.display());
            if names.is_empty() {
                detail.push_str("目前未读到已列入课程库的笔记。现有文件会全部保留。\n");
            } else {
                detail.push_str(&format!(
                    "目前可读取 {} 份笔记：\n{}",
                    names.len(),
                    names
                        .iter()
                        .take(5)
                        .map(|name| format!("• {name}"))
                        .collect::<Vec<_>>()
                        .join("\n")
                ));
                if names.len() > 5 {
                    detail.push_str(&format!("\n以及另外 {} 份笔记。", names.len() - 5));
                }
                detail.push('\n');
            }
            if unreadable > 0 {
                detail.push_str(&format!(
                    "另有 {unreadable} 份已列出的笔记暂时无法读取，文件会保留。\n"
                ));
            }
            detail
                .push_str("\n关联后继续使用这个文件夹，现有笔记、原视频、当前输入和任务都会保留。");
            let answer = this.update_in(cx, |this, window, cx| {
                this.storage_ui.cancel = None;
                window.close_dialog(cx);
                window.prompt(
                    PromptLevel::Info,
                    "重新关联此保存位置？",
                    Some(&detail),
                    &["重新关联", "取消"],
                    cx,
                )
            });
            let Ok(answer) = answer else {
                return;
            };
            let accepted = answer.await.ok() == Some(0);
            let _ = this.update_in(cx, |this, window, cx| {
                this.storage_ui.busy = false;
                this.storage_ui.progress = Progress::default();
                this.storage_ui.cancel = None;
                if !accepted {
                    this.start_next_task(cx);
                    cx.notify();
                    return;
                }
                let result = (|| -> Result<()> {
                    let workspace = this
                        .workspace
                        .as_mut()
                        .context("课程库登记记录暂时不可用")?;
                    let current = workspace
                        .state
                        .library(&library_id)
                        .context("课程库已不在登记位置中")?;
                    ensure!(
                        current.root == location.root
                            && storage::directory_stamp(&current.root)? == stamp,
                        "保存位置在确认期间已经变化，尚未重新关联。请重新检查这个位置。"
                    );
                    workspace.reassociate_library(&library_id)
                })();
                match result {
                    Ok(()) => {
                        this.storage_ui.error = None;
                        this.message = Some(format!(
                            "已重新关联「{}」。已有笔记保持完整，可以继续原任务。",
                            location.name
                        ));
                    }
                    Err(error) => {
                        this.storage_ui.error = Some(format!("尚未重新关联：{error:#}"));
                        this.open_storage_dialog(window, cx);
                    }
                }
                this.refresh_library(cx);
                this.start_next_task(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub fn restore_storage_state(&mut self, cx: &mut Context<Self>) {
        // 半残/损坏的迁移记录不能静默消失：在存储页给出可见错误与文件位置
        let (pending, corrupt) =
            storage::scan_journals(&self.preferences.root().join("storage"));
        self.storage_ui.pending = pending;
        if !corrupt.is_empty() {
            self.storage_ui.error = Some(format!(
                "{} 个迁移记录无法读取（原文件已保留，可手动检查或删除）：{}",
                corrupt.len(),
                corrupt
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join("、")
            ));
        }
        // If the registry transaction committed before the app closed, the
        // journal is merely behind; never offer to replay or undo that copy.
        self.storage_ui.pending.retain(|(path, journal)| {
            let committed = self.workspace.as_ref().is_some_and(|workspace| {
                workspace
                    .state
                    .library(&journal.library_id)
                    .is_some_and(|library| library.root == journal.destination)
            });
            if committed {
                let mut journal = journal.clone();
                journal.phase = Phase::Committed;
                let _ = storage::save_journal(path, &journal);
                false
            } else {
                true
            }
        });
        let pending_settings: Vec<_> = self
            .workspace
            .as_ref()
            .into_iter()
            .flat_map(|workspace| &workspace.state.storage_backups)
            .filter_map(|backup| {
                std::fs::read(&backup.journal_path)
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<Journal>(&bytes).ok())
                    .filter(|journal| {
                        !journal.settings_relocated
                            && journal.id == backup.id
                            && journal.source == backup.path
                            && journal.library_id == backup.library_id
                    })
                    .map(|journal| (backup.journal_path.clone(), journal))
            })
            .collect();
        for (path, mut journal) in pending_settings {
            match self.relocate_settings_paths(&journal.source, &journal.destination, cx) {
                Ok(()) => {
                    journal.settings_relocated = true;
                    if let Err(error) = storage::save_journal(&path, &journal) {
                        self.storage_ui.error =
                            Some(format!("模型位置已更新，迁移记录尚未保存：{error:#}"));
                    }
                }
                Err(error) => {
                    self.storage_ui.error = Some(format!(
                        "课程库已移动，模型位置尚未保存。旧位置备份已保留：{error:#}"
                    ))
                }
            }
        }
        if let Some(workspace) = &mut self.workspace {
            let completed: Vec<_> = workspace
                .state
                .storage_backups
                .iter()
                .filter(|backup| {
                    std::fs::read(&backup.journal_path)
                        .ok()
                        .and_then(|bytes| serde_json::from_slice::<Journal>(&bytes).ok())
                        .is_some_and(|journal| {
                            journal.cleanup_complete
                                && journal.id == backup.id
                                && journal.source == backup.path
                                && journal.library_id == backup.library_id
                        })
                })
                .map(|backup| backup.id.clone())
                .collect();
            if !completed.is_empty() {
                if let Err(error) = workspace.transaction(|state| {
                    state
                        .storage_backups
                        .retain(|backup| !completed.contains(&backup.id));
                    Ok(())
                }) {
                    self.storage_ui.error =
                        Some(format!("备份已清理，界面记录尚未保存：{error:#}"));
                }
            }
        }
        cx.notify();
    }

    pub fn poll_storage(&mut self, cx: &mut Context<Self>) {
        if !self.storage_ui.busy && self.job.is_none() {
            if let Some(id) = self.storage_ui.resume_task.take() {
                let can_resume = self
                    .workspace
                    .as_ref()
                    .and_then(|workspace| workspace.state.task(&id))
                    .is_some_and(|task| {
                        task.state == workspace::TaskState::Paused
                            && task.intent == workspace::Intent::Pause
                    });
                if can_resume {
                    self.set_task_intent(id, workspace::Intent::Run, cx);
                }
            }
        }
    }

    pub fn cancel_storage_for_close(&mut self) {
        if let Some(cancel) = &self.storage_ui.cancel {
            cancel.store(true, Ordering::Relaxed);
        }
    }

    pub fn begin_library_move(
        &mut self,
        library_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.storage_ui.busy {
            return;
        }
        self.storage_ui.relocation = None;
        self.storage_ui.association = false;
        let Some(location) = self
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.state.library(&library_id))
            .cloned()
        else {
            return;
        };
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("选择空文件夹作为课程库的新位置".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let answer = prompt.await;
            let _ = this.update_in(cx, |this, window, cx| {
                match answer {
                    Ok(Ok(Some(paths))) => {
                        if let Some(destination) = paths.into_iter().next() {
                            match storage::validate_destination(&location.root, &destination) {
                                Ok((source, destination)) => this.start_library_move(
                                    location.id.clone(),
                                    source,
                                    destination,
                                    None,
                                    window,
                                    cx,
                                ),
                                Err(error) => {
                                    this.storage_ui.error = Some(format!("{error:#}"));
                                    this.storage_ui.cleanup = false;
                                    this.storage_ui.progress = Progress::default();
                                    this.open_storage_dialog(window, cx);
                                }
                            }
                        }
                    }
                    Ok(Ok(None)) => {}
                    Ok(Err(error)) => {
                        this.storage_ui.error = Some(format!("无法选择保存位置：{error:#}"))
                    }
                    Err(error) => this.storage_ui.error = Some(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn start_library_move(
        &mut self,
        library_id: String,
        source: PathBuf,
        destination: PathBuf,
        resume: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.storage_ui.busy || !self.save_current_draft(cx) {
            return;
        }
        self.storage_ui.relocation = None;
        self.storage_ui.association = false;
        let valid = self
            .workspace
            .as_ref()
            .context("课程库登记记录暂时不可用")
            .and_then(|workspace| {
                validate_registered_destination(&workspace.state, &library_id, &destination)
            });
        if let Err(error) = valid {
            self.storage_ui.error = Some(format!("{error:#}"));
            self.storage_ui.progress = Progress::default();
            self.storage_ui.cleanup = false;
            self.open_storage_dialog(window, cx);
            return;
        }
        self.storage_ui.busy = true;
        self.storage_ui.cleanup = false;
        self.storage_ui.generation = self.storage_ui.generation.wrapping_add(1);
        let generation = self.storage_ui.generation;
        self.storage_ui.error = None;
        self.storage_ui.progress = Progress {
            message: "正在保存当前任务进度，完成后开始复制…".into(),
            completed: 0,
            total: 0,
        };
        let cancel = Arc::new(AtomicBool::new(false));
        self.storage_ui.cancel = Some(cancel.clone());
        if let Some(id) = self.active_task.clone() {
            let running = self
                .workspace
                .as_ref()
                .and_then(|workspace| workspace.state.task(&id))
                .is_some_and(|task| task.intent == workspace::Intent::Run);
            if running {
                self.storage_ui.resume_task = Some(id.clone());
                self.set_task_intent(id, workspace::Intent::Pause, cx);
            }
        } else if let Some(job) = &self.job {
            job.cancel();
        }
        self.open_storage_dialog(window, cx);
        let journal_dir = self.preferences.root().join("storage");
        cx.spawn_in(window, async move |this, cx| {
            let waiting = Instant::now();
            loop {
                if cancel.load(Ordering::Relaxed) || waiting.elapsed() > Duration::from_secs(30) {
                    let message = if cancel.load(Ordering::Relaxed) {
                        "已取消迁移，原保存位置未改变。"
                    } else {
                        "当前任务还在保存进度，迁移尚未开始。任务停止后可重新移动课程库。"
                    };
                    let _ = this.update_in(cx, |this, _, cx| {
                        this.finish_storage_error(message.into(), cx)
                    });
                    return;
                }
                let ready = this
                    .update_in(cx, |this, _, _| this.job.is_none())
                    .unwrap_or(false);
                if ready {
                    break;
                }
                smol::Timer::after(Duration::from_millis(100)).await;
            }
            let progress = Arc::new(Mutex::new(Progress::default()));
            let worker_progress = progress.clone();
            let worker_cancel = cancel.clone();
            // 迁移是大量同步文件读写；见 crate::spawn_blocking_io 的说明
            let worker = crate::spawn_blocking_io(move || {
                let report = |value| {
                    if let Ok(mut progress) = worker_progress.lock() {
                        *progress = value;
                    }
                };
                match resume {
                    Some(path) => storage::resume_move(&path, &worker_cancel, &report),
                    None => storage::prepare_move(
                        library_id,
                        &source,
                        &destination,
                        &journal_dir,
                        &worker_cancel,
                        &report,
                    ),
                }
            });
            let result = loop {
                match worker.try_recv() {
                    Ok(result) => break result,
                    Err(smol::channel::TryRecvError::Closed) => {
                        break Err(anyhow::anyhow!("迁移工作线程意外结束"));
                    }
                    Err(smol::channel::TryRecvError::Empty) => {}
                }
                let snapshot = progress.lock().ok().map(|value| value.clone());
                if let Some(snapshot) = snapshot {
                    let _ = this.update_in(cx, |this, _, cx| {
                        this.storage_ui.progress = snapshot;
                        cx.notify();
                    });
                }
                smol::Timer::after(Duration::from_millis(100)).await;
            };
            let final_progress = progress
                .lock()
                .ok()
                .map(|value| value.clone())
                .unwrap_or_default();
            let _ = this.update_in(cx, |this, window, cx| {
                if this.storage_ui.generation != generation {
                    return;
                }
                this.storage_ui.progress = final_progress;
                match result {
                    Ok(mut prepared) => {
                        if this.closing || cancel.load(Ordering::Relaxed) {
                            this.finish_storage_error(
                                "迁移副本已核验，保存位置尚未切换，原文件未删除。".into(),
                                cx,
                            );
                            return;
                        }
                        let committed = this
                            .workspace
                            .as_mut()
                            .context("课程库登记记录暂时不可用")
                            .and_then(|workspace| {
                                workspace.transaction(|state| {
                                    publish_location(state, &prepared.journal, &prepared.path)
                                })
                            });
                        if let Err(error) = committed {
                            this.finish_storage_error(
                                format!("副本已核验，保存位置尚未切换：{error:#}"),
                                cx,
                            );
                            return;
                        }
                        if this.library_root == prepared.journal.source {
                            this.library_root = prepared.journal.destination.clone();
                        }
                        prepared.journal.phase = Phase::Committed;
                        this.storage_ui.error = None;
                        match this.relocate_settings_paths(
                            &prepared.journal.source,
                            &prepared.journal.destination,
                            cx,
                        ) {
                            Ok(()) => prepared.journal.settings_relocated = true,
                            Err(error) => {
                                this.storage_ui.error = Some(format!(
                                    "课程库已移动，模型位置尚未保存。旧位置备份已保留：{error:#}"
                                ))
                            }
                        }
                        if let Err(error) = storage::save_journal(&prepared.path, &prepared.journal)
                        {
                            this.message =
                                Some(format!("课程库已移动，迁移记录稍后重试保存：{error:#}"));
                        }
                        this.storage_ui.busy = false;
                        this.storage_ui.cancel = None;
                        this.storage_ui.progress = Progress {
                            message: format!(
                                "课程库已移动到 {}。旧位置保留为已核验备份。",
                                prepared.journal.destination.display()
                            ),
                            completed: 1,
                            total: 1,
                        };
                        this.restore_storage_state(cx);
                        this.restore_draft(window, cx);
                        this.refresh_library(cx);
                        this.poll_storage(cx);
                        this.start_next_task(cx);
                    }
                    Err(error) => this.finish_storage_error(
                        format!("{error:#}。保存位置尚未切换，已复制的内容保留在目标位置。"),
                        cx,
                    ),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn finish_storage_error(&mut self, message: String, cx: &mut Context<Self>) {
        self.storage_ui.busy = false;
        self.storage_ui.cancel = None;
        self.storage_ui.error = Some(message);
        self.restore_storage_state(cx);
        self.refresh_library(cx);
        self.poll_storage(cx);
        self.start_next_task(cx);
        cx.notify();
    }

    fn open_storage_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let desktop = cx.entity();
        let content = cx.new(|cx| StorageDialog {
            _observation: cx.observe(&desktop, |_, _, cx| cx.notify()),
            desktop,
        });
        let title = self.storage_ui.title();
        window.open_dialog(cx, move |dialog, _, _| {
            dialog
                .title(title)
                .w(px(540.))
                .overlay_closable(false)
                .close_button(false)
                .keyboard(false)
                .child(content.clone())
        });
    }

    fn storage_operation_view(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let cancelling = self
            .storage_ui
            .cancel
            .as_ref()
            .is_some_and(|cancel| cancel.load(Ordering::Relaxed));
        let mut view = v_flex().gap_4().child(
            h_flex()
                .gap_2()
                .items_center()
                .when(self.storage_ui.busy, |row| {
                    row.child(crate::motion::spinner("storage-operation-busy", cx))
                })
                .when(!self.storage_ui.busy, |row| {
                    row.child(
                        if self.storage_ui.error.is_some() {
                            icons::warning()
                        } else {
                            icons::check_circle()
                        }
                        .size_6()
                        .text_color(color(
                            if self.storage_ui.error.is_some() {
                                WARNING
                            } else {
                                SUCCESS
                            },
                        )),
                    )
                })
                .child(
                    accessible_text(
                        "storage-operation-heading",
                        if cancelling {
                            "正在结束操作…"
                        } else if self.storage_ui.busy {
                            "正在处理文件…"
                        } else if self.storage_ui.error.is_some() {
                            "操作未完成"
                        } else {
                            "操作已完成"
                        },
                    )
                    .font_weight(FontWeight::SEMIBOLD),
                ),
        );
        if !self.storage_ui.progress.message.is_empty() {
            view = view.child(accessible_text(
                "storage-operation-state",
                self.storage_ui.progress.message.clone(),
            ));
        }
        if let Some(error) = &self.storage_ui.error {
            view = view.child(
                accessible_text("storage-operation-error", error.clone()).text_color(color(DANGER)),
            );
        }
        if self.storage_ui.busy {
            let progress = &self.storage_ui.progress;
            if progress.total > 0 {
                view = view.child(crate::motion::progress(
                    "storage-progress",
                    progress.completed as f32 / progress.total as f32,
                    window,
                    cx,
                ));
            }
            view = view.child(
                control("cancel-library-move")
                    .icon(icons::close())
                    .disabled(cancelling)
                    .loading(cancelling)
                    .label(if cancelling {
                        "正在结束…"
                    } else {
                        self.storage_ui.operation_kind().1
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(cancel) = &this.storage_ui.cancel {
                            cancel.store(true, Ordering::Relaxed);
                        }
                        this.storage_ui.progress.message =
                            this.storage_ui.operation_kind().2.into();
                        cx.notify();
                    })),
            );
        } else {
            if let Some(id) = self.storage_ui.relocation.clone().filter(|_| {
                self.storage_ui.error.is_some() && self.storage_ui.progress.completed == 0
            }) {
                view = view.child(
                    primary_pill("retry-library-relocation")
                        .label("重新选择课程库…")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            window.close_dialog(cx);
                            this.begin_library_relocation(id.clone(), window, cx);
                        })),
                );
            }
            view = view.child(
                control("close-storage-operation")
                    .icon(icons::close())
                    .label("关闭")
                    .on_click(|_, window, cx| window.close_dialog(cx)),
            );
        }
        view
    }

    pub fn storage_status_panel(&self, cx: &mut Context<Self>) -> Div {
        let mut view = v_flex().w_full().min_w_0().flex_shrink_0().gap_3();
        let mut has_content = false;
        if self.storage_ui.relocation.is_some()
            && !self.storage_ui.busy
            && self.storage_ui.error.is_none()
            && self.storage_ui.progress.completed > 0
            && self.storage_ui.progress.completed == self.storage_ui.progress.total
        {
            has_content = true;
            view = view.child(
                h_flex()
                    .gap_2()
                    .items_start()
                    .child(
                        icons::check_circle()
                            .size_4()
                            .flex_shrink_0()
                            .text_color(color(SUCCESS)),
                    )
                    .child(
                        accessible_text(
                            "storage-relocation-success",
                            self.storage_ui.progress.message.clone(),
                        )
                        .flex_1()
                        .min_w_0()
                        .whitespace_normal()
                        .text_sm(),
                    ),
            );
        }
        if let Some(access) = self.cached_library_access() {
            let message = match access.coverage() {
                storage::LibraryCoverage::Unavailable => Some(
                    "已登记的保存位置暂时都无法访问，笔记内容尚未读取；当前输入和任务记录仍保留。"
                        .into(),
                ),
                storage::LibraryCoverage::Partial => Some(format!(
                    "目前 {} 个保存位置可访问，{} 个暂时无法访问。课程库只显示和搜索可访问位置中的笔记。",
                    access.available.len(),
                    access.unavailable.len()
                )),
                storage::LibraryCoverage::Complete => None,
            };
            if let Some(message) = message {
                has_content = true;
                view = view.child(
                    accessible_text("storage-access-coverage", message)
                        .text_sm()
                        .text_color(color(MUTED)),
                );
            }
        } else {
            has_content = true;
            view = view.child(
                h_flex()
                    .gap_2()
                    .child(crate::motion::spinner("storage-locations-checking", cx))
                    .child(
                        accessible_text("storage-locations-checking-label", "正在检查保存位置…")
                            .text_sm(),
                    ),
            );
        }
        if let Some(error) = &self.storage_ui.error {
            has_content = true;
            view = view.child(
                accessible_text("storage-status-error", error.clone())
                    .text_sm()
                    .text_color(color(DANGER)),
            );
        }
        if !self.storage_ui.pending.is_empty() {
            view = view.child(semantic_label(
                "storage-pending-heading",
                "未完成迁移",
                icons::storage(),
            ));
        }
        for (index, (path, journal)) in self.storage_ui.pending.iter().enumerate() {
            has_content = true;
            let path = path.clone();
            let abandoned_path = path.clone();
            let journal = journal.clone();
            let destination = journal.destination.clone();
            view = view.child(
                card_surface()
                    .gap_2()
                    .min_w_0()
                    .child(accessible_text(
                        ("storage-pending-description", index),
                        format!(
                            "尚未完成迁移：{} → {}",
                            journal.source.display(),
                            journal.destination.display()
                        ),
                    ))
                    .child(
                        control(("resume-library-move", index))
                            .icon(icons::play_arrow())
                            .label("继续迁移并切换位置")
                            .disabled(self.storage_ui.busy)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.start_library_move(
                                    journal.library_id.clone(),
                                    journal.source.clone(),
                                    journal.destination.clone(),
                                    Some(path.clone()),
                                    window,
                                    cx,
                                )
                            })),
                    )
                    .child(
                        control(("open-move-copy", index))
                            .ghost()
                            .icon(icons::folder_open())
                            .label("打开目标副本")
                            .on_click(move |_, _, cx| cx.open_with_system(&destination)),
                    )
                    .child(
                        control(("abandon-library-move", index))
                            .ghost()
                            .icon(icons::close())
                            .label("结束这次迁移，保留副本")
                            .disabled(self.storage_ui.busy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let result = std::fs::read(&abandoned_path)
                                    .map_err(anyhow::Error::from)
                                    .and_then(|bytes| {
                                        serde_json::from_slice::<Journal>(&bytes)
                                            .map_err(anyhow::Error::from)
                                    })
                                    .and_then(|mut journal| {
                                        journal.phase = Phase::Abandoned;
                                        storage::save_journal(&abandoned_path, &journal)
                                    });
                                if let Err(error) = result {
                                    this.storage_ui.error =
                                        Some(format!("尚未保存迁移选择：{error:#}"));
                                }
                                this.restore_storage_state(cx);
                            })),
                    ),
            );
        }
        if let Some(workspace) = &self.workspace {
            if !workspace.state.storage_backups.is_empty() {
                view = view.child(semantic_label(
                    "storage-backups-heading",
                    "旧位置备份",
                    icons::folder_open(),
                ));
            }
            for (index, backup) in workspace.state.storage_backups.iter().enumerate() {
                has_content = true;
                let id = backup.id.clone();
                let path = backup.path.clone();
                view = view.child(
                    card_surface()
                        .gap_2()
                        .min_w_0()
                        .child(accessible_text(
                            ("storage-backup-description", index),
                            format!("旧位置备份：{}", backup.path.display()),
                        ))
                        .child(
                            h_flex()
                                .gap_2()
                                .flex_wrap()
                                .child(
                                    control(("open-library-backup", index))
                                        .icon(icons::folder_open())
                                        .label("打开备份位置")
                                        .on_click(move |_, _, cx| cx.open_with_system(&path)),
                                )
                                .child(
                                    quiet(("cleanup-library-backup", index))
                                        .icon(icons::delete())
                                        .label("清理旧位置备份…")
                                        .disabled(self.storage_ui.busy)
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.begin_backup_cleanup(id.clone(), window, cx)
                                        })),
                                ),
                        ),
                );
            }
        }
        // An empty flex child still contributes a gap in the settings column.
        // Healthy storage has no status row and must consume no layout slot.
        view.when(!has_content, |view| view.hidden())
    }


    /// Storage locations listed in the settings page; per-location operations
    /// live in this module alongside the migration flow.
    pub(super) fn storage_settings_page(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut view = v_flex().w_full().min_w_0().flex_shrink_0().gap_6().child(
            h_flex()
                .w_full()
                .min_w_0()
                .gap_3()
                .items_start()
                .flex_wrap()
                .child(
                    v_flex()
                        .min_w_0()
                        .gap_2()
                        .child(semantic_label(
                            "storage-locations-heading",
                            "保存位置",
                            icons::folder_open(),
                        ))
                        .child(theme::supporting_info(
                            "storage-policy",
                            "更改默认位置只影响后续生成的笔记。",
                        ))
                        .flex_1(),
                )
                .child(
                    // 与笔记库同一模式：页标题行尾的有标签刷新动作（review2-settings#4）
                    quiet("refresh-storage-locations")
                        .icon(icons::refresh())
                        .label("刷新")
                        .loading(self.loading)
                        .disabled(self.loading)
                        .on_click(cx.listener(|this, _, _, cx| this.refresh_library(cx))),
                ),
        );
        view = view.child(self.storage_status_panel(cx));
        if let Some(workspace) = &self.workspace {
            for (index, library) in workspace.state.libraries.iter().enumerate() {
                let root = library.root.clone();
                let id = library.id.clone();
                let move_id = id.clone();
                let relocate_id = id.clone();
                let reassociate_id = id.clone();
                let default = workspace.state.default_library == id;
                let check = self.cached_location_check(library);
                let offline = check.is_some_and(|check| !check.available);
                let needs_relocation =
                    check.is_some_and(|check| !check.available || check.problem.is_some());
                let needs_reassociation = check.is_some_and(|check| check.needs_reassociation);
                view = view.child(
                    card_surface()
                        .min_w_0()
                        .gap_3()
                        .child(
                            h_flex().w_full().min_w_0().gap_2().items_center().flex_wrap()
                                .child(semantic_label(("storage-location-name", index), library.name.clone(), icons::folder_open()).flex_1().min_w_0())
                                .when(default, |row| row.child(badge(BadgeKind::Neutral).child("默认保存位置")))
                                .when(offline, |row| row.child(badge(BadgeKind::Warning).child("暂时不可访问"))),
                        )
                        .child(text(("storage-location-path", index), root.display().to_string()).w_full().min_w_0().whitespace_normal().text_size(TEXT_AUX).text_color(color(MUTED)))
                        .when_some(check.and_then(|check| check.problem.clone()), |card, problem| {
                            card.child(text(("storage-location-problem", index), problem).text_size(TEXT_AUX).text_color(color(DANGER)))
                        })
                        .when(needs_reassociation, |card| {
                            card.child(text(("storage-association-needed", index), "关联记录缺失。重新关联后可继续使用，已有文件会保留。").text_size(TEXT_AUX).text_color(color(WARNING)))
                        })
                        .when(needs_relocation, |card| {
                            card.child(text(("storage-relocation-help", index), "文件夹已移动时，请重新定位到它现在的位置。").text_size(TEXT_AUX).text_color(color(MUTED)))
                        })
                        .child(
                            h_flex().w_full().min_w_0().gap_2().justify_end().flex_wrap()
                                .child(quiet(("open-storage-location", index)).icon(icons::folder_open()).label("打开位置").disabled(offline).on_click(move |_, _, cx| cx.open_with_system(&root)))
                                .when(!default, |row| {
                                    row.child(outline_pill(("default-storage-location", index)).icon(icons::check_circle()).label("设为默认").disabled(needs_relocation || needs_reassociation || self.storage_ui.busy)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if let Some(workspace) = &mut this.workspace {
                                                let result = workspace.state.library(&id)
                                                    .ok_or_else(|| anyhow::anyhow!("此位置已不在课程库中"))
                                                    .and_then(|library| tempfile::NamedTempFile::new_in(&library.root).map(drop).context("此位置暂时不能写入，请重新连接磁盘或恢复文件夹访问权限"))
                                                    .and_then(|_| workspace.transaction(|state| { state.default_library = id.clone(); Ok(()) }));
                                                if let Err(error) = result {
                                                    this.workspace_error = Some(format!("默认位置尚未更改：{error:#}"));
                                                } else {
                                                    this.message = Some("后续生成的笔记将保存在此位置，已有笔记和任务保持原位置".into());
                                                }
                                            }
                                            cx.notify();
                                        })))
                                })
                                .when(!needs_relocation && !needs_reassociation, |row| {
                                    row.child(outline_pill(("move-storage-location", index)).icon(icons::storage()).label("移动课程库…").disabled(self.storage_ui.busy)
                                        .on_click(cx.listener(move |this, _, window, cx| this.begin_library_move(move_id.clone(), window, cx))))
                                })
                                .when(needs_reassociation, |row| {
                                    row.child(outline_pill(("reassociate-storage-location", index)).icon(icons::storage()).label("重新关联…").disabled(self.storage_ui.busy)
                                        .on_click(cx.listener(move |this, _, window, cx| this.begin_library_reassociation(reassociate_id.clone(), window, cx))))
                                })
                                .when(needs_relocation, |row| {
                                    row.child(outline_pill(("relocate-storage-location", index)).icon(icons::folder_open()).label("重新定位…").disabled(self.storage_ui.busy)
                                        .on_click(cx.listener(move |this, _, window, cx| this.begin_library_relocation(relocate_id.clone(), window, cx))))
                                }),
                        ),
                );
            }
        }
        view = view.child(
            outline_pill("register-storage-location")
                .icon(icons::add())
                .label("添加课程库位置")
                .self_start()
                .on_click(
                    cx.listener(|this, _, window, cx| this.register_storage_location(window, cx)),
                ),
        );
        view.into_any_element()
    }


    fn begin_backup_cleanup(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.storage_ui.busy {
            return;
        }
        self.storage_ui.relocation = None;
        self.storage_ui.association = false;
        let Some(backup) = self
            .workspace
            .as_ref()
            .and_then(|workspace| {
                workspace
                    .state
                    .storage_backups
                    .iter()
                    .find(|backup| backup.id == id)
            })
            .cloned()
        else {
            return;
        };
        if self.preferences.references_storage_path(&backup.path) {
            self.storage_ui.cleanup = true;
            self.storage_ui.progress = Progress::default();
            self.storage_ui.error = Some("识别模型仍引用这个旧位置，备份已保留。请先在“生成笔记”设置中完成模型位置的保存，再清理备份。".into());
            self.open_storage_dialog(window, cx);
            return;
        }
        let answer = window.prompt(
            PromptLevel::Warning,
            "清理旧位置备份？",
            Some(&format!(
                "将删除 {} 中的旧位置备份。当前课程库仍保留在 {}。",
                backup.path.display(),
                backup.current_root.display()
            )),
            &["清理备份", "保留备份"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await.ok() != Some(0) {
                return;
            }
            let cancel = Arc::new(AtomicBool::new(false));
            let worker_cancel = cancel.clone();
            let roots = this
                .update_in(cx, |this, window, cx| {
                    this.storage_ui.busy = true;
                    this.storage_ui.cleanup = true;
                    this.storage_ui.cancel = Some(cancel.clone());
                    this.storage_ui.error = None;
                    this.storage_ui.progress = Progress {
                        message: "正在核验旧位置备份，确认内容未变化后清理…".into(),
                        completed: 0,
                        total: 0,
                    };
                    this.open_storage_dialog(window, cx);
                    cx.notify();
                    this.workspace
                        .as_ref()
                        .map(|workspace| {
                            workspace
                                .state
                                .libraries
                                .iter()
                                .map(|library| library.root.clone())
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default()
                })
                .unwrap_or_default();
            let worker = cx
                .background_executor()
                .spawn(async move { storage::cleanup_backup(&backup, &roots, &worker_cancel) });
            let result = worker.await;
            let _ = this.update_in(cx, |this, _, cx| {
                this.storage_ui.busy = false;
                this.storage_ui.cancel = None;
                this.storage_ui.progress = Progress::default();
                match result {
                    Ok(()) => {
                        if let Some(workspace) = &mut this.workspace {
                            match workspace.transaction(|state| {
                                state.storage_backups.retain(|backup| backup.id != id);
                                Ok(())
                            }) {
                                Ok(()) => {
                                    this.storage_ui.progress.message =
                                        "旧位置备份已清理，当前课程库保持完整。".into();
                                }
                                Err(error) => {
                                    this.storage_ui.error =
                                        Some(format!("备份已清理，界面记录尚未保存：{error:#}"))
                                }
                            }
                        }
                    }
                    Err(_) if cancel.load(Ordering::Relaxed) => {
                        this.storage_ui.error = Some("已取消清理，旧位置备份仍保留。".into())
                    }
                    Err(error) => this.storage_ui.error = Some(format!("{error:#}")),
                }
                this.refresh_library(cx);
                this.start_next_task(cx);
                cx.notify();
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::{
        LocationChecks, inspect_location, inspect_relocation, publish_location, publish_relocation,
        relocate_preview, scan_locations, validate_registered_destination,
    };
    use crate::{ConversionOptions, source, storage, workspace};
    use std::{path::PathBuf, sync::atomic::AtomicBool};

    fn test_location(root: PathBuf) -> workspace::LibraryLocation {
        workspace::LibraryLocation {
            id: "test-library".into(),
            name: "课程库".into(),
            root,
            previous_roots: Vec::new(),
        }
    }

    #[test]
    fn locating_a_moved_library_reopens_notes_and_preserves_task_intent() {
        let directory = tempfile::tempdir().unwrap();
        let old = directory.path().join("old");
        let new = directory.path().join("moved");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("video.mp4"), "original video").unwrap();
        let work = old.join("note-work");
        std::fs::create_dir_all(&work).unwrap();
        let target = course2md::artifact::Target {
            task_id: "task-relocate".into(),
            course_id: "course-relocate".into(),
            source_id: "local:original-video".into(),
            version_id: "v1".into(),
            course_dir: old.join("note"),
        };
        let sections = vec![course2md::timeline::Section {
            t: 0.,
            end: 1.,
            image: String::new(),
            speech: vec![course2md::timeline::TranscriptEvent {
                start: 0.,
                end: 1.,
                text: "Relocation must preserve this body.".into(),
                raw: None,
            }],
        }];
        let meta = course2md::fetch::VideoMeta {
            title: "Original title".into(),
            uploader: String::new(),
            duration: 0.,
            webpage_url: String::new(),
            extractor: "local".into(),
            id: "original-video".into(),
        };
        smol::block_on(course2md::artifact::publish(
            &target,
            &work,
            &meta,
            &sections,
            None,
            &[],
            Default::default(),
        ))
        .unwrap();
        std::fs::remove_dir_all(&work).unwrap();
        let old = old.canonicalize().unwrap();
        let record = directory.path().join("workspace.json");
        let mut workspace = workspace::Workspace::open_at(
            record.clone(),
            old.clone(),
            ConversionOptions::default(),
        )
        .unwrap();
        let id = workspace.state.default_library.clone();
        let source = source::Source {
            input: old.join("video.mp4").display().to_string(),
            identity: "local:original-video".into(),
            title: "Original title".into(),
            online: false,
            ..Default::default()
        };
        let draft = workspace.state.draft_mut().unwrap();
        draft.input = source.input.clone();
        draft.online = false;
        draft.source = Some(source.clone());
        draft.title = "Keep my custom title".into();
        draft.custom_title = true;
        draft.folder = Some(7);
        let draft_id = draft.id.clone();
        let plan = workspace::TaskPlan {
            operation: course2md::execution::Operation::Generate,
            source: source.clone(),
            source_id: source.identity.clone(),
            title: "Frozen task".into(),
            library_id: id.clone(),
            folder: Some(7),
            options: ConversionOptions::default(),
            subtitle: None,
            config: course2md::settings::ConfigFile::default(),
            asr_service: Some("fixed-service-version".into()),
            ai_service: None,
        };
        let (task_id, _) = workspace.state.enqueue(plan, None).unwrap();
        let task = workspace.state.task_mut(&task_id).unwrap();
        task.state = workspace::TaskState::Uncertain;
        task.intent = workspace::Intent::Pause;
        let work = task.work_dir.clone();
        workspace
            .state
            .reader_sources
            .insert("inside".into(), old.join("video.mp4"));
        let external = directory.path().join("external.mp4");
        workspace
            .state
            .reader_sources
            .insert("outside".into(), external.clone());
        let mut preview =
            crate::notes::read_preview(crate::notes::scan_library(&old).unwrap().courses.remove(0))
                .unwrap();
        let original_text = preview.plain_text.clone();
        workspace.state.task_mut(&task_id).unwrap().artifact = Some(preview.course.dir.clone());
        workspace.transaction(|_| Ok(())).unwrap();
        let binding = br#"{"task_id":"kept","source_id":"kept"}"#;
        std::fs::write(work.join("task-identity.json"), binding).unwrap();
        std::fs::write(
            work.join("control.json"),
            br#"{"intent":"pause","resend":[]}"#,
        )
        .unwrap();
        std::fs::rename(&old, &new).unwrap();
        let new = new.canonicalize().unwrap();
        assert!(!old.exists());

        let located = inspect_relocation(&workspace.state, &id, &new).unwrap();
        workspace
            .transaction(|state| publish_relocation(state, &located))
            .unwrap();
        let state = &workspace.state;
        assert_eq!(state.library(&id).unwrap().root, new);
        assert_eq!(state.default_library, id);
        assert_eq!(state.current_draft, draft_id);
        assert_eq!(state.draft().unwrap().title, "Keep my custom title");
        assert_eq!(state.draft().unwrap().folder, Some(7));
        assert_eq!(
            state.draft().unwrap().input,
            new.join("video.mp4").display().to_string()
        );
        let task = state.task(&task_id).unwrap();
        assert_eq!(task.state, workspace::TaskState::Uncertain);
        assert_eq!(task.intent, workspace::Intent::Pause);
        assert_eq!(
            task.plan.asr_service.as_deref(),
            Some("fixed-service-version")
        );
        assert_eq!(state.reader_sources["inside"], new.join("video.mp4"));
        assert_eq!(state.reader_sources["outside"], external);
        assert!(
            state.storage_backups.is_empty(),
            "locating must not invent a verified backup"
        );
        assert_eq!(
            std::fs::read(task.work_dir.join("task-identity.json")).unwrap(),
            binding
        );
        assert_eq!(
            std::fs::read(task.work_dir.join("control.json")).unwrap(),
            br#"{"intent":"pause","resend":[]}"#
        );
        assert_eq!(
            std::fs::read_to_string(new.join(".course2md-library-id")).unwrap(),
            id
        );
        relocate_preview(&mut preview, &old, &new);
        assert_eq!(preview.plain_text, original_text);
        assert!(preview.course.dir.starts_with(&new));
        let reread = crate::notes::read_preview(preview.course).unwrap();
        assert_eq!(reread.plain_text, original_text);
        let reopened =
            workspace::Workspace::open_at(record, old.clone(), ConversionOptions::default())
                .unwrap();
        assert_eq!(reopened.state.library(&id).unwrap().root, new);
        assert!(
            reopened
                .state
                .library(&id)
                .unwrap()
                .previous_roots
                .contains(&old)
        );
        assert!(
            !old.exists(),
            "recovery must not recreate the missing original directory"
        );
    }

    #[test]
    fn locating_rejects_missing_wrong_and_overlapping_library_identity_without_writes() {
        let directory = tempfile::tempdir().unwrap();
        let old = directory.path().join("old");
        let target = directory.path().join("target");
        std::fs::create_dir(&old).unwrap();
        std::fs::create_dir(&target).unwrap();
        let workspace = workspace::Workspace::open_at(
            directory.path().join("workspace.json"),
            old,
            ConversionOptions::default(),
        )
        .unwrap();
        workspace.save().unwrap();
        let id = workspace.state.default_library.clone();
        let before = std::fs::read(workspace.storage_path()).unwrap();
        assert!(inspect_relocation(&workspace.state, &id, &target).is_err());
        assert!(!target.join(".course2md-library-id").exists());
        std::fs::write(target.join(".course2md-library-id"), "another-library").unwrap();
        assert!(inspect_relocation(&workspace.state, &id, &target).is_err());
        assert_eq!(
            std::fs::read_to_string(target.join(".course2md-library-id")).unwrap(),
            "another-library"
        );
        std::fs::write(target.join(".course2md-library-id"), &id).unwrap();
        let mut state = workspace.state.clone();
        state.libraries.push(workspace::LibraryLocation {
            id: "other".into(),
            name: "Other".into(),
            root: target.clone(),
            previous_roots: Vec::new(),
        });
        assert!(inspect_relocation(&state, &id, &target).is_err());
        assert_eq!(std::fs::read(workspace.storage_path()).unwrap(), before);
    }

    #[test]
    fn locating_rechecks_registry_and_selected_directory_before_publication() {
        let directory = tempfile::tempdir().unwrap();
        let old = directory.path().join("old");
        let target = directory.path().join("target");
        std::fs::create_dir(&old).unwrap();
        std::fs::create_dir(&target).unwrap();
        let mut workspace = workspace::Workspace::open_at(
            directory.path().join("workspace.json"),
            old,
            ConversionOptions::default(),
        )
        .unwrap();
        let id = workspace.state.default_library.clone();
        std::fs::write(target.join(".course2md-library-id"), &id).unwrap();
        let located = inspect_relocation(&workspace.state, &id, &target).unwrap();
        let mut changed = workspace.state.clone();
        changed.libraries[0].root = directory.path().join("another-location");
        let before = changed.clone();
        assert!(publish_relocation(&mut changed, &located).is_err());
        assert!(changed == before);
        std::fs::rename(&target, directory.path().join("original-selected")).unwrap();
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join(".course2md-library-id"), &id).unwrap();
        let before = workspace.state.clone();
        assert!(
            workspace
                .transaction(|state| publish_relocation(state, &located))
                .is_err()
        );
        assert!(workspace.state == before);
        let located = inspect_relocation(&workspace.state, &id, &target).unwrap();
        std::fs::write(target.join(".course2md-library-id"), "different-library").unwrap();
        assert!(
            workspace
                .transaction(|state| publish_relocation(state, &located))
                .is_err()
        );
        assert!(workspace.state == before);
    }

    #[test]
    fn locating_keeps_the_original_registry_when_publication_cannot_be_saved() {
        let directory = tempfile::tempdir().unwrap();
        let old = directory.path().join("old");
        let target = directory.path().join("target");
        std::fs::create_dir(&old).unwrap();
        let record = directory.path().join("workspace.json");
        let mut workspace = workspace::Workspace::open_at(
            record.clone(),
            old.clone(),
            ConversionOptions::default(),
        )
        .unwrap();
        workspace.save().unwrap();
        let id = workspace.state.default_library.clone();
        let before = workspace.state.clone();
        let record_bytes = std::fs::read(&record).unwrap();
        std::fs::rename(&old, &target).unwrap();
        let located = inspect_relocation(&workspace.state, &id, &target).unwrap();
        // A directory at the output file is a portable publication failure, not
        // a chmod assertion that would silently pass under a privileged runner.
        std::fs::rename(&record, directory.path().join("original-record.json")).unwrap();
        std::fs::create_dir(&record).unwrap();
        assert!(
            workspace
                .transaction(|state| publish_relocation(state, &located))
                .is_err()
        );
        assert!(workspace.state == before);
        assert_eq!(
            std::fs::read(directory.path().join("original-record.json")).unwrap(),
            record_bytes
        );
        assert_eq!(
            std::fs::read_to_string(target.join(".course2md-library-id")).unwrap(),
            id
        );
        assert!(!old.exists());
    }

    #[test]
    fn slow_library_scan_runs_outside_the_calling_thread() {
        let directory = tempfile::tempdir().unwrap();
        let location = test_location(directory.path().to_owned());
        std::fs::write(directory.path().join(".course2md-library-id"), &location.id).unwrap();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let caller = std::thread::current().id();
        smol::block_on(async {
            let mut scan = Box::pin(scan_locations(vec![location.clone()], move |_| {
                started_tx.send(std::thread::current().id()).unwrap();
                release_rx
                    .recv_timeout(std::time::Duration::from_secs(2))
                    .unwrap();
            }));
            // Polling the real scan boundary returns while the worker remains
            // blocked. This checks thread isolation, not a native hang fixture.
            assert!(smol::future::poll_once(&mut scan).await.is_none());
            let worker = started_rx
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap();
            assert_ne!(worker, caller);
            let mut cache = LocationChecks::default();
            cache.begin(1, std::slice::from_ref(&location));
            assert!(cache.get(&location).is_none());
            release_tx.send(()).unwrap();
            let result = scan.await;
            assert!(result[0].1.available);
            assert!(!result[0].1.needs_reassociation);
        });
    }

    #[test]
    fn location_checks_reject_old_generations_and_changed_paths() {
        let directory = tempfile::tempdir().unwrap();
        let old = test_location(directory.path().join("old"));
        let new = test_location(directory.path().join("new"));
        std::fs::create_dir(&old.root).unwrap();
        std::fs::create_dir(&new.root).unwrap();
        std::fs::write(old.root.join(".course2md-library-id"), &old.id).unwrap();
        let mut cache = LocationChecks::default();
        cache.begin(1, std::slice::from_ref(&old));
        assert!(cache.finish(
            1,
            std::slice::from_ref(&old),
            vec![(old.clone(), inspect_location(&old))]
        ));
        assert!(cache.get(&old).unwrap().available);
        cache.begin(2, std::slice::from_ref(&old));
        assert!(
            cache.get(&old).is_some(),
            "same-path refresh keeps the last checked result"
        );
        assert!(!cache.finish(
            1,
            std::slice::from_ref(&old),
            vec![(old.clone(), inspect_location(&old))]
        ));
        assert!(!cache.finish(
            2,
            std::slice::from_ref(&new),
            vec![(old.clone(), inspect_location(&old))]
        ));
        assert!(
            cache.get(&new).is_none(),
            "a new path is pending, not offline"
        );
        cache.begin(3, std::slice::from_ref(&new));
        assert!(cache.get(&old).is_none());
        assert!(!cache.finish(
            2,
            std::slice::from_ref(&new),
            vec![(old.clone(), inspect_location(&old))]
        ));
        assert!(cache.finish(
            3,
            std::slice::from_ref(&new),
            vec![(new.clone(), inspect_location(&new))]
        ));
        assert!(cache.get(&new).unwrap().needs_reassociation);
    }

    #[test]
    fn registry_commit_preserves_ids_and_moves_only_library_owned_paths() {
        let directory = tempfile::tempdir().unwrap();
        let old = directory.path().join("old");
        let new = directory.path().join("new");
        std::fs::create_dir_all(old.join("media")).unwrap();
        std::fs::create_dir_all(&new).unwrap();
        std::fs::write(old.join("media/video.mp4"), "fixture video content").unwrap();
        let old = std::fs::canonicalize(old).unwrap();
        let new = std::fs::canonicalize(new).unwrap();
        let record_path = directory.path().join("workspace.json");
        let mut workspace = workspace::Workspace::open_at(
            record_path.clone(),
            old.clone(),
            ConversionOptions::default(),
        )
        .unwrap();
        let library_id = workspace.state.default_library.clone();
        let mut source = source::Source {
            input: old.join("media/video.mp4").display().to_string(),
            title: "My confirmed title".into(),
            identity: "local:content-stays-the-same".into(),
            online: false,
            cover: Some(old.join("media/cover.png")),
            ..Default::default()
        };
        let draft = workspace.state.draft_mut().unwrap();
        draft.online = false;
        draft.input = source.input.clone();
        draft.source = Some(source.clone());
        draft.title = "My custom title".into();
        draft.custom_title = true;
        draft.folder = Some(17);
        let original_draft_id = draft.id.clone();
        let mut config = course2md::settings::ConfigFile::default();
        config.defaults.model_dir = Some(old.join("models"));
        let plan = workspace::TaskPlan {
            operation: course2md::execution::Operation::Reprocess {
                base_version_dir: old.join("note/versions/old-version"),
                components: vec!["summary".into()],
                prior_work_dir: Some(old.join(".course2md/work/old-task")),
            },
            source: source.clone(),
            source_id: source.identity.clone(),
            title: "Frozen title".into(),
            library_id: library_id.clone(),
            folder: Some(17),
            options: ConversionOptions::default(),
            subtitle: Some(old.join("subtitles/selected.srt")),
            config,
            asr_service: Some("fixed-asr-version".into()),
            ai_service: Some("fixed-ai-version".into()),
        };
        let (task_id, _) = workspace.state.enqueue(plan, None).unwrap();
        let old_task_work = workspace.state.task(&task_id).unwrap().work_dir.clone();
        source.input = directory.path().join("outside.mp4").display().to_string();
        let outside_input = source.input.clone();
        let mut outside_draft =
            workspace::Draft::new(false, library_id.clone(), ConversionOptions::default());
        outside_draft.input = outside_input.clone();
        outside_draft.source = Some(source);
        workspace
            .state
            .reader_sources
            .insert("local:relocated-video".into(), old.join("media/video.mp4"));
        workspace
            .state
            .reader_sources
            .insert("local:outside-video".into(), PathBuf::from(&outside_input));
        workspace.transaction(|_| Ok(())).unwrap();
        let prepared = storage::prepare_move(
            library_id.clone(),
            &old,
            &new,
            &directory.path().join("journals"),
            &AtomicBool::new(false),
            &|_| {},
        )
        .unwrap();
        assert_eq!(workspace.state.library(&library_id).unwrap().root, old);
        // Exercise an external current input separately: the workbench now has
        // one form, and reopening must not revive a second managed draft.
        let mut external_input_state = workspace.state.clone();
        external_input_state.current_draft = outside_draft.id.clone();
        external_input_state.drafts = vec![outside_draft];
        publish_location(&mut external_input_state, &prepared.journal, &prepared.path).unwrap();
        assert_eq!(external_input_state.draft().unwrap().input, outside_input);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let permissions = std::fs::metadata(directory.path()).unwrap().permissions();
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o555))
                .unwrap();
            let failed = workspace
                .transaction(|state| publish_location(state, &prepared.journal, &prepared.path));
            std::fs::set_permissions(directory.path(), permissions).unwrap();
            assert!(failed.is_err());
            assert_eq!(workspace.state.library(&library_id).unwrap().root, old);
        }
        let before_commit = workspace::Workspace::open_at(
            record_path.clone(),
            old.clone(),
            ConversionOptions::default(),
        )
        .unwrap();
        assert_eq!(before_commit.state.library(&library_id).unwrap().root, old);
        assert_eq!(
            before_commit.state.task(&task_id).unwrap().work_dir,
            old_task_work
        );
        workspace
            .transaction(|state| publish_location(state, &prepared.journal, &prepared.path))
            .unwrap();
        let mut reopened =
            workspace::Workspace::open_at(record_path, old.clone(), ConversionOptions::default())
                .unwrap();
        let state = &reopened.state;
        let lagging: storage::Journal =
            serde_json::from_slice(&std::fs::read(&prepared.path).unwrap()).unwrap();
        assert_eq!(lagging.phase, storage::Phase::Verified);
        assert_eq!(state.library(&library_id).unwrap().root, new);
        assert_eq!(
            state.reader_sources["local:relocated-video"],
            new.join("media/video.mp4")
        );
        assert_eq!(
            state.reader_sources["local:outside-video"],
            PathBuf::from(&outside_input)
        );
        let draft = state
            .drafts
            .iter()
            .find(|draft| draft.id == original_draft_id)
            .unwrap();
        assert_eq!(draft.library_id, library_id);
        assert_eq!(draft.folder, Some(17));
        assert_eq!(draft.title, "My custom title");
        assert_eq!(
            draft.input,
            new.join("media/video.mp4").display().to_string()
        );
        assert_eq!(
            draft.source.as_ref().unwrap().identity,
            "local:content-stays-the-same"
        );
        assert_eq!(state.drafts.len(), 1);
        let task = state.task(&task_id).unwrap();
        assert_eq!(task.plan.title, "Frozen title");
        assert_eq!(task.plan.asr_service.as_deref(), Some("fixed-asr-version"));
        assert_eq!(
            task.plan.config.defaults.model_dir,
            Some(new.join("models"))
        );
        assert_eq!(
            task.work_dir,
            new.join(old_task_work.strip_prefix(&old).unwrap())
        );
        assert!(
            matches!(&task.plan.operation, course2md::execution::Operation::Reprocess {base_version_dir, prior_work_dir:Some(work),..}
            if base_version_dir.starts_with(&new) && work.starts_with(&new))
        );
        assert_eq!(state.storage_backups[0].path, old);
        assert!(
            state
                .library(&library_id)
                .unwrap()
                .previous_roots
                .contains(&old)
        );
        assert!(old.join("media/video.mp4").is_file());
        let final_root = directory.path().join("third");
        std::fs::create_dir(&final_root).unwrap();
        let second_move = storage::prepare_move(
            library_id.clone(),
            &new,
            &final_root,
            &directory.path().join("journals"),
            &AtomicBool::new(false),
            &|_| {},
        )
        .unwrap();
        reopened
            .transaction(|state| publish_location(state, &second_move.journal, &second_move.path))
            .unwrap();
        let location = reopened.state.library(&library_id).unwrap();
        assert!(location.previous_roots.contains(&old) && location.previous_roots.contains(&new));
        assert_eq!(
            reopened.state.reader_sources["local:relocated-video"],
            location.root.join("media/video.mp4")
        );
        assert!(
            reopened
                .state
                .storage_backups
                .iter()
                .all(|backup| backup.current_root == location.root)
        );
    }

    #[test]
    fn another_registered_library_is_rejected_even_when_empty() {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("first");
        let second = directory.path().join("second");
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        let mut workspace = workspace::Workspace::open_at(
            directory.path().join("workspace.json"),
            first,
            ConversionOptions::default(),
        )
        .unwrap();
        workspace.state.libraries.push(workspace::LibraryLocation {
            id: "other".into(),
            name: "Other library".into(),
            root: second.clone(),
            previous_roots: Vec::new(),
        });
        assert!(
            validate_registered_destination(
                &workspace.state,
                &workspace.state.default_library,
                &second
            )
            .is_err()
        );
        assert!(std::fs::read_dir(second).unwrap().next().is_none());
    }
}
