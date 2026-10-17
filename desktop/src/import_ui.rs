//! One start intent continues source preparation through to a readable note.
use super::*;

/// Hero and preview share this ruler; the lower workbench reflows independently.
const WORKBENCH_WIDTH: f32 = 760.;
use crate::{motion, preferences::ServicePurpose, theme::*};
use anyhow::Context as _;
use course2md::config::TranscriptSource;
use course2md::subtitle::{SubtitleEvidence, SubtitleReadError, SubtitleTrack};
use gpui_component::{
    checkbox::Checkbox,
    switch::Switch,
    menu::{DropdownMenu, PopupMenuItem},
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// A group heading in the processing panel's single field column.
fn import_option_section(
    id: impl Into<ElementId>,
    label: &'static str,
    content: impl IntoElement,
) -> Div {
    v_flex().w_full().min_w_0().gap_2()
        .child(accessible_text(id, label)
            .text_size(TEXT_BODY)
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(color(ACCENT)))
        .child(content)
}

/// A plain field label above its control.
fn import_option_label(
    id: impl Into<ElementId>,
    label: &'static str,
    content: impl IntoElement,
) -> Div {
    v_flex().w_full().min_w_0().gap_2()
        .child(accessible_text(id, label).text_size(TEXT_AUX).text_color(color(GRAY)))
        .child(content)
}

/// Secondary text below a control.
pub(crate) fn aux_text(content: impl IntoElement) -> Div {
    div().text_size(TEXT_AUX).text_color(color(GRAY)).child(content)
}

fn import_option_switch_row(label: &'static str, hint: &'static str, switch: impl IntoElement) -> Div {
    h_flex().w_full().min_w_0().min_h(rems(44. / 14.)).items_center().gap_4()
        .child(v_flex().flex_1().min_w_0().gap_1()
            .child(div().text_size(rems(16. / 14.)).child(label))
            .when(!hint.is_empty(), |view| view.child(
                aux_text(hint))))
        .child(div().flex_shrink_0().child(switch))
}

/// The first line of a message when it is already readable. A raw download
/// tool line (`WARNING:` / `ERROR:`) is not, and the caller supplies a summary.
fn readable_first_line(message: &str) -> Option<&str> {
    let first = message.lines().next().unwrap_or(message);
    (!first.contains("WARNING:") && !first.contains("ERROR:")).then_some(first)
}

fn help(text: impl Into<SharedString>) -> Div {
    let text = text.into();
    crate::theme::supporting_info(text_id("help", &text), text).text_sm()
}
fn issue(message: impl Into<SharedString>) -> Stateful<Div> {
    let message = message.into();
    error_text(text_id("issue", &message), message)
        .text_sm()
        .whitespace_normal()
}
fn text_id(kind: &str, value: &str) -> SharedString {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hash);
    format!("import-{kind}-{:x}", hash.finish()).into()
}

/// One AI processing switch in the processing options.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConversionAiOption {
    Proofread,
    Vision,
    Summary,
}

pub(crate) fn conversion_ai_option_label(option: ConversionAiOption) -> &'static str {
    match option {
        ConversionAiOption::Proofread => "校对文字和标点",
        ConversionAiOption::Vision => "校对时附带对应截图",
        ConversionAiOption::Summary => "在开头生成摘要",
    }
}

pub(crate) fn conversion_ai_option_enabled(
    options: &ConversionOptions,
    option: ConversionAiOption,
) -> bool {
    match option {
        ConversionAiOption::Proofread => options.llm,
        ConversionAiOption::Vision => options.vision,
        ConversionAiOption::Summary => options.summarize,
    }
}

pub(crate) fn apply_conversion_ai_option(
    options: &mut ConversionOptions,
    option: ConversionAiOption,
    enabled: bool,
) {
    match option {
        ConversionAiOption::Proofread => options.llm = enabled,
        ConversionAiOption::Vision => options.vision = enabled,
        ConversionAiOption::Summary => options.summarize = enabled,
    }
}

const TEXT_SOURCE_CHOICES: [(TranscriptSource, &str); 3] = [
    (TranscriptSource::Auto, "自动"),
    (TranscriptSource::Subtitle, "仅字幕"),
    (TranscriptSource::Asr, "语音识别"),
];

fn text_source_id(mode: TranscriptSource) -> &'static str {
    match mode {
        TranscriptSource::Auto => "auto",
        TranscriptSource::Subtitle => "subtitle",
        TranscriptSource::Asr => "asr",
    }
}

/// A shared heading for related conversion options. 图标由调用方显式给出（不做文案子串匹配）。
pub(crate) fn box_section(icon: Icon, label: &'static str) -> Div {
    v_flex().w_full().min_w_0().gap_3().child(
        h_flex()
            .gap_2()
            .items_center()
            .child(icon.size(px(20.)).text_color(color(ACCENT)))
            .child(
                accessible_text(text_id("box-section", label), label)
                    .text_size(TEXT_TITLE)
                    .font_weight(FontWeight::SEMIBOLD),
            ),
    )
}
/// A consequence or recovery fact kept beside its action.
pub(crate) fn conversion_fact(value: impl Into<SharedString>, warning: bool) -> Stateful<Div> {
    let value = value.into();
    accessible_text(text_id("conversion-fact", &value), value)
        .w_full()
        .min_w_0()
        .whitespace_normal()
        .text_size(TEXT_BODY)
        .text_color(color(if warning { WARNING } else { INK }))
        .when(warning, |row| row.font_weight(FontWeight::SEMIBOLD))
}

fn uses_speech(source: &source::Source, source_mode: TranscriptSource) -> bool {
    source_mode == TranscriptSource::Asr
        || (source_mode == TranscriptSource::Auto
            && source.selected_subtitle.is_none()
            && source.subtitle_request.is_none()
            && source.subtitle_read_error.is_none()
            && matches!(
                source.subtitles,
                SubtitleEvidence::NoneFound
                    | SubtitleEvidence::Unsupported { .. }
                    | SubtitleEvidence::Failed { .. }
            ))
}

fn automatic_subtitle_fallback(
    source: &mut source::Source,
    source_mode: TranscriptSource,
    error: Option<&str>,
) -> bool {
    if source_mode != TranscriptSource::Auto
        || source.selected_subtitle.is_some()
        || matches!(
            source.subtitle_read_error,
            Some(SubtitleReadError::Cancelled)
        )
    {
        return false;
    }
    let failure = error.map(str::to_owned).or_else(|| {
        source
            .subtitle_read_error
            .as_ref()
            .map(ToString::to_string)
            .or_else(|| match &source.subtitles {
                SubtitleEvidence::Failed { message } => Some(message.clone()),
                _ => None,
            })
    });
    let Some(message) = failure else {
        return false;
    };
    source.subtitles = SubtitleEvidence::Failed { message };
    source.subtitle_request = None;
    source.subtitle_read_error = None;
    true
}

fn subtitle_needs_confirmation(source: &source::Source, source_mode: TranscriptSource) -> bool {
    !uses_speech(source, source_mode)
        && (source.selected_subtitle.is_none()
            || source.subtitle_request.is_some()
            || source.subtitle_read_error.is_some())
}

/// Follow only this input's submitted recovery chain. A matching URL or the
/// selected queue item alone cannot establish that a task belongs to this form.
fn submitted_input_task<'a>(
    draft: &workspace::Draft,
    tasks: &'a [workspace::TaskRecord],
    input: &str,
) -> Option<&'a workspace::TaskRecord> {
    if draft.input != input || input.trim().is_empty() {
        return None;
    }
    let find = |id: &str| {
        let mut matches = tasks.iter().filter(|task| task.id == id);
        let task = matches.next()?;
        matches.next().is_none().then_some(task)
    };
    let mut task = find(draft.submitted_task.as_deref()?)?;
    let source_id = &task.plan.source_id;
    let online = task.plan.source.online;
    if source_id.is_empty()
        || task.plan.source.input != input
        || online != draft.online
        || draft
            .source
            .as_ref()
            .is_some_and(|source| source.identity != *source_id || source.online != online)
    {
        return None;
    }
    let mut visited = std::collections::BTreeSet::new();
    loop {
        if !visited.insert(task.id.as_str())
            || task.plan.source_id != *source_id
            || task.plan.source.identity != *source_id
            || task.plan.source.online != online
        {
            return None;
        }
        let Some(next) = task.handled_by.as_deref() else {
            return Some(task);
        };
        let next = find(next)?;
        if next.parent.as_deref() != Some(task.id.as_str()) {
            return None;
        }
        task = next;
    }
}

fn completed_input_task<'a>(
    draft: &workspace::Draft,
    tasks: &'a [workspace::TaskRecord],
    input: &str,
) -> Option<&'a workspace::TaskRecord> {
    submitted_input_task(draft, tasks, input).filter(|task| {
        matches!(
            task.state,
            workspace::TaskState::Complete | workspace::TaskState::Partial
        ) && task.artifact.is_some()
    })
}

/// A saved result is history when entering the workbench. A running or paused
/// recovery, an uncertain request, and any changes after submission stay in place.
fn retired_input_task<'a>(
    draft: &workspace::Draft,
    tasks: &'a [workspace::TaskRecord],
    input: &str,
) -> Option<&'a workspace::TaskRecord> {
    let task = submitted_input_task(draft, tasks, input)?;
    if !task.state.finished()
        || task.has_uncertain()
        || (task.state != workspace::TaskState::Cancelled && task.artifact.is_none())
    {
        return None;
    }
    let submitted = tasks
        .iter()
        .find(|task| Some(task.id.as_str()) == draft.submitted_task.as_deref())?;
    (draft.title == submitted.plan.title
        && draft.options == submitted.plan.options
        && draft.library_id == submitted.plan.library_id
        && draft.folder == submitted.plan.folder
        && draft.subtitle == submitted.plan.subtitle
        && draft
            .asr_service
            .as_ref()
            .is_none_or(|id| Some(id) == submitted.plan.asr_service.as_ref())
        && draft
            .ai_service
            .as_ref()
            .is_none_or(|id| Some(id) == submitted.plan.ai_service.as_ref()))
    .then_some(task)
}

/// A live navigation intent is separate from the saved task. Leaving its
/// originating input or note ends following without cancelling the work.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ConversionFollow {
    Preparing(u64),
    Task {
        id: String,
        source_revision: u64,
    },
    ReaderTask {
        id: String,
        base_version: PathBuf,
        reader_revision: u64,
    },
}

impl ConversionFollow {
    /// Opening a viewer owns the old version until the person closes it. Do not
    /// restore this intent afterwards, even if an in-flight read is still pending.
    pub(crate) fn interrupt_for_reader_viewer(follow: &mut Option<Self>) {
        if matches!(follow, Some(Self::ReaderTask { .. })) {
            *follow = None;
        }
    }

    pub(crate) fn submitted(self, revision: u64, id: String) -> Option<Self> {
        matches!(self, Self::Preparing(current) if current == revision).then_some(Self::Task {
            id,
            source_revision: revision,
        })
    }

    pub(crate) fn follows(&self, id: &str, revision: u64) -> bool {
        matches!(self, Self::Task { id: followed, source_revision }
            if followed == id && *source_revision == revision)
    }

    pub(crate) fn reader_reprocess(
        task: &workspace::TaskRecord,
        version: &std::path::Path,
        revision: u64,
    ) -> Option<Self> {
        let course2md::execution::Operation::Reprocess {
            base_version_dir,
            components,
            ..
        } = &task.plan.operation
        else {
            return None;
        };
        (base_version_dir == version && !components.is_empty() && !task.exports_only()).then(|| {
            Self::ReaderTask {
                id: task.id.clone(),
                base_version: base_version_dir.clone(),
                reader_revision: revision,
            }
        })
    }

    /// Check both before starting an automatic read and after its asynchronous
    /// load. The latter also retains the caller's existing read-generation guard.
    pub(crate) fn context_is_current(
        &self,
        page: Page,
        source_revision: u64,
        reader_revision: u64,
        version: Option<&std::path::Path>,
    ) -> bool {
        match self {
            Self::Preparing(_) => false,
            Self::Task {
                source_revision: expected,
                ..
            } => page == Page::New && *expected == source_revision,
            Self::ReaderTask {
                base_version,
                reader_revision: expected,
                ..
            } => {
                page == Page::Result
                    && *expected == reader_revision
                    && version == Some(base_version.as_path())
            }
        }
    }

    pub(crate) fn completed_reader_task(
        &self,
        task: &workspace::TaskRecord,
        page: Page,
        reader_revision: u64,
        version: Option<&std::path::Path>,
    ) -> bool {
        let Self::ReaderTask {
            id, base_version, ..
        } = self
        else {
            return false;
        };
        task.id == *id
            && self.context_is_current(page, 0, reader_revision, version)
            && Self::reader_reprocess(task, base_version, reader_revision).as_ref() == Some(self)
            && matches!(
                task.state,
                workspace::TaskState::Complete | workspace::TaskState::Partial
            )
            && task
                .artifact
                .as_ref()
                .is_some_and(|path| path != base_version)
    }

    pub(crate) fn completed_task<'a>(
        &self,
        on_workbench: bool,
        revision: u64,
        draft: &workspace::Draft,
        tasks: &'a [workspace::TaskRecord],
        input: &str,
    ) -> Option<&'a workspace::TaskRecord> {
        let task = completed_input_task(draft, tasks, input)?;
        (on_workbench && self.follows(&task.id, revision)).then_some(task)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConversionGate {
    Wait,
    Stop,
    Submit,
}

/// Normal asynchronous stages retain the first Start intent. Only an abandoned
/// source or an unresolved material choice ends this automatic continuation.
fn conversion_gate(
    requested_revision: Option<u64>,
    current_revision: u64,
    reading: bool,
    has_source: bool,
    needs_text_choice: bool,
    environment_ready: bool,
    library_ready: bool,
    existing_note: bool,
) -> ConversionGate {
    if requested_revision != Some(current_revision) {
        ConversionGate::Stop
    } else if reading {
        ConversionGate::Wait
    } else if !has_source {
        ConversionGate::Stop
    } else if needs_text_choice || !environment_ready || !library_ready {
        ConversionGate::Wait
    } else if existing_note {
        ConversionGate::Stop
    } else {
        ConversionGate::Submit
    }
}

impl Desktop {
    /// Explicit confirmation of an already visible preview owns submission.
    pub fn start_conversion(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.reading && self.page == Page::New {
            self.read_generation = self.read_generation.wrapping_add(1);
            self.reading = false;
        }
        let input = self.value(Field::Source, cx);
        if input != self.last_source_input {
            if !self.prepare_next_import(&input, window, cx) {
                return;
            }
            self.following_conversion = None;
            self.last_source_input = input.clone();
            self.invalidate_source();
        }
        if input.is_empty() {
            self.source_validation = Some(if self.online {
                "先粘贴视频链接".into()
            } else {
                "先选择一个视频".into()
            });
            if self.online {
                self.inputs[&Field::Source].update(cx, |input, cx| input.focus(window, cx));
            }
            cx.notify();
            return;
        }
        // Preview is a separate user step. Never queue an unseen source,
        // including callers returning from login or selecting a local file.
        if self.source_preview.is_none() {
            self.inspect_source(window, cx);
            return;
        }
        self.pending_conversion = Some(self.preview_generation);
        self.following_conversion = (self.page == Page::New)
            .then_some(ConversionFollow::Preparing(self.preview_generation));
        self.advance_conversion(window, cx);
    }

