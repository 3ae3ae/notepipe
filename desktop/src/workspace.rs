//! Durable user work. Views never supply the identity or settings of a retry.
use super::ConversionOptions;
use anyhow::{Context, Result, bail, ensure};
use course2md::settings::ConfigFile;
use serde::{Deserialize, Serialize};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const SCHEMA: u32 = 1;
/// Minimum spacing between throttled progress saves while a task streams events.
const PROGRESS_SAVE_INTERVAL: Duration = Duration::from_secs(2);
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub fn new_id(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{nanos:x}-{:x}-{sequence:x}", std::process::id())
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// 课程目录名：`course-{source_id 摘要前 32 字符}`——32 是与引擎 artifact 布局对齐的固定截断。
pub fn course_dir_name(source_id: &str) -> String {
    format!(
        "course-{}",
        &course2md::execution::digest(source_id.as_bytes())[..32]
    )
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct LibraryLocation {
    pub id: String,
    pub name: String,
    pub root: PathBuf,
    #[serde(default)]
    pub previous_roots: Vec<PathBuf>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Override {
    Provider,
    TextSource,
    Proofread,
    Summary,
    Vision,
    KeepVideo,
    Formats,
}

/// The single current input form. Its serialized type/field names are retained
/// so existing workspaces can be upgraded without changing task records.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Draft {
    pub id: String,
    pub revision: u64,
    pub online: bool,
    pub input: String,
    #[serde(default)]
    pub source: Option<crate::source::Source>,
    pub title: String,
    pub custom_title: bool,
    pub library_id: String,
    pub folder: Option<u64>,
    pub options: ConversionOptions,
    /// Remember the local branch when temporarily choosing a speech service.
    #[serde(default)]
    pub local_provider: Option<usize>,
    #[serde(default)]
    pub overrides: BTreeSet<Override>,
    #[serde(default)]
    pub subtitle: Option<PathBuf>,
    #[serde(default)]
    pub asr_service: Option<String>,
    #[serde(default)]
    pub ai_service: Option<String>,
    #[serde(default)]
    pub retry_of: Option<String>,
    #[serde(default, serialize_with = "serialize_optional_public_config")]
    pub base_config: Option<ConfigFile>,
    #[serde(default)]
    pub submitted_task: Option<String>,
    #[serde(default)]
    pub scroll: f32,
    pub updated: u64,
}

impl Draft {
    pub fn new(online: bool, library_id: String, defaults: ConversionOptions) -> Self {
        Self {
            id: new_id("input"),
            revision: 0,
            online,
            input: String::new(),
            source: None,
            title: String::new(),
            custom_title: false,
            library_id,
            folder: None,
            local_provider: (!defaults.uses_cloud_provider()).then_some(defaults.provider),
            options: defaults,
            overrides: BTreeSet::new(),
            subtitle: None,
            asr_service: None,
            ai_service: None,
            retry_of: None,
            base_config: None,
            submitted_task: None,
            scroll: 0.,
            updated: now(),
        }
    }

    pub fn change_source(&mut self, input: String) {
        if input == self.input {
            return;
        }
        self.revision = self.revision.wrapping_add(1);
        self.input = input;
        self.source = None;
        self.subtitle = None;
        self.submitted_task = None;
        self.updated = now();
        if !self.custom_title {
            self.title.clear();
        }
    }

    #[cfg(test)]
    pub fn accept_source(&mut self, revision: u64, source: crate::source::Source) -> bool {
        if self.revision != revision {
            return false;
        }
        if !self.custom_title {
            self.title = source.title.clone();
        }
        self.source = Some(source);
        self.updated = now();
        true
    }

    pub fn inherit(&mut self, defaults: &ConversionOptions) {
        let v = &mut self.options;
        if !self.overrides.contains(&Override::Provider) {
            v.provider = defaults.provider;
        }
        if !self.overrides.contains(&Override::TextSource) {
            v.source_mode = defaults.source_mode;
        }
        if !self.overrides.contains(&Override::Proofread) {
            v.llm = defaults.llm;
        }
        if !self.overrides.contains(&Override::Summary) {
            v.summarize = defaults.summarize;
        }
        if !self.overrides.contains(&Override::Vision) {
            v.vision = defaults.vision;
        }
        if !self.overrides.contains(&Override::KeepVideo) {
            v.keep_video = defaults.keep_video;
        }
        if !self.overrides.contains(&Override::Formats) {
            v.formats = defaults.formats;
        }
        v.resume = true;
    }

    /// Resume inheriting AI processing choices without changing other task choices.
    pub fn reset_ai_overrides(&mut self, defaults: &ConversionOptions) {
        self.options.llm = defaults.llm;
        self.options.summarize = defaults.summarize;
        self.options.vision = defaults.vision;
        for field in [Override::Proofread, Override::Summary, Override::Vision] {
            self.overrides.remove(&field);
        }
    }
}

/// Extra defense at the persistence boundary: even a caller-provided resolved
/// config cannot put its API keys in a task file or its backup.
fn serialize_public_config<S: serde::Serializer>(
    config: &ConfigFile,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    public_config(config).serialize(serializer)
}
fn serialize_optional_public_config<S: serde::Serializer>(
    config: &Option<ConfigFile>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    config.as_ref().map(public_config).serialize(serializer)
}

pub fn public_config(config: &ConfigFile) -> ConfigFile {
    let mut config = config.clone();
    config.asr_api.api_key.clear();
    config.llm.api_key.clear();
    config
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskPlan {
    #[serde(default)]
    pub operation: course2md::execution::Operation,
    pub source: crate::source::Source,
    pub source_id: String,
    pub title: String,
    pub library_id: String,
    pub folder: Option<u64>,
    pub options: ConversionOptions,
    pub subtitle: Option<PathBuf>,
    #[serde(serialize_with = "serialize_public_config")]
    pub config: ConfigFile,
    pub asr_service: Option<String>,
    pub ai_service: Option<String>,
}

impl TaskPlan {
    pub fn same_work(&self, other: &Self) -> bool {
        if self.operation != other.operation {
            return false;
        }
        if self.library_id != other.library_id
            || self.source_id != other.source_id
            || self.subtitle != other.subtitle
            || self.asr_service != other.asr_service
            || self.ai_service != other.ai_service
        {
            return false;
        }
        let mut left = public_config(&self.config);
        let mut right = public_config(&other.config);
        // Display and organization are not processing parameters.
        left.defaults.out = None;
        right.defaults.out = None;
        left.desktop = Default::default();
        right.desktop = Default::default();
        left == right && self.options == other.options
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Intent {
    Run,
    Pause,
    Cancel,
    Quit,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Queued,
    Running,
    Pausing,
    Paused,
    NeedsAttention,
    Uncertain,
    Complete,
    Partial,
    Cancelled,
}

impl TaskState {
    pub fn finished(self) -> bool {
        matches!(self, Self::Complete | Self::Partial | Self::Cancelled)
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Queued => "等待处理",
            Self::Running => "正在生成笔记",
            Self::Pausing => "正在保存进度",
            Self::Paused => "已暂停",
            Self::NeedsAttention => "需要处理",
            Self::Uncertain => "结果尚未确认",
            Self::Complete => "笔记已生成",
            Self::Partial => "笔记已保存，部分处理未完成",
            Self::Cancelled => "已取消",
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Stage {
    pub status: String,
    pub current: u64,
    pub total: u64,
    pub detail: Option<String>,
}

impl Stage {
    pub fn begin(&mut self) {
        *self = Self {
            status: "start".into(),
            ..Default::default()
        };
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskRecord {
    pub id: String,
    pub plan: TaskPlan,
    pub state: TaskState,
    pub intent: Intent,
    pub created: u64,
    pub updated: u64,
    pub parent: Option<String>,
    #[serde(default)]
    pub handled_by: Option<String>,
    pub work_dir: PathBuf,
    #[serde(default)]
    pub stages: BTreeMap<String, Stage>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub artifact: Option<PathBuf>,
    pub outcomes: serde_json::Value,
    #[serde(default)]
    pub unread: bool,
    #[serde(default)]
    pub logs: Vec<String>,
    #[serde(default)]
    pub blocked: Vec<BlockedRequest>,
    #[serde(default)]
    pub resend: Vec<String>,
}

impl TaskRecord {
    /// 是否有结果尚未确认的请求（BlockedRequest::is_uncertain 的任务级汇总）。
    pub fn has_uncertain(&self) -> bool {
        self.blocked.iter().any(BlockedRequest::is_uncertain)
    }

    pub fn exports_only(&self) -> bool {
        matches!(
            &self.plan.operation,
            course2md::execution::Operation::Reprocess { components, .. }
                if !components.is_empty() && components.iter().all(|part| part == "exports")
        )
    }
}

/// Export publication has two destinations. Derive them from the frozen task
/// and current library location, so moving a library needs no new path record.
fn task_export_directory(task: &TaskRecord, location: &LibraryLocation) -> Option<PathBuf> {
    if task.plan.library_id != location.id {
        return None;
    }
    if task.exports_only() {
        let course = crate::workspace::course_dir_name(&task.plan.source_id);
        let course2md::execution::Operation::Reprocess {
            base_version_dir, ..
        } = &task.plan.operation
        else {
            return None;
        };
        Some(
            location
                .root
                .join(course)
                .join("exports")
                .join(base_version_dir.file_name()?)
                .join(&task.id),
        )
    } else {
        task.artifact
            .as_ref()
            .map(|version| version.join("exports"))
    }
}

/// No filesystem access: rendering can offer the exact successful outputs from
/// the durable outcome record. Existence is checked when the user opens them.
pub(crate) fn task_export_files(task: &TaskRecord, location: &LibraryLocation) -> Vec<PathBuf> {
    if task.artifact.is_none()
        || matches!(
            task.state,
            TaskState::Queued | TaskState::Running | TaskState::Pausing
        )
    {
        return Vec::new();
    }
    let Some(exports) = task
        .outcomes
        .get("exports")
        .and_then(|exports| exports.as_object())
    else {
        return Vec::new();
    };
    let Some(directory) = task_export_directory(task, location) else {
        return Vec::new();
    };
    use course2md::config::OutputFormat;
    [OutputFormat::Md, OutputFormat::Html, OutputFormat::Json]
        .into_iter()
        .filter(|format| {
            exports
                .get(&format.to_string())
                .and_then(|outcome| outcome.get("status"))
                .and_then(|status| status.as_str())
                == Some("succeeded")
        })
        .map(|format| directory.join(course2md::portable::file_name(format)))
        .collect()
}

pub(crate) fn available_task_export(
    task: &TaskRecord,
    location: &LibraryLocation,
) -> Result<PathBuf> {
    task_export_files(task, location)
        .into_iter()
        .find(|path| path.is_file())
        .context("导出文件已移动或无法读取，可打开笔记后从“导出”菜单重新保存。")
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct BlockedRequest {
    pub reason: String,
    pub request_id: Option<String>,
    pub purpose: Option<String>,
    #[serde(default)]
    pub description: String,
    pub message: String,
}

impl BlockedRequest {
    /// 结果尚未确认的请求：reason 与 dispatch::REASON_UNCERTAIN 对齐的唯一判断点。
    pub fn is_uncertain(&self) -> bool {
        self.reason == course2md::dispatch::REASON_UNCERTAIN
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct ReadingPosition {
    pub paragraph: Option<String>,
    pub seconds: Option<f64>,
    pub offset: f32,
    #[serde(default)]
    pub within: f32,
    #[serde(default)]
    pub fraction: Option<f32>,
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct State {
    pub schema: u32,
    pub libraries: Vec<LibraryLocation>,
    pub default_library: String,
    pub drafts: Vec<Draft>,
    pub current_draft: String,
    pub tasks: Vec<TaskRecord>,
    pub selected_task: Option<String>,
    pub positions: BTreeMap<String, ReadingPosition>,
    pub reader_sources: BTreeMap<String, PathBuf>,
    pub collapsed: BTreeSet<String>,
    pub storage_backups: Vec<crate::storage::BackupRecord>,
}

impl State {
    fn initial(root: PathBuf, options: ConversionOptions) -> Self {
        let library = LibraryLocation {
            id: new_id("library"),
            name: "课程库".into(),
            root,
            previous_roots: Vec::new(),
        };
        let draft = Draft::new(true, library.id.clone(), options);
        Self {
            schema: SCHEMA,
            default_library: library.id.clone(),
            libraries: vec![library],
            current_draft: draft.id.clone(),
            drafts: vec![draft],
            tasks: Vec::new(),
            selected_task: None,
            positions: BTreeMap::new(),
            reader_sources: BTreeMap::new(),
            collapsed: BTreeSet::new(),
            storage_backups: Vec::new(),
        }
    }

    pub fn draft(&self) -> Option<&Draft> {
        self.drafts.iter().find(|d| d.id == self.current_draft)
    }
    pub fn draft_mut(&mut self) -> Option<&mut Draft> {
        self.drafts.iter_mut().find(|d| d.id == self.current_draft)
    }
    pub fn matches_input(&self, id: &str, revision: u64) -> bool {
        self.draft()
            .is_some_and(|input| input.id == id && input.revision == revision)
    }
    fn replace_input(&mut self, input: Draft) {
        self.current_draft = input.id.clone();
        self.drafts = vec![input];
    }
    fn retain_current_input(&mut self) {
        if let Some(input) = self.draft().cloned() {
            self.replace_input(input);
        }
    }
    pub fn library(&self, id: &str) -> Option<&LibraryLocation> {
        self.libraries.iter().find(|l| l.id == id)
    }
    pub fn task(&self, id: &str) -> Option<&TaskRecord> {
        self.tasks.iter().find(|t| t.id == id)
    }
    pub fn task_mut(&mut self, id: &str) -> Option<&mut TaskRecord> {
        self.tasks.iter_mut().find(|t| t.id == id)
    }
    pub fn next_task(&self) -> Option<&TaskRecord> {
        self.tasks.iter().find(|t| {
            t.state == TaskState::Queued && t.intent == Intent::Run && t.handled_by.is_none()
        })
    }

    /// Replace the current input; submitted task plans stay frozen.
    pub fn reset_input(
        &mut self,
        online: bool,
        options: ConversionOptions,
        destination: Option<(String, u64)>,
    ) -> String {
        let mut draft = Draft::new(online, self.default_library.clone(), options);
        if let Some((library, folder)) = destination {
            draft.library_id = library;
            draft.folder = Some(folder);
        }
        self.replace_input(draft);
        self.current_draft.clone()
    }

    pub fn switch_source_kind(&mut self, online: bool, options: ConversionOptions) {
        let Some(previous) = self.draft() else {
            self.reset_input(online, options, None);
            return;
        };
        if previous.online == online {
            return;
        }
        if previous.submitted_task.is_some() {
            let mut input = Draft::new(online, self.default_library.clone(), options);
            if previous.library_id == input.library_id {
                input.folder = previous.folder;
            }
            self.replace_input(input);
            return;
        }
        // Source modes belong to the same form. Keep common choices, clear the
        // incompatible source and detach any submitted/retry task association.
        let mut input = Draft::new(
            online,
            previous.library_id.clone(),
            previous.options.clone(),
        );
        input.folder = previous.folder;
        input.overrides = previous.overrides.clone();
        input.asr_service = previous.asr_service.clone();
        input.ai_service = previous.ai_service.clone();
        input.base_config = previous.base_config.clone();
        input.local_provider = previous.local_provider;
        self.replace_input(input);
    }

    /// Replacing a submitted source starts another conversion with saved defaults.
    /// Edits to an unsubmitted source keep the user's in-progress choices.
    pub fn prepare_next_import(&mut self, value: &str, defaults: ConversionOptions) -> bool {
        let Some(previous) = self.draft() else {
            return false;
        };
        if previous.submitted_task.is_none() || previous.input == value {
            return false;
        }
        let submitted = previous.submitted_task.clone();
        let mut input = Draft::new(previous.online, self.default_library.clone(), defaults);
        if previous.library_id == input.library_id {
            input.folder = previous.folder;
        }
        input.change_source(value.to_owned());
        if let Some(task) = submitted.as_deref().and_then(|id| self.task_mut(id))
            && task.state == TaskState::Complete
        {
            // Choosing the next video acknowledges the result just presented.
            task.unread = false;
        }
        self.replace_input(input);
        true
    }

    /// Replace the current form with a task's options without mutating that task.
    pub fn adjust_task(&mut self, id: &str) -> Result<()> {
        let task = self.task(id).context("任务不存在")?.clone();
        ensure!(
            task.handled_by.is_none(),
            "此任务已有后续处理，请打开对应任务继续"
        );
        ensure!(
            !matches!(
                task.state,
                TaskState::Running | TaskState::Pausing | TaskState::Queued
            ),
            "当前任务还在处理，请先暂停并等候当前步骤结束"
        );
        let mut draft = Draft::new(
            task.plan.source.online,
            task.plan.library_id.clone(),
            task.plan.options.clone(),
        );
        draft.input = task.plan.source.input.clone();
        draft.source = Some(task.plan.source);
        draft.title = task.plan.title;
        draft.custom_title = true;
        draft.folder = task.plan.folder;
        draft.subtitle = task.plan.subtitle;
        draft.asr_service = task.plan.asr_service;
        draft.ai_service = task.plan.ai_service;
        draft.retry_of = Some(id.to_owned());
        draft.overrides = [
            Override::Provider,
            Override::TextSource,
            Override::Proofread,
            Override::Summary,
            Override::Vision,
            Override::KeepVideo,
            Override::Formats,
        ]
        .into_iter()
        .collect();
        draft.base_config = Some(task.plan.config);
        self.replace_input(draft);
        Ok(())
    }

    pub fn enqueue(&mut self, plan: TaskPlan, parent: Option<String>) -> Result<(String, bool)> {
        ensure!(
            !plan.source_id.is_empty() && !plan.title.trim().is_empty(),
            "请先确认视频和笔记名称"
        );
        if let Some(id) = parent.as_deref() {
            let original = self.task(id).context("原任务记录不存在")?;
            if let Some(followup) = &original.handled_by {
                ensure!(
                    self.task(followup).is_some(),
                    "后续任务记录缺失，请先恢复任务记录"
                );
                return Ok((followup.clone(), false));
            }
            ensure!(
                !matches!(
                    original.state,
                    TaskState::Running | TaskState::Pausing | TaskState::Queued
                ),
                "原任务仍在处理，请先暂停并等候当前步骤结束"
            );
        }
        let location = self
            .library(&plan.library_id)
            .context("所选保存位置未登记")?;
        if let Some(existing) = self
            .tasks
            .iter()
            .find(|t| !t.state.finished() && t.handled_by.is_none() && t.plan.same_work(&plan))
        {
            return Ok((existing.id.clone(), false));
        }
        let id = new_id("task");
        let work_dir = location.root.join(".course2md/work").join(&id);
        let now = now();
        self.tasks.push(TaskRecord {
            id: id.clone(),
            plan,
            state: TaskState::Queued,
            intent: Intent::Run,
            created: now,
            updated: now,
            parent: parent.clone(),
            handled_by: None,
            work_dir,
            stages: BTreeMap::new(),
            error: None,
            artifact: None,
            outcomes: serde_json::Value::Null,
            unread: false,
            logs: Vec::new(),
            blocked: Vec::new(),
            resend: Vec::new(),
        });
        if let Some(parent) = parent.and_then(|id| self.task_mut(&id)) {
            parent.handled_by = Some(id.clone());
        }
        self.selected_task = Some(id.clone());
        Ok((id, true))
    }

    /// Reconcile only process-owned states. Confirmed user stops survive restarts.
    /// The engine still checks its durable remote-request ledger before any send.
    pub fn recover(&mut self) {
        // Older records had a child.parent link but no reverse link. Reconstruct it
        // before deciding which task may run so an obsolete parent cannot resume.
        let mut followups = self
            .tasks
            .iter()
            .filter_map(|task| {
                task.parent
                    .as_ref()
                    .map(|parent| (task.created, task.updated, task.id.clone(), parent.clone()))
            })
            .collect::<Vec<_>>();
        followups.sort();
        for (_, _, child, parent) in followups {
            if child != parent
                && let Some(task) = self.task_mut(&parent)
            {
                task.handled_by = Some(child);
            }
        }
        for task in &mut self.tasks {
            if task.handled_by.is_some() && !task.state.finished() {
                task.intent = Intent::Pause;
                task.state = TaskState::Paused;
                task.unread = false;
                continue;
            }
            if matches!(task.state, TaskState::Running | TaskState::Pausing) {
                task.state = match task.intent {
                    Intent::Run => TaskState::Queued,
                    Intent::Pause | Intent::Quit => TaskState::Paused,
                    Intent::Cancel => TaskState::Cancelled,
                };
                task.updated = now();
            }
            if task.state == TaskState::Queued && task.intent != Intent::Run {
                task.state = if task.intent == Intent::Cancel {
                    TaskState::Cancelled
                } else {
                    TaskState::Paused
                };
            }
        }
    }

    pub fn set_intent(&mut self, id: &str, intent: Intent) -> Result<()> {
        let task = self.task_mut(id).context("任务记录不存在")?;
        ensure!(
            !task.state.finished(),
            "这项任务已结束，请从结果页选择补做或生成新版"
        );
        ensure!(
            task.handled_by.is_none(),
            "这项任务已有后续处理，请打开对应任务继续。"
        );
        ensure!(
            intent != Intent::Run || !matches!(task.state, TaskState::Running | TaskState::Pausing),
            "这项任务正在处理当前操作"
        );
        ensure!(
            intent != Intent::Run
                || !task.has_uncertain(),
            "仍有请求结果尚未确认，请查看请求范围后选择是否重新发送"
        );
        task.intent = intent;
        task.updated = now();
        task.state = match intent {
            Intent::Run => TaskState::Queued,
            Intent::Quit
                if matches!(task.state, TaskState::NeedsAttention | TaskState::Uncertain) =>
            {
                task.state
            }
            Intent::Cancel if matches!(task.state, TaskState::Running | TaskState::Pausing) => {
                TaskState::Pausing
            }
            Intent::Cancel => TaskState::Cancelled,
            _ if matches!(task.state, TaskState::Running | TaskState::Pausing) => {
                TaskState::Pausing
            }
            _ => TaskState::Paused,
        };
        Ok(())
    }

    pub fn authorize_uncertain(&mut self, id: &str, requests: &[String]) -> Result<()> {
        let task = self.task_mut(id).context("任务记录不存在")?;
        ensure!(
            task.handled_by.is_none(),
            "这项任务已有后续处理，请打开对应任务继续"
        );
        ensure!(
            !matches!(
                task.state,
                TaskState::Running | TaskState::Pausing | TaskState::Queued
            ) && !task.state.finished(),
            "这项任务的状态已变化，请查看最新状态"
        );
        reconcile_receipts(task)?;
        let current: BTreeSet<_> = task
            .blocked
            .iter()
            .filter(|request| request.is_uncertain())
            .filter_map(|request| request.request_id.as_ref())
            .collect();
        let selected: BTreeSet<_> = requests.iter().collect();
        ensure!(
            !selected.is_empty() && current == selected,
            "待确认的请求已变化，请查看最新范围后再选择是否重新发送"
        );
        ensure!(
            selected.iter().all(|id| !task.resend.contains(id)),
            "这些请求已获得重新发送授权，请等待对应任务处理"
        );
        task.resend.extend(requests.iter().cloned());
        task.intent = Intent::Run;
        task.state = TaskState::Queued;
        task.error = None;
        task.updated = now();
        Ok(())
    }

    pub fn reprocess(
        &mut self,
        id: &str,
        components: Vec<String>,
        resend: Vec<String>,
    ) -> Result<String> {
        self.reprocess_with_service(id, components, resend, None)
    }

    /// Repair creates a new attempt with an explicitly chosen service. The
    /// original task and the unrelated current input remain unchanged.
    pub fn reprocess_with_service(
        &mut self,
        id: &str,
        mut components: Vec<String>,
        resend: Vec<String>,
        ai_service: Option<(String, ConfigFile)>,
    ) -> Result<String> {
        let mut original = self.task(id).context("任务不存在")?.clone();
        if let Some(next) = &original.handled_by {
            ensure!(
                self.task(next).is_some(),
                "后续任务记录缺失，请先恢复任务记录"
            );
            return Ok(next.clone());
        }
        ensure!(
            !matches!(
                original.state,
                TaskState::Running | TaskState::Pausing | TaskState::Queued
            ),
            "当前任务仍在处理，请先等候当前步骤结束"
        );
        let base = original
            .artifact
            .clone()
            .context("此任务尚无可补做的笔记正文")?;
        let manifest = course2md::artifact::read_manifest(&base.join("manifest.json"))?;
        ensure!(
            manifest.source_id == original.plan.source_id,
            "笔记来源与任务不匹配"
        );
        let value = original.outcomes.clone();
        components.sort();
        components.dedup();
        ensure!(!components.is_empty(), "请选择需要补做的内容");
        for component in &components {
            let failed = if component == "exports" {
                value
                    .get("exports")
                    .and_then(|v| v.as_object())
                    .is_some_and(|exports| exports.values().any(failed_outcome))
            } else {
                matches!(
                    component.as_str(),
                    "screenshots" | "proofreading" | "summary"
                ) && value.get(component).is_some_and(failed_outcome)
            };
            ensure!(
                failed,
                "这部分已经完成或没有要求处理，无需补做；可以重新导入视频并生成新版"
            );
        }
        if !resend.is_empty() {
            reconcile_receipts(&mut original)?;
            let unknown: BTreeSet<_> = original
                .blocked
                .iter()
                .filter(|request| request.is_uncertain())
                .filter_map(|request| request.request_id.as_ref())
                .collect();
            ensure!(
                !unknown.is_empty() && unknown == resend.iter().collect(),
                "待确认请求已变化，请查看最新范围后再选择是否重新发送"
            );
            // The user is continuing the original task, including a requested
            // summary that the uncertain proofreading prevented from starting.
            // Any summary receipt means it has its own attempt to recover; do
            // not infer permission to repeat it from a proofreading decision.
            if components.iter().any(|part| part == "proofreading")
                && !components.iter().any(|part| part == "summary")
                && original.plan.config.llm.summarize
                && value.get("summary").is_some_and(failed_outcome)
                && original.blocked.iter().any(|request| {
                    request.is_uncertain()
                        && request.purpose.as_deref() == Some("proofreading")
                })
                && !course2md::dispatch::receipts(&original.work_dir)?
                    .iter()
                    .any(|receipt| receipt.purpose == "summary")
            {
                components.push("summary".into());
                components.sort();
            }
        } else {
            ensure!(
                !original.has_uncertain(),
                "仍有请求结果尚未确认，请先查看请求范围"
            );
        }
        let mut plan = original.plan;
        if let Some((service, config)) = ai_service {
            ensure!(
                components
                    .iter()
                    .all(|part| matches!(part.as_str(), "proofreading" | "summary")),
                "服务修复只能用于未完成的校对或摘要"
            );
            plan.ai_service = Some(service);
            plan.config.llm = config.llm;
        }
        let only_exports = components.iter().all(|component| component == "exports");
        plan.config.llm.enabled = components
            .iter()
            .any(|component| component == "proofreading");
        plan.config.llm.summarize = components.iter().any(|component| component == "summary");
        plan.config.llm.vision &= plan.config.llm.enabled;
        plan.options.llm = plan.config.llm.enabled;
        plan.options.summarize = plan.config.llm.summarize;
        plan.options.vision = plan.config.llm.vision;
        if only_exports {
            use course2md::config::OutputFormat;
            let formats: Vec<_> = [OutputFormat::Md, OutputFormat::Html, OutputFormat::Json]
                .into_iter()
                .filter(|format| {
                    value
                        .get("exports")
                        .and_then(|exports| exports.get(format.to_string()))
                        .is_some_and(failed_outcome)
                })
                .collect();
            ensure!(
                !formats.is_empty(),
                "需要补做的导出格式记录不完整，请从笔记中重新选择导出格式"
            );
            plan.options.formats = [OutputFormat::Md, OutputFormat::Html, OutputFormat::Json]
                .map(|format| formats.contains(&format));
            plan.config.defaults.formats = Some(formats);
        }
        plan.operation = course2md::execution::Operation::Reprocess {
            base_version_dir: base,
            components,
            prior_work_dir: Some(original.work_dir),
        };
        plan.config.defaults.transcript_source =
            Some(course2md::config::TranscriptSource::Subtitle);
        let (next, created) = self.enqueue(plan, Some(id.to_owned()))?;
        if created && let Some(task) = self.task_mut(&next) {
            task.resend = resend;
        }
        Ok(next)
    }

    pub fn stop_session(&mut self) {
        let ids: Vec<_> = self
            .tasks
            .iter()
            .filter(|t| !t.state.finished() && t.intent == Intent::Run)
            .map(|t| t.id.clone())
            .collect();
        for id in ids {
            let _ = self.set_intent(&id, Intent::Quit);
        }
    }
}

pub struct Workspace {
    pub state: State,
    pub recovery: Option<String>,
    path: PathBuf,
    /// Last mirror attempt per task and registered location. A normal input
    /// save must not probe the disks holding every historical task.
    mirror_snapshots: RefCell<BTreeMap<String, (TaskRecord, LibraryLocation)>>,
    /// Last successful record write; high-frequency saves skip the disk until
    /// this ages past their interval.
    last_write: Cell<Option<Instant>>,
    /// Record bytes this process last wrote, so the backup copy needs no
    /// re-read of the file it replaces.
    last_record: RefCell<Option<Vec<u8>>>,
}

#[derive(Serialize, Deserialize)]
struct TaskMirror {
    schema: u32,
    task: TaskRecord,
}

fn read_record_bytes(path: &Path) -> Result<Vec<u8>> {
    use std::io::Read;
    ensure!(
        !std::fs::symlink_metadata(path)?.file_type().is_symlink(),
        "任务记录被链接替代，原文件已保留"
    );
    let mut file = std::fs::File::open(path)?;
    let metadata = file.metadata()?;
    ensure!(metadata.is_file(), "任务记录不是普通文件");
    const LIMIT: u64 = 32 * 1024 * 1024;
    ensure!(
        metadata.len() <= LIMIT,
        "任务记录超过可读取范围，原文件已保留"
    );
    let mut bytes = Vec::new();
    file.by_ref().take(LIMIT + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= LIMIT,
        "任务记录超过可读取范围，原文件已保留"
    );
    Ok(bytes)
}

fn validate_record_location(task: &TaskRecord, libraries: &[LibraryLocation]) -> Result<()> {
    ensure!(course2md::execution::valid_id(&task.id), "任务身份无效");
    let library = libraries
        .iter()
        .find(|library| library.id == task.plan.library_id)
        .context("任务保存位置记录缺失")?;
    ensure!(
        library.root.is_absolute()
            && task.work_dir == library.root.join(".course2md/work").join(&task.id),
        "任务工作目录与记录的课程库不匹配"
    );
    ensure!(
        !task.plan.source_id.trim().is_empty() && !task.plan.title.trim().is_empty(),
        "任务来源身份或名称记录缺失"
    );
    Ok(())
}

fn ensure_work_directory(task: &TaskRecord, library: &LibraryLocation) -> Result<()> {
    check_library(library)?;
    let mut current = library.root.clone();
    for component in [".course2md", "work", &task.id] {
        current.push(component);
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) => ensure!(
                metadata.is_dir() && !metadata.file_type().is_symlink(),
                "任务工作目录被其他文件或链接占用，原内容已保留"
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(&current)?
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn pause_recovered_control(task: &TaskRecord) -> Result<()> {
    if !task.work_dir.is_dir() {
        return Ok(());
    }
    let path = task.work_dir.join("control.json");
    let mut control = if path.is_file() {
        let bytes = read_record_bytes(&path)?;
        course2md::checkpoint::atomic_write(
            &task
                .work_dir
                .join(format!("{}.json", new_id("control-before-recovery"))),
            &bytes,
        )?;
        serde_json::from_slice::<serde_json::Value>(&bytes)
            .ok()
            .filter(|value| value.is_object())
            .unwrap_or_else(|| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };
    control["intent"] = serde_json::json!("pause");
    control["resend"] = serde_json::json!([]);
    course2md::checkpoint::atomic_write(&path, &serde_json::to_vec(&control)?)
}

fn failed_outcome(value: &serde_json::Value) -> bool {
    matches!(
        value.get("status").and_then(|v| v.as_str()),
        Some("failed" | "partial")
    )
}

pub fn check_library(location: &LibraryLocation) -> Result<()> {
    ensure!(
        location.root.is_dir(),
        "保存位置暂时不可访问。请连接对应磁盘，原任务与当前输入仍保留。"
    );
    let identity = std::fs::read_to_string(location.root.join(".course2md-library-id")).context(
        "这个保存位置尚未重新关联，请在设置的存储中选择“重新关联此保存位置”。原任务和文件仍保留。",
    )?;
    ensure!(
        identity.trim() == location.id,
        "保存位置对应其他课程库，请恢复原位置或在存储设置中重新登记"
    );
    Ok(())
}

fn create_library_marker(location: &LibraryLocation) -> Result<()> {
    use std::io::Write;
    ensure!(location.root.is_dir(), "保存位置暂时不可访问");
    let target = location.root.join(".course2md-library-id");
    if target.exists() {
        return check_library(location);
    }
    let mut temporary = tempfile::NamedTempFile::new_in(&location.root)?;
    temporary.write_all(location.id.as_bytes())?;
    temporary.as_file().sync_all()?;
    match temporary.persist_noclobber(&target) {
        Ok(_) => course2md::artifact::sync_dir(&location.root)?,
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            check_library(location)?
        }
        Err(error) => return Err(error.error.into()),
    }
    check_library(location)
}

fn reconcile_artifact(task: &mut TaskRecord, location: &LibraryLocation) -> Result<()> {
    if task.artifact.is_some() && !task.exports_only() {
        return Ok(());
    }
    let course = course_dir_name(&task.plan.source_id);
    let version = location.root.join(&course).join("versions").join(&task.id);
    if version.join("manifest.json").is_file() {
        let manifest = course2md::artifact::read_manifest(&version.join("manifest.json"))?;
        ensure!(
            manifest.task_id == task.id && manifest.source_id == task.plan.source_id,
            "已发布笔记与任务身份不匹配，原文件已保留"
        );
        course2md::artifact::validate_version(&version, &manifest)?;
        task.artifact = Some(version);
        task.outcomes = serde_json::to_value(&manifest.outcomes)?;
        task.state = if manifest.partial {
            TaskState::Partial
        } else {
            TaskState::Complete
        };
        task.error = None;
        task.unread = true;
        return Ok(());
    }
    if let course2md::execution::Operation::Reprocess {
        base_version_dir,
        components,
        ..
    } = &task.plan.operation
        && components.iter().all(|component| component == "exports")
        && task.work_dir.join("export-result.json").is_file()
    {
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(task.work_dir.join("export-result.json"))?)?;
        let outcomes = value
            .get("outcomes")
            .and_then(|v| v.as_object())
            .context("导出结果记录不完整")?;
        ensure!(!outcomes.is_empty(), "导出结果记录为空");
        let mut outcomes = outcomes.clone();
        let export_directory =
            task_export_directory(task, location).context("导出位置与任务记录不匹配")?;
        for (format, outcome) in &mut outcomes {
            if outcome.get("status").and_then(|v| v.as_str()) == Some("succeeded") {
                let format = match format.as_str() {
                    "md" => course2md::config::OutputFormat::Md,
                    "html" => course2md::config::OutputFormat::Html,
                    "json" => course2md::config::OutputFormat::Json,
                    _ => continue,
                };
                let expected = export_directory.join(course2md::portable::file_name(format));
                if !expected.is_file() {
                    *outcome = serde_json::to_value(course2md::artifact::Outcome::failed(
                        "导出文件已移动或无法读取，可以重新导出",
                    ))?;
                }
            }
        }
        let partial = outcomes.values().any(failed_outcome);
        let next_outcomes = serde_json::json!({"exports":outcomes});
        let next_state = if partial {
            TaskState::Partial
        } else {
            TaskState::Complete
        };
        task.unread |=
            task.artifact.is_none() || task.outcomes != next_outcomes || task.state != next_state;
        task.artifact = Some(base_version_dir.clone());
        task.outcomes = next_outcomes;
        task.state = next_state;
    }
    Ok(())
}

pub fn reconcile_receipts(task: &mut TaskRecord) -> Result<()> {
    let receipts = course2md::dispatch::receipts(&task.work_dir)?;
    task.blocked.retain(|request| !request.is_uncertain());
    for receipt in receipts.into_iter().filter(|r| {
        matches!(
            r.state,
            course2md::dispatch::State::Sending | course2md::dispatch::State::Uncertain
        )
    }) {
        task.blocked.push(BlockedRequest {
            reason: course2md::dispatch::REASON_UNCERTAIN.into(),
            request_id: Some(receipt.request_id),
            purpose: Some(receipt.purpose),
            description: receipt.description.clone(),
            message: if receipt.description.trim().is_empty() {
                receipt.message.unwrap_or_else(|| {
                    "请求已发送，尚未确认服务是否完成。已保存其他进度，没有自动重新发送。".into()
                })
            } else {
                format!(
                    "{}：{}",
                    receipt.description,
                    receipt.message.unwrap_or_else(|| {
                        "请求结果尚未确认；其他进度已保留，没有自动重新发送。".into()
                    })
                )
            },
        });
    }
    if !task.blocked.is_empty()
        && task.has_uncertain()
        && task.intent == Intent::Run
    {
        task.state = TaskState::Uncertain;
        task.unread = true;
    }
    Ok(())
}

impl Workspace {
    /// The UI explains the registered path and obtains an explicit association choice.
    pub fn reassociate_library(&mut self, id: &str) -> Result<()> {
        let library = self.state.library(id).context("保存位置未登记")?.clone();
        ensure!(
            library.root.is_dir(),
            "保存位置暂时不可访问，请先连接对应磁盘"
        );
        create_library_marker(&library)?;
        self.mirror_snapshots
            .borrow_mut()
            .retain(|_, (_, location)| location.id != id);
        Ok(())
    }

    /// Explicit repair. Originals are archived first; no recovered task may dispatch work.
    pub fn rebuild(root: PathBuf, options: ConversionOptions) -> Result<Self> {
        Self::rebuild_at(
            course2md::config::config_dir().join("desktop-workspace.json"),
            root,
            options,
        )
    }

    pub fn rebuild_at(path: PathBuf, root: PathBuf, options: ConversionOptions) -> Result<Self> {
        let parent = path.parent().context("任务记录缺少保存目录")?;
        std::fs::create_dir_all(parent)?;
        let _lock = course2md::runtime::lock_file(&path.with_extension("lock"))?;
        let backup = path.with_extension("json.bak");
        let originals: Vec<_> = [&path, &backup]
            .into_iter()
            .filter(|p| p.exists())
            .map(|p| Ok((p.clone(), read_record_bytes(p)?)))
            .collect::<Result<_>>()?;
        ensure!(
            !originals.is_empty(),
            "原任务记录不存在，请重新打开应用建立新记录"
        );
        let values: Vec<serde_json::Value> = originals
            .iter()
            .filter_map(|(_, bytes)| serde_json::from_slice(bytes).ok())
            .collect();
        ensure!(
            !values.iter().any(|v| v
                .get("schema")
                .and_then(|v| v.as_u64())
                .is_some_and(|schema| schema > SCHEMA as u64)),
            "这些任务由较新版本创建，请使用对应版本恢复，原文件已保留"
        );
        let archive = parent.join(new_id("workspace-recovery"));
        std::fs::create_dir(&archive).context("无法建立原始记录备份，尚未重建")?;
        for (original, bytes) in &originals {
            let name = original.file_name().context("原记录缺少文件名")?;
            course2md::checkpoint::atomic_write(&archive.join(name), bytes)
                .context("原始记录备份未完成，尚未重建")?;
        }
        let mut state = State::initial(root.clone(), options);
        if let Ok(marker) = std::fs::read_to_string(root.join(".course2md-library-id"))
            && course2md::execution::valid_id(marker.trim())
        {
            state.default_library = marker.trim().into();
            state.libraries[0].id = state.default_library.clone();
            state.drafts[0].library_id = state.default_library.clone();
        }
        // Libraries and tasks are independently recoverable even when another field is damaged.
        let mut locations = Vec::<LibraryLocation>::new();
        let mut records = BTreeMap::<String, TaskRecord>::new();
        let mut drafts = Vec::<Draft>::new();
        for value in &values {
            if let Some(items) = value.get("libraries").and_then(|v| v.as_array()) {
                for item in items {
                    if let Ok(location) = serde_json::from_value::<LibraryLocation>(item.clone())
                        && course2md::execution::valid_id(&location.id)
                        && location.root.is_absolute()
                        && !locations
                            .iter()
                            .any(|l| l.id == location.id || l.root == location.root)
                    {
                        locations.push(location);
                    }
                }
            }
            if let Some(items) = value.get("tasks").and_then(|v| v.as_array()) {
                for item in items {
                    if let Ok(task) = serde_json::from_value::<TaskRecord>(item.clone()) {
                        records.entry(task.id.clone()).or_insert(task);
                    }
                }
            }
            if let Some(items) = value.get("drafts").and_then(|v| v.as_array()) {
                for item in items {
                    if let Ok(draft) = serde_json::from_value::<Draft>(item.clone())
                        && !drafts.iter().any(|d| d.id == draft.id)
                    {
                        drafts.push(draft);
                    }
                }
            }
        }
        if !locations.iter().any(|l| l.root == root) {
            locations.push(state.libraries[0].clone());
        }
        state.libraries = locations;
        state.default_library = values
            .iter()
            .filter_map(|v| v.get("default_library").and_then(|v| v.as_str()))
            .find(|id| state.library(id).is_some())
            .map(str::to_owned)
            .unwrap_or_else(|| state.libraries[0].id.clone());
        if drafts.is_empty() {
            state.drafts[0].library_id = state.default_library.clone();
        } else {
            state.drafts = drafts;
            state.current_draft = values
                .iter()
                .filter_map(|v| v.get("current_draft").and_then(|v| v.as_str()))
                .find(|id| state.drafts.iter().any(|draft| &draft.id == id))
                .map(str::to_owned)
                .unwrap_or_else(|| state.drafts[0].id.clone());
        }
        let mut issues = Vec::new();
        for library in &state.libraries {
            if check_library(library).is_err() {
                continue;
            }
            let work_root = library.root.join(".course2md/work");
            if !work_root.is_dir() {
                continue;
            }
            let root_path = library.root.canonicalize()?;
            ensure!(
                work_root.canonicalize()?.starts_with(&root_path),
                "任务材料位置越出课程库，原记录已保留"
            );
            for entry in std::fs::read_dir(&work_root)? {
                let entry = entry?;
                if !entry.file_type()?.is_dir() || entry.file_type()?.is_symlink() {
                    continue;
                }
                let snapshot = entry.path().join("task-record.json");
                if !snapshot.is_file() {
                    continue;
                }
                let restored = read_record_bytes(&snapshot).and_then(|bytes| {
                    let mirror: TaskMirror = serde_json::from_slice(&bytes)?;
                    ensure!(
                        mirror.schema == SCHEMA && mirror.task.work_dir == entry.path(),
                        "任务镜像身份不匹配"
                    );
                    validate_record_location(&mirror.task, &state.libraries)?;
                    Ok(mirror.task)
                });
                match restored {
                    Ok(task) => {
                        records.entry(task.id.clone()).or_insert(task);
                    }
                    Err(_) => issues.push(format!(
                        "{} 的任务参数仍无法读取，材料已保留",
                        entry.file_name().to_string_lossy()
                    )),
                }
            }
        }
        for (_, mut task) in records {
            if validate_record_location(&task, &state.libraries).is_err() {
                issues.push(format!(
                    "《{}》的位置或身份记录不完整，原记录已保留",
                    task.plan.title
                ));
                continue;
            }
            // Repair loses evidence of the last user intent; never infer permission to send.
            task.resend.clear();
            if !task.state.finished() {
                task.intent = Intent::Pause;
                task.state = TaskState::Paused;
                task.error =
                    Some("任务记录已重建，已保存的材料仍保留。确认任务后可以继续。".into());
                task.unread = true;
            }
            if let Some(library) = state.library(&task.plan.library_id)
                && check_library(library).is_ok()
            {
                if let Err(error) = reconcile_artifact(&mut task, library)
                    .and_then(|_| reconcile_receipts(&mut task))
                {
                    task.state = TaskState::NeedsAttention;
                    task.error = Some(format!("已保留材料，结果记录尚未确认：{error:#}"));
                }
                pause_recovered_control(&task)
                    .context("无法保存恢复后的暂停指令，尚未替换原任务记录")?;
            }
            state.tasks.push(task);
        }
        state.tasks.sort_by_key(|task| task.created);
        let existing: BTreeSet<_> = state.tasks.iter().map(|task| task.id.clone()).collect();
        for task in &mut state.tasks {
            if task
                .handled_by
                .as_ref()
                .is_some_and(|id| !existing.contains(id))
            {
                task.handled_by = None;
            }
        }
        // Preserve reader mappings and migration backups only when their own records decode.
        for value in &values {
            if state.positions.is_empty() {
                state.positions = value
                    .get("positions")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or_default();
            }
            if state.reader_sources.is_empty() {
                state.reader_sources = value
                    .get("reader_sources")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or_default();
            }
            if state.storage_backups.is_empty() {
                state.storage_backups = value
                    .get("storage_backups")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or_default();
            }
        }
        state.retain_current_input();
        state.recover();
        course2md::checkpoint::atomic_write(&path, &serde_json::to_vec_pretty(&state)?)?;
        let recovery = format!(
            "已重建 {} 项任务记录，未启动处理。原始记录保存在 {}。{}",
            state.tasks.len(),
            archive.display(),
            if issues.is_empty() {
                String::new()
            } else {
                issues.join("；")
            }
        );
        Ok(Self {
            state,
            recovery: Some(recovery),
            path,
            mirror_snapshots: Default::default(),
            last_write: Cell::new(None),
            last_record: RefCell::new(None),
        })
    }

    pub fn open(root: PathBuf, options: ConversionOptions) -> Result<Self> {
        Self::open_at(
            course2md::config::config_dir().join("desktop-workspace.json"),
            root,
            options,
        )
    }
    pub fn open_at(path: PathBuf, root: PathBuf, options: ConversionOptions) -> Result<Self> {
        let mut recovery = None;
        if path.is_file()
            && let Ok(bytes) = read_record_bytes(&path)
            && let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes)
            && value
                .get("schema")
                .and_then(|v| v.as_u64())
                .is_some_and(|schema| schema > SCHEMA as u64)
        {
            bail!("任务记录由较新版本创建，请使用对应版本打开。原记录和备份均保留，尚未启动任务。");
        }
        let mut state = if path.exists() {
            match Self::read(&path) {
                Ok(state) => state,
                Err(error) => {
                    let backup = path.with_extension("json.bak");
                    match Self::read(&backup) {
                        Ok(state) => {
                            let saved =
                                path.with_extension(format!("{}.damaged", new_id("backup")));
                            std::fs::copy(&path, &saved).context("无法保留损坏的任务记录")?;
                            recovery = Some(
                                "已从备份恢复输入和任务。无法确认最后一次操作，未完成任务已暂停。"
                                    .into(),
                            );
                            let mut state = state;
                            for task in &mut state.tasks {
                                if !task.state.finished() {
                                    task.intent = Intent::Pause;
                                    task.state = TaskState::Paused;
                                    task.resend.clear();
                                }
                            }
                            state
                        }
                        Err(_) => {
                            return Err(error).context("输入和任务记录无法读取，原文件已保留");
                        }
                    }
                }
            }
        } else {
            let mut initial = State::initial(root.clone(), options);
            match std::fs::create_dir_all(&root) {
                Ok(()) => {
                    let marker = root.join(".course2md-library-id");
                    if let Ok(saved) = std::fs::read_to_string(&marker) {
                        if !saved.trim().is_empty() {
                            let id = saved.trim().to_owned();
                            initial.libraries[0].id = id.clone();
                            initial.default_library = id.clone();
                            initial.drafts[0].library_id = id;
                        }
                    } else if let Err(error) = course2md::checkpoint::atomic_write(
                        &marker,
                        initial.default_library.as_bytes(),
                    ) {
                        recovery = Some(format!(
                            "保存位置尚未准备好：{error:#}。可以在设置的存储中选择其他位置。"
                        ));
                    }
                }
                Err(error) => {
                    recovery = Some(format!(
                        "无法创建默认保存位置 {}：{error}。可以在设置的存储中选择其他位置。",
                        root.display()
                    ))
                }
            }
            initial
        };
        state.recover();
        for task in &mut state.tasks {
            let result = state
                .libraries
                .iter()
                .find(|location| location.id == task.plan.library_id)
                .map(|location| reconcile_artifact(task, location))
                .unwrap_or(Ok(()))
                .and_then(|_| reconcile_receipts(task));
            if let Err(error) = result {
                task.state = TaskState::NeedsAttention;
                task.error = Some(format!("请求记录暂时无法读取，尚未发送新请求：{error:#}"));
            }
        }
        Ok(Self {
            state,
            recovery,
            path,
            mirror_snapshots: Default::default(),
            last_write: Cell::new(None),
            last_record: RefCell::new(None),
        })
    }

    fn read(path: &Path) -> Result<State> {
        let state: State = serde_json::from_slice(
            &read_record_bytes(path).with_context(|| format!("读取 {}", path.display()))?,
        )?;
        ensure!(
            state.schema == SCHEMA,
            "任务记录版本不受支持，请使用创建这些任务的应用版本"
        );
        ensure!(
            state.library(&state.default_library).is_some(),
            "默认保存位置记录缺失"
        );
        ensure!(state.draft().is_some(), "当前输入记录缺失");
        let ids: BTreeSet<_> = state.tasks.iter().map(|t| &t.id).collect();
        ensure!(ids.len() == state.tasks.len(), "任务记录包含重复身份");
        for task in &state.tasks {
            validate_record_location(task, &state.libraries)?;
        }
        Ok(state)
    }
    pub fn transaction<T>(&mut self, edit: impl FnOnce(&mut State) -> Result<T>) -> Result<T> {
        let mut next = self.state.clone();
        let result = edit(&mut next)?;
        self.write(&next)?;
        self.state = next;
        Ok(result)
    }
    pub fn save(&self) -> Result<()> {
        self.write(&self.state)
    }
    /// Throttled progress save for the task event stream. Stage transitions,
    /// completion and exit flush through `save`; steady progress reaches the
    /// disk at most once per interval. Returns true when this call wrote.
    pub fn save_progress(&self) -> Result<bool> {
        if self
            .last_write
            .get()
            .is_some_and(|last| last.elapsed() < PROGRESS_SAVE_INTERVAL)
        {
            return Ok(false);
        }
        self.write(&self.state)?;
        Ok(true)
    }
    fn write(&self, state: &State) -> Result<()> {
        let parent = self.path.parent().context("任务记录缺少保存目录")?;
        std::fs::create_dir_all(parent)?;
        let _lock = course2md::runtime::lock_file(&self.path.with_extension("lock"))?;
        // A mirror is recovery evidence, never authority to resume. The main record
        // remains unchanged if a required mirror write fails.
        for task in &state.tasks {
            let Some(library) = state.library(&task.plan.library_id) else {
                continue;
            };
            if self
                .mirror_snapshots
                .borrow()
                .get(&task.id)
                .is_some_and(|(previous, location)| previous == task && location == library)
            {
                continue;
            }
            if check_library(library).is_err() {
                // Retry an unavailable location when the task/location changes,
                // it is explicitly reassociated, or the application restarts.
                self.mirror_snapshots
                    .borrow_mut()
                    .insert(task.id.clone(), (task.clone(), library.clone()));
                continue;
            }
            validate_record_location(task, &state.libraries)?;
            ensure_work_directory(task, library)?;
            let mirror = task.work_dir.join("task-record.json");
            let bytes = serde_json::to_vec_pretty(&TaskMirror {
                schema: SCHEMA,
                task: task.clone(),
            })?;
            if std::fs::read(&mirror).ok().as_deref() != Some(&bytes) {
                course2md::checkpoint::atomic_write(&mirror, &bytes)
                    .context("任务恢复副本未能保存，原任务记录仍保留")?;
            }
            self.mirror_snapshots
                .borrow_mut()
                .insert(task.id.clone(), (task.clone(), library.clone()));
        }
        let backup = self.path.with_extension("json.bak");
        let cached = self.last_record.borrow();
        if let Some(bytes) = cached.as_deref() {
            course2md::checkpoint::atomic_write(&backup, bytes)?;
        } else if self.path.is_file() && Self::read(&self.path).is_ok() {
            let bytes = std::fs::read(&self.path)?;
            course2md::checkpoint::atomic_write(&backup, &bytes)?;
        }
        drop(cached);
        let bytes = serde_json::to_vec_pretty(state)?;
        course2md::checkpoint::atomic_write(&self.path, &bytes)?;
        *self.last_record.borrow_mut() = Some(bytes);
        self.last_write.set(Some(Instant::now()));
        Ok(())
    }
    pub fn storage_path(&self) -> &Path {
        &self.path
    }
    pub fn register_library(
        &mut self,
        root: PathBuf,
        name: String,
        make_default: bool,
    ) -> Result<String> {
        ensure!(root.is_dir(), "请选择已有文件夹；新建课程库请使用新建操作");
        let canonical = root.canonicalize()?;
        if let Some(existing) = self
            .state
            .libraries
            .iter()
            .find(|l| l.root.canonicalize().ok().as_ref() == Some(&canonical))
        {
            let id = existing.id.clone();
            if make_default {
                self.transaction(|s| {
                    s.default_library = id.clone();
                    Ok(())
                })?;
            }
            return Ok(id);
        }
        let probe = tempfile::NamedTempFile::new_in(&canonical).context("所选位置不能写入")?;
        drop(probe);
        let identity = canonical.join(".course2md-library-id");
        let id = if identity.is_file() {
            let id = std::fs::read_to_string(&identity)?.trim().to_owned();
            if id.is_empty() {
                bail!("课程库身份记录为空，请检查所选位置");
            }
            if self.state.libraries.iter().any(|l| l.id == id) {
                bail!("这个库已经登记在另一个位置，请使用移动课程库操作");
            }
            id
        } else {
            let id = new_id("library");
            course2md::checkpoint::atomic_write(&identity, id.as_bytes())?;
            id
        };
        self.transaction(|s| {
            s.libraries.push(LibraryLocation {
                id: id.clone(),
                name,
                root: canonical,
                previous_roots: Vec::new(),
            });
            if make_default {
                s.default_library = id.clone();
            }
            Ok(id)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn source(input: &str, title: &str) -> crate::source::Source {
        serde_json::from_value(serde_json::json!({"input":input,"title":title,"author":"","duration":60.0,"cover":null,"cover_error":null})).unwrap()
    }
    fn plan(library: &str) -> TaskPlan {
        TaskPlan {
            operation: Default::default(),
            source: source("lecture.mp4", "课程 A"),
            source_id: "video-a".into(),
            title: "课程 A".into(),
            library_id: library.into(),
            folder: None,
            options: ConversionOptions::default(),
            subtitle: None,
            config: ConfigFile::default(),
            asr_service: None,
            ai_service: None,
        }
    }
    #[test]
    fn failed_commit_does_not_publish_an_unpersisted_task() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("state");
        let mut ws = Workspace::open_at(
            target.clone(),
            dir.path().join("library"),
            Default::default(),
        )
        .unwrap();
        std::fs::create_dir(&target).unwrap();
        let task = plan(&ws.state.default_library);
        assert!(ws.transaction(|state| state.enqueue(task, None)).is_err());
        assert!(ws.state.tasks.is_empty());
    }
    #[test]
    fn frozen_task_survives_input_changes_without_storing_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("state.json");
        let mut ws =
            Workspace::open_at(file.clone(), dir.path().join("library"), Default::default())
                .unwrap();
        let mut task = plan(&ws.state.default_library);
        task.config.asr_api.api_key = "asr-secret-do-not-store".into();
        task.config.llm.api_key = "ai-secret-do-not-store".into();
        let id = ws.transaction(|state| state.enqueue(task, None)).unwrap().0;
        ws.transaction(|s| {
            s.draft_mut().unwrap().change_source("B.mp4".into());
            s.task_mut(&id).unwrap().state = TaskState::Running;
            Ok(())
        })
        .unwrap();
        let restored =
            Workspace::open_at(file.clone(), dir.path().into(), Default::default()).unwrap();
        assert_eq!(
            restored.state.task(&id).unwrap().plan.source.input,
            "lecture.mp4"
        );
        assert_eq!(restored.state.task(&id).unwrap().state, TaskState::Queued);
        assert_eq!(restored.state.draft().unwrap().input, "B.mp4");
        for path in [file.clone(), file.with_extension("json.bak")] {
            assert!(
                !std::fs::read_to_string(path)
                    .unwrap()
                    .contains("secret-do-not-store")
            );
        }
    }

    #[test]
    fn input_only_save_does_not_probe_an_unchanged_task_mirror() {
        let dir = tempfile::tempdir().unwrap();
        let mut ws = test_workspace(dir.path());
        let task = plan(&ws.state.default_library);
        let id = ws.transaction(|state| state.enqueue(task, None)).unwrap().0;
        let mirror = ws
            .state
            .task(&id)
            .unwrap()
            .work_dir
            .join("task-record.json");
        std::fs::remove_file(&mirror).unwrap();
        std::fs::create_dir(&mirror).unwrap();
        ws.transaction(|state| {
            state
                .draft_mut()
                .unwrap()
                .change_source("next-video.mp4".into());
            Ok(())
        })
        .unwrap();
        assert!(mirror.is_dir());
        let before = std::fs::read(ws.storage_path()).unwrap();
        assert!(
            ws.transaction(|state| state.set_intent(&id, Intent::Pause))
                .is_err()
        );
        assert_eq!(std::fs::read(ws.storage_path()).unwrap(), before);
        assert_eq!(ws.state.task(&id).unwrap().intent, Intent::Run);
    }

    #[test]
    fn save_updates_in_place_task_events_and_initially_missing_mirrors() {
        let dir = tempfile::tempdir().unwrap();
        let mut ws = test_workspace(dir.path());
        let task = plan(&ws.state.default_library);
        let id = ws.transaction(|state| state.enqueue(task, None)).unwrap().0;
        let mirror = ws
            .state
            .task(&id)
            .unwrap()
            .work_dir
            .join("task-record.json");
        ws.state.task_mut(&id).unwrap().logs.push("实际进度".into());
        ws.save().unwrap();
        let saved: TaskMirror = serde_json::from_slice(&std::fs::read(&mirror).unwrap()).unwrap();
        assert_eq!(saved.task.logs, ["实际进度"]);
        std::fs::remove_file(&mirror).unwrap();
        let restored = test_workspace(dir.path());
        restored.save().unwrap();
        let repaired: TaskMirror = serde_json::from_slice(&std::fs::read(mirror).unwrap()).unwrap();
        assert_eq!(repaired.task.logs, ["实际进度"]);
    }

    #[test]
    fn source_revisions_and_default_overrides_are_independent() {
        let mut draft = Draft::new(true, "lib".into(), Default::default());
        draft.change_source("A".into());
        let old = draft.revision;
        draft.change_source("B".into());
        assert!(!draft.accept_source(old, source("A", "title-a")));
        assert!(draft.accept_source(draft.revision, source("B", "title-b")));
        draft.options.llm = true;
        draft.overrides.insert(Override::Proofread);
        let mut defaults = ConversionOptions::default();
        defaults.summarize = true;
        draft.inherit(&defaults);
        assert!(draft.options.llm && draft.options.summarize);
    }

    #[test]
    fn resetting_ai_choices_resumes_inheritance_without_changing_other_overrides_or_services() {
        let mut draft = Draft::new(true, "lib".into(), Default::default());
        draft.change_source("current-video".into());
        draft.title = "Current note".into();
        draft.custom_title = true;
        draft.folder = Some(42);
        draft.options.provider = 2;
        draft.options.source_mode = 1;
        draft.options.keep_video = true;
        draft.options.formats = [true, false, true];
        draft.options.llm = true;
        draft.options.summarize = false;
        draft.options.vision = true;
        draft.asr_service = Some("fixed-speech-service".into());
        draft.ai_service = Some("fixed-ai-service".into());
        draft.overrides = [
            Override::Provider,
            Override::TextSource,
            Override::Proofread,
            Override::Summary,
            Override::Vision,
            Override::KeepVideo,
            Override::Formats,
        ]
        .into_iter()
        .collect();
        let defaults = ConversionOptions {
            llm: false,
            summarize: true,
            vision: false,
            ..Default::default()
        };
        let mut expected = draft.clone();
        expected.options.llm = false;
        expected.options.summarize = true;
        expected.options.vision = false;
        expected.overrides = [
            Override::Provider,
            Override::TextSource,
            Override::KeepVideo,
            Override::Formats,
        ]
        .into_iter()
        .collect();
        draft.reset_ai_overrides(&defaults);
        assert_eq!(draft, expected);

        let later_defaults = ConversionOptions {
            llm: true,
            summarize: false,
            vision: true,
            ..Default::default()
        };
        draft.inherit(&later_defaults);
        expected.options.llm = true;
        expected.options.summarize = false;
        expected.options.vision = true;
        expected.options.resume = true;
        assert_eq!(draft, expected);
    }

    #[test]
    fn a_second_video_uses_defaults_without_mutating_the_submitted_plan() {
        let dir = tempfile::tempdir().unwrap();
        let mut ws = test_workspace(dir.path());
        let submitted = plan(&ws.state.default_library);
        let task = ws.state.enqueue(submitted.clone(), None).unwrap().0;
        let input = ws.state.draft_mut().unwrap();
        input.input = "first.mp4".into();
        input.title = "Only this video".into();
        input.custom_title = true;
        input.folder = Some(42);
        input.options.llm = true;
        input.overrides.insert(Override::Proofread);
        input.ai_service = Some("task-specific-service".into());
        input.base_config = Some(ConfigFile::default());
        input.submitted_task = Some(task.clone());
        let first_id = input.id.clone();
        let defaults = ConversionOptions::default();
        assert!(!ws.state.prepare_next_import("first.mp4", defaults.clone()));
        assert_eq!(ws.state.draft().unwrap().id, first_id);
        assert!(ws.state.prepare_next_import("second.mp4", defaults.clone()));
        let input = ws.state.draft().unwrap();
        assert_ne!(input.id, first_id);
        assert_eq!(input.input, "second.mp4");
        assert_eq!(input.options, defaults);
        assert_eq!(input.folder, Some(42));
        assert!(input.overrides.is_empty());
        assert!(input.ai_service.is_none() && input.base_config.is_none());
        assert!(input.title.is_empty() && !input.custom_title);
        assert!(ws.state.task(&task).unwrap().plan == submitted);
        let input = ws.state.draft_mut().unwrap();
        input.options.llm = true;
        input.overrides.insert(Override::Proofread);
        assert!(!ws.state.prepare_next_import("revised-second.mp4", defaults));
        assert!(ws.state.draft().unwrap().options.llm);
    }

    #[test]
    fn submitted_input_routes_use_the_new_default_without_moving_existing_tasks() {
        let original_root = tempfile::tempdir().unwrap();
        let next_root = tempfile::tempdir().unwrap();
        let mut ws = test_workspace(original_root.path());
        let original_library = ws.state.default_library.clone();
        let mut submitted = plan(&original_library);
        submitted.folder = Some(42);
        let task = ws.state.enqueue(submitted.clone(), None).unwrap().0;
        let input = ws.state.draft_mut().unwrap();
        input.input = "first.mp4".into();
        input.folder = Some(42);
        input.submitted_task = Some(task.clone());
        let next_library = ws
            .register_library(next_root.path().to_owned(), "Next library".into(), true)
            .unwrap();
        let unchanged_input = ws.state.draft().unwrap().clone();
        assert!(
            !ws.state
                .prepare_next_import("first.mp4", Default::default())
        );
        assert_eq!(ws.state.draft().unwrap(), &unchanged_input);

        for switch_kind in [false, true] {
            let mut state = ws.state.clone();
            if switch_kind {
                state.switch_source_kind(!unchanged_input.online, Default::default());
            } else {
                assert!(state.prepare_next_import("second.mp4", Default::default()));
            }
            let next = state.draft().unwrap();
            assert_eq!(next.library_id, next_library);
            assert_eq!(next.folder, None, "folder IDs belong to their library");
            assert!(state.task(&task).unwrap().plan == submitted);
        }
    }

    #[test]
    fn changing_the_default_keeps_an_unsubmitted_destination_and_folder() {
        let original_root = tempfile::tempdir().unwrap();
        let next_root = tempfile::tempdir().unwrap();
        let mut ws = test_workspace(original_root.path());
        let original_library = ws.state.default_library.clone();
        let input = ws.state.draft_mut().unwrap();
        input.input = "current.mp4".into();
        input.folder = Some(42);
        let online = input.online;
        ws.register_library(next_root.path().to_owned(), "Next library".into(), true)
            .unwrap();
        assert!(
            !ws.state
                .prepare_next_import("revised.mp4", Default::default())
        );
        assert_eq!(ws.state.draft().unwrap().library_id, original_library);
        assert_eq!(ws.state.draft().unwrap().folder, Some(42));
        ws.state.switch_source_kind(!online, Default::default());
        assert_eq!(ws.state.draft().unwrap().library_id, original_library);
        assert_eq!(ws.state.draft().unwrap().folder, Some(42));
    }

    #[test]
    fn replacing_the_form_keeps_submitted_task_plans() {
        let dir = tempfile::tempdir().unwrap();
        let mut ws = test_workspace(dir.path());
        let library = ws.state.default_library.clone();
        let submitted = plan(&library);
        let id = ws.state.enqueue(submitted.clone(), None).unwrap().0;
        let original_input = ws.state.current_draft.clone();
        let input = ws.state.draft_mut().unwrap();
        input.change_source("old-video.mp4".into());
        input.title = "旧输入名称".into();
        input.custom_title = true;
        input.folder = Some(42);
        input.subtitle = Some("old.srt".into());
        input.submitted_task = Some(id.clone());
        input.options.llm = true;
        input.overrides.insert(Override::Proofread);
        let defaults = ConversionOptions::default();
        for _ in 0..8 {
            ws.state.reset_input(true, defaults.clone(), None);
            let input = ws.state.draft().unwrap();
            assert_ne!(input.id, original_input);
            assert_eq!(ws.state.drafts.len(), 1);
            assert!(input.input.is_empty() && input.title.is_empty());
            assert!(input.source.is_none() && input.subtitle.is_none());
            assert!(input.submitted_task.is_none() && input.retry_of.is_none());
            assert!(input.overrides.is_empty());
            assert_eq!(input.options, defaults);
            assert_eq!(input.folder, None);
            assert!(ws.state.task(&id).unwrap().plan == submitted);
        }
        ws.state
            .reset_input(false, defaults, Some((library.clone(), 7)));
        assert_eq!(ws.state.draft().unwrap().folder, Some(7));
        assert_eq!(ws.state.draft().unwrap().library_id, library);
        ws.transaction(|_| Ok(())).unwrap();
        let restored = test_workspace(dir.path());
        assert_eq!(restored.state.drafts.len(), 1);
        assert_eq!(restored.state.draft().unwrap(), ws.state.draft().unwrap());
        assert!(restored.state.task(&id).unwrap().plan == submitted);
    }

    #[test]
    fn source_mode_switch_preserves_unsubmitted_choices_but_resets_submitted_overrides() {
        let mut state = State::initial("/tmp/library".into(), Default::default());
        let input = state.draft_mut().unwrap();
        input.change_source("https://example.test/online-a".into());
        input.title = "在线课程 A".into();
        input.custom_title = true;
        input.source = Some(source("https://example.test/online-a", "A"));
        input.subtitle = Some("captions.srt".into());
        input.folder = Some(42);
        input.options.formats = [false, true, true];
        input.options.llm = true;
        input.overrides.insert(Override::Formats);
        input.ai_service = Some("ai-service".into());
        input.retry_of = Some("older-task".into());
        let before = input.clone();
        state.switch_source_kind(true, Default::default());
        assert_eq!(state.draft().unwrap(), &before);
        state.switch_source_kind(false, Default::default());
        let local = state.draft().unwrap();
        assert_ne!(local.id, before.id);
        assert!(!local.online);
        assert!(local.input.is_empty() && local.title.is_empty());
        assert!(local.source.is_none() && local.subtitle.is_none());
        assert!(local.retry_of.is_none() && local.submitted_task.is_none());
        assert_eq!(local.options, before.options);
        assert_eq!(local.folder, before.folder);
        assert_eq!(local.ai_service, before.ai_service);
        assert_eq!(local.overrides, before.overrides);
        state
            .draft_mut()
            .unwrap()
            .change_source("local-b.mp4".into());
        state.switch_source_kind(true, Default::default());
        assert_eq!(state.drafts.len(), 1);
        assert!(state.draft().unwrap().online);
        assert!(state.draft().unwrap().input.is_empty());
        assert_eq!(state.draft().unwrap().options, before.options);
        state.draft_mut().unwrap().submitted_task = Some("submitted-task".into());
        let defaults = ConversionOptions::default();
        state.switch_source_kind(false, defaults.clone());
        let fresh = state.draft().unwrap();
        assert_eq!(fresh.options, defaults);
        assert!(fresh.overrides.is_empty() && fresh.ai_service.is_none());
        assert!(fresh.retry_of.is_none() && fresh.submitted_task.is_none());
        assert_eq!(fresh.folder, before.folder);
    }

    #[test]
    fn delayed_results_cannot_cross_new_note_or_mode_changes_even_with_the_same_revision() {
        let mut state = State::initial("/tmp/library".into(), Default::default());
        state
            .draft_mut()
            .unwrap()
            .change_source("same-input".into());
        let pending = state.draft().unwrap().clone();
        assert!(state.matches_input(&pending.id, pending.revision));
        state.reset_input(true, Default::default(), None);
        state
            .draft_mut()
            .unwrap()
            .change_source("same-input".into());
        assert_eq!(state.draft().unwrap().revision, pending.revision);
        assert!(!state.matches_input(&pending.id, pending.revision));
        let after_new = state.draft().unwrap().clone();
        state.switch_source_kind(false, Default::default());
        state
            .draft_mut()
            .unwrap()
            .change_source("same-input".into());
        assert_eq!(state.draft().unwrap().revision, after_new.revision);
        assert!(!state.matches_input(&after_new.id, after_new.revision));
        let current = state.draft().unwrap().clone();
        assert!(state.matches_input(&current.id, current.revision));
        assert!(
            state
                .draft_mut()
                .unwrap()
                .accept_source(current.revision, source("same-input", "当前读取结果"),)
        );
        assert_eq!(state.draft().unwrap().title, "当前读取结果");
        assert_eq!(state.drafts.len(), 1);
    }

    #[test]
    fn explicit_stop_survives_restart_while_duplicate_names_do_not_create_work() {
        let mut state = State::initial("/tmp/library".into(), Default::default());
        let mut task = plan(&state.default_library);
        let id = state.enqueue(task.clone(), None).unwrap().0;
        task.title = "另一个显示名".into();
        task.folder = Some(42);
        assert_eq!(state.enqueue(task, None).unwrap(), (id.clone(), false));
        state.task_mut(&id).unwrap().state = TaskState::Running;
        state.stop_session();
        state.recover();
        assert_eq!(state.task(&id).unwrap().state, TaskState::Paused);
        assert!(state.next_task().is_none());
    }

    #[test]
    fn adjusting_a_failed_task_replaces_the_form_and_preserves_the_task_after_restart() {
        let dir = tempfile::tempdir().unwrap();
        let mut ws = test_workspace(dir.path());
        let mut original = plan(&ws.state.default_library);
        original.title = "A 的手工名称".into();
        original.folder = Some(42);
        original.subtitle = Some(dir.path().join("french.srt"));
        original.options.llm = true;
        original.options.vision = true;
        original.options.formats = [false, false, true];
        original.asr_service = Some("asr-version-a".into());
        original.ai_service = Some("ai-version-a".into());
        original.config.defaults.asr_model = Some("qwen3-0.6b".into());
        let id = ws.state.enqueue(original.clone(), None).unwrap().0;
        ws.state.task_mut(&id).unwrap().state = TaskState::Running;
        ws.state.reset_input(true, Default::default(), None);
        let b = ws.state.draft_mut().unwrap();
        b.change_source("online-B".into());
        b.title = "B 的未提交计划".into();
        b.folder = Some(7);
        b.options.summarize = true;
        let b = b.clone();
        assert!(ws.state.adjust_task(&id).is_err());
        assert_eq!(ws.state.draft().unwrap(), &b);

        ws.state.task_mut(&id).unwrap().state = TaskState::NeedsAttention;
        ws.state.set_intent(&id, Intent::Run).unwrap();
        assert!(ws.state.next_task().unwrap().plan == original);
        assert_eq!(ws.state.draft().unwrap(), &b);
        ws.state.set_intent(&id, Intent::Pause).unwrap();
        ws.transaction(|state| state.adjust_task(&id)).unwrap();
        let revised = ws.state.draft_mut().unwrap();
        let options = revised.options.clone();
        revised.inherit(&ConversionOptions::default());
        assert_eq!(revised.options, options);
        assert_eq!(revised.input, original.source.input);
        assert_eq!(revised.source.as_ref(), Some(&original.source));
        assert_eq!(revised.title, original.title);
        assert_eq!(revised.folder, original.folder);
        assert_eq!(revised.library_id, original.library_id);
        assert_eq!(revised.subtitle, original.subtitle);
        assert_eq!(revised.asr_service, original.asr_service);
        assert_eq!(revised.ai_service, original.ai_service);
        assert_eq!(revised.retry_of.as_deref(), Some(id.as_str()));
        assert!(revised.base_config.as_ref() == Some(&original.config));
        let revision = revised.clone();
        drop(ws);
        let mut reopened = test_workspace(dir.path());
        assert_eq!(reopened.state.draft().unwrap(), &revision);
        assert_eq!(reopened.state.drafts.len(), 1);
        assert!(reopened.state.drafts.iter().all(|input| input.id != b.id));
        assert!(reopened.state.task(&id).unwrap().plan == original);
        assert_eq!(reopened.state.task(&id).unwrap().state, TaskState::Paused);
        let mut adjusted = original;
        adjusted.options.summarize = true;
        adjusted.config.llm.summarize = true;
        let (followup, created) = reopened
            .state
            .enqueue(adjusted.clone(), Some(id.clone()))
            .unwrap();
        assert!(created);
        assert_eq!(
            reopened.state.enqueue(adjusted, Some(id.clone())).unwrap(),
            (followup, false)
        );
        assert!(reopened.state.adjust_task(&id).is_err());
        assert_eq!(reopened.state.drafts.len(), 1);
        assert!(reopened.state.drafts.iter().all(|input| input.id != b.id));
    }

    #[test]
    fn cold_start_resumes_only_unstopped_work_and_keeps_unknown_requests_blocked() {
        let dir = tempfile::tempdir().unwrap();
        let mut ws = test_workspace(dir.path());
        let mut ids = Vec::new();
        for (index, intent) in [
            Intent::Run,
            Intent::Pause,
            Intent::Quit,
            Intent::Cancel,
            Intent::Run,
            Intent::Run,
        ]
        .into_iter()
        .enumerate()
        {
            let mut plan = plan(&ws.state.default_library);
            plan.source_id = format!("source-{index}");
            plan.source.input = format!("video-{index}.mp4");
            let id = ws.state.enqueue(plan, None).unwrap().0;
            let task = ws.state.task_mut(&id).unwrap();
            task.state = TaskState::Running;
            task.intent = intent;
            if index == 4 {
                write_unknown(task, 1);
            }
            if index == 5 {
                task.state = TaskState::NeedsAttention;
                task.error = Some("保留这次失败原因".into());
                task.unread = true;
            }
            ids.push(id);
        }
        ws.transaction(|_| Ok(())).unwrap();
        let plans = ws
            .state
            .tasks
            .iter()
            .map(|task| task.plan.clone())
            .collect::<Vec<_>>();
        drop(ws);
        let mut reopened = test_workspace(dir.path());
        for ((id, expected), plan) in ids
            .iter()
            .zip([
                TaskState::Queued,
                TaskState::Paused,
                TaskState::Paused,
                TaskState::Cancelled,
                TaskState::Uncertain,
                TaskState::NeedsAttention,
            ])
            .zip(plans)
        {
            let task = reopened.state.task(id).unwrap();
            assert_eq!(task.state, expected);
            assert!(task.plan == plan);
        }
        assert_eq!(reopened.state.next_task().unwrap().id, ids[0]);
        let unknown = reopened.state.task(&ids[4]).unwrap();
        assert_eq!(unknown.blocked.len(), 1);
        assert!(unknown.unread && unknown.resend.is_empty());
        reopened
            .transaction(|state| {
                state.stop_session();
                Ok(())
            })
            .unwrap();
        drop(reopened);
        let stopped = test_workspace(dir.path());
        assert!(stopped.state.next_task().is_none());
        let unknown = stopped.state.task(&ids[4]).unwrap();
        assert_eq!(unknown.state, TaskState::Uncertain);
        assert_eq!(unknown.intent, Intent::Quit);
        assert!(unknown.unread && unknown.resend.is_empty());
        let failed = stopped.state.task(&ids[5]).unwrap();
        assert_eq!(failed.state, TaskState::NeedsAttention);
        assert_eq!(failed.intent, Intent::Quit);
        assert_eq!(failed.error.as_deref(), Some("保留这次失败原因"));
        assert!(failed.unread);
    }

    fn test_workspace(dir: &Path) -> Workspace {
        Workspace::open_at(
            dir.join("workspace.json"),
            dir.join("library"),
            Default::default(),
        )
        .unwrap()
    }

    fn publish_note(state: &State, task_id: &str, partial: bool) -> PathBuf {
        let mut outcomes = course2md::artifact::Outcomes::default();
        outcomes.transcript = course2md::artifact::Outcome::succeeded();
        if partial {
            outcomes.proofreading = course2md::artifact::Outcome::failed("校对未完成");
        }
        publish_note_with_outcomes(state, task_id, outcomes)
    }

    fn publish_note_with_outcomes(
        state: &State,
        task_id: &str,
        outcomes: course2md::artifact::Outcomes,
    ) -> PathBuf {
        publish_note_with_exports(state, task_id, outcomes, &[])
    }

    fn publish_note_with_exports(
        state: &State,
        task_id: &str,
        outcomes: course2md::artifact::Outcomes,
        formats: &[course2md::config::OutputFormat],
    ) -> PathBuf {
        use course2md::{artifact, timeline};
        let task = state.task(task_id).unwrap();
        let course_id = crate::workspace::course_dir_name(&task.plan.source_id);
        let target = artifact::Target {
            task_id: task.id.clone(),
            version_id: task.id.clone(),
            source_id: task.plan.source_id.clone(),
            course_id: course_id.clone(),
            course_dir: state
                .library(&task.plan.library_id)
                .unwrap()
                .root
                .join(course_id),
        };
        let meta = course2md::fetch::VideoMeta {
            title: task.plan.title.clone(),
            uploader: String::new(),
            duration: 20.,
            webpage_url: task.plan.source.input.clone(),
            extractor: String::new(),
            id: String::new(),
        };
        let sections = vec![timeline::Section {
            t: 0.,
            end: 20.,
            image: String::new(),
            speech: vec![timeline::TranscriptEvent {
                start: 0.,
                end: 20.,
                text: "Preserved readable notes.".into(),
                raw: None,
            }],
        }];
        smol::block_on(artifact::publish(
            &target,
            &task.work_dir,
            &meta,
            &sections,
            None,
            formats,
            outcomes,
        ))
        .unwrap();
        target.version_dir()
    }

    fn write_unknown(task: &TaskRecord, attempt: u32) -> String {
        let id = format!("stable-request.{attempt}");
        let receipt = course2md::dispatch::Receipt {
            schema: 1,
            stable_id: "stable-request".into(),
            request_id: id.clone(),
            purpose: "proofreading".into(),
            description: "校对 00:00–00:20 的文字".into(),
            service_version: "version-a".into(),
            attempt,
            state: course2md::dispatch::State::Uncertain,
            http_status: None,
            response: None,
            message: Some("连接断开 / connection lost".into()),
            unsupported_response_format: false,
            retry_authorized: None,
        };
        std::fs::create_dir_all(task.work_dir.join("requests")).unwrap();
        std::fs::write(
            task.work_dir.join("requests/stable-request.json"),
            serde_json::to_vec(&receipt).unwrap(),
        )
        .unwrap();
        id
    }

    fn uncertain_proofreading_note(
        ws: &mut Workspace,
        summarize: bool,
        summary: course2md::artifact::Outcome,
    ) -> (String, String) {
        let mut snapshot = plan(&ws.state.default_library);
        snapshot.config.llm.enabled = true;
        snapshot.config.llm.summarize = summarize;
        snapshot.options.llm = true;
        snapshot.options.summarize = summarize;
        let id = ws.state.enqueue(snapshot, None).unwrap().0;
        let mut outcomes = course2md::artifact::Outcomes::default();
        outcomes.transcript = course2md::artifact::Outcome::succeeded();
        outcomes.proofreading = course2md::artifact::Outcome::failed("校对请求结果尚不确定");
        outcomes.summary = summary;
        let version = publish_note_with_outcomes(&ws.state, &id, outcomes.clone());
        let task = ws.state.task_mut(&id).unwrap();
        task.state = TaskState::Uncertain;
        task.artifact = Some(version);
        task.outcomes = serde_json::to_value(&outcomes).unwrap();
        let request = write_unknown(task, 1);
        (id, request)
    }

    #[test]
    fn proofreading_resend_continues_an_authorized_summary_that_was_never_sent() {
        let dir = tempfile::tempdir().unwrap();
        let mut ws = test_workspace(dir.path());
        let (id, request) = uncertain_proofreading_note(
            &mut ws,
            true,
            course2md::artifact::Outcome::failed("前一步请求结果尚不确定；没有继续发送"),
        );
        let original = ws.state.task(&id).unwrap().clone();
        let manifest_path = original.artifact.as_ref().unwrap().join("manifest.json");
        let manifest = std::fs::read(&manifest_path).unwrap();
        ws.state
            .draft_mut()
            .unwrap()
            .change_source("next-video.mp4".into());
        let draft = ws.state.draft().unwrap().clone();
        let next = ws
            .state
            .reprocess(&id, vec!["proofreading".into()], vec![request.clone()])
            .unwrap();
        let child = ws.state.task(&next).unwrap();
        let course2md::execution::Operation::Reprocess {
            components,
            base_version_dir,
            prior_work_dir,
        } = &child.plan.operation
        else {
            panic!("expected component recovery");
        };
        assert_eq!(components, &["proofreading", "summary"]);
        assert_eq!(Some(base_version_dir), original.artifact.as_ref());
        assert_eq!(prior_work_dir.as_ref(), Some(&original.work_dir));
        assert!(child.plan.config.llm.enabled && child.plan.config.llm.summarize);
        assert!(child.plan.options.llm && child.plan.options.summarize);
        assert_eq!(child.resend, vec![request.clone()]);
        assert!(ws.state.task(&id).unwrap().plan == original.plan);
        assert!(ws.state.draft().unwrap() == &draft);
        assert_eq!(std::fs::read(manifest_path).unwrap(), manifest);
        assert_eq!(
            ws.state
                .reprocess(&id, vec!["proofreading".into()], vec![request])
                .unwrap(),
            next
        );
        assert_eq!(ws.state.tasks.len(), 2);
    }

    #[test]
    fn proofreading_resend_does_not_invent_or_repeat_a_summary() {
        use course2md::artifact::Outcome;
        for (requested, summary) in [
            (false, Outcome::failed("未要求生成摘要")),
            (true, Outcome::not_requested()),
            (true, Outcome::succeeded()),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let mut ws = test_workspace(dir.path());
            let (id, request) = uncertain_proofreading_note(&mut ws, requested, summary);
            let next = ws
                .state
                .reprocess(&id, vec!["proofreading".into()], vec![request])
                .unwrap();
            let child = ws.state.task(&next).unwrap();
            let course2md::execution::Operation::Reprocess { components, .. } =
                &child.plan.operation
            else {
                panic!("expected component recovery");
            };
            assert_eq!(components, &["proofreading"]);
            assert!(!child.plan.config.llm.summarize && !child.plan.options.summarize);
        }
    }

    #[test]
    fn an_existing_summary_attempt_needs_its_own_recovery_decision() {
        use course2md::dispatch::{Receipt, State as ReceiptState};
        for receipt_state in [
            ReceiptState::Completed,
            ReceiptState::NotSent,
            ReceiptState::Rejected,
            ReceiptState::Failed,
            ReceiptState::Sending,
            ReceiptState::Uncertain,
        ] {
            let dir = tempfile::tempdir().unwrap();
            let mut ws = test_workspace(dir.path());
            let (id, request) = uncertain_proofreading_note(
                &mut ws,
                true,
                course2md::artifact::Outcome::failed("摘要未完成"),
            );
            let task = ws.state.task(&id).unwrap();
            let receipt = Receipt {
                schema: 1,
                stable_id: "summary-request".into(),
                request_id: "summary-request.1".into(),
                purpose: "summary".into(),
                description: "生成摘要".into(),
                service_version: "version-a".into(),
                attempt: 1,
                state: receipt_state.clone(),
                http_status: None,
                response: None,
                message: None,
                unsupported_response_format: false,
                retry_authorized: None,
            };
            std::fs::write(
                task.work_dir.join("requests/summary-request.json"),
                serde_json::to_vec(&receipt).unwrap(),
            )
            .unwrap();
            let recovery = ws
                .state
                .reprocess(&id, vec!["proofreading".into()], vec![request]);
            if matches!(
                receipt_state,
                ReceiptState::Sending | ReceiptState::Uncertain
            ) {
                assert!(
                    recovery.is_err(),
                    "{receipt_state:?} needs exact authorization"
                );
                assert_eq!(ws.state.tasks.len(), 1);
                assert!(ws.state.task(&id).unwrap().handled_by.is_none());
            } else {
                let next = recovery.unwrap();
                let child = ws.state.task(&next).unwrap();
                let course2md::execution::Operation::Reprocess { components, .. } =
                    &child.plan.operation
                else {
                    panic!("expected component recovery");
                };
                assert_eq!(components, &["proofreading"], "{receipt_state:?}");
                assert!(!child.plan.config.llm.summarize && !child.plan.options.summarize);
            }
        }
    }

    #[test]
    fn unknown_authorization_is_exact_and_cannot_be_reused_by_an_old_button() {
        let dir = tempfile::tempdir().unwrap();
        let mut ws = test_workspace(dir.path());
        let id = ws
            .state
            .enqueue(plan(&ws.state.default_library), None)
            .unwrap()
            .0;
        ws.state.task_mut(&id).unwrap().state = TaskState::Uncertain;
        let first = write_unknown(ws.state.task(&id).unwrap(), 1);
        ws.state.authorize_uncertain(&id, &[first.clone()]).unwrap();
        assert!(ws.state.authorize_uncertain(&id, &[first.clone()]).is_err());
        ws.state.task_mut(&id).unwrap().state = TaskState::Uncertain;
        assert!(ws.state.authorize_uncertain(&id, &[first.clone()]).is_err());
        let second = write_unknown(ws.state.task(&id).unwrap(), 2);
        assert!(ws.state.authorize_uncertain(&id, &[first]).is_err());
        ws.state.authorize_uncertain(&id, &[second]).unwrap();
        assert_eq!(ws.state.task(&id).unwrap().resend.len(), 2);
        assert!(
            ws.state.task(&id).unwrap().blocked[0]
                .message
                .contains("00:00–00:20")
        );
    }

    #[test]
    fn published_result_after_crash_is_not_enqueued_again() {
        let dir = tempfile::tempdir().unwrap();
        let mut ws = test_workspace(dir.path());
        let id = ws
            .state
            .enqueue(plan(&ws.state.default_library), None)
            .unwrap()
            .0;
        ws.state.task_mut(&id).unwrap().state = TaskState::Running;
        ws.save().unwrap();
        let version = publish_note(&ws.state, &id, false);
        let restored = test_workspace(dir.path());
        let task = restored.state.task(&id).unwrap();
        assert_eq!(task.state, TaskState::Complete);
        assert_eq!(task.artifact.as_ref(), Some(&version));
        assert!(restored.state.next_task().is_none());
    }

    #[test]
    fn partial_followup_is_single_and_does_not_rewrite_another_draft_or_successful_stage() {
        let dir = tempfile::tempdir().unwrap();
        let mut ws = test_workspace(dir.path());
        let id = ws
            .state
            .enqueue(plan(&ws.state.default_library), None)
            .unwrap()
            .0;
        let version = publish_note(&ws.state, &id, true);
        let outcomes = course2md::artifact::read_manifest(&version.join("manifest.json"))
            .unwrap()
            .outcomes;
        let original = ws.state.task_mut(&id).unwrap();
        original.state = TaskState::Partial;
        original.artifact = Some(version);
        original.outcomes = serde_json::to_value(&outcomes).unwrap();
        ws.state
            .draft_mut()
            .unwrap()
            .change_source("draft-b.mp4".into());
        let draft = ws.state.draft().unwrap().clone();
        assert!(
            ws.state
                .reprocess(&id, vec!["screenshots".into()], vec![])
                .is_err()
        );
        let followup = ws
            .state
            .reprocess(&id, vec!["proofreading".into()], vec![])
            .unwrap();
        assert_eq!(
            ws.state
                .reprocess(&id, vec!["proofreading".into()], vec![])
                .unwrap(),
            followup
        );
        assert_eq!(ws.state.tasks.len(), 2);
        assert!(ws.state.draft().unwrap() == &draft);
        assert!(ws.state.set_intent(&id, Intent::Run).is_err());
        let child = ws.state.task(&followup).unwrap();
        assert_eq!(child.parent.as_deref(), Some(id.as_str()));
        assert_eq!(child.plan.source.input, "lecture.mp4");
        assert_eq!(
            child.plan.config.defaults.transcript_source,
            Some(course2md::config::TranscriptSource::Subtitle)
        );
    }

    #[test]
    fn service_repair_creates_a_scoped_attempt_and_preserves_the_old_plan_and_input() {
        let dir = tempfile::tempdir().unwrap();
        let mut ws = test_workspace(dir.path());
        let mut snapshot = plan(&ws.state.default_library);
        snapshot.ai_service = Some("old-ai-version".into());
        snapshot.config.llm.base_url = "https://old.example.test/v1".into();
        snapshot.config.llm.model = "old-model".into();
        let id = ws.state.enqueue(snapshot.clone(), None).unwrap().0;
        let version = publish_note(&ws.state, &id, true);
        let outcomes = course2md::artifact::read_manifest(&version.join("manifest.json"))
            .unwrap()
            .outcomes;
        let original = ws.state.task_mut(&id).unwrap();
        original.state = TaskState::Partial;
        original.artifact = Some(version.clone());
        original.outcomes = serde_json::to_value(&outcomes).unwrap();
        ws.state
            .draft_mut()
            .unwrap()
            .change_source("another-video.mp4".into());
        let input = ws.state.draft().unwrap().clone();
        let mut repaired = ConfigFile::default();
        repaired.llm.base_url = "https://repaired.example.test/v1".into();
        repaired.llm.model = "repaired-model".into();
        // A service repair cannot silently rerun successful work or non-AI stages.
        assert!(
            ws.state
                .reprocess_with_service(
                    &id,
                    vec!["screenshots".into()],
                    vec![],
                    Some(("new-ai-version".into(), repaired.clone()))
                )
                .is_err()
        );
        let followup = ws
            .state
            .reprocess_with_service(
                &id,
                vec!["proofreading".into()],
                vec![],
                Some(("new-ai-version".into(), repaired.clone())),
            )
            .unwrap();
        assert!(ws.state.task(&id).unwrap().plan == snapshot);
        assert_eq!(
            ws.state.task(&id).unwrap().artifact.as_ref(),
            Some(&version)
        );
        assert!(ws.state.draft().unwrap() == &input);
        let child = ws.state.task(&followup).unwrap();
        assert_eq!(child.plan.ai_service.as_deref(), Some("new-ai-version"));
        assert_eq!(child.plan.config.llm.model, "repaired-model");
        assert_eq!(
            child.plan.config.llm.base_url,
            "https://repaired.example.test/v1"
        );
        assert!(child.plan.config.llm.enabled);
        assert!(!child.plan.config.llm.summarize);
        assert!(matches!(
            &child.plan.operation,
            course2md::execution::Operation::Reprocess { base_version_dir, components, .. }
                if base_version_dir == &version && components == &["proofreading"]
        ));
        let again = ws
            .state
            .reprocess_with_service(
                &id,
                vec!["proofreading".into()],
                vec![],
                Some(("new-ai-version".into(), repaired)),
            )
            .unwrap();
        assert_eq!(again, followup);
        assert_eq!(ws.state.tasks.len(), 2);
    }

    #[test]
    fn damaged_primary_and_backup_are_preserved_before_paused_mirror_rebuild() {
        let dir = tempfile::tempdir().unwrap();
        let mut ws = test_workspace(dir.path());
        let mut snapshot = plan(&ws.state.default_library);
        snapshot.config.llm.api_key = "fake-test-credential".into();
        let id = ws.state.enqueue(snapshot, None).unwrap().0;
        ws.state.task_mut(&id).unwrap().state = TaskState::Running;
        ws.state.task_mut(&id).unwrap().resend = vec!["old-attempt.1".into()];
        ws.save().unwrap();
        let work = ws.state.task(&id).unwrap().work_dir.clone();
        let mirror = std::fs::read_to_string(work.join("task-record.json")).unwrap();
        assert!(!mirror.contains("fake-test-credential"));
        std::fs::write(work.join("partial-material.txt"), "saved words").unwrap();
        std::fs::write(
            work.join("control.json"),
            r#"{"intent":"run","resend":["old-attempt.1"]}"#,
        )
        .unwrap();
        std::fs::write(ws.storage_path(), b"{broken primary").unwrap();
        std::fs::write(
            ws.storage_path().with_extension("json.bak"),
            b"{broken backup",
        )
        .unwrap();
        assert!(
            Workspace::open_at(
                ws.path.clone(),
                dir.path().join("library"),
                Default::default()
            )
            .is_err()
        );
        let rebuilt = Workspace::rebuild_at(
            ws.path.clone(),
            dir.path().join("library"),
            Default::default(),
        )
        .unwrap();
        assert_eq!(rebuilt.state.task(&id).unwrap().state, TaskState::Paused);
        assert_eq!(rebuilt.state.task(&id).unwrap().intent, Intent::Pause);
        assert!(rebuilt.state.task(&id).unwrap().resend.is_empty());
        assert!(rebuilt.state.next_task().is_none());
        assert_eq!(
            std::fs::read_to_string(work.join("partial-material.txt")).unwrap(),
            "saved words"
        );
        let control: serde_json::Value =
            serde_json::from_slice(&std::fs::read(work.join("control.json")).unwrap()).unwrap();
        assert_eq!(control["intent"], "pause");
        assert_eq!(control["resend"], serde_json::json!([]));
        let archive = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .find(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("workspace-recovery-")
            })
            .unwrap()
            .path();
        assert_eq!(
            std::fs::read(archive.join("workspace.json")).unwrap(),
            b"{broken primary"
        );
        assert_eq!(
            std::fs::read(archive.join("workspace.json.bak")).unwrap(),
            b"{broken backup"
        );
        let reopened = Workspace::open_at(
            ws.path.clone(),
            dir.path().join("library"),
            Default::default(),
        )
        .unwrap();
        assert!(reopened.state.next_task().is_none());
        assert_eq!(reopened.state.tasks.len(), 1);
    }

    #[test]
    fn mirror_failure_keeps_primary_record_and_offline_location_is_not_recreated() {
        let dir = tempfile::tempdir().unwrap();
        let mut ws = test_workspace(dir.path());
        let id = ws
            .state
            .enqueue(plan(&ws.state.default_library), None)
            .unwrap()
            .0;
        ws.save().unwrap();
        let bytes = std::fs::read(ws.storage_path()).unwrap();
        let mirror = ws
            .state
            .task(&id)
            .unwrap()
            .work_dir
            .join("task-record.json");
        std::fs::remove_file(&mirror).unwrap();
        std::fs::create_dir(&mirror).unwrap();
        assert!(
            ws.transaction(|state| state.set_intent(&id, Intent::Pause))
                .is_err()
        );
        assert_eq!(std::fs::read(ws.storage_path()).unwrap(), bytes);
        let root = ws.state.libraries[0].root.clone();
        std::fs::rename(&root, dir.path().join("disconnected-library")).unwrap();
        ws.transaction(|state| state.set_intent(&id, Intent::Pause))
            .unwrap();
        let reopened = test_workspace(dir.path());
        assert!(!root.exists());
        assert_eq!(reopened.state.task(&id).unwrap().state, TaskState::Paused);
    }

    #[test]
    fn backup_recovery_does_not_assume_a_lost_final_run_intent() {
        let dir = tempfile::tempdir().unwrap();
        let mut ws = test_workspace(dir.path());
        let id = ws
            .state
            .enqueue(plan(&ws.state.default_library), None)
            .unwrap()
            .0;
        ws.state.task_mut(&id).unwrap().state = TaskState::Running;
        ws.save().unwrap();
        ws.save().unwrap();
        std::fs::write(ws.storage_path(), b"damaged").unwrap();
        let recovered = test_workspace(dir.path());
        assert_eq!(recovered.state.task(&id).unwrap().state, TaskState::Paused);
        assert!(recovered.state.next_task().is_none());
    }

    #[test]
    fn older_parent_links_recover_to_latest_followup_without_requeuing_parent() {
        let dir = tempfile::tempdir().unwrap();
        let mut ws = test_workspace(dir.path());
        let parent = ws
            .state
            .enqueue(plan(&ws.state.default_library), None)
            .unwrap()
            .0;
        ws.state.task_mut(&parent).unwrap().state = TaskState::NeedsAttention;
        let mut changed = plan(&ws.state.default_library);
        changed.title = "changed display title".into();
        changed.options.summarize = true;
        let next = ws.state.enqueue(changed, Some(parent.clone())).unwrap().0;
        ws.state.task_mut(&parent).unwrap().handled_by = None;
        ws.state.task_mut(&parent).unwrap().state = TaskState::Running;
        ws.state.recover();
        let original = ws.state.task(&parent).unwrap();
        assert_eq!(original.handled_by.as_deref(), Some(next.as_str()));
        assert_eq!(original.intent, Intent::Pause);
        assert_eq!(ws.state.next_task().unwrap().id, next);
    }

    #[test]
    fn export_only_followup_needs_no_ai_service_and_retries_only_failed_formats() {
        use course2md::config::OutputFormat;
        let dir = tempfile::tempdir().unwrap();
        let mut ws = test_workspace(dir.path());
        let id = ws
            .state
            .enqueue(plan(&ws.state.default_library), None)
            .unwrap()
            .0;
        let version = publish_note(&ws.state, &id, false);
        let original = ws.state.task_mut(&id).unwrap();
        original.state = TaskState::Partial;
        original.artifact = Some(version);
        original.plan.config.llm.enabled = true;
        original.plan.config.llm.summarize = true;
        original.plan.config.defaults.formats = Some(vec![OutputFormat::Md, OutputFormat::Html]);
        original.outcomes = serde_json::json!({"exports":{
            "md":{"status":"succeeded"}, "html":{"status":"failed","message":"disk full"}
        }});
        let next = ws
            .state
            .reprocess(&id, vec!["exports".into()], vec![])
            .unwrap();
        let child = ws.state.task(&next).unwrap();
        assert!(!child.plan.config.llm.enabled && !child.plan.config.llm.summarize);
        assert_eq!(
            child.plan.config.defaults.formats,
            Some(vec![OutputFormat::Html])
        );
        let refs = crate::preferences::ServiceRefs {
            asr: Some("stopped-asr".into()),
            llm: Some("stopped-ai".into()),
        };
        let needed = refs.required_for(&child.plan.config);
        assert!(needed.asr.is_none() && needed.llm.is_none());
    }

    #[test]
    fn published_exports_remain_reachable_after_a_lost_completion_event() {
        use course2md::{artifact, config::OutputFormat, portable};
        let dir = tempfile::tempdir().unwrap();
        let mut ws = test_workspace(dir.path());
        let id = ws
            .state
            .enqueue(plan(&ws.state.default_library), None)
            .unwrap()
            .0;
        ws.state.task_mut(&id).unwrap().state = TaskState::Running;
        ws.save().unwrap();
        let mut outcomes = artifact::Outcomes::default();
        outcomes.transcript = artifact::Outcome::succeeded();
        let version = publish_note_with_exports(
            &ws.state,
            &id,
            outcomes,
            &[OutputFormat::Md, OutputFormat::Html],
        );
        let restored = test_workspace(dir.path());
        assert_eq!(restored.state.task(&id).unwrap().state, TaskState::Complete);
        let task = restored.state.task(&id).unwrap();
        let location = restored.state.library(&task.plan.library_id).unwrap();
        let markdown = version
            .join("exports")
            .join(portable::file_name(OutputFormat::Md));
        let html = version
            .join("exports")
            .join(portable::file_name(OutputFormat::Html));
        assert_eq!(task.state, TaskState::Complete);
        assert_eq!(
            task_export_files(task, location),
            vec![markdown.clone(), html.clone()]
        );
        assert_eq!(available_task_export(task, location).unwrap(), markdown);
        assert!(restored.state.next_task().is_none());

        // A missing first format must not hide another successful output, and
        // the readable body's directory is never substituted for missing exports.
        std::fs::remove_file(&markdown).unwrap();
        assert_eq!(available_task_export(task, location).unwrap(), html);
        std::fs::remove_file(&html).unwrap();
        assert!(available_task_export(task, location).is_err());
        assert!(version.join("course.md").is_file());
        assert!(task.plan == ws.state.task(&id).unwrap().plan);
    }

    #[test]
    fn export_only_completion_reopens_its_own_files_and_recovers_missing_outputs() {
        use course2md::{artifact, config::OutputFormat, portable};
        let dir = tempfile::tempdir().unwrap();
        let mut ws = test_workspace(dir.path());
        let id = ws
            .state
            .enqueue(plan(&ws.state.default_library), None)
            .unwrap()
            .0;
        let mut outcomes = artifact::Outcomes::default();
        outcomes.transcript = artifact::Outcome::succeeded();
        outcomes
            .exports
            .insert("html".into(), artifact::Outcome::failed("disk full"));
        let base = publish_note_with_exports(&ws.state, &id, outcomes, &[OutputFormat::Md]);
        let manifest = artifact::read_manifest(&base.join("manifest.json")).unwrap();
        let original = ws.state.task_mut(&id).unwrap();
        original.state = TaskState::Partial;
        original.artifact = Some(base.clone());
        original.outcomes = serde_json::to_value(&manifest.outcomes).unwrap();
        let original_plan = original.plan.clone();
        let old_markdown = base
            .join("exports")
            .join(portable::file_name(OutputFormat::Md));
        let old_bytes = std::fs::read(&old_markdown).unwrap();
        let manifest_bytes = std::fs::read(base.join("manifest.json")).unwrap();
        let next = ws
            .state
            .reprocess(&id, vec!["exports".into()], vec![])
            .unwrap();
        ws.state.task_mut(&next).unwrap().state = TaskState::Running;
        ws.save().unwrap();
        let child = ws.state.task(&next).unwrap();
        let next_plan = child.plan.clone();
        let location = ws.state.library(&child.plan.library_id).unwrap();
        let output = task_export_directory(child, location)
            .unwrap()
            .join(portable::file_name(OutputFormat::Html));
        portable::export(&base, OutputFormat::Html, &output).unwrap();
        std::fs::write(
            child.work_dir.join("export-result.json"),
            serde_json::to_vec(&serde_json::json!({
                "schema": 1,
                "outputs": [output],
                "outcomes": {"html": {"status": "succeeded"}}
            }))
            .unwrap(),
        )
        .unwrap();

        let mut restored = test_workspace(dir.path());
        let task = restored.state.task(&next).unwrap();
        let location = restored.state.library(&task.plan.library_id).unwrap();
        assert_eq!(task.state, TaskState::Complete);
        assert_eq!(task.artifact.as_ref(), Some(&base));
        assert_eq!(task_export_files(task, location), vec![output.clone()]);
        assert_eq!(available_task_export(task, location).unwrap(), output);
        assert!(!output.starts_with(&base));
        assert!(task.plan == next_plan);
        assert!(restored.state.task(&id).unwrap().plan == original_plan);
        assert!(restored.state.next_task().is_none());
        restored.state.task_mut(&next).unwrap().unread = false;
        restored.save().unwrap();

        // Opening the app does not announce a previously read result again.
        let reopened = test_workspace(dir.path());
        let task = reopened.state.task(&next).unwrap();
        assert!(!task.unread);
        assert_eq!(
            available_task_export(task, reopened.state.library(&task.plan.library_id).unwrap())
                .unwrap(),
            output
        );

        std::fs::remove_file(&output).unwrap();
        let mut missing = test_workspace(dir.path());
        let task = missing.state.task(&next).unwrap();
        let location = missing.state.library(&task.plan.library_id).unwrap();
        assert_eq!(task.state, TaskState::Partial);
        assert!(task.unread && task_export_files(task, location).is_empty());
        assert!(available_task_export(task, location).is_err());
        let retry = missing
            .state
            .reprocess(&next, vec!["exports".into()], vec![])
            .unwrap();
        let retry = missing.state.task(&retry).unwrap();
        assert!(retry.exports_only());
        assert_eq!(
            retry.plan.config.defaults.formats,
            Some(vec![OutputFormat::Html])
        );
        assert_eq!(std::fs::read(old_markdown).unwrap(), old_bytes);
        assert_eq!(
            std::fs::read(base.join("manifest.json")).unwrap(),
            manifest_bytes
        );
        assert!(missing.state.task(&next).unwrap().plan == next_plan);
        assert!(missing.state.task(&id).unwrap().plan == original_plan);
    }

    #[test]
    fn explicit_library_reassociation_retains_identity_and_never_overwrites_a_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let mut ws = test_workspace(dir.path());
        ws.save().unwrap();
        let marker = ws.state.libraries[0].root.join(".course2md-library-id");
        let id = ws.state.default_library.clone();
        std::fs::remove_file(&marker).unwrap();
        ws.reassociate_library(&id).unwrap();
        assert_eq!(std::fs::read_to_string(&marker).unwrap(), id);
        std::fs::write(&marker, "a-different-library").unwrap();
        assert!(ws.reassociate_library(&id).is_err());
        assert_eq!(
            std::fs::read_to_string(marker).unwrap(),
            "a-different-library"
        );
    }

    #[test]
    fn a_future_primary_schema_is_not_replaced_by_an_older_valid_backup() {
        let dir = tempfile::tempdir().unwrap();
        let ws = test_workspace(dir.path());
        ws.save().unwrap();
        ws.save().unwrap();
        let mut future = serde_json::to_value(&ws.state).unwrap();
        future["schema"] = serde_json::json!(SCHEMA + 1);
        let bytes = serde_json::to_vec(&future).unwrap();
        std::fs::write(ws.storage_path(), &bytes).unwrap();
        assert!(
            Workspace::open_at(
                ws.path.clone(),
                dir.path().join("library"),
                Default::default()
            )
            .is_err()
        );
        assert!(
            Workspace::rebuild_at(
                ws.path.clone(),
                dir.path().join("library"),
                Default::default()
            )
            .is_err()
        );
        assert_eq!(std::fs::read(ws.storage_path()).unwrap(), bytes);
    }
}