    pub fn advance_conversion_when_ready(&mut self, cx: &mut Context<Self>) {
        if self.pending_conversion.is_none() {
            return;
        }
        let Some(handle) = cx.windows().first().copied() else {
            return;
        };
        let desktop = cx.entity().downgrade();
        cx.defer(move |cx| {
            let _ = cx.update_window(handle, |_, window, cx| {
                let _ = desktop.update(cx, |desktop, cx| desktop.advance_conversion(window, cx));
            });
        });
    }

    pub fn advance_conversion(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(generation) = self.pending_conversion else {
            return;
        };
        let reading = self.preview_cancel.is_some() || self.subtitle_loading;
        if generation == self.preview_generation
            && !reading
            && let Some(source) = &mut self.source_preview
            && automatic_subtitle_fallback(
                source,
                self.task_options.source_mode,
                self.subtitle_error.as_deref(),
            )
        {
            self.subtitle_error = None;
            if !self.save_current_draft(cx) {
                self.pending_conversion = None;
                return;
            }
        }
        match conversion_gate(
            self.pending_conversion,
            self.preview_generation,
            reading,
            self.source_preview.is_some(),
            self.subtitle_attention_required(),
            self.environment.is_some(),
            !self.loading,
            self.existing_source_note().is_some(),
        ) {
            ConversionGate::Wait => cx.notify(),
            ConversionGate::Stop => {
                self.pending_conversion = None;
                cx.notify();
            }
            ConversionGate::Submit => self.enqueue_current(window, cx),
        }
    }

    fn subtitle_issue(
        &self,
        kind: &str,
        message: &str,
        cx: &mut Context<Self>,
    ) -> Div {
        let summary = if source::is_login_failure(message) {
            "字幕需要登录，或改用语音识别"
        } else {
            readable_first_line(message).unwrap_or("无法读取字幕，可重试或改用语音识别")
        };
        let details = if summary != message { message } else { "" };
        let id = text_id(kind, message);
        let expanded = self.expanded_subtitle_issue.as_ref() == Some(&id);
        v_flex()
            .min_w_0()
            .gap_2()
            .child(help(summary.to_owned()))
            .when(!details.trim().is_empty(), |view| {
                view.child(
                    quiet(id.clone())
                        .icon(if expanded {
                            crate::icons::chevron_up()
                        } else {
                            crate::icons::chevron_down()
                        })
                        .self_start()
                        .label(if expanded {
                            "收起技术详情"
                        } else {
                            "技术详情"
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.expanded_subtitle_issue =
                                if expanded { None } else { Some(id.clone()) };
                            cx.notify();
                        })),
                )
                .child(motion::disclosure(
                    text_id("subtitle-technical", message),
                    expanded,
                    v_flex().child(help(details.to_owned()))))
            })
    }

    fn import_base_config(&self) -> course2md::settings::ConfigFile {
        self.workspace
            .as_ref()
            .and_then(|workspace| workspace.state.draft())
            .and_then(|draft| draft.base_config.clone())
            .unwrap_or_else(|| self.preferences.defaults_config())
    }

    pub fn confirm_subtitle(
        &mut self,
        track: SubtitleTrack,
        explicit: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(source) = self.source_preview.clone() else {
            return;
        };
        if let Some(cancel) = self.subtitle_cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.subtitle_generation = self.subtitle_generation.wrapping_add(1);
        let generation = self.subtitle_generation;
        let source_generation = self.preview_generation;
        let token = self
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.state.draft())
            .map(|draft| (draft.id.clone(), draft.revision));
        let source_id = source.identity.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        self.subtitle_cancel = Some(cancel.clone());
        self.subtitle_loading = true;
        self.subtitle_error = None;
        if explicit {
            // The requested text source takes effect before I/O. A failed
            // caption read must never silently keep the previous ASR choice.
            self.task_options.source_mode = TranscriptSource::Subtitle;
        }
        if let Some(source) = &mut self.source_preview {
            source.subtitle_request = Some(track.clone());
            source.subtitle_read_error = None;
        }
        if !self.save_current_draft(cx) {
            self.subtitle_loading = false;
            self.subtitle_cancel = None;
            return;
        }
        self.preview_workers += 1;
        let task = crate::spawn_blocking_io(move || source::read_subtitle(&source, &track, cancel));
        cx.spawn(async move |this, cx| {
            let result = task.recv().await.unwrap_or_else(|_| {
                Err(course2md::subtitle::SubtitleReadError::Failed {
                    message: "字幕读取意外中断".into(),
                })
            });
            let _ = this.update(cx, |this, cx| {
                this.preview_workers = this.preview_workers.saturating_sub(1);
                if this.subtitle_generation != generation
                    || this.preview_generation != source_generation
                {
                    return;
                }
                if !this
                    .source_preview
                    .as_ref()
                    .is_some_and(|source| source.identity == source_id)
                {
                    return;
                }
                if let Some((id, revision)) = &token
                    && !this
                        .workspace
                        .as_ref()
                        .is_some_and(|workspace| workspace.state.matches_input(id, *revision))
                {
                    return;
                }
                this.subtitle_loading = false;
                this.subtitle_cancel = None;
                match result {
                    Ok(subtitle) => {
                        if let Some(source) = &mut this.source_preview {
                            source.selected_subtitle = Some(subtitle);
                            source.subtitle_request = None;
                            source.subtitle_read_error = None;
                        }
                        if explicit {
                            this.task_options.source_mode = TranscriptSource::Subtitle;
                        }
                        this.save_current_draft(cx);
                    }
                    Err(SubtitleReadError::Cancelled) => {
                        this.pending_conversion = None;
                    }
                    Err(error) => {
                        this.subtitle_error = Some(error.to_string());
                        if let Some(source) = &mut this.source_preview {
                            source.subtitle_read_error = Some(error);
                        }
                        this.save_current_draft(cx);
                    }
                }
                this.advance_conversion_when_ready(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn choose_attached_subtitle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(source) = self.source_preview.clone() else {
            return;
        };
        let source_generation = self.preview_generation;
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("选择 SRT 或 VTT 字幕".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = prompt.await;
            let _ = this.update_in(cx, |this, _, cx| {
                if this.preview_generation != source_generation
                    || !this
                        .source_preview
                        .as_ref()
                        .is_some_and(|current| current.identity == source.identity)
                {
                    return;
                }
                match result {
                    Ok(Ok(Some(paths))) => {
                        if let Some(path) = paths.into_iter().next() {
                            if !course2md::subtitle::is_subtitle_file(&path) {
                                this.subtitle_error = Some("请选择 SRT 或 VTT 字幕文件".into());
                            } else {
                                let track = course2md::subtitle::file_track(path, None);
                                if let Some(source) = &mut this.source_preview {
                                    match &mut source.subtitles {
                                        SubtitleEvidence::Found { tracks, .. } => {
                                            if !tracks
                                                .iter()
                                                .any(|candidate| candidate.id == track.id)
                                            {
                                                tracks.push(track.clone());
                                            }
                                        }
                                        _ => {
                                            source.subtitles = SubtitleEvidence::Found {
                                                tracks: vec![track.clone()],
                                                warning: None,
                                            }
                                        }
                                    }
                                }
                                this.confirm_subtitle(track, true, cx);
                            }
                        }
                    }
                    Ok(Ok(None)) => {}
                    Ok(Err(error)) => {
                        this.subtitle_error = Some(format!("无法打开字幕选择器：{error:#}"))
                    }
                    Err(error) => this.subtitle_error = Some(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn retry_subtitles(&mut self, cx: &mut Context<Self>) {
        let Some(source) = self.source_preview.clone() else {
            return;
        };
        if let Some(cancel) = self.subtitle_cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.subtitle_generation = self.subtitle_generation.wrapping_add(1);
        let generation = self.subtitle_generation;
        let source_generation = self.preview_generation;
        let identity = source.identity.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        self.subtitle_cancel = Some(cancel.clone());
        self.subtitle_loading = true;
        self.subtitle_error = None;
        if let Some(source) = &mut self.source_preview {
            source.subtitle_read_error = Some(SubtitleReadError::Failed {
                message: "字幕重新读取未完成".into(),
            });
        }
        if !self.save_current_draft(cx) {
            self.subtitle_loading = false;
            self.subtitle_cancel = None;
            return;
        }
        let token = self
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.state.draft())
            .map(|draft| (draft.id.clone(), draft.revision));
        self.preview_workers += 1;
        let task = crate::spawn_blocking_io(move || source::refresh_subtitles(&source, cancel));
        cx.spawn(async move |this, cx| {
            let result = task
                .recv()
                .await
                .unwrap_or_else(|_| Err(anyhow::anyhow!("字幕读取意外中断")));
            let _ = this.update(cx, |this, cx| {
                this.preview_workers = this.preview_workers.saturating_sub(1);
                if this.subtitle_generation != generation
                    || this.preview_generation != source_generation
                    || token.as_ref().is_some_and(|(id, revision)| {
                        !this
                            .workspace
                            .as_ref()
                            .is_some_and(|workspace| workspace.state.matches_input(id, *revision))
                    })
                    || !this
                        .source_preview
                        .as_ref()
                        .is_some_and(|source| source.identity == identity)
                {
                    return;
                }
                this.subtitle_loading = false;
                this.subtitle_cancel = None;
                match result {
                    Ok(mut evidence) => {
                        if let SubtitleEvidence::Found { tracks, .. } = &mut evidence {
                            let original = this
                                .source_preview
                                .as_ref()
                                .and_then(|source| source.original_language.as_deref());
                            course2md::subtitle::sort_tracks(
                                tracks,
                                &this.preferences.generation().preferred_subtitle_languages,
                                "zh-Hans",
                                original,
                            );
                        }
                        let old_id = this
                            .source_preview
                            .as_ref()
                            .and_then(|source| source.selected_subtitle.as_ref())
                            .map(|subtitle| subtitle.track_id.clone());
                        let track = match &old_id {
                            Some(id) => evidence.tracks().iter().find(|track| &track.id == id),
                            None => evidence.tracks().first(),
                        }
                        .cloned();
                        if old_id.is_some() && track.is_none() {
                            this.subtitle_error = Some("原字幕无法重新读取".into());
                        }
                        if let Some(source) = &mut this.source_preview {
                            source.subtitles = evidence;
                            source.subtitle_read_error = this
                                .subtitle_error
                                .clone()
                                .map(|message| SubtitleReadError::Failed { message });
                        }
                        this.save_current_draft(cx);
                        if let Some(track) = track {
                            this.confirm_subtitle(track, false, cx);
                        }
                    }
                    Err(error) => {
                        this.subtitle_error = Some(format!("{error:#}"));
                        if let Some(source) = &mut this.source_preview {
                            source.subtitle_read_error = this
                                .subtitle_error
                                .clone()
                                .map(|message| SubtitleReadError::Failed { message });
                        }
                        this.save_current_draft(cx);
                    }
                }
                this.advance_conversion_when_ready(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn use_speech(&mut self, cx: &mut Context<Self>) {
        if let Some(cancel) = self.subtitle_cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.subtitle_generation = self.subtitle_generation.wrapping_add(1);
        self.subtitle_loading = false;
        self.subtitle_error = None;
        self.task_options.source_mode = TranscriptSource::Asr;
        if let Some(source) = &mut self.source_preview {
            source.subtitle_request = None;
            source.subtitle_read_error = None;
        }
        self.save_current_draft(cx);
        self.advance_conversion_when_ready(cx);
        cx.notify();
    }

    fn use_confirmed_subtitle(&mut self, cx: &mut Context<Self>) {
        if !self
            .source_preview
            .as_ref()
            .is_some_and(|source| source.selected_subtitle.is_some())
        {
            return;
        }
        if let Some(cancel) = self.subtitle_cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.subtitle_generation = self.subtitle_generation.wrapping_add(1);
        self.subtitle_loading = false;
        self.subtitle_error = None;
        self.task_options.source_mode = TranscriptSource::Subtitle;
        if let Some(source) = &mut self.source_preview {
            source.subtitle_request = None;
            source.subtitle_read_error = None;
        }
        self.save_current_draft(cx);
        self.advance_conversion_when_ready(cx);
        cx.notify();
    }

    fn select_source_input(&mut self, input: String, window: &mut Window, cx: &mut Context<Self>) {
        self.inputs[&Field::Source].update(cx, |state, cx| state.set_value(input, window, cx));
        self.inspect_source(window, cx);
    }

    fn toggle_candidate(&mut self, input: &str, cx: &mut Context<Self>) {
        if !self.selected_candidates.remove(input) {
            self.selected_candidates.insert(input.to_owned());
        }
        cx.notify();
    }

    fn set_candidate_selected(&mut self, input: &str, selected: bool, cx: &mut Context<Self>) {
        if selected {
            self.selected_candidates.insert(input.to_owned());
        } else {
            self.selected_candidates.remove(input);
        }
        cx.notify();
    }

    fn drop_source_files(
        &mut self,
        files: &[PathBuf],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.online {
            self.switch_source_kind(false, window, cx);
        }
        match files {
            [path] => {
                self.inputs[&Field::Source].update(cx, |state, cx| {
                    state.set_value(path.display().to_string(), window, cx)
                });
                self.inspect_source(window, cx);
            }
            [] => {}
            _ => {
                self.invalidate_source();
                self.source_collection_title = Some("选择要处理的视频".into());
                self.source_candidates = files
                    .iter()
                    .map(|path| source::SourceCandidate {
                        input: path.display().to_string(),
                        title: path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned(),
                        identity: None,
                        duration: None,
                        thumbnail: None,
                    })
                    .collect();
                cx.notify();
            }
        }
    }

    /// Source navigation is separate from the editable surface.
    fn source_kind_tabs(&self, cx: &mut Context<Self>) -> Div {
        div().w_full().flex().justify_center().child(
            SingleChoiceGroup::new("source-kind", "视频来源")
                .options([("online", "视频链接"), ("local", "本地文件")])
                .icon("online", icons::link())
                .icon("local", icons::movie())
                .selected(if self.online { "online" } else { "local" })
                .on_change(cx.listener(|this, value: &SharedString, window, cx| {
                    this.switch_source_kind(value.as_ref() == "online", window, cx);
                })),
        )
    }

    pub(super) fn subtitle_attention_required(&self) -> bool {
        self.subtitle_loading
            || self.subtitle_error.is_some()
            || self.source_preview.as_ref().is_some_and(|source| {
                subtitle_needs_confirmation(source, self.task_options.source_mode)
            })
    }

    /// 选项监听器共享的收尾：草稿落盘成功后推进转换就绪，随后刷新界面。
    fn persist_task_options(&mut self, cx: &mut Context<Self>) {
        if self.save_current_draft(cx) {
            self.advance_conversion_when_ready(cx);
        }
        cx.notify();
    }

    /// Visible field, local file target, and status belonging to this source.
    pub fn box_source_input(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let mut view = v_flex().w_full().min_w_0().gap_4();
        let focused = self.inputs[&Field::Source]
            .read(cx)
            .focus_handle(cx)
            .is_focused(window);
        if self.online {
            let validation = self
                .source_preview
                .is_none()
                .then(|| self.source_validation.clone())
                .flatten();
            view = view.child(
                v_flex()
                    .gap_2()
                    // One elevated pill owns the field and its forward action, so
                    // the start button reads as part of the input.
                    .child(
                        h_flex()
                            .w_full()
                            .min_w_0()
                            .h(rems(64. / 14.))
                            .pl(rems(20. / 14.))
                            .pr(rems(10. / 14.))
                            .gap(px(12.))
                            .items_center()
                            .rounded(RADIUS_PILL)
                            .bg(color(SURFACE))
                            // The field's own ring is off inside the pill, so the pill
                            // carries focus: a 2px accent outline, as on other fields.
                            .border_2()
                            .border_color(color(if validation.is_some() || self.preview_error.is_some() {
                                DANGER
                            } else if focused {
                                ACCENT
                            } else {
                                CARD_LINE
                            }))
                            .child(Icon::new(icons::link()).size(rems(24. / 14.)).text_color(color(GRAY)))
                            .child(
                                div().flex_1().min_w_0().child(
                                    text_input(&self.inputs[&Field::Source])
                                        .appearance(false)
                                        .aria_label("视频链接")
                                        .w_full()
                                        .min_w_0()
                                        .px_0()
                                        .text_size(TEXT_TITLE),
                                ),
                            )
                            .when(validation.is_some() || self.preview_error.is_some(), |row| row.child(
                                icons::warning().size(rems(24. / 14.)).text_color(color(DANGER))))
                            // The link pill always ends in its action; an unavailable
                            // start stays visible as a disabled button.
                            .child(
                                nav_action("generation-options", self.generation_options_open, cx)
                                    .child(icons::tune().size(rems(24. / 14.)))
                                    .rounded(RADIUS_PILL)
                                    .w(rems(48. / 14.))
                                    .h(rems(48. / 14.))
                                    .accessibility_label("处理选项")
                                    .tooltip("处理选项")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.generation_options_open =
                                            !this.generation_options_open;
                                        cx.notify();
                                    })),
                            )
                            .child(self.preview_source_button(is_compact(window), cx)),
                    )
                    .when_some(validation, |view, message| view.child(issue(message))),
            );
        } else {
            view = self.local_source_input(view, cx);
        }
        if self.preview_cancel.is_some() {
            view = view.child(v_flex().w_full().min_w_0().gap_3()
                .child(motion::indeterminate_progress("source-reading-progress", cx))
                .child(h_flex().w_full().min_w_0().p_4().gap_4().items_center()
                    .rounded(RADIUS_CARD).bg(color(SURFACE))
                    .child(div().w(rems(128. / 14.)).max_w(relative(0.28))
                        .aspect_ratio(16. / 9.).flex_shrink_0().rounded(RADIUS_SMALL).bg(color(INSET)))
                    .child(v_flex().flex_1().min_w_0().gap_3()
                        .child(accessible_text("import-reading-state", "正在读取视频信息…")
                            .text_size(TEXT_BODY).text_color(color(GRAY)))
                        .child(div().w(relative(0.72)).h(rems(12. / 14.)).rounded_full().bg(color(INSET)))
                        .child(div().w(relative(0.45)).h(rems(10. / 14.)).rounded_full().bg(color(INSET))))
                    .child(quiet("cancel-source-read").icon(crate::icons::close())
                        .accessibility_label("取消读取视频").tooltip("取消读取视频")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.invalidate_source(); cx.notify();
                        })))));
        } else if self.source_preview.is_some()
            && self.workbench_linked_task(cx).is_none() {
            view = view.child(self.box_selected_video(window, cx));
        }
        if let Some(title) = &self.source_collection_title {
            view = self.candidate_list_view(view, title, cx);
        }
        view = self.source_preview_error(view, cx);
        if self.online {
            view = view.child(self.local_upload_hint(cx));
        }
        view
    }

    fn local_upload_hint(&self, cx: &mut Context<Self>) -> Div {
                h_flex()
                    .w_full()
                    .justify_center()
                    .items_center()
                    .flex_wrap()
                    .gap_3()
                    .pt(rems(8. / 14.))
                    .child(
                        accessible_text("import-drop-hint", "或把本地视频拖进窗口")
                            .text_size(TEXT_BODY)
                            .text_color(color(GRAY)),
                    )
                    .child(
                        outline_pill("choose-local-video")
                            .icon(icons::file_upload())
                            .label("选择文件")
                            .text_color(color(ACCENT))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.switch_source_kind(false, window, cx);
                                this.pick_video(window, cx);
                            })),
                    )
    }

    /// 候选合集列表：在线来源走多选批量入队；本地多文件拖入保持单选。
    /// 身份可确认的候选才可勾选；其余保持单条打开。
    fn candidate_list_view(&self, view: Div, title: &str, cx: &mut Context<Self>) -> Div {
        let mut view = view;
        // 在线列表走多选批量入队；本地多文件拖入保持单选（本地身份要做内容哈希，
        // 逐个走原读取确认流程）。身份可确认的候选才可勾选；其余保持单条打开。
        let batch = self.online;
        let identities: Vec<Option<String>> = self
            .source_candidates
            .iter()
            .map(course2md::fetch::candidate_identity)
            .collect();
        let noted: Vec<bool> = identities
            .iter()
            .map(|identity| {
                identity.as_ref().is_some_and(|identity| {
                    self.courses.iter().any(|course| {
                        course
                            .manifest
                            .as_ref()
                            .is_some_and(|manifest| manifest.source_id == *identity)
                    })
                })
            })
            .collect();
        let selectable: Vec<String> = self
            .source_candidates
            .iter()
            .zip(&identities)
            .zip(&noted)
            .filter(|((_, identity), noted)| batch && identity.is_some() && !**noted)
            .map(|((candidate, _), _)| candidate.input.clone())
            .collect();
        let selected_count = self.selected_candidates.len();
        let all_selected = !selectable.is_empty()
            && selectable
                .iter()
                .all(|input| self.selected_candidates.contains(input));
        // 手动勾选了已有笔记的候选：入队会生成新版本，后果与操作同处可见
        let noted_selected = self
            .source_candidates
            .iter()
            .zip(&noted)
            .filter(|(candidate, noted)| {
                **noted && self.selected_candidates.contains(&candidate.input)
            })
            .count();
        let any_batchable = identities.iter().any(Option::is_some);
        let mut list = v_flex()
            .id("source-candidates")
            // 视口按字号缩放（约 3 行 + 露出第 4 行一角）；内边距让
            // 行的 hover/press 涂层不贴容器发丝边
            .max_h(rems(20.))
            .overflow_y_scroll()
            .p_1()
            .rounded(RADIUS_CARD)
            .border_1()
            .border_color(color(CARD_LINE));
        for (index, candidate) in self.source_candidates.iter().enumerate() {
            let input = candidate.input.clone();
            let untitled = candidate.title.trim().is_empty()
                || candidate.title.trim() == candidate.input.trim();
            let title = if untitled {
                format!("视频 {}", index + 1)
            } else {
                candidate.title.clone()
            };
            let (preview, text) = self.candidate_row_parts(candidate, &title, untitled);
            let batchable = batch && identities[index].is_some();
            if batch {
                // 与导出行同一多选模式：Checkbox 自身可点（键盘 Tab+空格可切换），
                // 预览/标题区作为兄弟可点区域；行不再嵌套两层 on_click。
                // 无身份候选不可勾选，同一行样式改为箭头、点击走单条读取确认流程。
                let mut content = h_flex()
                    .id(("source-candidate-text", index))
                    .flex_1()
                    .min_w_0()
                    .gap(px(12.))
                    .items_center()
                    .cursor_pointer();
                if batchable {
                    let toggle_input = candidate.input.clone();
                    content =
                        content.on_click(cx.listener(move |this, _, _, cx| {
                            this.toggle_candidate(&toggle_input, cx)
                        }));
                } else {
                    content = content.on_click(cx.listener(move |this, _, window, cx| {
                        this.select_source_input(input.clone(), window, cx)
                    }));
                }
                content = content.child(preview).child(text);
                if batchable {
                    content = content.when(noted[index], |row| {
                        row.child(badge(BadgeKind::Success).child("已有笔记"))
                    });
                } else {
                    content = content.child(icons::arrow_forward().size(px(18.)).flex_shrink_0());
                }
                let mut row = h_flex()
                    .id(("source-candidate", index))
                    .w_full()
                    .min_w_0()
                    .gap(px(12.))
                    .items_center()
                    .py_3()
                    .px_2()
                    .rounded(RADIUS_SMALL)
                    .hover(|style| style.bg(color(INSET)));
                if batchable {
                    let selected = self.selected_candidates.contains(&candidate.input);
                    let check_input = candidate.input.clone();
                    row = row.child(
                        Checkbox::new(("source-candidate-check", index))
                            .debug_selector(move || format!("source-candidate-checkbox-{index}"))
                            .accessibility_label(format!("选择 {title}"))
                            .checked(selected)
                            .flex_shrink_0()
                            .on_click(cx.listener(move |this, value: &bool, _, cx| {
                                this.set_candidate_selected(&check_input, *value, cx)
                            })),
                    );
                }
                list = list.child(row.child(content));
            } else {
                list = list.child(
                    quiet(("source-candidate", index))
                        .w_full()
                        .h_auto()
                        .py_3()
                        .rounded(RADIUS_SMALL)
                        .justify_start()
                        .tooltip(candidate.input.clone())
                        .accessibility_label(format!("选择 {title}"))
                        .child(preview)
                        .child(text)
                        .child(icons::arrow_forward().size(px(18.)).flex_shrink_0())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.select_source_input(input.clone(), window, cx)
                        })),
                );
            }
        }
        let selectable_inputs = selectable.clone();
        view = view.child(motion::enter(
            text_id("source-candidates", title),
            v_flex()
                .gap_3()
                .child(
                    accessible_text("import-collection-title", title)
                        .font_weight(FontWeight::SEMIBOLD),
                )
                .child(list)
                .when(batch && any_batchable, |view| {
                    view.child(
                        h_flex()
                            .w_full()
                            .items_center()
                            .justify_between()
                            .gap_3()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .min_w_0()
                                    .child(
                                        quiet("toggle-all-candidates")
                                            .label(if all_selected {
                                                "全不选"
                                            } else {
                                                "全选"
                                            })
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                if all_selected {
                                                    this.selected_candidates.clear();
                                                } else {
                                                    this.selected_candidates
                                                        .extend(selectable_inputs.iter().cloned());
                                                }
                                                cx.notify();
                                            })),
                                    )
                                    .when(noted_selected > 0, |row| {
                                        row.child(
                                            div()
                                                .min_w_0()
                                                .text_size(TEXT_AUX)
                                                .text_color(color(WARNING))
                                                .text_ellipsis()
                                                .child(format!(
                                                    "{noted_selected} 个已有笔记，将生成新版"
                                                )),
                                        )
                                    }),
                            )
                            .child(
                                primary_pill("start-batch-conversion")
                                    .icon(icons::arrow_forward())
                                    .label(if selected_count > 0 {
                                        format!("整理选中的 {selected_count} 个视频")
                                    } else {
                                        "整理选中的视频".to_owned()
                                    })
                                    // 空选择即不可执行：与开始转换同一纪律
                                    .disabled(selected_count == 0)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.enqueue_selected_candidates(cx)
                                    })),
                            ),
                    )
                }),
        ));
        view
    }

    /// 候选行共享的 leading 预览槽与标题文本：批量行与单选行仅容器与点击行为不同。
    fn candidate_row_parts(
        &self,
        candidate: &source::SourceCandidate,
        title: &str,
        untitled: bool,
    ) -> (Div, Div) {
        let cover = self.source_candidate_covers.get(&candidate.input).cloned();
        // 统一的 leading 预览槽：已缓存的首帧/封面，
        // 未就绪或无预览时回退到影片图标，保证每行文字起点对齐
        let preview = div()
            .w(rems(5.6))
            .h(rems(3.15))
            .flex_shrink_0()
            .rounded(RADIUS_SMALL)
            .overflow_hidden()
            .bg(color(INSET))
            .flex()
            .items_center()
            .justify_center()
            .when_some(cover, |slot, cover| {
                slot.child(img(cover).w_full().h_full().object_fit(ObjectFit::Cover))
            })
            .when(
                !self.source_candidate_covers.contains_key(&candidate.input),
                |slot| slot.child(icons::movie().size(px(18.)).text_color(color(GRAY))),
            );
        let text = v_flex()
            .flex_1()
            .min_w_0()
            .items_start()
            .gap_1()
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .whitespace_normal()
                    .text_ellipsis()
                    .line_clamp(2)
                    .font_weight(FontWeight::MEDIUM)
                    // 标题是行的扫描锚点：主墨色
                    .text_color(color(INK))
                    .child(title.to_owned()),
            )
            .when(untitled, |view| {
                view.child(
                    div()
                        .w_full()
                        .min_w_0()
                        .text_size(TEXT_AUX)
                        .text_color(color(GRAY))
                        .text_ellipsis()
                        .child(candidate.input.clone()),
                )
            })
            .when_some(
                candidate
                    .duration
                    .filter(|value| *value > 0. && value.is_finite() && !untitled),
                |view, seconds| {
                    view.child(
                        div()
                            .w_full()
                            .min_w_0()
                            .text_size(TEXT_AUX)
                            .text_color(color(GRAY))
                            .text_ellipsis()
                            .child(course2md::render::fmt_ts(seconds)),
                    )
                },
            );
        (preview, text)
    }

    /// Keep source failures beside the field, with retry/login/details still reachable.
    fn source_preview_error(&self, view: Div, cx: &mut Context<Self>) -> Div {
        let mut view = view;
        if let Some(error) = &self.preview_error
            && !self.source_candidates.is_empty()
        {
            view = view.child(help(error.clone()));
        }
        if let Some(error) = &self.preview_error
            && self.source_candidates.is_empty()
        {
            let details = motion::disclosure(
                "source-error-disclosure",
                self.show_preview_details,
                v_flex().child(help(error.clone())));
            view = view.child(motion::enter(
                text_id("source-error", error),
                v_flex().w_full().min_w_0().gap_2()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_start()
                            .child(
                                crate::icons::error()
                                    .size(px(20.))
                                    .text_color(color(DANGER))
                                    .flex_shrink_0(),
                            )
                            .child(
                                issue(if source::is_login_failure(error) {
                                    "视频需要登录后读取，请登录后重试预览".to_owned()
                                } else {
                                    readable_first_line(error)
                                        .unwrap_or("暂时无法读取这个视频，请检查链接或稍后重试")
                                        .to_owned()
                                })
                                .flex_1()
                                .min_w_0(),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .flex_wrap()
                            .child(
                                outline_pill("retry-source")
                                    .icon(icons::refresh())
                                    .label("重试预览")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.inspect_source(window, cx)
                                    })),
                            )
                            .when(
                                self.online
                                    && source::is_login_failure(error)
                                    && course2md::auth::is_bilibili_url(
                                        &self.value(Field::Source, cx),
                                    ),
                                |row| {
                                    row.child(
                                        outline_pill("source-login-repair")
                                            .icon(icons::bilibili())
                                            .label("登录 Bilibili")
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.open_account_dialog(window, cx)
                                            })),
                                    )
                                },
                            )
                            .child(
                                quiet("source-error-details")
                                    .icon(if self.show_preview_details {
                                        crate::icons::chevron_up()
                                    } else {
                                        crate::icons::chevron_down()
                                    })
                                    .label(if self.show_preview_details {
                                        "收起技术详情"
                                    } else {
                                        "技术详情"
                                    })
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.show_preview_details = !this.show_preview_details;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(details),
            ));
        }
        view
    }

    /// 本地来源输入：空输入为拖放区，已选文件为文件名卡。
    fn local_source_input(&self, view: Div, cx: &mut Context<Self>) -> Div {
        let mut view = view;
        let input = self.value(Field::Source, cx);
        if input.is_empty() {
            view = view.child(
                v_flex()
                    .id("video-drop-zone")
                    .gap_4()
                    .p_6()
                    .items_center()
                    .rounded(RADIUS_CARD)
                    .bg(color(INSET))
                    .border_1()
                    .border_color(color(CONTROL))
                    .child(
                        icons::file_upload()
                            .size(rems(32. / 14.))
                            .text_color(color(ACCENT)),
                    )
                    .child(
                        accessible_text("import-drop-instruction", "拖入本地视频")
                            .text_size(TEXT_BODY)
                            .font_weight(FontWeight::MEDIUM),
                    )
                    .child(
                        primary_pill("choose-video")
                            .icon(crate::icons::folder_open())
                            .label("选择视频")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.pick_video(window, cx)),
                            ),
                    )
                    .on_drop(
                        cx.listener(|this, paths: &gpui::ExternalPaths, window, cx| {
                            this.drop_source_files(paths.paths(), window, cx)
                        }),
                    ),
            );
        } else {
            let filename = PathBuf::from(&input)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| input.clone());
            view = view.child(
                h_flex()
                    .id("selected-local-video")
                    .w_full()
                    .min_w_0()
                    .items_center()
                    .flex_wrap()
                    .gap_3()
                    .p_4()
                    .rounded(RADIUS_CARD)
                    .bg(color(SURFACE))
                    .border_1()
                    .border_color(color(HAIRLINE))
                    .child(
                        semantic_label(
                            "selected-local-video-name",
                            filename,
                            icons::movie()
                                .size(rems(20. / 14.))
                                .text_color(color(ACCENT)),
                        )
                        .flex_1()
                        .min_w(rems(18.))
                        .max_w_full(),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .flex_wrap()
                            .items_center()
                            .child(
                                outline_pill("choose-video")
                                    .icon(crate::icons::folder_open())
                                    .label("更换视频")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.pick_video(window, cx)
                                    })),
                            )
                            .when(self.can_start_input(cx), |row| {
                                row.child(self.preview_source_button(false, cx))
                            }),
                    )
                    .on_drop(
                        cx.listener(|this, paths: &gpui::ExternalPaths, window, cx| {
                            this.drop_source_files(paths.paths(), window, cx)
                        }),
                    ),
            );
            if self.source_preview.is_none()
                && let Some(error) = &self.source_validation
            {
                view = view.child(issue(error.clone()));
            }
        }
        view
    }

    /// The video identity remains compact; paths and account tools are details.
    fn box_selected_video(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let Some(source) = &self.source_preview else { return v_flex(); };
        let compact = is_compact(window);
        let duration = (source.duration.is_finite() && source.duration > 0.)
            .then(|| course2md::render::fmt_ts(source.duration));
        let mut cover = div().relative().w(rems(if compact { 128. / 14. } else { 160. / 14. }))
            .max_w(relative(0.28)).aspect_ratio(16. / 9.).flex_shrink_0().rounded(RADIUS_SMALL)
            .bg(color(SECONDARY_CONTAINER)).overflow_hidden()
            .when_some(source.cover.clone(), |cover, path| cover.child(
                img(path).size_full().rounded(RADIUS_SMALL).object_fit(ObjectFit::Cover)));
        if let Some(duration) = duration {
            cover = cover.child(div().absolute().right(px(4.)).bottom(px(4.)).px_1().py(px(1.))
                .rounded(px(3.)).bg(color(INK)).text_color(color(CANVAS))
                .text_size(rems(10. / 14.)).child(duration));
        }
        let pending = source.subtitle_request.is_some() || source.subtitle_read_error.is_some()
            || self.subtitle_error.is_some();
        let confirmed_subtitle = !self.subtitle_loading && !pending && !self.import_uses_speech()
            && source.selected_subtitle.is_some();
        let subtitle_status = if self.subtitle_loading { "正在读取字幕…" }
            else if pending { "文字来源待确认" }
            else if self.import_uses_speech() { "将识别视频声音" }
            else if confirmed_subtitle { "有字幕，将直接使用" }
            else { "文字来源待确认" };
        let platform = if !source.online { "本地视频".to_owned() }
            else {
                match course2md::config::known_video_site(&source.input) {
                    Some("bilibili") => "Bilibili".to_owned(),
                    Some("youtube") => "YouTube".to_owned(),
                    _ => url::Url::parse(&source.input).ok()
                        .and_then(|url| url.host_str().map(str::to_owned)).unwrap_or_default(),
                }
            };
        let metadata = [platform, source.author.clone()].into_iter()
            .filter(|part| !part.trim().is_empty()).collect::<Vec<_>>().join(" · ");
        let details = v_flex().gap_3().child(help(source.input.clone())).when(
            source.online && course2md::auth::is_bilibili_url(&source.input),
            |view| view.child(self.source_account_row(cx)),
        );
        let selected = h_flex().w_full().min_w_0().gap_4().items_center()
            .child(cover)
            .child(v_flex().min_w_0().flex_1().gap(rems(6. / 14.))
                .child(accessible_text("import-selected-title", source.title.clone())
                    .text_size(rems(16. / 14.)).line_height(rems(22. / 14.)).font_weight(FontWeight::SEMIBOLD)
                    .whitespace_normal().text_ellipsis().line_clamp(2))
                .when(!metadata.is_empty(), |view| view.child(
                    div().text_size(TEXT_BODY).text_color(color(GRAY)).child(metadata)))
                .child(h_flex().min_w_0().child(badge(BadgeKind::Neutral)
                    .rounded(RADIUS_SMALL).text_size(rems(12. / 14.))
                    .when(confirmed_subtitle, |badge| badge.child(icons::check().size_3()))
                    .child(subtitle_status))))
            .child(v_flex().flex_shrink_0().items_end().justify_between().gap_2()
                .child(quiet("clear-source-preview").icon(icons::close())
                    .tooltip("移除预览").accessibility_label("移除预览")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.invalidate_source(); this.save_current_draft(cx); cx.notify();
                    })))
                .child(h_flex().gap_1()
                    .child(quiet("source-details").icon(if self.show_preview_details { crate::icons::chevron_up() } else { crate::icons::chevron_down() })
                        .w(rems(32. / 14.)).h(rems(32. / 14.)).px_0()
                        .accessibility_label("来源详情").tooltip("来源详情")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.show_preview_details = !this.show_preview_details; cx.notify();
                        })))
                    .when(self.online, |row| row.child(quiet("reread-source").icon(icons::refresh())
                        .w(rems(32. / 14.)).h(rems(32. / 14.)).px_0()
                        .accessibility_label("重新读取").tooltip("重新读取")
                        .on_click(cx.listener(|this, _, window, cx| this.inspect_source(window, cx)))))));
        v_flex().w_full().min_w_0().child(v_flex().id("import-selected-source")
            .w_full().min_w_0().p(rems(12. / 14.)).rounded(RADIUS_CARD).bg(color(SURFACE))
            .border_1().border_color(color(CARD_LINE)).gap_3()
            .on_drop(cx.listener(|this, paths: &gpui::ExternalPaths, window, cx| {
                this.drop_source_files(paths.paths(), window, cx)
            }))
            .child(selected)
            .when(!self.online && self.can_start_input(cx), |card| card.child(
                h_flex().w_full().justify_end().child(self.box_bottom_row(cx))))
            .child(motion::disclosure("source-details-disclosure", self.show_preview_details, details)))
    }

    fn text_source_view(&self, cx: &mut Context<Self>) -> Div {
        let Some(source) = &self.source_preview else {
            return v_flex();
        };
        let speech = self.import_uses_speech();
        let failure = self.subtitle_error.as_deref().or_else(|| {
            if subtitle_needs_confirmation(source, self.task_options.source_mode)
                && let SubtitleEvidence::Failed { message } = &source.subtitles
            {
                Some(message.as_str())
            } else {
                None
            }
        });
        let needs_text_choice =
            subtitle_needs_confirmation(source, self.task_options.source_mode) || failure.is_some();
        let description = if self.subtitle_loading {
            "正在读取字幕…".to_owned()
        } else if !speech
            && (source.subtitle_request.is_some() || source.subtitle_read_error.is_some())
        {
            match &source.selected_subtitle {
                Some(_) => "字幕还没读取完成".into(),
                None => "字幕还没读取完成".into(),
            }
        } else if speech {
            "识别视频声音".into()
        } else if let Some(subtitle) = &source.selected_subtitle {
            // 自动选择的解析结果就是当前值，与显式选择同样只显示字幕名
            subtitle.label.clone()
        } else {
            match &source.subtitles {
                SubtitleEvidence::Unchecked => "未检查字幕".into(),
                SubtitleEvidence::Found { .. } => "请选择字幕".into(),
                SubtitleEvidence::NoneFound | SubtitleEvidence::Unsupported { .. } => {
                    "尚未选择文字来源".into()
                }
                SubtitleEvidence::Failed { .. } => "字幕还没读取完成".into(),
            }
        };
        let mut view = v_flex().w_full().min_w_0().gap_3().child(
            h_flex()
                .gap_3()
                .items_center()
                .child(if self.subtitle_loading {
                    motion::spinner("subtitle-reading-spinner", cx)
                } else {
                    (if speech {
                        icons::mic()
                    } else {
                        icons::subtitles()
                    })
                    .size(px(20.))
                    .text_color(color(GRAY))
                    .into_any_element()
                })
                .child(
                    accessible_text("import-text-source-state", description)
                        .font_weight(FontWeight::MEDIUM)
                        // 值与其「更换」动作收成一个对象行，不用 flex_1 把两者钉到两端
                        .min_w_0()
                        .whitespace_normal(),
                )
                .when(!needs_text_choice, |row| {
                    row.child(
                        quiet("change-text-source")
                            .icon(if self.show_options {
                                crate::icons::chevron_up()
                            } else {
                                crate::icons::chevron_down()
                            })
                            .label(if self.show_options {
                                "收起"
                            } else {
                                "更换"
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.show_options = !this.show_options;
                                cx.notify();
                            })),
                    )
                }),
        );
        if self.subtitle_loading {
            view = view.child(
                quiet("cancel-subtitle-read")
                    .icon(crate::icons::close())
                    .self_start()
                    .label("取消读取字幕")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.pending_conversion = None;
                        if let Some(cancel) = this.subtitle_cancel.take() {
                            cancel.store(true, Ordering::Relaxed);
                        }
                        this.subtitle_generation = this.subtitle_generation.wrapping_add(1);
                        this.subtitle_loading = false;
                        if let Some(source) = &mut this.source_preview {
                            source.subtitle_request = None;
                            source.subtitle_read_error = None;
                        }
                        this.save_current_draft(cx);
                        cx.notify();
                    })),
            );
        }
        if let Some(error) = failure {
            let failure_details = self.subtitle_issue("subtitle-read", error, cx);
            view = view.child(motion::enter(
                text_id("subtitle-failure", error),
                v_flex()
                    .gap_3()
                    .p_4()
                    .rounded(RADIUS_CARD)
                    .bg(color(WARNING_BG))
                    .child(
                        h_flex()
                            .gap_2()
                            .items_start()
                            .child(
                                crate::icons::warning()
                                    .size(px(20.))
                                    .text_color(color(WARNING))
                                    .flex_shrink_0(),
                            )
                            .child(
                                accessible_text("subtitle-failed", "字幕未读取成功")
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(color(WARNING)),
                            ),
                    )
                    .child(failure_details),
            ));
            if let Some(old) = &source.selected_subtitle {
                view = view.child(
                    outline_pill("use-previous-subtitle")
                        .self_start()
                        .icon(icons::subtitles())
                        .label("继续使用之前的字幕")
                        .tooltip(old.label.clone())
                        .on_click(cx.listener(|this, _, _, cx| this.use_confirmed_subtitle(cx))),
                );
            }
        }
        let show_text_choices = self.show_options || needs_text_choice;
        if speech && !show_text_choices {
            match &source.subtitles {
                SubtitleEvidence::NoneFound => view = view.child(help("未找到字幕")),
                SubtitleEvidence::Unsupported { message } => {
                    view = view.child(help(message.clone()))
                }
                _ => {}
            }
        }
        {
            let mut tracks = source.subtitles.tracks().to_vec();
            if let Some(pending) = &source.subtitle_request
                && !tracks.iter().any(|track| track.id == pending.id)
            {
                tracks.push(pending.clone());
            }
            let cached_id = source
                .selected_subtitle
                .as_ref()
                .map(|subtitle| subtitle.track_id.clone());
            let mut choices: Vec<_> = tracks
                .iter()
                .map(|track| (format!("track:{}", track.id), track.label()))
                .collect();
            if let Some(cached) = &source.selected_subtitle
                && !tracks.iter().any(|track| track.id == cached.track_id)
            {
                choices.push((format!("track:{}", cached.track_id), cached.label.clone()));
            }
            choices.push(("speech".into(), "识别视频声音".into()));
            let selected = if speech {
                Some("speech".to_owned())
            } else {
                source
                    .subtitle_request
                    .as_ref()
                    .map(|track| track.id.as_str())
                    .or(cached_id.as_deref())
                    .map(|id| format!("track:{id}"))
            };
            let mut options = v_flex().gap_2().child(
                div().child(
                    SingleChoiceGroup::new("import-text-source", "笔记的文字来源")
                        .map(|group| choices.iter().fold(group, |group, (value, _)| {
                            group.icon(value.clone(), if value == "speech" {
                                icons::mic()
                            } else {
                                icons::subtitles()
                            })
                        }))
                        .options(choices)
                        .when_some(selected, |group, value| group.selected(value))
                        .on_change(cx.listener(move |this, value: &SharedString, _, cx| {
                            if value.as_ref() == "speech" {
                                this.use_speech(cx);
                            } else if let Some(id) = value.strip_prefix("track:") {
                                if cached_id.as_deref() == Some(id) {
                                    this.use_confirmed_subtitle(cx);
                                } else if let Some(track) =
                                    tracks.iter().find(|track| track.id == id)
                                {
                                    this.confirm_subtitle(track.clone(), true, cx);
                                }
                            }
                        })),
                ),
            );
            if self.subtitle_error.is_none()
                && let SubtitleEvidence::Found {
                    warning: Some(message),
                    ..
                } = &source.subtitles
            {
                options = options.child(self.subtitle_issue(
                    "subtitle-warning",
                    &format!("部分字幕无法读取：{message}"),
                    cx,
                ));
            }
            if let SubtitleEvidence::Unsupported { message } = &source.subtitles {
                options = options.child(help(message.clone()));
            }
            if matches!(source.subtitles, SubtitleEvidence::NoneFound) {
                options = options.child(help("未找到字幕"));
            }
            options = options.child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(
                        outline_pill("retry-subtitles")
                            .icon(icons::refresh())
                            .label("重新读取字幕")
                            .disabled(self.subtitle_loading)
                            .on_click(cx.listener(|this, _, _, cx| this.retry_subtitles(cx))),
                    )
                    .when(!source.online, |view| {
                        view.child(
                            outline_pill("attach-subtitles")
                                .icon(icons::file_upload())
                                .label("选择字幕文件")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.choose_attached_subtitle(window, cx)
                                })),
                        )
                    })
                    .when(
                        source.online
                            && course2md::auth::is_bilibili_url(&source.input)
                            && self
                                .subtitle_error
                                .as_deref()
                                .or(match &source.subtitles {
                                    SubtitleEvidence::Failed { message } => Some(message.as_str()),
                                    _ => None,
                                })
                                .is_some_and(source::is_login_failure),
                        |view| {
                            view.child(
                                outline_pill("login-for-subtitles")
                                    .icon(icons::bilibili())
                                    .label("登录 Bilibili")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_account_dialog(window, cx)
                                    })),
                            )
                        },
                    ),
            );
            view = view.child(motion::disclosure(
                "text-source-choices",
                show_text_choices,
                options));
        }
        view
    }

    fn import_text_mode_options(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let selector = SingleChoiceGroup::new("conversion-text-mode", "文字来源方式")
            .segmented()
            .options(TEXT_SOURCE_CHOICES.map(|(mode, label)| (text_source_id(mode), label)))
            .icon(text_source_id(TranscriptSource::Auto), icons::auto_fix())
            .icon(text_source_id(TranscriptSource::Subtitle), icons::subtitles())
            .icon(text_source_id(TranscriptSource::Asr), icons::mic())
            .selected(text_source_id(self.task_options.source_mode))
            .on_change(cx.listener(|this, value: &SharedString, _, cx| {
                let Some(mode) = TEXT_SOURCE_CHOICES
                    .iter()
                    .map(|(mode, _)| *mode)
                    .find(|mode| text_source_id(*mode) == value.as_ref())
                else {
                    return;
                };
                if mode == TranscriptSource::Asr {
                    this.use_speech(cx);
                } else {
                    this.task_options.source_mode = mode;
                    this.save_current_draft(cx);
                    this.advance_conversion_when_ready(cx);
                    cx.notify();
                }
            }));
        let mut view = v_flex()
            .w_full()
            .min_w_0()
            .gap(rems(24. / 14.))
            .child(import_option_section(
                "conversion-text-mode-label",
                "文字来源",
                v_flex().w_full().gap_2().child(selector).when(
                    self.task_options.source_mode == TranscriptSource::Auto,
                    |view| {
                        view.child(
                            aux_text("有字幕时直接用字幕，没有字幕时识别视频声音"),
                        )
                    },
                ),
            ));
        if self.task_options.source_mode != TranscriptSource::Subtitle {
            view = view.child(outlined_field_label(
                "conversion-speech-label",
                "语音识别",
                TEXT_AUX,
                self.import_speech_options(window, cx),
            ));
        }
        view
    }

    fn import_ai_options(&self, cx: &mut Context<Self>) -> Div {
        let mut choices = v_flex().w_full().min_w_0().gap_2();
        for (index, option) in [
            ConversionAiOption::Proofread,
            ConversionAiOption::Summary,
            ConversionAiOption::Vision,
        ]
        .into_iter()
        .enumerate()
        {
            choices = choices.child(import_option_switch_row(
                conversion_ai_option_label(option), "",
                processing_switch(Switch::new(("import-ai-option", index)))
                    .checked(conversion_ai_option_enabled(&self.task_options, option))
                    .disabled(option == ConversionAiOption::Vision && !self.task_options.llm)
                    .on_click(cx.listener(move |this, enabled, _, cx| {
                        apply_conversion_ai_option(&mut this.task_options, option, *enabled);
                        this.persist_task_options(cx);
                    })),
            ));
        }
        if self.task_options.llm && self.import_base_config().llm.prompt
            .as_ref().is_some_and(|prompt| !prompt.trim().is_empty()) {
            choices = choices.child(help("使用自定义校对规则"));
        }
        let mut view = v_flex()
            .w_full()
            .min_w_0()
            .gap(rems(24. / 14.))
            .child(import_option_section("import-ai-label", "AI 处理", choices));
        if self.task_options.llm || self.task_options.summarize {
            let mut service = v_flex()
                .w_full()
                .min_w_0()
                .gap_2()
                .child(self.task_service_picker(ServicePurpose::Ai, cx));
            if let Some(selected) = self.selected_task_service(ServicePurpose::Ai) {
                service = service.child(aux_text(
                    format!(
                        "{}会发送到 {}",
                        if self.task_options.llm && self.task_options.vision {
                            "文字及对应截图"
                        } else {
                            "文字"
                        },
                        selected.config.host()
                    ),
                ));
            }
            view = view.child(if self.selected_task_service(ServicePurpose::Ai).is_some() {
                outlined_field_label("import-ai-service-label", "AI 服务", TEXT_AUX, service)
            } else {
                import_option_label("import-ai-service-label", "AI 服务", service)
            });
        }
        view
    }

    fn reset_task_ai_overrides(&mut self, cx: &mut Context<Self>) {
        if !self.save_current_draft(cx) {
            return;
        }
        let defaults = ConversionOptions::from_config(&self.preferences.defaults_config());
        let Some(workspace) = &mut self.workspace else {
            return;
        };
        if let Err(error) = workspace.transaction(|state| {
            state
                .draft_mut()
                .context("当前视频输入暂时不可用")?
                .reset_ai_overrides(&defaults);
            Ok(())
        }) {
            self.workspace_error = Some(format!("本次选项尚未恢复默认：{error:#}"));
            cx.notify();
            return;
        }
        self.task_options.llm = defaults.llm;
        self.task_options.summarize = defaults.summarize;
        self.task_options.vision = defaults.vision;
        self.advance_conversion_when_ready(cx);
        cx.notify();
    }

    fn import_speech_options(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let cloud = self.task_options.uses_cloud_provider();
        let label = if cloud {
            "识别服务".to_owned()
        } else {
            let (_, model, _) = self.import_model_request();
            format!("本机 · {}", course2md::config::model_label(&model))
        };
        let engine = if cloud {
            String::new()
        } else {
            self.local_engine_name().to_owned()
        };
        let current = self.task_options.provider;
        let entity = cx.entity().downgrade();
        let providers: Vec<_> = PROVIDERS
            .iter()
            .enumerate()
            .filter(|(index, _)| {
                use course2md::config::AsrProvider;
                // Automatic, CPU and the cloud service are always offered.
                let provider = crate::asr_provider_from_index(*index);
                matches!(provider, None | Some(AsrProvider::Cpu | AsrProvider::Api))
                    || *index == current
                    || provider.zip(self.environment.as_ref())
                        .is_some_and(|(provider, environment)| environment.supports(provider))
            })
            .map(|(index, (_, label))| (index, *label))
            .collect();
        let selector = control("import-speech-location")
            .h(rems(56. / 14.))
            .font_weight(FontWeight::NORMAL).text_size(rems(16. / 14.))
            .rounded(RADIUS_SMALL)
            .w_full()
            .min_w_0()
            .accessibility_label("语音识别方式")
            .child(div().flex_1().min_w_0().text_left().child(label))
            .child(
                div()
                    .text_color(color(GRAY))
                    .font_weight(FontWeight::NORMAL)
                    .child(engine),
            )
            .child(crate::icons::chevron_down().size_4())
            .dropdown_menu(move |menu, _, _| {
                providers.iter().fold(menu, |menu, &(index, label)| {
                    let entity = entity.clone();
                    menu.item(
                        PopupMenuItem::new(label)
                            .checked(index == current)
                            .on_click(move |_, _, cx| {
                                let _ = entity.update(cx, |this, cx| {
                                    this.task_options.provider = index;
                                    this.persist_task_options(cx);
                                });
                            }),
                    )
                })
            });
        let mut view = v_flex().w_full().min_w_0().gap_2().child(selector);
        if cloud {
            view = view
                .child(self.task_service_picker(ServicePurpose::Speech, cx))
                .child(
                    aux_text("音频会发送到所选识别服务"),
                );
        } else {
            let (provider, model, root) = self.import_model_request();
            let snapshot = self.setup_model_snapshot(provider, Some(&model), &root);
            let ready = snapshot.device_issue.is_none()
                && !snapshot.preparing
                && snapshot.result.as_ref().is_some_and(|result| {
                    result.as_ref().is_ok_and(|status| {
                        matches!(
                            status.state,
                            course2md::models::status::CacheState::Cached
                                | course2md::models::status::CacheState::Loaded
                        )
                    })
                });
            if ready {
                view = view.child(aux_text(
                    format!(
                        "模型已下载 · {}",
                        course2md::config::model_label(&model)
                    ),
                ));
            } else if snapshot.device_issue.is_none()
                && !snapshot.checking
                && !snapshot.preparing
                && snapshot.notice.is_none()
                && !snapshot.cancelled
                && snapshot.result.as_ref().is_some_and(|result| {
                    result.as_ref().is_ok_and(|status| {
                        status.state == course2md::models::status::CacheState::Missing
                            && status.can_prepare
                            && status.last_error.is_none()
                    })
                })
            {
                // A routine first download needs one status line and one action.
                // Checks, failures and active transfers retain the full recovery panel below.
                let prepare_model = model.clone();
                let prepare_root = root.clone();
                let details = cx.weak_entity();
                view = view.child(
                    h_flex().w_full().min_w_0().gap_2().flex_wrap().items_center()
                        .child(div().flex_1().min_w_0().text_size(TEXT_AUX)
                            .text_color(color(GRAY))
                            .child(format!("{} · 待下载",
                                course2md::config::model_label(&model))))
                        .child(outline_pill("import-prepare-model")
                            .icon(icons::download()).label("下载模型")
                            .disabled(self.job.is_some())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.prepare_setup_model(provider, Some(&prepare_model), &prepare_root, cx);
                            })))
                        .child(quiet("import-model-details")
                            .icon(icons::ellipsis()).w(CONTROL_HEIGHT).px_0()
                            .accessibility_label("模型检查与设置")
                            .tooltip("模型检查与设置")
                            .dropdown_menu(move |menu, _, _| {
                                let recheck = details.clone();
                                let settings = details.clone();
                                menu.item(PopupMenuItem::new("重新检查模型")
                                    .icon(icons::refresh())
                                    .on_click(move |_, _, cx| {
                                        let _ = recheck.update(cx, |this, cx| this.refresh_model_diagnostics(cx));
                                    }))
                                    .item(PopupMenuItem::new("模型设置")
                                        .icon(icons::settings())
                                        .on_click(move |_, window, cx| {
                                            let _ = settings.update(cx, |this, cx| this.open_settings(window, cx));
                                        }))
                            })),
                ).when(self.job.is_some(), |view| view.child(
                    aux_text("当前任务完成后才能下载模型")));
            } else {
                view = view.child(self.model_readiness_panel_with(
                    provider,
                    Some(&model),
                    &root,
                    false,
                    window,
                    cx,
                ));
            }
        }
        view
    }

    fn import_uses_speech(&self) -> bool {
        self.source_preview
            .as_ref()
            .is_some_and(|source| uses_speech(source, self.task_options.source_mode))
    }

    fn missing_import_service(&self) -> Option<ServicePurpose> {
        let separate_subtitle = self.task_options.source_mode != TranscriptSource::Asr
            && self
                .workspace
                .as_ref()
                .and_then(|workspace| workspace.state.draft())
                .is_some_and(|draft| draft.subtitle.is_some());
        [
            (
                ServicePurpose::Speech,
                self.task_options.uses_cloud_provider()
                    && self.import_uses_speech()
                    && !separate_subtitle,
            ),
            (
                ServicePurpose::Ai,
                self.task_options.llm || self.task_options.summarize,
            ),
        ]
        .into_iter()
        .find_map(|(purpose, required)| {
            (required
                && self.selected_task_service(purpose).is_none_or(|version| {
                    self.preferences
                        .service_retired_in_snapshot(&version.service_id)
                }))
            .then_some(purpose)
        })
    }

    fn import_model_request(&self) -> (course2md::config::AsrProvider, String, PathBuf) {
        let provider = self.actual_local_provider();
        let mut config = self.import_base_config();
        // 与计划构造同一回填规则：空白时按识别方式补默认模型（NPU 专属解析）。
        crate::task_ui::fill_default_asr_model(&mut config, provider);
        let model = config.defaults.asr_model.unwrap_or_default();
        let root = course2md::config::model_dir_from(config.defaults.model_dir.as_deref());
        (provider, model, root)
    }

    fn actual_local_provider(&self) -> course2md::config::AsrProvider {
        use course2md::config::AsrProvider;
        crate::asr_provider_from_index(self.task_options.provider)
            .filter(|provider| *provider != AsrProvider::Api)
            .unwrap_or_else(|| self.recommended_local_provider())
    }
    fn local_engine_name(&self) -> &'static str {
        crate::provider_label(Some(self.actual_local_provider()))
    }

    fn import_destination(&self, cx: &mut Context<Self>) -> Div {
        let current = self
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.state.draft())
            .map(|draft| draft.library_id.clone());
        let libraries = self
            .workspace
            .as_ref()
            .map(|workspace| workspace.state.libraries.clone())
            .unwrap_or_default();
        let selected = libraries
            .iter()
            .find(|library| Some(&library.id) == current.as_ref())
            .cloned();
        let mut view = v_flex().w_full().min_w_0().gap_2();
        if libraries.len() > 1 {
            let entity = cx.entity().downgrade();
            let label = selected
                .as_ref()
                .map(|library| library.name.clone())
                .unwrap_or_else(|| "选择保存位置".into());
            view = view.child(
                control("import-library")
                    .icon(crate::icons::folder_open())
                    .w_full()
                    .min_w_0()
                    .accessibility_label(format!("保存到：{label}"))
                    .tooltip(label.clone())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(label),
                    )
                    .child(crate::icons::chevron_down().size_4().flex_shrink_0())
                    .dropdown_menu(move |menu, _, _| {
                        libraries.iter().fold(menu, |menu, library| {
                            let id = library.id.clone();
                            let entity = entity.clone();
                            menu.item(
                                PopupMenuItem::new(format!(
                                    "{} · {}",
                                    library.name,
                                    library.root.display()
                                ))
                                .checked(current.as_ref() == Some(&id))
                                .on_click(move |_, _, cx| {
                                    let _ = entity.update(cx, |this, cx| {
                                        if !this.save_current_draft(cx) {
                                            return;
                                        }
                                        if let Some(workspace) = &mut this.workspace {
                                            match workspace.transaction(|state| {
                                                if let Some(draft) = state.draft_mut() {
                                                    draft.library_id = id.clone();
                                                    draft.folder = None;
                                                }
                                                Ok(())
                                            }) {
                                                Ok(()) => {
                                                    this.target_folder = None;
                                                    this.advance_conversion_when_ready(cx);
                                                }
                                                Err(error) => {
                                                    this.workspace_error =
                                                        Some(format!("保存位置尚未更新：{error:#}"))
                                                }
                                            }
                                        }
                                        cx.notify();
                                    });
                                }),
                            )
                        })
                    }),
            );
        }
        view = view.child(
            h_flex()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(self.folder_picker(None, 0, cx)),
                )
                .child(
                    outline_pill("import-new-folder")
                        .icon(icons::create_new_folder())
                        .w(rems(48. / 14.)).h(rems(48. / 14.)).px_0()
                        .accessibility_label("新建文件夹")
                        .tooltip("新建文件夹")
                        .on_click(
                            cx.listener(|this, _, window, cx| this.begin_folder(None, window, cx)),
                        ),
                ),
        );
        v_flex().w_full().min_w_0()
            .when(self.source_preview.is_some(), |view| view.child(
                import_option_label(
                    "import-title-label", "笔记名称",
                    crate::focus_scroll::RevealFocus::new(
                        ("import-title-focus", self.validation_attempt),
                        text_input(&self.inputs[&Field::Title]).w_full(),
                        self.scrolls[Page::New as usize].clone(),
                    ),
                ),
            ))
            .child(outlined_field_label(
            "import-destination-label",
            "保存到",
            TEXT_AUX,
            view,
        ))
    }

    fn import_exports(&self, cx: &mut Context<Self>) -> Div {
        let mut choices = h_flex().w_full().min_w_0().gap_2().flex_wrap();
        for (index, label) in ["Markdown", "网页", "JSON"].into_iter().enumerate() {
            choices = choices.child(
                filter_chip(
                    ("import-export", index),
                    self.task_options.formats[index],
                    cx,
                )
                .when(self.task_options.formats[index], |button| button.icon(icons::check()))
                .label(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.task_options.formats[index] = !this.task_options.formats[index];
                    this.save_current_draft(cx);
                    cx.notify();
                })),
            );
        }
        v_flex().w_full().min_w_0().child(import_option_section(
            "import-exports-label",
            "同时导出",
            choices,
        )).when(self.online, |view| view.child(
            div().w_full().min_w_0().pt(rems(24. / 14.))
                .child(import_option_switch_row(
                    "保留原视频", "供离线播放，会占用更多磁盘空间",
                    processing_switch(Switch::new("import-keep-video"))
                        .checked(self.task_options.keep_video)
                        .on_click(cx.listener(|this, value, _, cx| {
                            this.task_options.keep_video = *value;
                            this.save_current_draft(cx);
                            cx.notify();
                        })),
                )),
        ))
    }

    fn existing_source_note(&self) -> Option<Course> {
        let source = self.source_preview.as_ref()?;
        let preferred = self
            .workspace
            .as_ref()
            .and_then(|workspace| {
                workspace
                    .state
                    .draft()
                    .and_then(|draft| workspace.state.library(&draft.library_id))
            })
            .map(|library| &library.root);
        self.courses
            .iter()
            .filter(|course| {
                course
                    .manifest
                    .as_ref()
                    .is_some_and(|manifest| manifest.source_id == source.identity)
            })
            .min_by_key(|course| !preferred.is_some_and(|root| course.dir.starts_with(root)))
            .cloned()
    }

    fn can_start_input(&self, cx: &App) -> bool {
        // 候选列表展示时，批量/单选操作在列表自己的行动区；输入行的「开始转换」
        // 对合集链接必然失败，不再同屏出现两个 primary
        self.source_collection_title.is_none()
            && (self.online || !self.value(Field::Source, cx).is_empty())
            && self.preview_error.is_none()
            && self.existing_source_note().is_none()
            && self.current_input_task(cx).is_none_or(|task| {
                matches!(
                    task.state,
                    workspace::TaskState::Complete | workspace::TaskState::Cancelled
                ) && !(self.reading
                    && self
                        .following_conversion
                        .as_ref()
                        .is_some_and(|follow| follow.follows(&task.id, self.preview_generation)))
            })
    }

    fn preview_source_button(&self, compact: bool, cx: &mut Context<Self>) -> Div {
        // Enter remains bound to inspect_source. Only this explicitly relabeled
        // button changes to conversion once an eligible preview is on screen.
        let start = self.online && self.source_preview.is_some()
            && self.workbench_linked_task(cx).is_none() && self.can_start_input(cx);
        let disabled = self.preview_cancel.is_some() || self.value(Field::Source, cx).is_empty()
            || (start && self.pending_conversion.is_some());
        let label = if start { "开始转换" } else { "预览视频" };
        h_flex().items_center().child(
            theme::disabled_when(primary_pill("preview-source"), disabled, cx)
                .h(rems(48. / 14.)).icon(icons::arrow_forward())
                .when(!compact || start, |button| button.label(label))
                .when(compact && !start, |button| button.w(rems(48. / 14.)).px_0())
                .accessibility_label(label).tooltip(label)
                .track_focus(&self.import_submit_focus).disabled(disabled)
                .on_click(cx.listener(move |this, _, window, cx| {
                    if start { this.start_conversion(window, cx); }
                    else { this.inspect_source(window, cx); }
                })),
        )
    }

    /// Conversion starts only from the explicit action beside the preview.
    fn box_bottom_row(&self, cx: &mut Context<Self>) -> Div {
        let disabled = self.pending_conversion.is_some()
            || self.source_preview.is_none() || self.preview_cancel.is_some();
        h_flex().items_center().child(
            theme::disabled_when(primary_pill("start-conversion"), disabled, cx)
                .icon(icons::arrow_forward()).label("开始转换")
                .disabled(disabled)
                .on_click(cx.listener(|this, _, window, cx| this.start_conversion(window, cx))),
        )
    }

    pub(super) fn current_input_task(&self, cx: &App) -> Option<&workspace::TaskRecord> {
        let state = &self.workspace.as_ref()?.state;
        submitted_input_task(state.draft()?, &state.tasks, &self.value(Field::Source, cx))
    }

    /// The source card and hero share one definition of the submitted task.
    fn workbench_linked_task(&self, cx: &App) -> Option<&workspace::TaskRecord> {
        let opening_result = self.reading && self.following_conversion.is_some();
        self.current_input_task(cx).filter(|task| {
            (opening_result
                && matches!(
                    task.state,
                    workspace::TaskState::Complete | workspace::TaskState::Partial
                ))
                || !task.state.finished()
                || (task.state == workspace::TaskState::Partial
                    && task.artifact.is_some()
                    && !crate::task_ui::task_component_failures(task).is_empty())
        })
    }

    /// Called by explicit workbench entry and initial restoration, never render.
    /// Terminal tasks remain in the task list and recent notes after their input
    /// has become a fresh form; an in-flight or edited input is never replaced.
    pub(super) fn prepare_workbench_input(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.pending_conversion.is_some() || self.following_conversion.is_some() {
            return true;
        }
        let input = self.value(Field::Source, cx);
        let has_old_result = self.workspace.as_ref().is_some_and(|workspace| {
            workspace.state.draft().is_some_and(|draft| {
                retired_input_task(draft, &workspace.state.tasks, &input).is_some()
            })
        });
        if !has_old_result {
            return true;
        }
        if !self.save_current_draft(cx) {
            return false;
        }
        let defaults = ConversionOptions::from_config(&self.preferences.defaults_config());
        let Some(workspace) = &mut self.workspace else {
            return true;
        };
        let Some(draft) = workspace.state.draft() else {
            return true;
        };
        if retired_input_task(draft, &workspace.state.tasks, &input).is_none() {
            return true;
        }
        let destination = (draft.library_id == workspace.state.default_library)
            .then(|| {
                draft
                    .folder
                    .map(|folder| (draft.library_id.clone(), folder))
            })
            .flatten();
        match workspace.transaction(|state| {
            state.reset_input(self.online, defaults, destination);
            Ok(())
        }) {
            Ok(()) => {
                self.completed_source = None;
                self.invalidate_source();
                self.restore_draft(window, cx);
                self.source_editor_open = true;
                self.generation_options_open = false;
                true
            }
            Err(error) => {
                self.workspace_error = Some(format!("新的视频输入尚未建立：{error:#}"));
                cx.notify();
                false
            }
        }
    }

    pub fn new_page(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let opening_result = self.reading && self.following_conversion.is_some();
        let linked_state = self.workbench_linked_task(cx).map(|task| task.state);
        if linked_state.is_none()
            && !self.task_options.uses_cloud_provider()
            && (self.import_uses_speech()
                || (self.generation_options_open && self.task_options.source_mode != TranscriptSource::Subtitle))
        {
            let (provider, model, root) = self.import_model_request();
            self.ensure_model_diagnostic(provider, Some(&model), &root, cx);
        }
        let cancelled_notice = self
            .current_input_task(cx)
            .filter(|task| task.state == workspace::TaskState::Cancelled)
            .map(|task| (task.id.clone(), "任务已取消".to_owned()));
        let compact = is_compact(window);
        let input = v_flex().w_full().min_w_0().gap_4()
            .when(!self.online, |input| input.child(self.source_kind_tabs(cx)))
            .child(self.box_source_input(window, cx));
        let mut view = v_flex()
            .gap(rems(24. / 14.))
            .w_full().min_w_0().max_w(rems(WORKBENCH_WIDTH / 14.)).mx_auto()
            .child(v_flex().w_full().items_center().gap(rems(8. / 14.))
                .child(accessible_text("workbench-title", "把视频整理成笔记")
                    .role(Role::Heading).text_size(rems(if compact { 28. / 14. } else { 36. / 14. }))
                    .line_height(rems(if compact { 36. / 14. } else { 44. / 14. })).font_weight(FontWeight::SEMIBOLD))
                .child(div().text_center().text_size(rems(if compact { 14. / 14. } else { 16. / 14. }))
                    .line_height(rems(24. / 14.)).text_color(color(GRAY))
                    .child("粘贴链接或拖入本地视频，自动生成带截图的笔记")))
            .child(input);
        if let Some((id, message)) = cancelled_notice {
            view = view.child(info_callout(
                SharedString::from(format!("cancelled-input-task-{id}")),
                message,
            ));
        }
        let has_linked_task = linked_state.is_some();
        // Queued and running tasks are shown by the active task list below.
        let settled_task = linked_state
            .filter(|state| !matches!(state,
                workspace::TaskState::Queued | workspace::TaskState::Running | workspace::TaskState::Pausing))
            .and_then(|_| self.workbench_linked_task(cx).cloned());
        if let Some(task) = settled_task {
            let task_view = match task.state {
                workspace::TaskState::Complete | workspace::TaskState::Partial
                    if opening_result =>
                {
                    self.box_task_opening_result(&task, cx)
                }
                _ => self.box_task_attention(&task, cx),
            };
            view = view.child(
                // The current task is the workbench's hero object, as on the
                // canvas's progress board: large radius and raised surface.
                task_view
                    .p(rems(28. / 14.))
                    .rounded(RADIUS_HERO)
                    .bg(color(SURFACE))
                    .border_1()
                    .border_color(color(CARD_LINE))
                    .shadow(shadow_hero()),
            );
        } else if !has_linked_task {
            let text_required = self.subtitle_attention_required();
            if self.source_preview.is_some() {
                let mut source = v_flex().w_full().min_w_0().gap_4();
                if text_required {
                    source = source.child(
                        box_section(icons::subtitles(), "文字来源")
                            .child(self.text_source_view(cx)),
                    );
                }
                source = source.child(self.conversion_recovery(cx));
                view = view.child(source);
            } else if text_required {
                view = view.child(
                    box_section(icons::subtitles(), "文字来源")
                        .child(self.text_source_view(cx)),
                );
            }
        }
        let scale = f32::from(window.rem_size()) / 14.;
        let column = crate::views::shell_content_width(Page::New, window);
        let wide = column >= 960. * scale;
        let show_options = self.generation_options_open
            && self.preview_cancel.is_none() && self.source_candidates.is_empty()
            && self.current_input_task(cx).is_none_or(|task| task.state.finished());
        let active = self.workbench_active_tasks_section(window, cx);
        let recent = self.recent_notes_section(1, cx);
        let options = if show_options {
            Some(v_flex().w_full().min_w_0().p(rems(24. / 14.))
                .gap(rems(24. / 14.)).rounded(RADIUS_HERO).bg(color(SURFACE))
                .child(v_flex().w_full().gap(rems(4. / 14.))
                    .child(h_flex().w_full().items_center()
                        .child(div().flex_1().text_size(rems(24. / 14.))
                            .line_height(rems(32. / 14.)).font_weight(FontWeight::MEDIUM).child("处理选项"))
                        .child(quiet("close-generation-options").icon(crate::icons::close())
                            .w(CONTROL_HEIGHT).px_0().accessibility_label("收起处理选项")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.generation_options_open = false; cx.notify();
                            }))))
                    .child(aux_text("仅对本次生效")))
                .child(self.import_text_mode_options(window, cx))
                .child(div().h(px(1.)).bg(color(HAIRLINE)))
                .child(self.import_ai_options(cx))
                .child(div().h(px(1.)).bg(color(HAIRLINE)))
                .child(self.import_destination(cx))
                .child(self.import_exports(cx))
                .child(h_flex().w_full().justify_end().pt(rems(8. / 14.))
                    .child(quiet("reset-task-overrides").icon(icons::refresh()).label("恢复 AI 默认")
                        .on_click(cx.listener(|this, _, _, cx| this.reset_task_ai_overrides(cx))))))
        } else { None };
        let mut lower = div().flex().w_full().min_w_0().gap(rems(24. / 14.)).items_start();
        if wide {
            if let Some(recent) = recent {
                lower = lower.child(div().flex_1().min_w_0().child(recent));
            }
            if let Some(options) = options {
                lower = lower.child(div().w(rems(if column < 1100. * scale { 360. / 14. } else { 400. / 14. })).flex_shrink_0().child(options));
            }
        } else {
            lower = lower.flex_col();
            if let Some(options) = options { lower = lower.child(options); }
            if let Some(recent) = recent { lower = lower.child(recent); }
        }
        let mut view = v_flex().w_full().min_w_0().pt(rems(if compact { 40. / 14. } else { 56. / 14. }))
            .gap(rems(if compact { 38. / 14. } else { 46. / 14. })).child(view)
            .when_some(active, |view, active| view.child(active)).child(lower);
        if let Some(error) = &self.workspace_error {
            view = view.child(motion::enter(
                text_id("workspace-error", error),
                v_flex()
                    .p_4()
                    .rounded(RADIUS_CARD)
                    .bg(color(DANGER_BG))
                    .child(issue(error.clone())),
            ));
        }
        // The whole workbench accepts a dropped video, not just the local tab.
        view.on_drop(
            cx.listener(|this, paths: &gpui::ExternalPaths, window, cx| {
                this.drop_source_files(paths.paths(), window, cx)
            }),
        )
        .into_any_element()
    }

    /// Attach last to the full-window `workbench-drop` group, outside scrolling content.
    pub(super) fn workbench_drop_overlay(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        div().id("workbench-drop-overlay").absolute().inset_0()
            .invisible()
            .group_drag_over::<gpui::ExternalPaths>("workbench-drop", |style| style.visible())
            .bg(color(CANVAS).opacity(0.88)).p(rems(24. / 14.))
            .child(v_flex().size_full().items_center().justify_center().gap_4()
                .rounded(RADIUS_HERO).border_2().border_dashed().border_color(color(ACCENT))
                .child(h_flex().size(rems(88. / 14.)).rounded_full()
                    .items_center().justify_center().bg(color(SECONDARY_CONTAINER))
                    .child(icons::file_upload().size(rems(40. / 14.)).text_color(color(ACCENT))))
                .child(div().text_size(rems(28. / 14.)).font_weight(FontWeight::SEMIBOLD).child("松开以添加视频")))
            .on_drop(cx.listener(|this, paths: &gpui::ExternalPaths, window, cx| {
                this.drop_source_files(paths.paths(), window, cx);
            }))
    }

    /// Only unresolved choices interrupt a submitted conversion.
    fn conversion_recovery(&mut self, cx: &mut Context<Self>) -> Div {
        let busy = self.job.is_some()
            || self.workspace.as_ref().is_some_and(|workspace| {
                workspace.state.tasks.iter().any(|task| {
                    matches!(
                        task.state,
                        crate::workspace::TaskState::Queued | crate::workspace::TaskState::Running
                    )
                })
            });
        let reading = self.preview_cancel.is_some() || self.subtitle_loading;
        let preferences_issue = self.ordinary_preferences_submit_issue();
        let preferences_blocked = preferences_issue.is_some();
        let submission_error = preferences_issue
            .as_ref()
            .map(|issue| issue.message.clone())
            .or_else(|| {
                if self.source_preview.is_some() && !reading && !self.subtitle_attention_required()
                {
                    self.submission_issue()
                } else {
                    None
                }
            });
        let checking_components = !preferences_blocked
            && self.environment.is_none()
            && submission_error.is_some();
        let mut inset = v_flex().w_full().min_w_0().gap_3();
        let mut actions = h_flex().gap_2().flex_wrap();
        if self.environment.as_ref().is_some_and(|environment| {
            !environment.engine
                || !environment.ffmpeg
                || !environment.ffprobe
                || (self.online && !environment.ytdlp)
        }) {
            actions = actions.child(
                outline_pill("repair-conversion-components")
                    .icon(icons::settings())
                    .label("检查所需组件")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_settings(window, cx);
                        this.select_settings_tab(crate::settings_ui::SettingsTab::Application, window, cx);
                    })),
            );
        }
        if let Some(issue) = preferences_issue {
            actions = actions.child(
                outline_pill("repair-generation-preferences")
                    .icon(icons::refresh())
                    .label(if issue.can_retry {
                        "重试保存生成选项"
                    } else {
                        "重置处理选项"
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if !this.save_current_draft(cx) {
                            return;
                        }
                        let saved = if issue.can_retry {
                            this.retry_ordinary_preferences(issue.group, cx)
                        } else {
                            this.restore_ordinary_preferences(issue.group, window, cx)
                        };
                        if saved {
                            let focus = this.import_submit_focus.clone();
                            window.defer(cx, move |window, cx| focus.focus(window, cx));
                        }
                    })),
            );
        }
        if let Some(library) = self
            .workspace
            .as_ref()
            .and_then(|workspace| {
                workspace
                    .state
                    .draft()
                    .and_then(|draft| workspace.state.library(&draft.library_id))
            })
            .filter(|library| {
                self.cached_location_check(library)
                    .is_some_and(|check| check.needs_reassociation)
            })
        {
            let id = library.id.clone();
            actions = actions.child(
                outline_pill("reassociate-import-location")
                    .icon(crate::icons::folder_open())
                    .label("重新关联此保存位置")
                    .disabled(self.storage_ui.busy)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.begin_library_reassociation(id.clone(), window, cx)
                    })),
            );
        }
        if let Some(task) = self.matching_current_task().cloned() {
            let id = task.id.clone();
            let location = self
                .task_library(&task)
                .map(|library| library.name.clone())
                .unwrap_or_else(|| "原保存位置".into());
            inset = inset.child(conversion_fact(
                format!("已有相同处理任务：《{}》（{location}）", task.plan.title),
                true,
            ));
            actions = actions.child(
                primary_pill("show-matching-task")
                    .track_focus(&self.import_submit_focus)
                    .self_start()
                    .icon(icons::task())
                    .label("查看任务")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.save_current_draft(cx);
                        this.select_task(&id, cx);
                        this.navigate(Page::Task, cx);
                        cx.notify();
                    })),
            );
        } else if let Some(course) = self.existing_source_note() {
            let location = self
                .course_location(&course)
                .map(|library| library.name.clone())
                .unwrap_or_else(|| "笔记库".into());
            inset = inset.child(info_callout(
                "existing-note-notice",
                format!("已有笔记，保存在「{location}」"),
            ));
            actions = actions.child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(
                        primary_pill("open-existing-note")
                            .icon(crate::icons::book_open())
                            .label("打开已有笔记")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.save_current_draft(cx);
                                this.open_course(course.clone(), cx);
                            })),
                    )
                    .child(
                        outline_pill("generate-new-version")
                            .icon(icons::refresh())
                            .track_focus(&self.import_submit_focus)
                            .label("生成新版笔记")
                            .disabled(reading || self.workspace.is_none() || preferences_blocked)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.following_conversion =
                                    Some(ConversionFollow::Preparing(this.preview_generation));
                                this.enqueue_current(window, cx);
                            })),
                    ),
            );
            if busy {
                inset = inset.child(conversion_fact("将排在当前任务之后", false));
            }
        } else if let Some(purpose) = self.missing_import_service() {
            actions = actions.child(
                primary_pill("configure-conversion-service")
                    .icon(icons::settings())
                    .label(match purpose {
                        ServicePurpose::Speech => "设置语音服务",
                        ServicePurpose::Ai => "设置 AI 服务",
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_task_service_editor(purpose, window, cx);
                    })),
            );
        }
        if self.pending_conversion.is_some()
            && self.source_validation.is_some()
            && !reading
            && self.missing_import_service().is_none()
            && self.existing_source_note().is_none()
            && self.matching_current_task().is_none()
        {
            actions = actions.child(
                outline_pill("retry-conversion-validation")
                    .icon(icons::refresh())
                    .label("重试转换")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.start_conversion(window, cx);
                    })),
            );
        }
        if self.pending_conversion.is_some() && !reading {
            actions = actions.child(
                quiet("cancel-pending-conversion")
                    .icon(icons::close())
                    .label("取消转换")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.pending_conversion = None;
                        this.following_conversion = None;
                        cx.notify();
                    })),
            );
        }
        inset
            .when_some(submission_error, |view, message| {
                if checking_components {
                    view.child(
                        h_flex()
                            .min_w_0()
                            .gap_2()
                            .items_center()
                            .child(motion::spinner("import-component-check-spinner", cx))
                            .child(
                                accessible_text("import-component-check", message)
                                    .role(Role::Status)
                                    .text_sm()
                                    .font_weight(FontWeight::NORMAL)
                                    .whitespace_normal()
                                    .text_color(color(GRAY)),
                            ),
                    )
                } else {
                    view.child(
                        accessible_text("import-submit-error", message)
                            .role(Role::Alert)
                            .text_sm()
                            .whitespace_normal()
                            .text_color(color(DANGER)),
                    )
                }
            })
            .child(actions)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ConversionFollow, ConversionGate, automatic_subtitle_fallback, completed_input_task,
        conversion_gate, submitted_input_task, subtitle_needs_confirmation, uses_speech,
    };
    use course2md::config::TranscriptSource;
    use crate::{ConversionOptions, source, workspace};
    use course2md::subtitle::{CachedSubtitle, SubtitleEvidence, SubtitleReadError};

    #[test]
    fn preview_completion_without_confirmation_never_submits() {
        for reading in [true, false] {
            for environment_ready in [true, false] {
                for library_ready in [true, false] {
                    assert_eq!(conversion_gate(None, 7, reading, true, false,
                        environment_ready, library_ready, false), ConversionGate::Stop);
                }
            }
        }
    }

    #[test]
    fn confirmed_start_waits_for_remaining_prerequisites() {
        // The same Start revision survives each independently completing prerequisite.
        let start = Some(7);
        for (reading, source, text_choice, environment) in [
            (true, false, false, false),
            (true, true, true, true),
            (false, true, false, false),
            (false, true, true, true),
        ] {
            assert_eq!(
                conversion_gate(
                    start,
                    7,
                    reading,
                    source,
                    text_choice,
                    environment,
                    true,
                    false
                ),
                ConversionGate::Wait,
            );
        }
        assert_eq!(
            conversion_gate(start, 7, false, true, false, true, true, false),
            ConversionGate::Submit
        );
        // Source replacement and duplicate content require a fresh intent/choice.
        assert_eq!(
            conversion_gate(start, 8, true, false, false, true, true, false),
            ConversionGate::Stop
        );
        assert_eq!(
            conversion_gate(start, 7, false, true, false, true, true, true),
            ConversionGate::Stop
        );
        assert_eq!(
            conversion_gate(start, 7, false, false, false, true, true, false),
            ConversionGate::Stop
        );
    }

    #[test]
    fn first_start_waits_for_library_scan_before_existing_note_decision() {
        // A restored source and the environment can be ready before the async
        // library scan replaces its initially empty (or stale) note list.
        let start = Some(7);
        for cached_note in [false, true] {
            assert_eq!(
                conversion_gate(start, 7, false, true, false, true, false, cached_note),
                ConversionGate::Wait,
            );
        }
        // The original Start is still pending when the scan reports a result.
        assert_eq!(
            conversion_gate(start, 7, false, true, false, true, true, true),
            ConversionGate::Stop,
        );
        assert_eq!(
            conversion_gate(start, 7, false, true, false, true, true, false),
            ConversionGate::Submit,
        );
        assert_eq!(
            conversion_gate(start, 8, false, true, false, true, false, false),
            ConversionGate::Stop,
        );
    }

    #[test]
    fn explicit_new_version_follows_new_task_instead_of_previous_result() {
        let (mut draft, mut tasks) = submitted_recovery_chain();
        tasks[0].state = workspace::TaskState::Complete;
        tasks[0].artifact = Some("versions/previous".into());
        tasks[0].handled_by = None;
        tasks[0].error = None;
        tasks[1].id = "new-version".into();
        tasks[1].parent = None;
        tasks[1].plan.operation = Default::default();
        draft.submitted_task = Some(tasks[1].id.clone());
        let following = ConversionFollow::Preparing(7)
            .submitted(7, tasks[1].id.clone())
            .unwrap();
        // The old version remains readable but is not the explicit new intent.
        assert!(
            following
                .completed_task(true, 7, &draft, &tasks, &draft.input)
                .is_none()
        );
        tasks[1].state = workspace::TaskState::Complete;
        tasks[1].artifact = Some("versions/new-version".into());
        assert_eq!(
            following
                .completed_task(true, 7, &draft, &tasks, &draft.input)
                .unwrap()
                .id,
            "new-version",
        );
    }

    #[test]
    fn completion_opens_only_the_result_still_followed_in_the_current_input() {
        let (draft, mut tasks) = submitted_recovery_chain();
        let followed = ConversionFollow::Preparing(7)
            .submitted(7, "recovery".into())
            .unwrap();
        assert!(
            ConversionFollow::Preparing(7)
                .submitted(8, "recovery".into())
                .is_none()
        );
        assert!(
            followed
                .completed_task(true, 7, &draft, &tasks, &draft.input)
                .is_none()
        );
        tasks[1].state = workspace::TaskState::Complete;
        assert!(
            followed
                .completed_task(true, 7, &draft, &tasks, &draft.input)
                .is_none()
        );
        tasks[1].artifact = Some("versions/recovery".into());
        assert_eq!(
            followed
                .completed_task(true, 7, &draft, &tasks, &draft.input)
                .unwrap()
                .id,
            "recovery"
        );
        assert!(
            followed
                .completed_task(false, 7, &draft, &tasks, &draft.input)
                .is_none()
        );
        assert!(
            followed
                .completed_task(true, 8, &draft, &tasks, &draft.input)
                .is_none()
        );
        assert!(
            followed
                .completed_task(true, 7, &draft, &tasks, "new video")
                .is_none()
        );
        let other = ConversionFollow::Task {
            id: "original".into(),
            source_revision: 7,
        };
        assert!(
            other
                .completed_task(true, 7, &draft, &tasks, &draft.input)
                .is_none()
        );
        tasks[1].state = workspace::TaskState::Partial;
        assert!(
            followed
                .completed_task(true, 7, &draft, &tasks, &draft.input)
                .is_some()
        );
    }

    #[test]
    fn reader_repair_opens_the_new_version_only_while_the_original_note_is_still_open() {
        let (_, mut tasks) = submitted_recovery_chain();
        let original = std::path::Path::new("versions/original");
        let follow = ConversionFollow::reader_reprocess(&tasks[1], original, 12).unwrap();
        // Submitting a recovery is not enough: it must publish a readable result.
        assert!(!follow.completed_reader_task(&tasks[1], super::Page::Result, 12, Some(original)));
        tasks[1].artifact = Some("versions/recovery".into());
        for state in [
            workspace::TaskState::Complete,
            workspace::TaskState::Partial,
        ] {
            tasks[1].state = state;
            assert!(follow.completed_reader_task(
                &tasks[1],
                super::Page::Result,
                12,
                Some(original)
            ));
        }
        for page in [
            super::Page::New,
            super::Page::Library,
            super::Page::Task,
            super::Page::Settings,
        ] {
            assert!(!follow.completed_reader_task(&tasks[1], page, 12, Some(original)));
        }
        // Leaving and returning to the same note has a new read generation.
        assert!(!follow.completed_reader_task(&tasks[1], super::Page::Result, 13, Some(original)));
        assert!(!follow.completed_reader_task(
            &tasks[1],
            super::Page::Result,
            12,
            Some(std::path::Path::new("versions/another-note")),
        ));
        assert!(!follow.completed_reader_task(&tasks[1], super::Page::Result, 12, None));
        let mut unrelated = tasks[1].clone();
        unrelated.id = "another-repair".into();
        assert!(!follow.completed_reader_task(&unrelated, super::Page::Result, 12, Some(original)));
        for state in [
            workspace::TaskState::Running,
            workspace::TaskState::Paused,
            workspace::TaskState::Uncertain,
        ] {
            tasks[1].state = state;
            assert!(!follow.completed_reader_task(
                &tasks[1],
                super::Page::Result,
                12,
                Some(original)
            ));
        }
    }

    #[test]
    fn reader_repair_guard_excludes_exports_and_rechecks_the_note_when_loading_finishes() {
        let (_, mut tasks) = submitted_recovery_chain();
        let original = std::path::Path::new("versions/original");
        let follow = ConversionFollow::reader_reprocess(&tasks[1], original, 12).unwrap();
        assert!(follow.context_is_current(super::Page::Result, 99, 12, Some(original)));
        assert!(!follow.context_is_current(super::Page::Result, 99, 13, Some(original)));
        assert!(!follow.context_is_current(super::Page::Library, 99, 12, Some(original)));
        assert!(!follow.context_is_current(
            super::Page::Result,
            99,
            12,
            Some(std::path::Path::new("versions/other")),
        ));

        let course2md::execution::Operation::Reprocess { components, .. } =
            &mut tasks[1].plan.operation
        else {
            unreachable!();
        };
        *components = vec!["exports".into()];
        tasks[1].state = workspace::TaskState::Complete;
        tasks[1].artifact = Some(original.to_owned());
        assert!(ConversionFollow::reader_reprocess(&tasks[1], original, 12).is_none());
        assert!(!follow.completed_reader_task(&tasks[1], super::Page::Result, 12, Some(original)));
        // Even a spurious new artifact cannot turn an export-only task into navigation.
        tasks[1].artifact = Some("versions/recovery".into());
        assert!(!follow.completed_reader_task(&tasks[1], super::Page::Result, 12, Some(original)));
        tasks[1].plan.operation = Default::default();
        assert!(ConversionFollow::reader_reprocess(&tasks[1], original, 12).is_none());
    }

    #[test]
    fn opening_a_reader_viewer_ends_repair_following_even_if_it_closes_before_the_read_finishes() {
        let (_, mut tasks) = submitted_recovery_chain();
        let original = std::path::Path::new("versions/original");
        let captured = ConversionFollow::reader_reprocess(&tasks[1], original, 12).unwrap();
        tasks[1].state = workspace::TaskState::Complete;
        tasks[1].artifact = Some("versions/recovery".into());
        for read_started in [false, true] {
            let mut active = Some(captured.clone());
            if read_started {
                assert!(active.as_ref().unwrap().completed_reader_task(
                    &tasks[1],
                    super::Page::Result,
                    12,
                    Some(original),
                ));
            }
            ConversionFollow::interrupt_for_reader_viewer(&mut active);
            assert!(active.is_none());
            // Closing the viewer only restores its underlying note. The old
            // asynchronous callback still has a ticket, but no longer owns intent.
            assert_ne!(active.as_ref(), Some(&captured));
            assert!(
                !active
                    .as_ref()
                    .is_some_and(|follow| follow.completed_reader_task(
                        &tasks[1],
                        super::Page::Result,
                        12,
                        Some(original),
                    ))
            );
        }
        // A viewer must not cancel a distinct workbench conversion.
        let mut input = Some(ConversionFollow::Task {
            id: "conversion".into(),
            source_revision: 7,
        });
        let expected = input.clone();
        ConversionFollow::interrupt_for_reader_viewer(&mut input);
        assert_eq!(input, expected);
    }

    #[test]
    fn completion_belongs_to_the_submitted_input_and_requires_a_readable_result() {
        let root = tempfile::tempdir().unwrap();
        let mut workspace = workspace::Workspace::open_at(
            root.path().join("workspace.json"),
            root.path().join("library"),
            Default::default(),
        )
        .unwrap();
        let state = &mut workspace.state;
        let source = source::Source {
            input: "https://www.bilibili.com/video/BVfixture".into(),
            identity: "video-fixture".into(),
            online: true,
            ..Default::default()
        };
        let plan = workspace::TaskPlan {
            operation: Default::default(),
            source: source.clone(),
            source_id: source.identity.clone(),
            title: "已提交的标题".into(),
            library_id: state.default_library.clone(),
            folder: None,
            options: Default::default(),
            subtitle: None,
            config: Default::default(),
            asr_service: None,
            ai_service: None,
        };
        let (id, _) = state.enqueue(plan.clone(), None).unwrap();
        let draft = state.draft_mut().unwrap();
        draft.change_source(source.input.clone());
        draft.submitted_task = Some(id.clone());
        assert!(
            completed_input_task(state.draft().unwrap(), &state.tasks, &source.input).is_none()
        );
        state.task_mut(&id).unwrap().state = workspace::TaskState::Complete;
        assert!(
            completed_input_task(state.draft().unwrap(), &state.tasks, &source.input).is_none()
        );
        state.task_mut(&id).unwrap().artifact = Some(root.path().join("published-version"));
        assert_eq!(
            completed_input_task(state.draft().unwrap(), &state.tasks, &source.input)
                .unwrap()
                .id,
            id,
        );
        // The text can change before its observer replaces the submitted draft.
        // That next input must remain visible even during this intermediate state.
        for next in ["", "https://www.bilibili.com/video/BVnext"] {
            assert!(completed_input_task(state.draft().unwrap(), &state.tasks, next).is_none());
        }
        state.task_mut(&id).unwrap().state = workspace::TaskState::Partial;
        assert!(
            completed_input_task(state.draft().unwrap(), &state.tasks, &source.input).is_some()
        );
        state.task_mut(&id).unwrap().state = workspace::TaskState::Complete;
        state.prepare_next_import("https://www.bilibili.com/video/BVnext", Default::default());
        assert!(
            completed_input_task(state.draft().unwrap(), &state.tasks, &source.input).is_none()
        );
        assert!(state.task(&id).unwrap().plan == plan);
    }

    fn submitted_recovery_chain() -> (workspace::Draft, Vec<workspace::TaskRecord>) {
        let source = source::Source {
            input: "https://www.bilibili.com/video/BVfixture".into(),
            identity: "video-fixture".into(),
            online: true,
            ..Default::default()
        };
        let mut draft = workspace::Draft::new(true, "library".into(), Default::default());
        draft.change_source(source.input.clone());
        draft.source = Some(source.clone());
        draft.submitted_task = Some("original".into());
        let mut original = workspace::TaskRecord {
            id: "original".into(),
            plan: workspace::TaskPlan {
                operation: Default::default(),
                source_id: source.identity.clone(),
                source,
                title: "已提交的课程".into(),
                library_id: "library".into(),
                folder: None,
                options: Default::default(),
                subtitle: None,
                config: Default::default(),
                asr_service: None,
                ai_service: None,
            },
            state: workspace::TaskState::Uncertain,
            intent: workspace::Intent::Pause,
            created: 0,
            updated: 0,
            parent: None,
            handled_by: None,
            work_dir: "work/original".into(),
            stages: Default::default(),
            error: Some("原摘要请求结果未确认".into()),
            artifact: Some("versions/original".into()),
            outcomes: serde_json::Value::Null,
            unread: false,
            logs: Vec::new(),
            blocked: Vec::new(),
            resend: Vec::new(),
        };
        let mut recovery = original.clone();
        recovery.id = "recovery".into();
        recovery.parent = Some(original.id.clone());
        recovery.state = workspace::TaskState::Queued;
        recovery.intent = workspace::Intent::Run;
        recovery.error = None;
        recovery.artifact = None;
        recovery.work_dir = "work/recovery".into();
        recovery.plan.operation = course2md::execution::Operation::Reprocess {
            base_version_dir: "versions/original".into(),
            components: vec!["summary".into()],
            prior_work_dir: Some(original.work_dir.clone()),
        };
        original.handled_by = Some(recovery.id.clone());
        (draft, vec![original, recovery])
    }

    #[test]
    fn workbench_entry_retires_only_unchanged_terminal_inputs() {
        let (mut draft, mut tasks) = submitted_recovery_chain();
        draft.title = tasks[0].plan.title.clone();
        for state in [
            workspace::TaskState::Complete,
            workspace::TaskState::Partial,
            workspace::TaskState::Cancelled,
        ] {
            tasks[1].state = state;
            tasks[1].artifact = Some("versions/recovery".into());
            assert_eq!(
                super::retired_input_task(&draft, &tasks, &draft.input)
                    .unwrap()
                    .id,
                "recovery"
            );
        }
        for state in [
            workspace::TaskState::Queued,
            workspace::TaskState::Running,
            workspace::TaskState::Pausing,
            workspace::TaskState::Paused,
            workspace::TaskState::NeedsAttention,
            workspace::TaskState::Uncertain,
        ] {
            tasks[1].state = state;
            assert!(super::retired_input_task(&draft, &tasks, &draft.input).is_none());
        }
        tasks[1].state = workspace::TaskState::Partial;
        let unchanged = draft.clone();
        draft.title = "尚未提交的新名称".into();
        assert!(super::retired_input_task(&draft, &tasks, &draft.input).is_none());
        draft = unchanged.clone();
        draft.options.vision = !draft.options.vision;
        assert!(super::retired_input_task(&draft, &tasks, &draft.input).is_none());
        draft = unchanged.clone();
        draft.folder = Some(42);
        assert!(super::retired_input_task(&draft, &tasks, &draft.input).is_none());
        draft = unchanged;
        assert!(super::retired_input_task(&draft, &tasks, "新视频").is_none());
        tasks[1].blocked.push(workspace::BlockedRequest {
            reason: course2md::dispatch::REASON_UNCERTAIN.into(),
            request_id: Some("pending-summary".into()),
            purpose: Some("summary".into()),
            description: "摘要".into(),
            message: "结果未知".into(),
        });
        assert!(super::retired_input_task(&draft, &tasks, &draft.input).is_none());
        tasks[1].blocked.clear();
        tasks[1].artifact = None;
        assert!(super::retired_input_task(&draft, &tasks, &draft.input).is_none());
    }

    #[test]
    fn workbench_follows_the_same_recovery_for_queued_running_and_completed_results() {
        let (draft, mut tasks) = submitted_recovery_chain();
        let original = tasks[0].clone();
        for state in [workspace::TaskState::Queued, workspace::TaskState::Running] {
            tasks[1].state = state;
            assert_eq!(
                submitted_input_task(&draft, &tasks, &draft.input)
                    .unwrap()
                    .id,
                "recovery"
            );
            assert!(completed_input_task(&draft, &tasks, &draft.input).is_none());
        }
        tasks[1].state = workspace::TaskState::Complete;
        assert!(completed_input_task(&draft, &tasks, &draft.input).is_none());
        tasks[1].artifact = Some("versions/recovery".into());
        assert_eq!(
            completed_input_task(&draft, &tasks, &draft.input)
                .unwrap()
                .id,
            "recovery"
        );

        let mut final_attempt = tasks[1].clone();
        final_attempt.id = "final-attempt".into();
        final_attempt.parent = Some("recovery".into());
        final_attempt.state = workspace::TaskState::Running;
        final_attempt.artifact = None;
        tasks[1].state = workspace::TaskState::Uncertain;
        tasks[1].handled_by = Some(final_attempt.id.clone());
        tasks.push(final_attempt);
        assert_eq!(
            submitted_input_task(&draft, &tasks, &draft.input)
                .unwrap()
                .id,
            "final-attempt"
        );
        assert!(completed_input_task(&draft, &tasks, &draft.input).is_none());
        tasks[2].state = workspace::TaskState::Complete;
        tasks[2].artifact = Some("versions/final-attempt".into());
        assert_eq!(
            completed_input_task(&draft, &tasks, &draft.input)
                .unwrap()
                .id,
            "final-attempt"
        );
        // A cancelled recovery still belongs to this input for inline feedback,
        // but must not fall back to an older readable result or follow new input.
        tasks[2].state = workspace::TaskState::Cancelled;
        tasks[2].artifact = None;
        assert_eq!(
            submitted_input_task(&draft, &tasks, &draft.input)
                .unwrap()
                .id,
            "final-attempt"
        );
        assert!(completed_input_task(&draft, &tasks, &draft.input).is_none());
        assert!(submitted_input_task(&draft, &tasks, "another-video").is_none());
        assert!(tasks[0] == original);
        assert_eq!(draft.submitted_task.as_deref(), Some("original"));
    }

    #[test]
    fn recovery_lookup_rejects_broken_cycles_or_unrelated_task_links() {
        for case in [
            "missing",
            "self-cycle",
            "cycle",
            "wrong-parent",
            "other-source",
            "inconsistent-source",
            "other-kind",
            "duplicate-id",
            "unrelated-submission",
        ] {
            let (mut draft, mut tasks) = submitted_recovery_chain();
            match case {
                "missing" => tasks[0].handled_by = Some("missing".into()),
                "self-cycle" => {
                    tasks[0].handled_by = Some("original".into());
                    tasks[0].parent = Some("original".into());
                }
                "cycle" => {
                    tasks[1].handled_by = Some("original".into());
                    tasks[0].parent = Some("recovery".into());
                }
                "wrong-parent" => tasks[1].parent = Some("unrelated".into()),
                "other-source" => {
                    tasks[1].plan.source_id = "other-video".into();
                    tasks[1].plan.source.identity = "other-video".into();
                }
                "inconsistent-source" => tasks[1].plan.source.identity = "other-video".into(),
                "other-kind" => tasks[1].plan.source.online = false,
                "duplicate-id" => tasks.push(tasks[1].clone()),
                "unrelated-submission" => {
                    tasks[1].plan.source_id = "other-video".into();
                    tasks[1].plan.source.identity = "other-video".into();
                    draft.submitted_task = Some("recovery".into());
                }
                _ => unreachable!(),
            }
            assert!(
                submitted_input_task(&draft, &tasks, &draft.input).is_none(),
                "{case}"
            );
            assert!(
                completed_input_task(&draft, &tasks, &draft.input).is_none(),
                "{case}"
            );
        }
    }

    #[test]
    fn a_recovered_result_cannot_replace_new_or_changed_input() {
        let (mut draft, mut tasks) = submitted_recovery_chain();
        tasks[1].state = workspace::TaskState::Complete;
        tasks[1].artifact = Some("versions/recovery".into());
        for input in ["", "https://www.bilibili.com/video/BVnext"] {
            assert!(submitted_input_task(&draft, &tasks, input).is_none());
            assert!(completed_input_task(&draft, &tasks, input).is_none());
        }
        draft.source.as_mut().unwrap().identity = "newly-inspected-content".into();
        assert!(submitted_input_task(&draft, &tasks, &draft.input).is_none());
        draft.source = None;
        draft.input = "https://www.bilibili.com/video/BVnext".into();
        assert!(submitted_input_task(&draft, &tasks, &draft.input).is_none());
    }

    #[test]
    fn automatic_mode_uses_speech_after_login_or_download_failure() {
        let track = course2md::subtitle::file_track("lesson.srt".into(), None);
        for metadata_failed in [true, false] {
            let mut source = source::Source {
                subtitles: if metadata_failed {
                    SubtitleEvidence::Failed {
                        message: "WARNING: subtitles require login".into(),
                    }
                } else {
                    SubtitleEvidence::Found {
                        tracks: vec![track.clone()],
                        warning: None,
                    }
                },
                subtitle_request: (!metadata_failed).then(|| track.clone()),
                subtitle_read_error: (!metadata_failed).then(|| SubtitleReadError::Failed {
                    message: "字幕文件没有下载成功".into(),
                }),
                ..Default::default()
            };
            assert!(automatic_subtitle_fallback(&mut source, TranscriptSource::Auto, None));
            assert!(uses_speech(&source, TranscriptSource::Auto));
            assert!(!subtitle_needs_confirmation(&source, TranscriptSource::Auto));
            assert!(source.subtitle_request.is_none());
            assert!(source.subtitle_read_error.is_none());
        }
    }

    #[test]
    fn explicit_subtitle_and_cancelled_reads_never_fall_back_to_speech() {
        let mut source = source::Source {
            subtitles: SubtitleEvidence::Failed {
                message: "字幕需要登录".into(),
            },
            subtitle_read_error: Some(SubtitleReadError::Failed {
                message: "字幕需要登录".into(),
            }),
            ..Default::default()
        };
        let original = source.clone();
        assert!(!automatic_subtitle_fallback(&mut source, TranscriptSource::Subtitle, None));
        assert_eq!(source, original);
        assert!(subtitle_needs_confirmation(&source, TranscriptSource::Subtitle));

        source.subtitle_read_error = Some(SubtitleReadError::Cancelled);
        let cancelled = source.clone();
        assert!(!automatic_subtitle_fallback(&mut source, TranscriptSource::Auto, None));
        assert_eq!(source, cancelled);
        assert!(!uses_speech(&source, TranscriptSource::Auto));
    }

    #[test]
    fn pending_or_failed_subtitles_remain_required_even_with_cached_text() {
        let track = course2md::subtitle::file_track("lesson.srt".into(), None);
        let mut source = source::Source {
            identity: "lesson".into(),
            subtitles: SubtitleEvidence::Found {
                tracks: vec![track.clone()],
                warning: None,
            },
            ..Default::default()
        };
        assert!(subtitle_needs_confirmation(&source, TranscriptSource::Auto));
        source.selected_subtitle = Some(CachedSubtitle {
            source_identity: source.identity.clone(),
            track_id: track.id.clone(),
            label: "已读取字幕".into(),
            path: "lesson.srt".into(),
            events: Vec::new(),
        });
        assert!(!subtitle_needs_confirmation(&source, TranscriptSource::Auto));

        source.subtitle_request = Some(track);
        assert!(subtitle_needs_confirmation(&source, TranscriptSource::Auto));
        source.subtitle_request = None;
        source.subtitle_read_error = Some(SubtitleReadError::Failed {
            message: "无法读取新字幕，原正文仍保留".into(),
        });
        assert!(subtitle_needs_confirmation(&source, TranscriptSource::Auto));

        // Cancelling the replacement or choosing the cached text restores the
        // confirmed source without opening unrelated generation preferences.
        source.subtitle_read_error = None;
        assert!(!subtitle_needs_confirmation(&source, TranscriptSource::Subtitle));
        source.selected_subtitle = None;
        assert!(subtitle_needs_confirmation(&source, TranscriptSource::Subtitle));
        assert!(!subtitle_needs_confirmation(&source, TranscriptSource::Asr));
    }

    #[test]
    fn caption_intent_and_unconfirmed_reads_never_imply_speech_recognition() {
        let mut source = source::Source {
            subtitles: SubtitleEvidence::NoneFound,
            ..Default::default()
        };
        assert!(uses_speech(&source, TranscriptSource::Auto));
        assert!(!uses_speech(&source, TranscriptSource::Subtitle));
        source.subtitle_request = Some(course2md::subtitle::file_track("lesson.srt".into(), None));
        assert!(!uses_speech(&source, TranscriptSource::Auto));
        assert!(!uses_speech(&source, TranscriptSource::Subtitle));
        source.subtitle_request = None;
        source.subtitle_read_error = Some(SubtitleReadError::Failed {
            message: "字幕文件无法读取".into(),
        });
        assert!(!uses_speech(&source, TranscriptSource::Auto));
        assert!(uses_speech(&source, TranscriptSource::Asr));
        source.subtitle_read_error = None;
        source.subtitles = SubtitleEvidence::Unchecked;
        assert!(!uses_speech(&source, TranscriptSource::Auto));
        source.subtitles = SubtitleEvidence::Failed {
            message: "请求失败".into(),
        };
        assert!(uses_speech(&source, TranscriptSource::Auto));
        assert!(!uses_speech(&source, TranscriptSource::Subtitle));
    }
}
