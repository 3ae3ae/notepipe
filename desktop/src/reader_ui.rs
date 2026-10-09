//! Reading navigation is bound to one immutable note version, independently of exports.
use super::*;
use crate::{notes::PreviewBlock, reader_navigation as nav, theme::*};
use gpui_component::{
    button::*,
    menu::{DropdownMenu, PopupMenu, PopupMenuItem},
    scroll::{Scrollbar, ScrollbarMode},
};
use std::{
    cell::RefCell,
    collections::HashMap,
    ops::Range,
    rc::Rc,
    sync::{Arc, atomic::AtomicBool},
};

const READER_MEASURE: Rems = rems(52.);
/// Minimum spacing between polled reading-position saves during scrolling.
const READING_POSITION_INTERVAL: Duration = Duration::from_secs(5);

actions!(
    course2md_reader,
    [
        FindInNote,
        NextMatch,
        PreviousMatch,
        CloseFind,
        PreviousImage,
        NextImage,
        ZoomIn,
        ZoomOut,
        FitImage,
        CloseImage,
        NextReaderView,
        PreviousReaderView,
        ReaderPageUp,
        ReaderPageDown,
        ReaderStart,
        ReaderEnd
    ]
);

#[derive(Clone)]
struct Match {
    block: usize,
    range: Range<usize>,
}
#[derive(Clone)]
struct Frame {
    anchor: String,
    path: Option<PathBuf>,
    seconds: Option<f64>,
    caption: Option<String>,
    transcript: String,
    body_anchor: Option<String>,
    width: u32,
    height: u32,
}
#[derive(Clone)]
struct Version {
    course: Course,
    label: String,
}
#[derive(Default)]
struct ReaderData {
    frames: Vec<Frame>,
    versions: Vec<Version>,
    issues: Vec<String>,
    export_folder: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum OfflineVideo {
    #[default]
    NotRequested,
    Missing,
    Available(PathBuf),
}

#[derive(Clone)]
struct OfflineVideoRequest {
    library_root: PathBuf,
    task_id: String,
    work_dir: PathBuf,
    version_dir: PathBuf,
}

impl OfflineVideoRequest {
    fn for_version(
        course: &Course,
        task: &workspace::TaskRecord,
        library: &workspace::LibraryLocation,
    ) -> Option<Self> {
        let manifest = course.manifest.as_ref()?;
        if !task.plan.source.online
            || !task.plan.options.keep_video
            || task.id != manifest.task_id
            || task.plan.source_id != manifest.source_id
            || task.artifact.as_ref() != Some(&course.dir)
            || task.plan.library_id != library.id
        {
            return None;
        }
        Some(Self {
            library_root: library.root.clone(),
            task_id: task.id.clone(),
            work_dir: task.work_dir.clone(),
            version_dir: course.dir.clone(),
        })
    }

    /// Called only by reader workers, including the final check before opening.
    fn inspect(&self) -> OfflineVideo {
        self.checked_path()
            .map(OfflineVideo::Available)
            .unwrap_or(OfflineVideo::Missing)
    }

    fn checked_path(&self) -> Option<PathBuf> {
        let mut components = std::path::Path::new(&self.task_id).components();
        if !matches!(components.next(), Some(std::path::Component::Normal(_)))
            || components.next().is_some()
        {
            return None;
        }
        let expected_work = self
            .library_root
            .join(".course2md/work")
            .join(&self.task_id);
        if self.work_dir != expected_work {
            return None;
        }
        let root = self.library_root.canonicalize().ok()?;
        let work = self.work_dir.canonicalize().ok()?;
        if work != root.join(".course2md/work").join(&self.task_id)
            || !self.version_dir.canonicalize().ok()?.starts_with(&root)
        {
            return None;
        }
        let media = work.join("media.mp4").canonicalize().ok()?;
        let metadata = media.metadata().ok()?;
        (media.parent() == Some(work.as_path()) && metadata.is_file() && metadata.len() > 0)
            .then_some(media)
    }
}

struct ImageViewer {
    scroll: ScrollHandle,
    viewport_scroll: ScrollHandle,
    viewport_size: gpui::Size<Pixels>,
    frames: Vec<Frame>,
    filtered: bool,
    index: usize,
    title: String,
    source: Option<nav::SourceTarget>,
    source_available: bool,
    version: PathBuf,
    zoom: Option<f32>,
    details_open: Option<bool>,
    return_focus: Option<FocusHandle>,
    focus: FocusHandle,
}

#[derive(Clone)]
struct ExportFeedback {
    title: String,
    format: course2md::config::OutputFormat,
    result: Result<PathBuf, String>,
}

#[derive(Default)]
struct ExportState {
    generation: u64,
    pending: Option<(u64, PathBuf)>,
    feedback: BTreeMap<PathBuf, ExportFeedback>,
}

impl ExportState {
    fn begin(&mut self, version: PathBuf) -> Option<u64> {
        if self.pending.is_some() {
            return None;
        }
        self.generation = self.generation.wrapping_add(1);
        self.pending = Some((self.generation, version));
        Some(self.generation)
    }

    fn is_pending(&self, generation: u64, version: &std::path::Path) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|(current, source)| *current == generation && source == version)
    }

    fn cancel(&mut self, generation: u64, version: &std::path::Path) -> bool {
        if !self.is_pending(generation, version) {
            return false;
        }
        self.pending = None;
        true
    }

    fn finish(
        &mut self,
        generation: u64,
        version: &std::path::Path,
        feedback: ExportFeedback,
    ) -> bool {
        if !self.cancel(generation, version) {
            return false;
        }
        self.feedback.insert(version.to_path_buf(), feedback);
        true
    }
}

fn exported_file_label(path: &std::path::Path) -> String {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "导出文件".into());
    format!("已导出 · {name}")
}

fn reader_export_items(
    mut menu: PopupMenu,
    desktop: WeakEntity<Desktop>,
    course: Course,
    folder: Option<PathBuf>,
    exporting: bool,
) -> PopupMenu {
    if let Some(folder) = folder {
        menu = menu
            .item(
                PopupMenuItem::new("打开导出文件夹")
                    .icon(icons::folder_open())
                    .on_click(move |_, _, cx| {
                        if let Ok(url) = url::Url::from_directory_path(&folder) {
                            cx.open_url(url.as_str());
                        }
                    }),
            )
            .separator();
    }
    for (markdown, label, icon) in [
        (false, "复制整份笔记 · 纯文本", icons::content_copy()),
        (true, "复制整份笔记 · Markdown 文本", icons::article()),
    ] {
        let weak = desktop.clone();
        let version = course.dir.clone();
        menu = menu.item(
            PopupMenuItem::new(label)
                .icon(icon)
                .on_click(move |_, _, cx| {
                    let _ = weak.update(cx, |this, cx| {
                        if let Some(preview) = this
                            .preview
                            .as_ref()
                            .filter(|preview| preview.course.dir == version)
                        {
                            let text = if markdown {
                                &preview.markdown_text
                            } else {
                                &preview.plain_text
                            };
                            cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                            this.message = Some(
                                if markdown {
                                    "已复制 Markdown 文本，不含图片引用。"
                                } else {
                                    "已复制纯文本，不含图片。"
                                }
                                .into(),
                            );
                            cx.notify();
                        }
                    });
                }),
        );
    }
    menu = menu.separator();
    for (format, label) in [
        (
            course2md::config::OutputFormat::Md,
            "导出 Markdown 包（含图片）…",
        ),
        (
            course2md::config::OutputFormat::Html,
            "导出网页文件（可独立阅读）…",
        ),
        (
            course2md::config::OutputFormat::Json,
            "导出 JSON 数据（不含图片文件）…",
        ),
    ] {
        let weak = desktop.clone();
        let source = course.clone();
        menu = menu.item(
            PopupMenuItem::new(label)
                .icon(icons::download())
                .disabled(exporting)
                .on_click(move |_, window, cx| {
                    let _ = weak.update(cx, |this, cx| {
                        this.export_course(source.clone(), format, window, cx);
                    });
                }),
        );
    }
    menu
}

pub(crate) struct State {
    controls_scroll: ScrollHandle,
    information_scroll: ScrollHandle,
    toc_scroll: ScrollHandle,
    view_focus: [FocusHandle; 2],
    menu_focus: FocusHandle,
    find: Entity<InputState>,
    gallery_find: Entity<InputState>,
    gallery_search_position: Option<workspace::ReadingPosition>,
    focus: FocusHandle,
    find_open: bool,
    info_open: bool,
    processing_details_open: bool,
    // None follows the available reading width; an explicit choice survives resizing.
    toc_open: Option<bool>,
    find_return_focus: Option<FocusHandle>,
    matches: Vec<Match>,
    match_index: usize,
    clear_find: bool,
    loaded: Option<PathBuf>,
    generation: u64,
    data_loading: bool,
    frames: Rc<Vec<Frame>>,
    versions: Vec<Version>,
    issues: Vec<String>,
    pending_restore: Option<workspace::ReadingPosition>,
    pending_search: Option<Match>,
    restoring: bool,
    restore_generation: u64,
    last_position: Option<(String, workspace::ReadingPosition)>,
    layout: Option<(f32, f32, f32)>,
    item_layout: Rc<RefCell<nav::ReadingLayout>>,
    // The note tab's article flow is a variable-height `list`: persistent
    // state, the flattened item sequence it describes and per-item focus
    // containers that keep a focused control mounted while scrolled out.
    note_list: ListState,
    note_items: Rc<Vec<NoteItem>>,
    note_focus: Rc<Vec<FocusHandle>>,
    note_items_key: Option<(PathBuf, bool, usize)>,
    note_list_rem: f32,
    viewer: Option<ImageViewer>,
    exports: BTreeMap<PathBuf, PathBuf>,
    export_folder: Option<PathBuf>,
    export_state: ExportState,
    source_loading: bool,
    /// 进行中的重新定位探测取消标志：换笔记/新探测/完成时置位
    source_probe_cancel: Option<Arc<AtomicBool>>,
    source: Option<nav::SourceTarget>,
    source_available: bool,
    offline_video: OfflineVideo,
    offline_opening: bool,
    _subscriptions: Vec<Subscription>,
}
impl State {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Desktop>) -> Self {
        let find = cx.new(|cx| InputState::new(window, cx).placeholder("查找笔记内容"));
        let gallery_find = cx.new(|cx| InputState::new(window, cx).placeholder("搜索截图对应文字"));
        let subscription = cx.subscribe_in(&find, window, |this, _, event, window, cx| {
            match event {
                InputEvent::Change => {
                    this.reader_ui.find_open =
                        !this.reader_ui.find.read(cx).value().trim().is_empty();
                    this.update_reader_matches(cx);
                    if this.result_tab == 0 {
                        this.move_reader_match(0, cx);
                    }
                }
                InputEvent::PressEnter { shift, .. } => {
                    this.move_reader_match(if *shift { -1 } else { 1 }, cx)
                }
                _ => {}
            }
            let _ = window;
        });
        let gallery_subscription =
            cx.subscribe_in(&gallery_find, window, |this, _, event, _, cx| {
                if matches!(event, InputEvent::Change) && this.result_tab == 1 {
                    let searching = !this
                        .reader_ui
                        .gallery_find
                        .read(cx)
                        .value()
                        .trim()
                        .is_empty();
                    if searching {
                        if this.reader_ui.gallery_search_position.is_none() {
                            this.save_reading_position(cx);
                            this.reader_ui.gallery_search_position =
                                this.capture_reading_position();
                        }
                        this.reader_scroll.set_offset(point(px(0.), px(0.)));
                    } else if let Some(position) = this.reader_ui.gallery_search_position.take() {
                        this.reader_ui.pending_restore = Some(position);
                        this.reader_ui.restore_generation += 1;
                        this.reader_ui.restoring = false;
                    }
                    cx.notify();
                }
            });
        cx.bind_keys([
            KeyBinding::new("cmd-f", FindInNote, Some("CourseReader")),
            KeyBinding::new("cmd-g", NextMatch, Some("CourseReader")),
            KeyBinding::new("cmd-shift-g", PreviousMatch, Some("CourseReader")),
            KeyBinding::new("escape", CloseFind, Some("CourseReader")),
            KeyBinding::new("left", PreviousImage, Some("ReaderImage")),
            KeyBinding::new("right", NextImage, Some("ReaderImage")),
            KeyBinding::new("=", ZoomIn, Some("ReaderImage")),
            KeyBinding::new("+", ZoomIn, Some("ReaderImage")),
            KeyBinding::new("-", ZoomOut, Some("ReaderImage")),
            KeyBinding::new("0", FitImage, Some("ReaderImage")),
            KeyBinding::new("escape", CloseImage, Some("ReaderImage")),
            KeyBinding::new("right", NextReaderView, Some("ReaderViews")),
            KeyBinding::new("left", PreviousReaderView, Some("ReaderViews")),
            KeyBinding::new("pageup", ReaderPageUp, Some("CourseReader && !Input")),
            KeyBinding::new("pagedown", ReaderPageDown, Some("CourseReader && !Input")),
            KeyBinding::new("home", ReaderStart, Some("CourseReader && !Input")),
            KeyBinding::new("end", ReaderEnd, Some("CourseReader && !Input")),
        ]);
        Self {
            controls_scroll: ScrollHandle::new(),
            information_scroll: ScrollHandle::new(),
            toc_scroll: ScrollHandle::new(),
            view_focus: std::array::from_fn(|_| cx.focus_handle()),
            menu_focus: cx.focus_handle(),
            find,
            gallery_find,
            gallery_search_position: None,
            focus: cx.focus_handle(),
            find_open: false,
            info_open: false,
            processing_details_open: false,
            toc_open: None,
            find_return_focus: None,
            matches: Vec::new(),
            match_index: 0,
            clear_find: false,
            loaded: None,
            generation: 0,
            data_loading: false,
            frames: Rc::default(),
            versions: Vec::new(),
            issues: Vec::new(),
            pending_restore: None,
            pending_search: None,
            restoring: false,
            restore_generation: 0,
            last_position: None,
            layout: None,
            item_layout: Rc::default(),
            note_list: ListState::new(0, ListAlignment::Top, px(1000.)),
            note_items: Rc::default(),
            note_focus: Rc::default(),
            note_items_key: None,
            note_list_rem: f32::NAN,
            viewer: None,
            exports: BTreeMap::new(),
            export_folder: None,
            export_state: ExportState::default(),
            source_loading: false,
            source_probe_cancel: None,
            source: None,
            source_available: false,
            offline_video: OfflineVideo::NotRequested,
            offline_opening: false,
            _subscriptions: vec![subscription, gallery_subscription],
        }
    }
}
struct ImageDialog {
    desktop: Entity<Desktop>,
    _observation: Subscription,
    footer: bool,
}
impl Render for ImageDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.desktop.update(cx, |desktop, cx| {
            if self.footer {
                desktop.reader_image_actions(window, cx)
            } else {
                desktop.reader_image_content(window, cx)
            }
        })
    }
}
fn block_text(block: &PreviewBlock) -> Option<&str> {
    match block {
        PreviewBlock::Heading { text, .. } | PreviewBlock::Paragraph { text, .. } => Some(text),
        _ => None,
    }
}
fn block_anchor(block: &PreviewBlock, index: usize) -> String {
    match block {
        PreviewBlock::Heading { anchor, .. } | PreviewBlock::Paragraph { anchor, .. } => {
            anchor.clone()
        }
        PreviewBlock::Image(path) => format!(
            "image:{index}:{}",
            path.file_name().unwrap_or_default().to_string_lossy()
        ),
    }
}
/// Position-store key for a manifest course at one tab (`:0` is the note
/// tab). Single source of the format: the library's "继续阅读" probe and the
/// reader's save/restore must build identical keys. A stored offset below
/// the `-8.` sentinel means "past the first item" (see note_position_index).
pub(crate) fn manifest_reading_key(course_id: &str, version_id: &str, tab: usize) -> String {
    format!("{course_id}:{version_id}:{tab}")
}
fn block_time(blocks: &[PreviewBlock], index: usize) -> Option<f64> {
    for block in blocks.iter().take(index + 1).rev() {
        // An explicitly untimed heading ends the previous time's scope.
        if let PreviewBlock::Heading { seconds, .. } = block {
            return *seconds;
        }
    }
    None
}
fn capture_reader_position(
    layout: &nav::ReadingLayout,
    offset: f32,
    anchor: impl FnOnce(usize) -> (Option<String>, Option<f64>),
) -> Option<workspace::ReadingPosition> {
    // The top is a document boundary, not the first measured paragraph. Keep it
    // fixed while images/metadata arrive and when the reading measure changes.
    if offset >= 0. {
        return Some(workspace::ReadingPosition::default());
    }
    let (index, within, height) = layout.top_item(offset)?;
    let (paragraph, seconds) = anchor(index);
    Some(workspace::ReadingPosition {
        paragraph,
        seconds,
        offset,
        within,
        fraction: Some(nav::within_fraction(within, height)),
    })
}
fn heading_times(blocks: &[PreviewBlock]) -> impl Iterator<Item = (usize, Option<f64>)> + '_ {
    blocks.iter().enumerate().map(|(index, block)| {
        (
            index,
            match block {
                PreviewBlock::Heading { seconds, .. } => *seconds,
                _ => None,
            },
        )
    })
}
fn note_position_index(
    blocks: &[PreviewBlock],
    position: &workspace::ReadingPosition,
) -> Option<usize> {
    position
        .paragraph
        .as_ref()
        .and_then(|anchor| {
            blocks
                .iter()
                .enumerate()
                .position(|(index, block)| block_anchor(block, index) == *anchor)
        })
        .or_else(|| {
            position.seconds.and_then(|seconds| {
                nav::nearest_time(heading_times(blocks), seconds).map(|(index, _)| index)
            })
        })
}
fn restored_reader_offset(
    layout: &nav::ReadingLayout,
    index: Option<usize>,
    position: &workspace::ReadingPosition,
) -> f32 {
    index
        .and_then(|index| layout.restore(index, position.fraction, position.within))
        .unwrap_or(position.offset.min(0.))
}

/// The note tab's article is one flat sequence of list items: the scrolling
/// title on short windows, the summary label and its paragraphs, then every
/// remaining block in order. Items keep the block indexing of the full
/// document so anchors, search targets and reading positions are unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NoteItem {
    Title,
    SummaryLabel(Option<usize>),
    Block(usize),
}

fn note_summary_block(block: &PreviewBlock) -> bool {
    match block {
        PreviewBlock::Heading { anchor, .. } => anchor == "summary",
        PreviewBlock::Paragraph { anchor, .. } => {
            anchor == "summary-tldr" || anchor.starts_with("key-point-")
        }
        _ => false,
    }
}

fn note_items(blocks: &[PreviewBlock], short_reader: bool) -> Vec<NoteItem> {
    let mut items = Vec::with_capacity(blocks.len() + 2);
    if short_reader {
        items.push(NoteItem::Title);
    }
    let mut summary_label = None;
    let mut summary_paragraphs = Vec::new();
    for (index, block) in blocks.iter().enumerate() {
        match block {
            PreviewBlock::Heading { anchor, .. } if anchor == "summary" => {
                summary_label = Some(index);
            }
            PreviewBlock::Paragraph { anchor, .. }
                if anchor == "summary-tldr" || anchor.starts_with("key-point-") =>
            {
                summary_paragraphs.push(index);
            }
            _ => {}
        }
    }
    if !summary_paragraphs.is_empty() {
        items.push(NoteItem::SummaryLabel(summary_label));
        items.extend(summary_paragraphs.into_iter().map(NoteItem::Block));
    }
    for (index, block) in blocks.iter().enumerate() {
        if !note_summary_block(block) {
            items.push(NoteItem::Block(index));
        }
    }
    items
}

/// The document block an item carries, when it carries one.
fn note_item_block(item: NoteItem) -> Option<usize> {
    match item {
        NoteItem::Title => None,
        NoteItem::SummaryLabel(block) => block,
        NoteItem::Block(block) => Some(block),
    }
}

/// The block at or after the item, matching how the pixel-era layout lookup
/// skipped unmeasured decorations like the scrolling title.
fn note_top_block(items: &[NoteItem], item: usize) -> Option<usize> {
    items
        .get(item..)
        .unwrap_or(&[])
        .iter()
        .find_map(|item| note_item_block(*item))
        .or_else(|| {
            items
                .get(..item)
                .unwrap_or(&[])
                .iter()
                .rev()
                .find_map(|item| note_item_block(*item))
        })
}

fn note_block_item(items: &[NoteItem], block: usize) -> Option<usize> {
    items
        .iter()
        .position(|item| note_item_block(*item) == Some(block))
}

/// The note tab's reading position from the list's logical scroll top: the top
/// item's block anchor, the pixel offset inside it and, once measured, the
/// fraction of its height.
fn note_capture_position(
    items: &[NoteItem],
    blocks: &[PreviewBlock],
    list: &ListState,
    layout: &nav::ReadingLayout,
) -> Option<workspace::ReadingPosition> {
    let top = list.logical_scroll_top();
    // The top is a document boundary, not the first measured paragraph.
    if top.item_ix == 0 && top.offset_in_item <= px(0.) {
        return Some(workspace::ReadingPosition::default());
    }
    let index = note_top_block(items, top.item_ix)?;
    let within = -f32::from(top.offset_in_item);
    // Items above the scroll top are only measured once visited, so the pixel
    // offset under-reports right after a restore. The persisted offset feeds a
    // coarse "has a reading position" check (`< -8.`): past the first item the
    // note is by definition not at its top, so keep the value past that marker.
    let offset = f32::from(list.scroll_px_offset_for_scrollbar().y).min(if top.item_ix == 0 {
        0.
    } else {
        -9.
    });
    Some(workspace::ReadingPosition {
        paragraph: blocks.get(index).map(|block| block_anchor(block, index)),
        seconds: block_time(blocks, index),
        offset,
        within,
        fraction: layout
            .item_height(index)
            .map(|height| nav::within_fraction(within, height)),
    })
}

/// Where the note list should scroll to restore a position or reveal a search
/// match: the item index and, once the item is measured, the precise in-item
/// offset (the found line with one line of context, or the saved fractional
/// position). `None` scrolls to the document top.
fn note_restore_target(
    items: &[NoteItem],
    blocks: &[PreviewBlock],
    position: &workspace::ReadingPosition,
    search: Option<(usize, usize)>,
    layout: &nav::ReadingLayout,
) -> Option<(usize, Option<f32>)> {
    let block = search
        .map(|(block, _)| block)
        .or_else(|| note_position_index(blocks, position));
    let item_ix = block.and_then(|block| note_block_item(items, block))?;
    let offset = search
        .and_then(|(block, byte)| layout.search_within(block, byte))
        .or_else(|| {
            block.and_then(|block| {
                layout
                    .item_height(block)
                    .map(|height| -nav::restore_within(position.fraction, position.within, height))
            })
        });
    Some((item_ix, offset))
}
fn frame_label(title: &str, frame: &Frame, index: usize) -> String {
    match frame.seconds {
        Some(time) => format!("{title}，{} 的截图", course2md::render::fmt_ts(time)),
        None => format!("{title}，第 {} 张图片", index + 1),
    }
}

fn matching_frames(frames: &[Frame], query: &str) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    frames
        .iter()
        .enumerate()
        .filter(|(_, frame)| {
            query.is_empty()
                || frame.transcript.to_lowercase().contains(&query)
                || frame
                    .caption
                    .as_ref()
                    .is_some_and(|caption| caption.to_lowercase().contains(&query))
                || frame
                    .seconds
                    .is_some_and(|seconds| course2md::render::fmt_ts(seconds).contains(&query))
        })
        .map(|(index, _)| index)
        .collect()
}

fn frame_excerpt(frame: &Frame, query: &str) -> String {
    let query = query.trim();
    if !query.is_empty() {
        for text in std::iter::once(frame.transcript.as_str()).chain(frame.caption.as_deref()) {
            if let Some(found) = nav::text_matches(text, query).first() {
                let mut start = text[..found.start]
                    .char_indices()
                    .rev()
                    .nth(15)
                    .map(|(index, _)| index)
                    .unwrap_or(0);
                while start > 0
                    && text.as_bytes()[start].is_ascii_alphanumeric()
                    && text.as_bytes()[start - 1].is_ascii_alphanumeric()
                {
                    start -= 1;
                }
                let mut end = text[found.end..]
                    .char_indices()
                    .nth(100)
                    .map(|(index, _)| found.end + index)
                    .unwrap_or(text.len());
                while end < text.len()
                    && text.as_bytes()[end - 1].is_ascii_alphanumeric()
                    && text.as_bytes()[end].is_ascii_alphanumeric()
                {
                    end += 1;
                }
                return format!(
                    "{}{}{}",
                    if start > 0 { "…" } else { "" },
                    text[start..end].trim(),
                    if end < text.len() { "…" } else { "" },
                );
            }
        }
    }
    if !frame.transcript.trim().is_empty() {
        frame.transcript.clone()
    } else {
        frame
            .caption
            .clone()
            .filter(|caption| !caption.trim().is_empty())
            .unwrap_or_else(|| "这张截图没有对应文字".into())
    }
}

fn reader_outline(preview: &notes::Preview) -> Vec<(usize, String, Option<f64>)> {
    let headings = preview
        .blocks
        .iter()
        .enumerate()
        .filter_map(|(index, block)| {
            if let PreviewBlock::Heading { text, seconds, .. } = block {
                Some((index, text.clone(), *seconds))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    let topics = preview
        .document
        .as_ref()
        .and_then(|document| document.summary.as_ref())
        .map(|summary| {
            summary
                .outline
                .iter()
                .map(|item| (item.t, item.title.clone()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    nav::content_outline(&headings, &topics)
}
/// Selectable paragraph text with word-level find highlights. Selection plumbing
/// mirrors gpui_base::SelectableText (which accepts no highlight runs).
struct ReaderText {
    id: ElementId,
    handle: Option<gpui_base::TextSelectionHandle>,
    text: SharedString,
    styled_text: StyledText,
    document_order: u64,
    search_target: Option<ReaderSearchTarget>,
}

struct ReaderSearchTarget {
    block: usize,
    byte: usize,
    positions: Rc<RefCell<nav::ReadingLayout>>,
}
impl ReaderText {
    fn new(
        id: impl Into<ElementId>,
        text: impl Into<SharedString>,
        document_order: u64,
        highlights: Vec<(Range<usize>, HighlightStyle)>,
    ) -> Self {
        let text = text.into();
        let styled = StyledText::new(text.clone());
        ReaderText {
            id: id.into(),
            handle: None,
            styled_text: if highlights.is_empty() {
                styled
            } else {
                styled.with_highlights(highlights)
            },
            text,
            document_order,
            search_target: None,
        }
    }

    fn with_search_target(mut self, target: Option<ReaderSearchTarget>) -> Self {
        self.search_target = target;
        self
    }
}
impl IntoElement for ReaderText {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}
impl Element for ReaderText {
    type RequestLayoutState = gpui_base::TextSelectionHandle;
    type PrepaintState = Hitbox;
    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let handle = self.handle.clone().unwrap_or_else(|| {
            window.with_element_state(
                global_id.expect("ReaderText must have a stable element id"),
                |retained: Option<gpui_base::TextSelectionHandle>, _| {
                    let handle = retained.unwrap_or_else(|| {
                        gpui_base::TextSelectionHandle::new(self.text.clone(), cx)
                    });
                    (handle.clone(), handle)
                },
            )
        });
        let (layout_id, ()) = self
            .styled_text
            .request_layout(global_id, inspector_id, window, cx);
        (layout_id, handle)
    }
    fn prepaint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        handle: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.styled_text
            .prepaint(global_id, inspector_id, bounds, &mut (), window, cx);
        if let Some(target) = &self.search_target {
            let layout = self.styled_text.layout();
            if let Some(position) = layout.position_for_index(target.byte) {
                // Window-absolute like the owning item's record: the base
                // cancels in `search_within`, and the list state is borrowed
                // while items prepaint, so it cannot be queried here.
                target.positions.borrow_mut().record_search_line(
                    target.block,
                    target.byte,
                    f32::from(position.y),
                    f32::from(layout.line_height()),
                );
            }
        }
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        handle.register(
            gpui_base::TextSelectionRegistration::new(hitbox.clone(), bounds)
                .with_document_order(self.document_order)
                .with_text_bounds(vec![bounds]),
            window,
            cx,
        );
        hitbox
    }
    fn paint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        handle: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let layout = self.styled_text.layout().clone();
        let selected_text_before = gpui_base::TextSelection::selected_text(window, cx);
        let projection = handle.update_runs(
            &[
                gpui_base::TextSelectionRun::new(self.text.clone(), layout.clone(), bounds)
                    .with_document_order(self.document_order),
            ],
            cx,
        );
        if selected_text_before != gpui_base::TextSelection::selected_text(window, cx) {
            window.refresh();
        }
        for range in projection.ranges().iter().flatten().cloned() {
            paint_text_selection(&layout, range, window);
        }
        self.styled_text.paint(
            global_id,
            inspector_id,
            bounds,
            &mut (),
            &mut (),
            window,
            cx,
        );
    }
}
/// Quad painting identical to gpui_base::SelectableText's selection pass.
fn paint_text_selection(layout: &gpui::TextLayout, range: Range<usize>, window: &mut Window) {
    let (Some(start), Some(end)) = (
        layout.position_for_index(range.start),
        layout.position_for_index(range.end),
    ) else {
        return;
    };
    let line_height = layout.line_height();
    let bounds = layout.bounds();
    let quads = if start.y == end.y {
        vec![Bounds::from_corners(
            start,
            Point::new(end.x, end.y + line_height),
        )]
    } else {
        let mut quads = vec![Bounds::from_corners(
            start,
            Point::new(bounds.right(), start.y + line_height),
        )];
        if end.y > start.y + line_height {
            quads.push(Bounds::from_corners(
                Point::new(bounds.left(), start.y + line_height),
                Point::new(bounds.right(), end.y),
            ));
        }
        quads.push(Bounds::from_corners(
            Point::new(bounds.left(), end.y),
            Point::new(end.x, end.y + line_height),
        ));
        quads
    };
    for quad in quads {
        window.paint_quad(PaintQuad {
            bounds: quad,
            background: color(SELECTION).into(),
            corner_radii: gpui::Corners::default(),
            border_widths: gpui::Edges::default(),
            border_color: transparent_black(),
            border_style: gpui::BorderStyle::default(),
        });
    }
}
fn paragraph(
    id: impl Into<ElementId>,
    text: String,
    order: usize,
    highlights: Vec<(Range<usize>, HighlightStyle)>,
    search_target: Option<ReaderSearchTarget>,
) -> Stateful<Div> {
    let id = id.into();
    div()
        .id(id.clone())
        .role(Role::Paragraph)
        .aria_label(text.clone())
        .text_size(TEXT_READER)
        .font_weight(FontWeight::NORMAL)
        .line_height(relative(1.7))
        .child(
            ReaderText::new(id, text, order as u64, highlights).with_search_target(search_target),
        )
}

/// Everything a virtualized note-flow item needs, gathered once per frame so
/// the list's render closure only builds the visible window of blocks.
struct NoteFlow {
    items: Rc<Vec<NoteItem>>,
    focus: Rc<Vec<FocusHandle>>,
    preview: Rc<notes::Preview>,
    frames: Rc<Vec<Frame>>,
    source: Option<nav::SourceTarget>,
    outline: Vec<(f64, String)>,
    frame_index_by_path: HashMap<PathBuf, usize>,
    missing_by_anchor: HashMap<String, Vec<usize>>,
    matches: Vec<Match>,
    match_index: usize,
    find_open: bool,
    data_loading: bool,
    item_layout: Rc<RefCell<nav::ReadingLayout>>,
    note_list: ListState,
    desktop: WeakEntity<Desktop>,
}

impl NoteFlow {
    fn chapter_title(&self, seconds: Option<f64>) -> Option<String> {
        let seconds = seconds?;
        // 章节边界语义：标题属于「最后一个不晚于它的目录章节」——
        // 截图时间戳几乎从不与大纲时间重合，abs<1.5 的匹配永远落空（M7）
        self.outline
            .iter()
            .filter(|(time, _)| *time <= seconds + 1.5)
            .max_by(|(a, _), (b, _)| a.total_cmp(b))
            .map(|(_, title)| title.clone())
    }

    /// Find highlights group per block; the current match reads stronger.
    /// `matches` arrive in block order, so one binary search scopes a block.
    fn highlight_runs(&self, index: usize) -> Vec<(Range<usize>, HighlightStyle)> {
        if !self.find_open {
            return Vec::new();
        }
        let start = self.matches.partition_point(|found| found.block < index);
        self.matches[start..]
            .iter()
            .take_while(|found| found.block == index)
            .enumerate()
            .map(|(offset, found)| {
                let current = start + offset == self.match_index;
                (
                    found.range.clone(),
                    if current {
                        HighlightStyle {
                            background_color: Some(color(FIND_CURRENT).into()),
                            underline: Some(UnderlineStyle {
                                thickness: px(1.),
                                color: Some(color(FIND_CURRENT_LINE).into()),
                                wavy: false,
                            }),
                            ..Default::default()
                        }
                    } else {
                        HighlightStyle {
                            background_color: Some(color(FIND_HIGHLIGHT).into()),
                            ..Default::default()
                        }
                    },
                )
            })
            .collect()
    }

    fn search_target(&self, index: usize) -> Option<ReaderSearchTarget> {
        if !self.find_open {
            return None;
        }
        self.matches
            .get(self.match_index)
            .filter(|found| found.block == index)
            .map(|found| ReaderSearchTarget {
                block: index,
                byte: found.range.start,
                positions: self.item_layout.clone(),
            })
    }

    /// One list item: uniform spacing between items, a focus container that
    /// keeps a focused control mounted off-viewport, and the position record
    /// that reading positions and search jumps resolve against.
    fn item_wrapper(&self, index: usize, content: AnyElement) -> Stateful<Div> {
        let block = note_item_block(self.items[index]);
        let mut wrapper = div()
            .w_full()
            .min_w_0()
            .flex_shrink_0()
            .when(index + 1 != self.items.len(), |view| view.mb_3());
        if let Some(block) = block {
            let positions = self.item_layout.clone();
            wrapper = wrapper.on_children_prepainted(move |bounds, _, _| {
                if let Some(bounds) = bounds.first() {
                    // Window-absolute records: only same-frame differences and
                    // heights are read (search_within / item_height), so no
                    // scroll-base adjustment is needed — and the list state is
                    // mutably borrowed while items prepaint, so it must not be
                    // queried here.
                    positions.borrow_mut().record(
                        block,
                        f32::from(bounds.top()),
                        f32::from(bounds.size.height),
                    );
                }
            });
        }
        wrapper
            .child(content)
            .id(("note-item", index))
            .track_focus(&self.focus[index])
            .tab_stop(false)
    }

    fn reveal(
        &self,
        id: ElementId,
        child: AnyElement,
        full_width: bool,
    ) -> crate::focus_scroll::RevealFocus {
        let view = crate::focus_scroll::RevealFocus::in_list(id, child, self.note_list.clone());
        if full_width { view } else { view.inline() }
    }
}

/// Render one note-flow item. Mirrors the pre-virtualization article order:
/// scrolling title, summary label and paragraphs, then remaining blocks.
fn render_note_item(flow: &NoteFlow, item_ix: usize, window: &mut Window) -> AnyElement {
    let Some(&item) = flow.items.get(item_ix) else {
        return div().into_any_element();
    };
    let preview = &flow.preview;
    let content = match item {
        NoteItem::Title => theme::accessible_text("reader-title", preview.course.title.clone())
            .role(Role::Heading)
            .flex_shrink_0()
            .min_w_0()
            .whitespace_normal()
            .text_size(TEXT_TITLE)
            .font_weight(FontWeight::SEMIBOLD)
            .into_any_element(),
        NoteItem::SummaryLabel(_) => h_flex()
            .gap_2()
            .items_center()
            .child(icons::article().size(rems(18. / 14.)).flex_shrink_0())
            .child(
                theme::accessible_text("reader-summary-label", "摘要")
                    .role(Role::Heading)
                    .text_size(TEXT_TITLE)
                    .font_weight(FontWeight::SEMIBOLD),
            )
            .into_any_element(),
        NoteItem::Block(index) => match &preview.blocks[index] {
            PreviewBlock::Heading {
                text,
                anchor,
                seconds,
            } => {
                let marks = flow.highlight_runs(index);
                let chapter = flow.chapter_title(*seconds).filter(|_| text != "摘要");
                // 章节标题只在本章第一个标题出现：同一章的后续时间戳标题保持为 chip（review2#1）
                let chapter_changed = chapter.is_some()
                    && chapter
                        != preview.blocks[..index].iter().rev().find_map(|block| {
                            if let PreviewBlock::Heading { seconds, .. } = block {
                                flow.chapter_title(*seconds)
                            } else {
                                None
                            }
                        });
                let outlined = chapter.filter(|_| chapter_changed);
                // 无大纲且标题文本就是时间戳时，chip 独自承担章节标题。
                let bare_timestamp = outlined.is_none()
                    && seconds.is_some_and(|s| course2md::render::fmt_ts(s) == *text);
                let display = outlined.unwrap_or_else(|| text.clone());
                let heading: Option<AnyElement> = if bare_timestamp && marks.is_empty() {
                    None
                } else if marks.is_empty() {
                    Some(
                        theme::accessible_text(("reader-heading", index), display)
                            .role(Role::Heading)
                            .text_size(TEXT_TITLE)
                            .font_weight(FontWeight::SEMIBOLD)
                            .into_any_element(),
                    )
                } else {
                    Some(
                        div()
                            .id(("reader-heading-wrap", index))
                            .role(Role::Heading)
                            .aria_label(text.clone())
                            .text_size(TEXT_TITLE)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(
                                ReaderText::new(
                                    ("reader-heading-marks", index),
                                    text.clone(),
                                    index as u64,
                                    marks,
                                )
                                .with_search_target(flow.search_target(index)),
                            )
                            .into_any_element(),
                    )
                };
                v_flex()
                    .id(SharedString::from(anchor.clone()))
                    .gap_2()
                    .when(index > 0, |view| view.pt_4())
                    .child(
                        h_flex()
                            .flex_wrap()
                            .gap_2()
                            .items_center()
                            .when_some(*seconds, |row, seconds| {
                                let chip = div()
                                    .text_size(TEXT_READER)
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(color(GRAY))
                                    .flex_shrink_0()
                                    .whitespace_nowrap()
                                    .child(course2md::render::fmt_ts(seconds));
                                row.child(if bare_timestamp {
                                    chip.id(("reader-heading", index))
                                        .role(Role::Heading)
                                        .aria_label(text.clone())
                                } else {
                                    chip.id(("reader-heading-chip", index))
                                })
                            })
                            .children(heading),
                    )
                    .children(
                        flow.missing_by_anchor
                            .get(anchor.as_str())
                            .into_iter()
                            .flatten()
                            .map(|&frame_index| {
                                let frame = &flow.frames[frame_index];
                                theme::accessible_text(
                                    ("missing-body-image", frame_index),
                                    format!(
                                        "{}无法读取。对应正文保留在下方。",
                                        frame_label(&preview.course.title, frame, frame_index)
                                    ),
                                )
                                .text_sm()
                                .text_color(color(WARNING))
                            }),
                    )
                    .into_any_element()
            }
            PreviewBlock::Paragraph { text, anchor } => paragraph(
                SharedString::from(anchor.clone()),
                text.clone(),
                index,
                flow.highlight_runs(index),
                flow.search_target(index),
            )
            .into_any_element(),
            PreviewBlock::Image(path) => {
                let frame_index = flow.frame_index_by_path.get(path).copied();
                let frame = frame_index.and_then(|i| flow.frames.get(i));
                if frame_index.is_none() && !flow.data_loading {
                    return flow
                        .item_wrapper(
                            item_ix,
                            theme::accessible_text(
                                ("unreadable-inline-image", index),
                                "这张截图无法读取；对应正文仍可阅读。",
                            )
                            .text_sm()
                            .text_color(color(WARNING))
                            .into_any_element(),
                        )
                        .into_any_element();
                }
                let label = frame
                    .map(|frame| frame_label(&preview.course.title, frame, frame_index.unwrap()))
                    .unwrap_or_else(|| format!("{}，正文图片", preview.course.title));
                let aspect_ratio = frame
                    .filter(|frame| frame.width > 0 && frame.height > 0)
                    .map(|frame| frame.width as f32 / frame.height as f32)
                    .unwrap_or(16. / 9.);
                let rem_size = f32::from(window.rem_size());
                let available_height =
                    (f32::from(window.bounds().size.height) - rem_size * (40. / 14.) - 64.).max(0.);
                let desktop = flow.desktop.clone();
                v_flex()
                    .w_full()
                    .gap_2()
                    .child(
                        flow.reveal(
                            ("reveal-note-image", index).into(),
                            control(("note-image", index))
                                .ghost()
                                .p_0()
                                // 行内截图限高：默认窗口内摘要与正文能衔接（M7）
                                .w(px((available_height * 0.24).min(160.) * aspect_ratio))
                                .max_w_full()
                                .h_auto()
                                .min_h(px(0.))
                                .border_0()
                                .rounded(RADIUS_SMALL)
                                .aspect_ratio(aspect_ratio)
                                .accessibility_label(format!("放大{label}"))
                                // 不再用 tooltip 文案提示放大：它会盖住摘要/正文；点击图本身即放大
                                .disabled(frame_index.is_none())
                                .child(
                                    img(path.clone())
                                        .size_full()
                                        .rounded(RADIUS_SMALL)
                                        .object_fit(ObjectFit::Contain)
                                        .with_fallback(move || {
                                            theme::accessible_text(
                                                (
                                                    "failed-reader-image",
                                                    frame_index.unwrap_or(0)
                                                ),
                                                "这张截图无法读取；对应正文仍可阅读。",
                                            )
                                            .into_any_element()
                                        }),
                                )
                                .on_click(move |_, window, cx| {
                                    if let Some(index) = frame_index {
                                        let _ = desktop.update(cx, |this, cx| {
                                            this.open_reader_image(index, window, cx);
                                        });
                                    }
                                })
                                .into_any_element(),
                            true,
                        ),
                    )
                    // 「从此处观看」贴着它作用的截图（图下左对齐），不再挤进章节标题行（review2#1）
                    .when_some(
                        frame
                            .and_then(|frame| frame.seconds)
                            .and_then(|seconds| {
                                flow.source
                                    .as_ref()
                                    .and_then(|source| nav::seek_url(source, seconds))
                            }),
                        |view, url| {
                            view.child(
                                h_flex().w_full().justify_start().child(
                                    quiet(("seek-image", index))
                                        .icon(icons::play_arrow())
                                        .label("从此处观看")
                                        .min_h(rems(1.6))
                                        .accessibility_label("在原视频打开这个位置")
                                        .on_click(move |_, _, cx| cx.open_url(&url)),
                                ),
                            )
                        },
                    )
                    .when_some(
                        frame.and_then(|frame| frame.caption.as_ref()),
                        |figure, caption| {
                            figure.child(
                                theme::accessible_text(("figure-caption", index), caption.clone())
                                    .text_size(TEXT_AUX)
                                    .font_weight(FontWeight::NORMAL)
                                    .text_color(color(GRAY)),
                            )
                        },
                    )
                    .into_any_element()
            }
        },
    };
    flow.item_wrapper(item_ix, content).into_any_element()
}

impl Desktop {
    pub(super) fn reader_viewer_open(&self) -> bool {
        self.reader_ui.viewer.is_some()
    }

    fn scroll_reader_page(&mut self, direction: f32, cx: &mut Context<Self>) {
        if self.result_tab == 0 {
            let step = self.reader_ui.note_list.viewport_bounds().size.height * 0.85 * direction;
            self.reader_ui.note_list.scroll_by(-step);
        } else {
            let step = self.reader_scroll.bounds().size.height * 0.85 * direction;
            let offset = self.reader_scroll.offset() + point(px(0.), step);
            self.reader_scroll.set_offset(offset);
        }
        cx.notify();
    }
    pub fn save_library_presentation(&mut self, cx: &mut Context<Self>) {
        let mut preferences = self.application_edit_base();
        preferences.desktop.library_cards = self.desktop_settings.library_cards;
        preferences.desktop.library_group_folders = self.desktop_settings.library_group_folders;
        self.commit_application(preferences, cx);
        cx.notify();
    }
    fn reading_key(&self) -> Option<String> {
        let preview = self.preview.as_ref()?;
        Some(match &preview.course.manifest {
            Some(manifest) => manifest_reading_key(
                &manifest.course_id,
                &manifest.version_id,
                self.result_tab,
            ),
            None => {
                let identity = self
                    .course_location(&preview.course)
                    .and_then(|library| {
                        preview
                            .course
                            .dir
                            .strip_prefix(&library.root)
                            .ok()
                            .map(|relative| format!("{}:{}", library.id, relative.display()))
                    })
                    .unwrap_or_else(|| preview.course.dir.display().to_string());
                format!("{identity}:{}", self.result_tab)
            }
        })
    }
    fn capture_reading_position(&self) -> Option<workspace::ReadingPosition> {
        let preview = self.preview.as_ref()?;
        if self.result_tab == 0 {
            return note_capture_position(
                &self.reader_ui.note_items,
                &preview.blocks,
                &self.reader_ui.note_list,
                &self.reader_ui.item_layout.borrow(),
            );
        }
        capture_reader_position(
            &self.reader_ui.item_layout.borrow(),
            f32::from(self.reader_scroll.offset().y),
            |index| {
                self.reader_ui
                    .frames
                    .get(index)
                    .map(|frame| (Some(frame.anchor.clone()), frame.seconds))
                    .unwrap_or((None, None))
            },
        )
    }
    pub fn save_reading_position(&mut self, cx: &mut Context<Self>) {
        self.persist_reading_position(false, cx);
    }
    /// Polled while the reader scrolls: persist at most once per interval, so
    /// continuous scrolling does not clone and rewrite the workspace each tick.
    /// Navigation and exit call `save_reading_position` for an immediate flush.
    pub fn poll_reading_position(&mut self, cx: &mut Context<Self>) {
        self.persist_reading_position(true, cx);
    }
    fn persist_reading_position(&mut self, throttled: bool, cx: &mut Context<Self>) {
        if self.page != Page::Result
            || self.reading
            || (self.result_tab == 1 && self.reader_ui.gallery_search_position.is_some())
            || self.reader_ui.restoring
            || self.reader_ui.pending_restore.is_some()
        {
            return;
        }
        let (Some(key), Some(position)) = (self.reading_key(), self.capture_reading_position())
        else {
            return;
        };
        if self.reader_ui.last_position.as_ref() == Some(&(key.clone(), position.clone())) {
            return;
        }
        // Skipped ticks leave `last_position` stale, keeping the position dirty
        // for the next poll or the immediate flush on navigation and exit.
        if throttled
            && self
                .reader_position_saved_at
                .is_some_and(|saved| saved.elapsed() < READING_POSITION_INTERVAL)
        {
            return;
        }
        if let Some(workspace) = &mut self.workspace {
            if let Err(error) = workspace.transaction(|state| {
                state.positions.insert(key.clone(), position.clone());
                Ok(())
            }) {
                self.workspace_error = Some(format!("阅读位置尚未保存：{error:#}"));
                cx.notify();
                return;
            }
        }
        self.reader_position_saved_at = Some(Instant::now());
        self.reader_saved_offset = position.offset;
        self.reader_ui.last_position = Some((key, position));
    }
    pub fn restore_reading_position(&mut self, cx: &mut Context<Self>) {
        self.ensure_reader_data(cx);
        self.reader_ui.pending_search = None;
        self.reader_ui.pending_restore = Some(
            self.reading_key()
                .and_then(|key| self.workspace.as_ref()?.state.positions.get(&key))
                .cloned()
                .unwrap_or_default(),
        );
        self.reader_ui.restore_generation += 1;
        self.reader_ui.restoring = false;
        self.reader_ui.last_position = None;
        cx.notify();
    }
    fn position_index(&self, position: &workspace::ReadingPosition) -> Option<usize> {
        if self.result_tab == 0 {
            note_position_index(&self.preview.as_ref()?.blocks, position)
        } else {
            position
                .paragraph
                .as_ref()
                .and_then(|anchor| {
                    self.reader_ui
                        .frames
                        .iter()
                        .position(|frame| frame.anchor == *anchor)
                })
                .or_else(|| {
                    position.seconds.and_then(|seconds| {
                        nav::nearest_time(
                            self.reader_ui
                                .frames
                                .iter()
                                .enumerate()
                                .map(|(i, frame)| (i, frame.seconds)),
                            seconds,
                        )
                        .map(|(i, _)| i)
                    })
                })
        }
    }
    fn schedule_reader_restore(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.result_tab == 1 && self.reader_ui.data_loading {
            return;
        }
        let Some(position) = self.reader_ui.pending_restore.take() else {
            return;
        };
        self.reader_ui.restoring = true;
        let search = self.reader_ui.pending_search.take();
        let generation = self.reader_ui.restore_generation;
        let key = self.reading_key();
        cx.on_next_frame(window, move |this, window, cx| {
            if this.reader_ui.restore_generation != generation || this.reading_key() != key {
                return;
            }
            if this.result_tab == 0 {
                this.apply_note_restore(position, search, 0, window, cx);
                return;
            }
            let positions = this.reader_ui.item_layout.borrow();
            let target = search
                .as_ref()
                .and_then(|found| positions.search_offset(found.block, found.range.start))
                .unwrap_or_else(|| {
                    restored_reader_offset(&positions, this.position_index(&position), &position)
                });
            drop(positions);
            let target = px(target);
            let maximum = this.reader_scroll.max_offset().y;
            this.reader_scroll
                .set_offset(point(px(0.), target.max(-maximum).min(px(0.))));
            this.reader_ui.restoring = false;
            this.reader_ui.last_position = None;
            cx.notify();
        });
    }

    /// Position the virtualized note flow at a saved position or search match.
    /// The target item renders once it becomes the scroll top, so the precise
    /// in-item offset (the found line, or the measured fractional position) is
    /// applied on a following pass once the item has been measured again.
    fn apply_note_restore(
        &mut self,
        position: workspace::ReadingPosition,
        search: Option<Match>,
        pass: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let list = self.reader_ui.note_list.clone();
        let items = self.reader_ui.note_items.clone();
        let Some(preview) = self.preview.clone() else {
            self.reader_ui.restoring = false;
            cx.notify();
            return;
        };
        let search_key = search
            .as_ref()
            .map(|found| (found.block, found.range.start));
        let target = note_restore_target(
            &items,
            &preview.blocks,
            &position,
            search_key,
            &self.reader_ui.item_layout.borrow(),
        );
        let Some((item_ix, precise)) = target else {
            list.scroll_to(ListOffset {
                item_ix: 0,
                offset_in_item: px(0.),
            });
            self.reader_ui.restoring = false;
            self.reader_ui.last_position = None;
            cx.notify();
            return;
        };
        if let Some(offset) = precise {
            list.scroll_to(ListOffset {
                item_ix,
                offset_in_item: px(offset.max(0.)),
            });
            self.reader_ui.restoring = false;
            self.reader_ui.last_position = None;
            cx.notify();
            return;
        }
        // The item has not been measured this session: land on it first, then
        // refine the in-item offset once its height is known.
        list.scroll_to(ListOffset {
            item_ix,
            offset_in_item: px((-position.within).max(0.)),
        });
        const RESTORE_MEASURE_PASSES: usize = 4;
        if pass + 1 < RESTORE_MEASURE_PASSES {
            let generation = self.reader_ui.restore_generation;
            let key = self.reading_key();
            cx.on_next_frame(window, move |this, window, cx| {
                if this.reader_ui.restore_generation != generation || this.reading_key() != key {
                    return;
                }
                this.apply_note_restore(position, search, pass + 1, window, cx);
            });
        } else {
            self.reader_ui.restoring = false;
            self.reader_ui.last_position = None;
        }
        cx.notify();
    }
    pub(crate) fn sync_reader_tab_stops(&mut self) {
        for (index, focus) in self.reader_ui.view_focus.iter_mut().enumerate() {
            *focus = focus.clone().tab_stop(index == self.result_tab);
        }
    }
    pub(crate) fn ensure_reader_data(&mut self, cx: &mut Context<Self>) {
        let Some(preview) = &self.preview else {
            return;
        };
        if self.reader_ui.loaded.as_ref() == Some(&preview.course.dir) {
            return;
        }
        self.reader_ui.loaded = Some(preview.course.dir.clone());
        self.reader_ui
            .controls_scroll
            .set_offset(point(px(0.), px(0.)));
        self.reader_ui
            .information_scroll
            .set_offset(point(px(0.), px(0.)));
        self.reader_ui.frames = Rc::default();
        self.reader_ui.versions.clear();
        self.reader_ui.issues.clear();
        self.reader_ui.source = None;
        // 换笔记：中止进行中的重新定位探测
        if let Some(cancel) = self.reader_ui.source_probe_cancel.take() {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.reader_ui.source_loading = false;
        self.reader_ui.source_available = false;
        self.reader_ui.export_folder = None;
        self.reader_ui.offline_video = OfflineVideo::NotRequested;
        self.reader_ui.offline_opening = false;
        self.reader_ui.matches.clear();
        self.reader_ui.gallery_search_position = None;
        self.reader_ui.find_open = false;
        self.reader_ui.info_open = false;
        self.reader_ui.processing_details_open = false;
        self.reader_ui.clear_find = true;
        self.reader_ui.layout = None;
        self.reader_ui.note_items_key = None;
        self.refresh_reader_data(cx);
    }
    fn refresh_reader_data(&mut self, cx: &mut Context<Self>) {
        let Some(preview) = self.preview.clone() else {
            return;
        };
        let path = preview.course.dir.clone();
        let source = self.unmapped_reader_source();
        let offline_request = self.reader_offline_video_request();
        let locations = self
            .workspace
            .as_ref()
            .map(|workspace| {
                workspace
                    .state
                    .libraries
                    .iter()
                    .map(|library| (library.root.clone(), library.previous_roots.clone()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        self.reader_ui.generation += 1;
        let generation = self.reader_ui.generation;
        self.reader_ui.data_loading = true;
        self.reader_ui.offline_opening = false;
        cx.spawn(async move |this, cx| {
            // 解析全文与离线检查是同步文件/子进程工作；见 crate::spawn_blocking_io 的说明
            let (data, source, source_available, offline_video) = crate::spawn_blocking_io(move || {
                let source = source.map(|source| match source {
                    nav::SourceTarget::Local(path) => {
                        nav::SourceTarget::Local(nav::relocated_source(&path, &locations))
                    }
                    other => other,
                });
                let available = source.as_ref().is_some_and(|source| match source {
                    nav::SourceTarget::Web(_) => true,
                    nav::SourceTarget::Local(path) => path.is_file(),
                });
                let offline_video = offline_request
                    .map(|request| request.inspect())
                    .unwrap_or_default();
                (load_reader_data(&preview), source, available, offline_video)
            })
            .recv()
            .await
            .unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                if this.reader_ui.generation != generation
                    || this
                        .preview
                        .as_ref()
                        .is_none_or(|preview| preview.course.dir != path)
                {
                    return;
                }
                if this.reader_ui.pending_restore.is_none() && !this.reader_ui.restoring {
                    this.reader_ui.pending_restore = this.capture_reading_position();
                }
                this.reader_ui.frames = Rc::new(data.frames);
                this.reader_ui.versions = data.versions;
                this.reader_ui.issues = data.issues;
                this.reader_ui.export_folder = data.export_folder;
                this.reader_ui.source = source;
                this.reader_ui.source_available = source_available;
                this.reader_ui.offline_video = offline_video;
                this.reader_ui.data_loading = false;
                this.reader_ui.layout = None;
                this.reader_ui.restore_generation += 1;
                this.reader_ui.restoring = false;
                cx.notify();
            });
        })
        .detach();
    }
    fn reader_source_key(&self) -> Option<String> {
        let preview = self.preview.as_ref()?;
        Some(
            preview
                .course
                .manifest
                .as_ref()
                .map(|m| {
                    if m.source_id.is_empty() {
                        m.course_id.clone()
                    } else {
                        m.source_id.clone()
                    }
                })
                .unwrap_or_else(|| preview.course.storage_dir().display().to_string()),
        )
    }
    fn reader_source(&self) -> Option<nav::SourceTarget> {
        self.reader_ui.source.clone()
    }
    fn reader_offline_video_request(&self) -> Option<OfflineVideoRequest> {
        let course = &self.preview.as_ref()?.course;
        let manifest = course.manifest.as_ref()?;
        let state = &self.workspace.as_ref()?.state;
        let task = state.task(&manifest.task_id)?;
        let library = state.library(&task.plan.library_id)?;
        OfflineVideoRequest::for_version(course, task, library)
    }
    fn play_reader_offline_video(&mut self, cx: &mut Context<Self>) {
        if self.reader_ui.offline_opening {
            return;
        }
        let Some(request) = self.reader_offline_video_request() else {
            return;
        };
        let version = request.version_dir.clone();
        let generation = self.reader_ui.generation;
        self.reader_ui.offline_opening = true;
        cx.spawn(async move |this, cx| {
            let status = crate::spawn_blocking_io(move || request.inspect())
                .recv()
                .await
                .unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                if this.reader_ui.generation != generation
                    || this
                        .preview
                        .as_ref()
                        .is_none_or(|preview| preview.course.dir != version)
                {
                    return;
                }
                this.reader_ui.offline_opening = false;
                if this.page == Page::Result {
                    if let OfflineVideo::Available(path) = &status {
                        cx.open_with_system(path);
                    } else {
                        this.message =
                            Some("此版本的离线视频暂不可用，仍可打开原视频网页。".into());
                    }
                }
                this.reader_ui.offline_video = status;
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn unmapped_reader_source(&self) -> Option<nav::SourceTarget> {
        if let Some(path) = self
            .reader_source_key()
            .and_then(|key| self.workspace.as_ref()?.state.reader_sources.get(&key))
        {
            return Some(nav::SourceTarget::Local(path.clone()));
        }
        let preview = self.preview.as_ref()?;
        let raw = preview
            .document
            .as_ref()
            .map(|doc| (doc.meta.webpage_url.as_str(), doc.meta.extractor == "local"))
            .or_else(|| {
                preview
                    .metadata
                    .iter()
                    .find(|(key, _)| key == "来源")
                    .map(|(_, value)| (value.as_str(), false))
            })?;
        nav::source_target(raw.0, raw.1)
    }
    fn open_reader_source(&mut self, seconds: Option<f64>, cx: &mut Context<Self>) {
        let Some(source) = self.reader_source() else {
            return;
        };
        if let Some(url) = seconds.and_then(|time| nav::seek_url(&source, time)) {
            cx.open_url(&url);
            return;
        }
        match source {
            nav::SourceTarget::Web(url) => cx.open_url(&url),
            nav::SourceTarget::Local(path) if path.is_file() => cx.open_with_system(&path),
            nav::SourceTarget::Local(path) => {
                self.message = Some(format!(
                    "原视频不在 {}。请使用“重新定位原视频”选择它的新位置。",
                    path.display()
                ));
                cx.notify();
            }
        }
    }
    fn relocate_reader_source(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.reader_ui.source_loading {
            return;
        }
        let Some(key) = self.reader_source_key() else {
            return;
        };
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("选择原视频".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let path = match receiver.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                _ => None,
            };
            let Some(path) = path else {
                return;
            };
            let cancel = Arc::new(AtomicBool::new(false));
            let stored = cancel.clone();
            let probing = cancel.clone();
            let _ = this.update(cx, |this, cx| {
                // 前一次探测若还在跑，先取消再开始新的
                if let Some(previous) = this.reader_ui.source_probe_cancel.take() {
                    previous.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                this.reader_ui.source_probe_cancel = Some(stored);
                this.reader_ui.source_loading = true;
                cx.notify();
            });
            let input = path.to_string_lossy().into_owned();
            let identity = key.clone();
            let checked = crate::spawn_blocking_io(move || {
                let source::SourceProbe::Single(source) =
                    source::probe(input, false, probing)?
                    else {
                        anyhow::bail!("请选择可读取的视频文件");
                    };
                    anyhow::ensure!(
                        !identity.starts_with("local:sha256:") || identity == source.identity,
                        "这个文件与生成笔记时的视频内容不同，请选择同一个原视频"
                    );
                    Ok::<_, anyhow::Error>(())
            });
            let checked = checked
                .recv()
                .await
                .unwrap_or_else(|_| Err(anyhow::anyhow!("检查原视频的工作线程意外结束")));
            let _ = this.update(cx, |this, cx| {
                this.reader_ui.source_loading = false;
                // 只清自己的标志：更晚开始的探测持有另一个 Arc
                if this
                    .reader_ui
                    .source_probe_cancel
                    .as_ref()
                    .is_some_and(|current| Arc::ptr_eq(current, &cancel))
                {
                    this.reader_ui.source_probe_cancel = None;
                }
                let result = checked.and_then(|_| {
                    this.workspace
                        .as_mut()
                        .ok_or_else(|| anyhow::anyhow!("课程库记录暂时不可写"))?
                        .transaction(|state| {
                            state.reader_sources.insert(key, path.clone());
                            Ok(())
                        })
                });
                this.message = Some(match result {
                    Ok(()) => {
                        this.refresh_reader_data(cx);
                        format!("已定位原视频：{}", path.display())
                    }
                    Err(error) => format!("原视频位置没有更改：{error:#}"),
                });
                cx.notify();
            });
        })
        .detach();
    }
    pub fn open_reader_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.preview.is_none() {
            return;
        }
        if !self.reader_ui.find_open {
            self.reader_ui.find_return_focus = window.focused(cx);
        }
        let input = if self.result_tab == 0 {
            self.reader_ui.find_open = true;
            self.reader_ui.find.clone()
        } else {
            self.reader_ui.gallery_find.clone()
        };
        input.update(cx, |input, cx| input.focus(window, cx));
        if self.result_tab == 0 {
            self.update_reader_matches(cx);
        }
        cx.notify();
    }
    fn close_reader_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.reader_ui.find_open = false;
        let input = if self.result_tab == 0 {
            self.reader_ui.find.clone()
        } else {
            self.reader_ui.gallery_find.clone()
        };
        input.update(cx, |input, cx| input.set_value("", window, cx));
        if let Some(focus) = self.reader_ui.find_return_focus.take() {
            focus.focus(window, cx);
        } else {
            self.reader_ui.focus.focus(window, cx);
        }
        cx.notify();
    }
    fn update_reader_matches(&mut self, cx: &mut Context<Self>) {
        let query = self.reader_ui.find.read(cx).value().to_string();
        self.reader_ui.matches = self
            .preview
            .as_ref()
            .map(|preview| {
                preview
                    .blocks
                    .iter()
                    .enumerate()
                    .flat_map(|(block, value)| {
                        block_text(value)
                            .map(|text| nav::text_matches(text, &query))
                            .unwrap_or_default()
                            .into_iter()
                            .map(move |range| Match { block, range })
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.reader_ui.match_index = 0;
        cx.notify();
    }
    fn move_reader_match(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.result_tab != 0 {
            return;
        }
        let count = self.reader_ui.matches.len();
        if count == 0 {
            return;
        }
        self.reader_ui.match_index =
            (self.reader_ui.match_index as isize + delta).rem_euclid(count as isize) as usize;
        let found = self.reader_ui.matches[self.reader_ui.match_index].clone();
        let fraction = self
            .preview
            .as_ref()
            .and_then(|preview| preview.blocks.get(found.block))
            .and_then(block_text)
            .map(|text| {
                text[..found.range.start].chars().count() as f32
                    / text.chars().count().max(1) as f32
            })
            .unwrap_or(0.);
        self.jump_reader_block(found.block, fraction, cx);
        self.reader_ui.pending_search = Some(found);
    }
    fn jump_reader_block(&mut self, index: usize, fraction: f32, cx: &mut Context<Self>) {
        self.reader_ui.pending_search = None;
        if self.result_tab != 0 {
            self.save_reading_position(cx);
            self.result_tab = 0;
            self.sync_reader_tab_stops();
        }
        let Some(preview) = &self.preview else {
            return;
        };
        let Some(block) = preview.blocks.get(index) else {
            return;
        };
        self.reader_ui.pending_restore = Some(workspace::ReadingPosition {
            paragraph: Some(block_anchor(block, index)),
            seconds: block_time(&preview.blocks, index),
            fraction: Some(fraction),
            ..Default::default()
        });
        self.reader_ui.restore_generation += 1;
        self.reader_ui.restoring = false;
        cx.notify();
    }
    fn open_reader_version(&mut self, course: Course, cx: &mut Context<Self>) {
        self.load_reader_version(course, false, cx);
    }
    fn load_reader_version(&mut self, course: Course, refresh: bool, cx: &mut Context<Self>) {
        if self.reading
            || (!refresh
                && self
                    .preview
                    .as_ref()
                    .is_some_and(|preview| preview.course.dir == course.dir))
        {
            return;
        }
        self.save_reading_position(cx);
        let position = self.capture_reading_position();
        let tab = self.result_tab;
        self.read_generation += 1;
        let generation = self.read_generation;
        self.reading = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { notes::read_preview(course) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.read_generation != generation {
                    return;
                }
                this.reading = false;
                match result {
                    Ok(mut preview) => {
                        preview.course.title = this.course_display_title(&preview.course);
                        this.preview = Some(preview);
                        this.result_tab = tab;
                        this.sync_reader_tab_stops();
                        if refresh {
                            this.reader_ui.loaded = None;
                        }
                        // 事件路径触发数据加载（渲染不再负责）
                        this.ensure_reader_data(cx);
                        let has_saved_position = this.reading_key().is_some_and(|key| {
                            this.workspace.as_ref().is_some_and(|workspace| {
                                workspace.state.positions.contains_key(&key)
                            })
                        });
                        this.restore_reading_position(cx);
                        if refresh {
                            this.reader_ui.pending_restore = position.clone();
                            this.message = Some("已重新读取这份笔记。".into());
                        }
                        if !refresh
                            && !has_saved_position
                            && let Some(seconds) = position.and_then(|position| position.seconds)
                        {
                            let blocks = &this.preview.as_ref().unwrap().blocks;
                            let nearest =
                                nav::nearest_time(heading_times(blocks), seconds);
                            if let Some((index, exact)) = nearest {
                                this.reader_ui.pending_restore = Some(workspace::ReadingPosition {
                                    seconds: Some(seconds),
                                    fraction: Some(0.),
                                    ..Default::default()
                                });
                                if !exact {
                                    this.message = Some(format!(
                                        "本版没有原来的 {}，已定位到最近的 {}。",
                                        course2md::render::fmt_ts(seconds),
                                        course2md::render::fmt_ts(
                                            block_time(blocks, index).unwrap()
                                        )
                                    ));
                                }
                            } else {
                                this.reader_ui.pending_restore = Some(Default::default());
                                this.message = Some("本版没有可对应的时间，已从开头显示。".into());
                            }
                        }
                    }
                    Err(error) => {
                        this.message = Some(format!(
                            "这个版本暂时无法打开：{error:#}。当前笔记仍可阅读。"
                        ))
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
    pub fn export_course(
        &mut self,
        course: Course,
        format: course2md::config::OutputFormat,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.exporting {
            return;
        }
        let version = course.dir.clone();
        let Some(generation) = self.reader_ui.export_state.begin(version.clone()) else {
            return;
        };
        self.exporting = true;
        let title = self.course_display_title(&course);
        let stem = title
            .chars()
            .map(|c| if "/\\:*?\"<>|".contains(c) { '_' } else { c })
            .collect::<String>();
        let extension = match format {
            course2md::config::OutputFormat::Md => "zip",
            course2md::config::OutputFormat::Html => "html",
            course2md::config::OutputFormat::Json => "json",
        };
        let suggested = format!("{stem}.{extension}");
        let receiver = cx.prompt_for_new_path(&self.output(cx), Some(&suggested));
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let path = match receiver.await {
                Ok(Ok(Some(path))) => path,
                Ok(Ok(None)) => {
                    let _ = this.update(cx, |this, cx| {
                        if this.reader_ui.export_state.cancel(generation, &version) {
                            this.exporting = false;
                            cx.notify();
                        }
                    });
                    return;
                }
                _ => {
                    let _ = this.update(cx, |this, cx| {
                        if this.reader_ui.export_state.finish(
                            generation,
                            &version,
                            ExportFeedback {
                                title,
                                format,
                                result: Err("无法打开文件选择器，请重试。".into()),
                            },
                        ) {
                            this.exporting = false;
                            cx.notify();
                        }
                    });
                    return;
                }
            };
            if !this
                .update(cx, |this, _| {
                    this.reader_ui.export_state.is_pending(generation, &version)
                })
                .unwrap_or(false)
            {
                return;
            }
            let target = path.clone();
            let source = version.clone();
            let exported = cx
                .background_executor()
                .spawn(async move {
                    let protected = source.canonicalize()?;
                    let parent = target
                        .parent()
                        .ok_or_else(|| anyhow::anyhow!("导出位置无效"))?
                        .canonicalize()?;
                    anyhow::ensure!(
                        !parent.starts_with(&protected)
                            && !parent.ancestors().any(|ancestor| ancestor
                                .join("manifest.json")
                                .is_file()
                                && ancestor.parent().is_some_and(|parent| parent
                                    .file_name()
                                    .is_some_and(|name| name == "versions"))),
                        "请将导出文件保存到笔记版本目录以外，避免覆盖已保存的正文和图片。"
                    );
                    // The native save panel owns the explicit overwrite decision; the old file
                    // remains intact until a complete export is ready beside it.
                    if target.exists() {
                        let temp = parent.join(format!(
                            ".course2md-export-{}.{}",
                            workspace::new_id("file"),
                            extension
                        ));
                        let result =
                            course2md::portable::export(&source, format, &temp).and_then(|_| {
                                std::fs::rename(&temp, &target).map_err(anyhow::Error::from)
                            });
                        if result.is_err() {
                            let _ = std::fs::remove_file(&temp);
                        }
                        result
                    } else {
                        course2md::portable::export(&source, format, &target).map(|_| ())
                    }
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                let result = exported
                    .map(|()| path.clone())
                    .map_err(|error| format!("{error:#}。笔记正文仍可阅读。"));
                let success = result.is_ok();
                if !this.reader_ui.export_state.finish(
                    generation,
                    &version,
                    ExportFeedback {
                        title,
                        format,
                        result,
                    },
                ) {
                    return;
                }
                this.exporting = false;
                if success {
                    this.reader_ui.exports.insert(version, path);
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn course_export_feedback(
        &self,
        course: &Course,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        let feedback = self
            .reader_ui
            .export_state
            .feedback
            .get(&course.dir)?
            .clone();
        let id = format!("export-feedback:{}", course.dir.display());
        let source = course.dir.clone();
        let dismiss = quiet(SharedString::from(format!("{id}:dismiss")))
            .icon(icons::close())
            .w(CONTROL_HEIGHT)
            .px_0()
            .accessibility_label("关闭这份笔记的导出提示")
            .tooltip("关闭提示")
            .on_click(cx.listener(move |this, _, _, cx| {
                this.reader_ui.export_state.feedback.remove(&source);
                cx.notify();
            }));
        Some(match feedback.result {
            Ok(path) => {
                let label = exported_file_label(&path);
                h_flex()
                    .w_full()
                    .min_w_0()
                    .gap_2()
                    .items_center()
                    .child(
                        icons::check()
                            .size(rems(18. / 14.))
                            .flex_shrink_0()
                            .text_color(color(SUCCESS)),
                    )
                    .child(
                        theme::accessible_text(SharedString::from(id.clone()), label.clone())
                            .aria_label(format!("《{}》{label}", feedback.title))
                            .flex_1()
                            .min_w_0()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(TEXT_AUX)
                            .font_weight(FontWeight::NORMAL)
                            .text_color(color(GRAY)),
                    )
                    .child(
                        quiet(SharedString::from(format!("{id}:reveal")))
                            .icon(icons::folder_open())
                            .label("打开导出位置")
                            .tooltip(format!("显示《{}》的导出文件", feedback.title))
                            .on_click(move |_, _, cx| cx.reveal_path(&path)),
                    )
                    .child(dismiss)
            }
            Err(error) => {
                let retry = course.clone();
                v_flex()
                    .w_full()
                    .min_w_0()
                    .gap_1()
                    .child(
                        theme::accessible_text(
                            SharedString::from(id.clone()),
                            format!("导出未完成：{error}"),
                        )
                        .text_size(TEXT_AUX)
                        .font_weight(FontWeight::NORMAL)
                        .text_color(color(DANGER))
                        .whitespace_normal(),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                outline_pill(SharedString::from(format!("{id}:retry")))
                                    .icon(icons::refresh())
                                    .label("重试导出")
                                    .disabled(self.exporting)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.export_course(
                                            retry.clone(),
                                            feedback.format,
                                            window,
                                            cx,
                                        );
                                    })),
                            )
                            .child(dismiss),
                    )
            }
        })
    }

    fn select_reader_view(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.result_tab != index {
            self.save_reading_position(cx);
            self.result_tab = index;
            self.sync_reader_tab_stops();
            if index == 1
                && !self
                    .reader_ui
                    .gallery_find
                    .read(cx)
                    .value()
                    .trim()
                    .is_empty()
            {
                self.reader_ui.pending_restore = None;
                self.reader_scroll.set_offset(point(px(0.), px(0.)));
            } else {
                self.restore_reading_position(cx);
            }
        }
        self.reader_ui.view_focus[index].focus(window, cx);
        cx.notify();
    }
    pub fn reader_page(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(preview) = self.preview.clone().map(Rc::new) else {
            return crate::motion::enter(
                "reader-opening",
                h_flex()
                    .items_center()
                    .gap_2()
                    .py_6()
                    .child(crate::motion::spinner("reader-opening-spinner", cx))
                    .child(theme::accessible_text("opening-note", "正在打开笔记…")));
        };
        // 渲染只读已提交快照：数据加载在 preview 变更的事件路径触发（ensure_reader_data），
        // tab stops 在各 result_tab 赋值点同步
        if self.reader_ui.clear_find {
            self.reader_ui.clear_find = false;
            self.reader_ui
                .find
                .update(cx, |input, cx| input.set_value("", window, cx));
            self.reader_ui
                .gallery_find
                .update(cx, |input, cx| input.set_value("", window, cx));
        }
        let layout = (
            f32::from(window.bounds().size.width),
            f32::from(window.bounds().size.height),
            f32::from(window.rem_size()),
        );
        if self.reader_ui.layout.is_some_and(|old| old != layout)
            && self.reader_ui.pending_restore.is_none()
            && !self.reader_ui.restoring
        {
            self.reader_ui.pending_restore = self.capture_reading_position();
            self.reader_ui.restore_generation += 1;
        }
        self.reader_ui.layout = Some(layout);
        self.schedule_reader_restore(window, cx);
        let rem_size = f32::from(window.rem_size());
        let content_width = crate::views::shell_content_width(Page::Result, window);
        // A single frame aligns navigation, title, tools and document content.
        // Reserve the same reading axis when the user hides the adjacent contents.
        let reader_width = content_width.min(
            f32::from(READER_MEASURE.to_pixels(window.rem_size()))
                + f32::from(TOC_PANEL.to_pixels(window.rem_size()))
                + 24.,
        );
        let toc_fits_beside = reader_width >= rem_size * 56. + 24.;
        let available_height = (layout.1 - rem_size * (40. / 14.) - 64.).max(0.);
        let compact = reader_width < rem_size * 60. || available_height < rem_size * 36.;
        let short_reader = available_height < rem_size * 24.;
        let root = v_flex()
            .id("reader-page")
            .track_focus(&self.reader_ui.focus)
            .key_context("CourseReader")
            .on_action(
                cx.listener(|this, _: &FindInNote, window, cx| this.open_reader_find(window, cx)),
            )
            .on_action(cx.listener(|this, _: &NextMatch, _, cx| this.move_reader_match(1, cx)))
            .on_action(cx.listener(|this, _: &PreviousMatch, _, cx| this.move_reader_match(-1, cx)))
            .on_action(cx.listener(|this, _: &ReaderPageUp, _, cx| this.scroll_reader_page(1., cx)))
            .on_action(
                cx.listener(|this, _: &ReaderPageDown, _, cx| this.scroll_reader_page(-1., cx)),
            )
            .on_action(cx.listener(|this, _: &ReaderStart, _, cx| {
                if this.result_tab == 0 {
                    this.reader_ui.note_list.scroll_to(ListOffset {
                        item_ix: 0,
                        offset_in_item: px(0.),
                    });
                } else {
                    this.reader_scroll.set_offset(point(px(0.), px(0.)));
                }
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ReaderEnd, _, cx| {
                if this.result_tab == 0 {
                    this.reader_ui.note_list.scroll_to_end();
                } else {
                    this.reader_scroll.scroll_to_bottom();
                }
                cx.notify();
            }))
            .on_action(cx.listener(move |this, _: &CloseFind, window, cx| {
                if !toc_fits_beside && this.reader_ui.toc_open == Some(true) {
                    this.reader_ui.toc_open = Some(false);
                    cx.notify();
                } else if (this.result_tab == 0 && this.reader_ui.find_open)
                    || (this.result_tab == 1
                        && !this.reader_ui.gallery_find.read(cx).value().is_empty())
                {
                    this.close_reader_find(window, cx);
                } else {
                    cx.propagate();
                }
            }))
            .h_full()
            .min_h_0()
            .w_full()
            .max_w(px(reader_width))
            .mx_auto()
            .gap_3()
            .when(compact, |view| view.gap_2());
        // Short windows reserve fixed space for reading controls. Document
        // identity scrolls with the content instead of consuming a second header.
        let controls_scroll = self.reader_ui.controls_scroll.clone();
        let mut page = v_flex()
            .id("reader-controls")
            .w_full()
            .min_w_0()
            .gap_3()
            .flex_shrink_0()
            .when(compact, |view| view.gap_1())
            .when(!compact, |view| {
                view.max_h(relative(0.45))
                    .when(self.reader_ui.info_open, |view| {
                        view.max_h(px(available_height * 0.45))
                    })
                    .overflow_y_scroll()
                    .track_scroll(&controls_scroll)
            });
        // The return and heading share the article axis at every width. A
        // separate return row preserves the title's measure with enlarged text.
        let outline: Vec<(f64, String)> = preview
            .document
            .as_ref()
            .and_then(|document| document.summary.as_ref())
            .map(|summary| {
                summary
                    .outline
                    .iter()
                    .map(|item| (item.t, item.title.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let headings = reader_outline(&preview);
        let header = self.reader_metadata_row(&preview, compact, short_reader, &controls_scroll, cx);
        let mut compact_metadata = None;
        let (short_back, scrolling_title) = if short_reader {
            (Some(header.back), Some(header.title))
        } else {
            page = page.child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .items_start()
                    .gap_3()
                    .child(header.back)
                    .child(header.title),
            );
            (None, None)
        };
        if let Some(meta) = header.meta {
            if compact {
                compact_metadata = Some(meta);
            } else {
                page = page.child(meta);
            }
        }
        let toolbar = self.reader_toolbar(
            &preview,
            &headings,
            short_back,
            compact,
            short_reader,
            toc_fits_beside,
            cx,
        );

        page = page.child(
            v_flex()
                .w_full()
                .min_w_0()
                .gap_2()
                .py_2()
                .border_b_1()
                .border_color(color(HAIRLINE))
                .child(toolbar.bar)
                .children(toolbar.compact_search)
                .children(toolbar.short_matches),
        );
        let (fixed_header, mut page) = if compact {
            (
                Some(page.into_any_element()),
                v_flex()
                    .id("reader-extra-controls")
                    .w_full()
                    .min_w_0()
                    .min_h_0()
                    .flex_shrink_0()
                    .gap_2()
                    .max_h(px(available_height * 0.28))
                    .overflow_y_scroll()
                    .track_scroll(&controls_scroll),
            )
        } else {
            (None, page)
        };
        page = page.children(self.course_export_feedback(&preview.course, cx));
        if self.reading {
            page = page.child(crate::motion::enter(
                "reader-version-loading",
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(crate::motion::spinner("reader-version-spinner", cx))
                    .child(theme::accessible_text(
                        "reader-version-loading-label",
                        "正在打开笔记…",
                    ))));
        }
        let information_panel = self.reader_info_panel(
            &preview,
            compact,
            compact_metadata,
            available_height,
            rem_size,
            window,
            cx,
        );
        page = page.children(self.reader_notices(&preview, &controls_scroll, window, cx));
        let reading_note = self.result_tab == 0;
        let items = note_items(&preview.blocks, short_reader);
        // The note flow's persistent list state follows the loaded note and the
        // item sequence derived from it; text-scale changes only re-measure.
        let items_key = (
            preview.course.dir.clone(),
            short_reader,
            preview.blocks.len(),
        );
        if self.reader_ui.note_items_key.as_ref() != Some(&items_key) {
            let focus: Vec<FocusHandle> = (0..items.len()).map(|_| cx.focus_handle()).collect();
            self.reader_ui.note_list.reset(items.len());
            self.reader_ui
                .note_list
                .splice_focusable(0..items.len(), focus.iter().cloned().map(Some));
            self.reader_ui.note_focus = Rc::new(focus);
            self.reader_ui.note_items_key = Some(items_key);
            self.reader_ui.note_list_rem = rem_size;
        } else if self.reader_ui.note_list_rem != rem_size {
            self.reader_ui.note_list.remeasure();
            self.reader_ui.note_list_rem = rem_size;
        }
        self.reader_ui.note_items = Rc::new(items);
        let top_index = if reading_note {
            let top = self.reader_ui.note_list.logical_scroll_top();
            note_top_block(&self.reader_ui.note_items, top.item_ix).unwrap_or(0)
        } else {
            self.reader_ui
                .item_layout
                .borrow()
                .top_item(f32::from(self.reader_scroll.offset().y))
                .map(|(index, _, _)| index)
                .unwrap_or(0)
        };
        self.reader_ui.item_layout = Rc::default();
        let item_layout = self.reader_ui.item_layout.clone();
        let current_chapter = headings
            .iter()
            .filter(|(index, _, _)| *index <= top_index)
            .last()
            .map(|(index, _, _)| *index);
        // Match the exported HTML's continuous article: one text measure,
        // regular 1.7-line body copy, and spacing between sections. The note
        // tab's blocks form a variable-height list: only the visible window
        // and its overdraw are built each frame.
        let gallery_query = self.reader_ui.gallery_find.read(cx).value().to_string();
        let article: AnyElement = if reading_note {
            // One pass over the frames up front; the item closure queries per
            // image path and per body anchor instead of rescanning the frames.
            let frame_index_by_path: HashMap<PathBuf, usize> = self
                .reader_ui
                .frames
                .iter()
                .enumerate()
                .filter_map(|(index, frame)| frame.path.clone().map(|path| (path, index)))
                .fold(HashMap::new(), |mut map, (path, index)| {
                    map.entry(path).or_insert(index);
                    map
                });
            let mut missing_by_anchor: HashMap<String, Vec<usize>> = HashMap::new();
            for (index, frame) in self.reader_ui.frames.iter().enumerate() {
                if frame.path.is_none()
                    && let Some(anchor) = frame.body_anchor.clone()
                {
                    missing_by_anchor.entry(anchor).or_default().push(index);
                }
            }
            let flow = NoteFlow {
                items: self.reader_ui.note_items.clone(),
                focus: self.reader_ui.note_focus.clone(),
                preview: preview.clone(),
                frames: self.reader_ui.frames.clone(),
                source: self.reader_source(),
                outline,
                frame_index_by_path,
                missing_by_anchor,
                matches: self.reader_ui.matches.clone(),
                match_index: self.reader_ui.match_index,
                find_open: self.reader_ui.find_open,
                data_loading: self.reader_ui.data_loading,
                item_layout,
                note_list: self.reader_ui.note_list.clone(),
                desktop: cx.weak_entity(),
            };
            div()
                .id("note-reader")
                .role(Role::Document)
                .aria_label(preview.course.title.clone())
                .relative()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .w_full()
                .h_full()
                .max_w(READER_MEASURE)
                .child(
                    list(
                        self.reader_ui.note_list.clone(),
                        move |index, window, _cx| render_note_item(&flow, index, window),
                    )
                    .w_full()
                    .h_full()
                    .py_3(),
                )
                // 正文滚动容器也兑现「始终显示滚动条」偏好（review4#4）
                .child(crate::backend::vertical_scrollbar_for(&self.reader_ui.note_list))
                .into_any_element()
        } else {
            self.reader_gallery(
                &preview,
                &gallery_query,
                reader_width,
                rem_size,
                item_layout,
                scrolling_title,
                window,
                cx,
            )
        };
        // Wide reading opens the contents automatically. Manual choices are retained
        // across tab changes, note changes and subsequent window resizing.
        let toc_open = self.reader_ui.toc_open.unwrap_or(toc_fits_beside)
            && !headings.is_empty()
            && self.result_tab == 0;
        let toc_side = toc_open && toc_fits_beside;
        let body = if toc_side {
            h_flex()
                .flex_1()
                .min_h_0()
                .w_full()
                .gap(px(24.))
                .items_stretch()
                .child(article)
                .child(self.reader_toc_panel(&headings, current_chapter, true, cx))
                .into_any_element()
        } else if toc_open {
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .w_full()
                .h_full()
                .child(article)
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .bg(color(SURFACE))
                        .child(self.reader_toc_panel(&headings, current_chapter, false, cx)),
                )
                .into_any_element()
        } else {
            article
        };
        let controls = if let Some(header) = fixed_header {
            v_flex()
                .w_full()
                .min_h_0()
                .flex_shrink_0()
                .child(header)
                .child(page)
                .into_any_element()
        } else {
            page.into_any_element()
        };
        let controls = if self.reader_ui.info_open {
            v_flex()
                .w_full()
                .min_w_0()
                .flex_shrink_0()
                .gap_2()
                .child(controls)
                .children(information_panel)
                .into_any_element()
        } else {
            controls
        };
        crate::motion::state_enter(
            SharedString::from(format!("reader-open:{}", preview.course.dir.display())),
            root.child(controls)
                .child(v_flex().flex_1().min_h_0().w_full().child(body)))
    }
}

struct ReaderHeaderParts {
    back: AnyElement,
    title: AnyElement,
    meta: Option<AnyElement>,
}

struct ReaderToolbarParts {
    bar: Div,
    compact_search: Option<Div>,
    short_matches: Option<Div>,
}

impl Desktop {
    /// Return row, title and the meta facts line. Compact layouts get the meta
    /// line back for the information panel instead of an inline row.
    fn reader_metadata_row(
        &mut self,
        preview: &Rc<notes::Preview>,
        compact: bool,
        short_reader: bool,
        controls_scroll: &ScrollHandle,
        cx: &mut Context<Self>,
    ) -> ReaderHeaderParts {
        let reveal = |id: &'static str, child: AnyElement| {
            if compact {
                child
            } else {
                crate::focus_scroll::RevealFocus::new(id, child, controls_scroll.clone())
                    .inline()
                    .into_any_element()
            }
        };
        let return_label = match self.result_origin {
            Page::New => "工作台",
            Page::Task => "任务",
            Page::Settings => "设置",
            _ => "我的笔记",
        };
        let title = theme::accessible_text("reader-title", preview.course.title.clone())
            .role(Role::Heading)
            .when(!short_reader, |title| title.flex_1())
            .when(short_reader, |title| title.flex_shrink_0())
            .min_w_0()
            .whitespace_normal()
            .when(compact && !short_reader, |title| {
                title.whitespace_nowrap().text_ellipsis()
            })
            .text_size(if compact { TEXT_TITLE } else { TEXT_DISPLAY })
            .font_weight(FontWeight::SEMIBOLD);
        let title = if compact && !short_reader {
            control("reader-title-details")
                .ghost()
                .flex_1()
                .min_w_0()
                .h_auto()
                .p_0()
                .justify_start()
                .tooltip(preview.course.title.clone())
                .accessibility_label("查看完整标题与笔记详情")
                .child(title)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.reader_ui.info_open = !this.reader_ui.info_open;
                    cx.notify();
                }))
                .into_any_element()
        } else {
            title.into_any_element()
        };
        let back = reveal(
            "reveal-reader-back",
            quiet("reader-back")
                .icon(IconName::ArrowLeft)
                .w(CONTROL_HEIGHT)
                .px_0()
                .accessibility_label(format!("返回{return_label}"))
                .tooltip(format!("返回{return_label}"))
                .on_click(cx.listener(|this, _, _, cx| this.navigate(this.result_origin, cx)))
                .into_any_element(),
        );
        // Meta line 1: 作者 · 时长 · 平台 BV 号 · 打开原视频；无数据的字段省略。
        let mut facts: Vec<String> = Vec::new();
        for (label, value) in &preview.metadata {
            if label == "来源" || value.trim().is_empty() {
                continue;
            }
            facts.push(value.clone());
        }
        if let Some(document) = &preview.document {
            let platform = match document.meta.extractor.as_str() {
                "local" => "本地视频".to_owned(),
                "bilibili" => "Bilibili".to_owned(),
                "youtube" => "YouTube".to_owned(),
                "" => String::new(),
                other => {
                    let mut chars = other.chars();
                    let name = chars
                        .next()
                        .map(|first| first.to_uppercase().collect::<String>())
                        .unwrap_or_default()
                        + chars.as_str();
                    name
                }
            };
            if !platform.is_empty() {
                facts.push(platform);
            }
        }
        let mut meta_facts = h_flex().gap_2().flex_wrap().items_center();
        if !facts.is_empty() {
            meta_facts = meta_facts.child(
                theme::accessible_text("reader-facts", facts.join(" · "))
                    .text_size(TEXT_AUX)
                    .text_color(color(GRAY)),
            );
        }
        if let Some(source) = self.reader_source() {
            match &source {
                nav::SourceTarget::Web(url) => {
                    meta_facts = meta_facts.child(reveal(
                        "reveal-reader-source",
                        (quiet("reader-source")
                            .icon(icons::external_link())
                            .label("打开原视频")
                            .min_h(rems(1.6))
                            .accessibility_label(format!("打开原视频：{url}"))
                            .on_click(
                                cx.listener(|this, _, _, cx| this.open_reader_source(None, cx)),
                            ))
                        .into_any_element(),
                    ));
                }
                nav::SourceTarget::Local(path) => {
                    let exists = self.reader_ui.source_available;
                    meta_facts = meta_facts
                        .when(exists, |row| {
                            row.child(reveal(
                                "reveal-reader-source",
                                quiet("reader-source")
                                    .icon(icons::movie())
                                    .label("打开原视频")
                                    .tooltip(path.display().to_string())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.open_reader_source(None, cx)
                                    }))
                                    .into_any_element(),
                            ))
                        })
                        .when(!exists, |row| {
                            row.child(
                                theme::accessible_text(
                                    "missing-original-video",
                                    "原视频暂不可用，笔记仍可阅读",
                                )
                                .text_size(TEXT_AUX)
                                .text_color(color(GRAY)),
                            )
                            .child(reveal(
                                "reveal-relocate-reader-source",
                                quiet("relocate-reader-source")
                                    .icon(IconName::FolderOpen)
                                    .label(if self.reader_ui.source_loading {
                                        "正在核对视频…"
                                    } else {
                                        "重新定位原视频"
                                    })
                                    .loading(self.reader_ui.source_loading)
                                    .disabled(self.reader_ui.source_loading)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.relocate_reader_source(window, cx)
                                    }))
                                    .into_any_element(),
                            ))
                        });
                }
            }
        }
        match &self.reader_ui.offline_video {
            OfflineVideo::Available(_) => {
                meta_facts = meta_facts.child(reveal(
                    "reveal-reader-offline-video",
                    quiet("reader-offline-video")
                        .icon(icons::movie())
                        .label("播放离线视频")
                        .loading(self.reader_ui.offline_opening)
                        .disabled(self.reader_ui.offline_opening)
                        .on_click(cx.listener(|this, _, _, cx| this.play_reader_offline_video(cx)))
                        .into_any_element(),
                ));
            }
            OfflineVideo::Missing => {
                meta_facts = meta_facts
                    .child(
                        theme::accessible_text(
                            "reader-offline-video-missing",
                            "此版本的离线视频暂不可用，仍可打开原视频网页。",
                        )
                        .text_size(TEXT_AUX)
                        .text_color(color(MUTED)),
                    )
                    .child(reveal(
                        "reveal-retry-reader-offline-video",
                        quiet("retry-reader-offline-video")
                            .icon(icons::refresh())
                            .label("重试播放离线视频")
                            .loading(self.reader_ui.offline_opening)
                            .disabled(self.reader_ui.offline_opening)
                            .on_click(
                                cx.listener(|this, _, _, cx| this.play_reader_offline_video(cx)),
                            )
                            .into_any_element(),
                    ));
            }
            OfflineVideo::NotRequested => {}
        }
        // A single version needs no selector. Keep its revision and date in
        // generation information; multiple versions retain a visible choice.
        if let Some(manifest) = &preview.course.manifest {
            meta_facts = meta_facts.child(div().flex_1());
            if self.reader_ui.versions.len() > 1 {
                let weak = cx.weak_entity();
                let versions = self.reader_ui.versions.clone();
                let current = preview.course.dir.clone();
                meta_facts = meta_facts.child(reveal(
                    "reveal-choose-note-version",
                    (quiet("choose-note-version")
                        .icon(icons::history())
                        .label(format!("版本 {}", manifest.revision))
                        .tooltip("选择笔记版本")
                        .child(Icon::new(IconName::ChevronDown).size_4().flex_shrink_0())
                        .min_h(rems(1.6))
                        .disabled(self.reading)
                        .dropdown_menu(move |menu, _, _| {
                            let mut menu = menu;
                            for version in &versions {
                                let weak = weak.clone();
                                let course = version.course.clone();
                                let label = if course.dir == current {
                                    format!("{}（当前）", version.label)
                                } else {
                                    version.label.clone()
                                };
                                menu = menu.item(
                                    PopupMenuItem::new(label)
                                        .icon(icons::history())
                                        .checked(course.dir == current)
                                        .on_click(move |_, _, cx| {
                                            let _ = weak.update(cx, |this, cx| {
                                                this.open_reader_version(course.clone(), cx)
                                            });
                                        }),
                                );
                            }
                            menu
                        }))
                    .into_any_element(),
                ));
            }
        }
        let meta = if !facts.is_empty()
            || self.reader_source().is_some()
            || self.reader_ui.offline_video != OfflineVideo::NotRequested
            || preview.course.manifest.is_some()
        {
            Some(meta_facts.into_any_element())
        } else {
            None
        };
        ReaderHeaderParts {
            back,
            title,
            meta,
        }
    }

    /// Find field, match navigation, view switcher, contents toggle, export
    /// menu and the overflow menu.
    fn reader_toolbar(
        &mut self,
        preview: &Rc<notes::Preview>,
        headings: &[(usize, String, Option<f64>)],
        short_back: Option<AnyElement>,
        compact: bool,
        short_reader: bool,
        toc_fits_beside: bool,
        cx: &mut Context<Self>,
    ) -> ReaderToolbarParts {
        let note_query = self.reader_ui.find.read(cx).value().to_string();
        let gallery_query = self.reader_ui.gallery_find.read(cx).value().to_string();
        let gallery_indices = matching_frames(&self.reader_ui.frames, &gallery_query);
        let reading_note = self.result_tab == 0;
        let active_query = if reading_note {
            &note_query
        } else {
            &gallery_query
        };
        let input = if reading_note {
            self.reader_ui.find.clone()
        } else {
            self.reader_ui.gallery_find.clone()
        };
        let mut search = h_flex()
            .min_w_0()
            .flex_1()
            .when(!short_reader, |row| row.flex_basis(rems(18.)))
            .gap_1()
            .items_center()
            .child(
                div().flex_1().min_w_0().child(
                    text_input(&input)
                        .w_full()
                        .aria_label(if reading_note {
                            "查找笔记内容"
                        } else {
                            "搜索截图对应文字"
                        })
                        .prefix(
                            icons::search()
                                .size(rems(18. / 14.))
                                .text_color(color(GRAY)),
                        )
                        .cleanable(true),
                ),
            );
        let mut short_matches = None;
        if !active_query.trim().is_empty() {
            let count = if reading_note {
                self.reader_ui.matches.len()
            } else {
                gallery_indices.len()
            };
            let mut matches = h_flex().gap_1().items_center().child(
                theme::accessible_text(
                    "find-status",
                    if reading_note && count > 0 {
                        format!("{}/{count}", self.reader_ui.match_index + 1)
                    } else if reading_note {
                        "无匹配".into()
                    } else {
                        format!("{count} 张")
                    },
                )
                .flex_shrink_0()
                .whitespace_nowrap()
                .text_size(TEXT_AUX)
                .text_color(color(GRAY)),
            );
            if reading_note {
                for (id, label, icon, delta) in [
                    ("previous-match", "上一处", IconName::ArrowUp, -1),
                    ("next-match", "下一处", IconName::ArrowDown, 1),
                ] {
                    matches =
                        matches.child(
                            quiet(id)
                                .icon(icon)
                                .w(CONTROL_HEIGHT)
                                .px_0()
                                .accessibility_label(label)
                                .tooltip(label)
                                .disabled(count == 0)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.move_reader_match(delta, cx)
                                })),
                        );
                }
            }
            if short_reader {
                short_matches = Some(matches.w_full().justify_end());
            } else {
                search = search.child(matches);
            }
        }
        let mut toolbar = h_flex()
            .w_full()
            .min_w_0()
            .gap_2()
            .when(!compact, |row| row.flex_wrap())
            .items_center()
            .children(short_back)
            .when(!short_reader, |row| {
                row.child(
                    SingleChoiceGroup::new("reader-tabs", "阅读视图")
                        .tabs()
                        .options([("note", "笔记"), ("images", "截图")])
                        .icon("note", icons::article())
                        .icon("images", icons::image())
                        .focus_handles(self.reader_ui.view_focus.iter().cloned())
                        .selected(if reading_note { "note" } else { "images" })
                        .on_change(cx.listener(|this, selected: &SharedString, window, cx| {
                            this.select_reader_view(
                                usize::from(selected.as_ref() == "images"),
                                window,
                                cx,
                            );
                        })),
                )
            });
        let mut search_row = h_flex()
            .flex_1()
            .min_w_0()
            .gap_2()
            .items_center()
            .child(search);
        if !headings.is_empty() && reading_note {
            let toc_on = self.reader_ui.toc_open.unwrap_or(toc_fits_beside);
            search_row = search_row.child(
                // 目录轨打开时以 accent 文字色表达状态（持久含义经由文字色，不是选中面）
                quiet("note-contents")
                    .icon(icons::toc())
                    .label("目录")
                    .when(toc_on, |button| button.text_color(color(ACCENT_STRONG)))
                    .accessibility_label(if toc_on {
                        "收起目录"
                    } else {
                        "打开目录"
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.reader_ui.pending_restore = this.capture_reading_position();
                        this.reader_ui.restore_generation += 1;
                        this.reader_ui.toc_open = Some(!toc_on);
                        cx.notify();
                    })),
            );
        }
        let (compact_search, short_search) = if short_reader {
            (None, Some(search_row))
        } else if compact {
            toolbar = toolbar.child(div().flex_1());
            (Some(search_row.w_full()), None)
        } else {
            toolbar = toolbar.child(search_row);
            (None, None)
        };
        let output_weak = cx.entity().downgrade();
        let exporting = self.exporting;
        let exporting_current = self
            .reader_ui
            .export_state
            .pending
            .as_ref()
            .is_some_and(|(_, source)| *source == preview.course.dir);
        let export_course = preview.course.clone();
        let export_folder = self.reader_ui.export_folder.clone();
        if !short_reader {
            toolbar = toolbar.child(
                outline_pill("export-note")
                    .icon(icons::download())
                    .label(if exporting_current {
                        "正在导出…"
                    } else {
                        "导出"
                    })
                    .tooltip("复制或导出整份笔记")
                    .accessibility_label("复制或导出整份笔记")
                    .child(Icon::new(IconName::ChevronDown).size_4().flex_shrink_0())
                    .loading(exporting_current)
                    .disabled(exporting)
                    .dropdown_menu(move |menu, _, _| {
                        reader_export_items(
                            menu,
                            output_weak.clone(),
                            export_course.clone(),
                            export_folder.clone(),
                            exporting,
                        )
                    }),
            );
        }
        let more_weak = cx.weak_entity();
        let original = self.reader_source();
        let has_source = original.is_some();
        let missing_local = matches!(original, Some(nav::SourceTarget::Local(_)))
            && !self.reader_ui.source_available;
        let exported = self.reader_ui.exports.get(&preview.course.dir).cloned();
        let short_export_course = preview.course.clone();
        let short_export_folder = self.reader_ui.export_folder.clone();
        toolbar = toolbar
            .child(
                quiet("reader-more")
                    .when(short_reader, |button| {
                        button.track_focus(&self.reader_ui.menu_focus)
                    })
                    .icon(if short_reader {
                        if reading_note {
                            icons::article()
                        } else {
                            icons::image()
                        }
                    } else {
                        icons::ellipsis()
                    })
                    .label(if short_reader {
                        if reading_note { "笔记" } else { "截图" }
                    } else {
                        "更多"
                    })
                    .when(short_reader, |button| {
                        button.child(Icon::new(IconName::ChevronDown).size_4().flex_shrink_0())
                    })
                    .tooltip(if short_reader {
                        "切换视图、导出与笔记详情"
                    } else {
                        "更多笔记操作"
                    })
                    .accessibility_label(if short_reader {
                        "阅读视图与笔记操作"
                    } else {
                        "更多笔记操作"
                    })
                    .dropdown_menu(move |menu, window, cx| {
                        let details = more_weak.clone();
                        let source = more_weak.clone();
                        let files = more_weak.clone();
                        let mut menu = menu;
                        if short_reader {
                            for (index, label, icon) in
                                [(0, "笔记", icons::article()), (1, "截图", icons::image())]
                            {
                                let weak = more_weak.clone();
                                menu = menu.item(
                                    PopupMenuItem::new(label)
                                        .icon(icon)
                                        .checked((index == 0) == reading_note)
                                        .on_click(move |_, window, cx| {
                                            let _ = weak.update(cx, |this, cx| {
                                                this.select_reader_view(index, window, cx);
                                                this.reader_ui.menu_focus.focus(window, cx);
                                            });
                                        }),
                                );
                            }
                            let output = more_weak.clone();
                            let course = short_export_course.clone();
                            let folder = short_export_folder.clone();
                            menu = menu
                                .separator()
                                .submenu_with_icon(
                                    Some(icons::download()),
                                    "复制与导出",
                                    window,
                                    cx,
                                    move |menu, _, _| {
                                        reader_export_items(
                                            menu,
                                            output.clone(),
                                            course.clone(),
                                            folder.clone(),
                                            exporting,
                                        )
                                    },
                                )
                                .separator();
                        }
                        menu = menu
                            .item(PopupMenuItem::new("笔记详情").icon(icons::info()).on_click(
                                move |_, _, cx| {
                                    let _ = details.update(cx, |this, cx| {
                                        this.reader_ui.info_open = !this.reader_ui.info_open;
                                        cx.notify();
                                    });
                                },
                            ))
                            .when(has_source, |menu| {
                                menu.item(
                                    PopupMenuItem::new(if missing_local {
                                        "定位原视频…"
                                    } else {
                                        "打开原视频"
                                    })
                                    .icon(icons::movie())
                                    .on_click(
                                        move |_, window, cx| {
                                            let _ = source.update(cx, |this, cx| {
                                                if missing_local {
                                                    this.relocate_reader_source(window, cx);
                                                } else {
                                                    this.open_reader_source(None, cx);
                                                }
                                            });
                                        },
                                    ),
                                )
                            })
                            .separator()
                            .item(
                                PopupMenuItem::new("显示笔记保存位置")
                                    .icon(icons::folder_open())
                                    .on_click(move |_, _, cx| {
                                        let _ = files.update(cx, |this, cx| {
                                            if let Some(preview) = &this.preview {
                                                cx.reveal_path(&preview.course.dir);
                                            }
                                        });
                                    }),
                            );
                        if let Some(path) = exported.clone() {
                            menu = menu.item(
                                PopupMenuItem::new("显示上次导出的文件")
                                    .icon(icons::folder_open())
                                    .on_click(move |_, _, cx| cx.reveal_path(&path)),
                            );
                        }
                        menu
                    }),
            )
            .children(short_search);
        ReaderToolbarParts {
            bar: toolbar,
            compact_search,
            short_matches,
        }
    }

    /// Collapsible note details: revision, source video, subtitle and model facts.
    fn reader_info_panel(
        &mut self,
        preview: &Rc<notes::Preview>,
        compact: bool,
        compact_metadata: Option<AnyElement>,
        available_height: f32,
        rem_size: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let mut details = vec![("标题", icons::article(), preview.course.title.clone())];
        let local_source = self.reader_source().and_then(|source| match source {
            nav::SourceTarget::Local(path) => Some(path),
            _ => None,
        });
        if let Some(manifest) = &preview.course.manifest {
            let stamp = nav::timestamp_local(manifest.created_at_ms);
            let date = stamp.get(..10).unwrap_or(&stamp);
            details.push((
                "版本",
                icons::history(),
                format!("{} · {date} 生成", manifest.revision),
            ));
        }
        if let Some(path) = &local_source {
            details.push((
                "原视频",
                icons::movie(),
                path.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
            ));
        }
        if let Some(manifest) = &preview.course.manifest {
            if let Some(task) = self.workspace.as_ref().and_then(|workspace| {
                workspace
                    .state
                    .tasks
                    .iter()
                    .find(|task| task.id == manifest.task_id)
            }) {
                if let Some(subtitle) = &task.plan.source.selected_subtitle {
                    details.push(("所选字幕", icons::subtitles(), subtitle.label.clone()));
                }
                if task.plan.config.defaults.transcript_source
                    == Some(course2md::config::TranscriptSource::Asr)
                {
                    if let Some(model) = &task.plan.config.defaults.asr_model {
                        details.push(("语音识别模型", icons::mic(), model.clone()));
                    }
                    if let Some(provider) = task.plan.config.defaults.provider {
                        details.push(("识别设备", icons::computer(), provider.as_str().to_owned()));
                    }
                }
                if task.plan.options.llm || task.plan.options.summarize {
                    details.push((
                        "AI 模型",
                        icons::auto_fix(),
                        task.plan.config.llm.model.clone(),
                    ));
                }
            }
        }
        if !details.is_empty() || compact_metadata.is_some() {
            let information_scroll = self.reader_ui.information_scroll.clone();
            let frame_height = (available_height * 0.42).min(rem_size * 16.);
            let padding = if compact {
                rem_size * 0.5
            } else {
                rem_size * 0.75
            };
            let viewport_height = (frame_height
                - f32::from(CONTROL_HEIGHT.to_pixels(window.rem_size()))
                - padding * 2.
                - rem_size * 0.5
                - 2.)
                .max(rem_size * 2.);
            let mut content =
                v_flex()
                    .w_full()
                    .min_w_0()
                    .gap_2()
                    .pr(px(14.))
                    .children(details.into_iter().enumerate().map(
                        |(index, (label, icon, value))| {
                            detail_row(
                                ("reader-info-label", index),
                                label,
                                icon,
                                theme::accessible_text(("reader-info-value", index), value)
                                    .whitespace_normal()
                                    .text_size(TEXT_BODY)
                                    .font_weight(FontWeight::NORMAL),
                            )
                            .flex_shrink_0()
                        },
                    ))
                    .children(compact_metadata);
            if local_source.is_some() && self.reader_ui.source_available {
                content = content.child(
                    crate::focus_scroll::RevealFocus::new(
                        "reveal-relocate-reader-source-detail",
                        quiet("relocate-reader-source-detail")
                            .icon(IconName::FolderOpen)
                            .label(if self.reader_ui.source_loading {
                                "正在核对视频…"
                            } else {
                                "重新定位原视频"
                            })
                            .loading(self.reader_ui.source_loading)
                            .disabled(self.reader_ui.source_loading)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.relocate_reader_source(window, cx)
                            }))
                            .into_any_element(),
                        information_scroll.clone(),
                    )
                    .inline(),
                );
            }
            Some(disclosure(
                "reader-information-panel",
                self.reader_ui.info_open,
                v_flex()
                    .w_full()
                    .min_w_0()
                    .flex_shrink_0()
                    .gap_2()
                    .p_3()
                    .when(compact, |panel| panel.p_2())
                    .bg(color(INSET))
                    .border_1()
                    .border_color(color(HAIRLINE))
                    .rounded(RADIUS_CARD)
                    .child(
                        h_flex()
                            .w_full()
                            .min_w_0()
                            .items_center()
                            .gap_2()
                            .child(semantic_label(
                                "reader-information-title",
                                "笔记详情",
                                Icon::new(IconName::Info),
                            ))
                            .child(div().flex_1())
                            .child(
                                quiet("collapse-reader-information")
                                    .icon(IconName::ChevronUp)
                                    .label("收起")
                                    .accessibility_label("收起笔记详情")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.reader_ui.info_open = false;
                                        this.reader_ui.focus.focus(window, cx);
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .relative()
                            .w_full()
                            .min_w_0()
                            .child(
                                div()
                                    .id("reader-information-scroll")
                                    .w_full()
                                    .min_w_0()
                                    .max_h(px(viewport_height))
                                    .overflow_y_scroll()
                                    .track_scroll(&information_scroll)
                                    .child(content),
                            )
                            // 与设置页一致：macOS「始终显示滚动条」偏好开启时常驻（review4#4）
                            .child(crate::backend::vertical_scrollbar(&information_scroll)),
                    ),
                window,
                cx,
            ).into_any_element())
        } else {
            None
        }
    }

    /// New-version, incomplete-processing and file-reload notices.
    fn reader_notices(
        &mut self,
        preview: &Rc<notes::Preview>,
        controls_scroll: &ScrollHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let reveal = |id: &'static str, child: AnyElement| {
            crate::focus_scroll::RevealFocus::new(id, child, controls_scroll.clone()).inline()
        };
        let mut notices = Vec::new();
        if let Some(current) = &preview.course.manifest {
            if let Some(newer) = self
                .courses
                .iter()
                .find(|course| {
                    course.manifest.as_ref().is_some_and(|m| {
                        m.course_id == current.course_id && m.revision > current.revision
                    })
                })
                .cloned()
            {
                notices.push(crate::motion::enter(
                    "reader-new-version-notice",
                    h_flex()
                        .gap_2()
                        .items_center()
                        .flex_wrap()
                        .p_3()
                        .bg(color(ACCENT_SOFT))
                        .rounded(RADIUS_CARD)
                        .child(
                            icons::history()
                                .size(px(18.))
                                .text_color(color(ACCENT_STRONG)),
                        )
                        .child(theme::accessible_text(
                            "newer-note-version",
                            "这份笔记已有新版。",
                        ))
                        .child(reveal(
                            "reveal-open-newer-version",
                            (outline_pill("open-newer-version")
                                .icon(icons::history())
                                .label("查看新版")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.open_reader_version(newer.clone(), cx)
                                })))
                            .into_any_element(),
                        ))));
            }
        }
        if let Some(notice) = processing_notice(&preview.processing_issues) {
            let mut problem = v_flex()
                .gap_3()
                .p_4()
                .bg(color(WARNING_BG))
                .border_1()
                .border_color(color(WARNING_BG))
                .rounded(RADIUS_CARD)
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .flex_wrap()
                        .child(badge(BadgeKind::Warning).child("需要处理"))
                        .child(
                            theme::accessible_text("reader-incomplete-processing", notice)
                                .text_sm(),
                        ),
                );
            let task_id = preview
                .course
                .manifest
                .as_ref()
                .map(|manifest| manifest.task_id.clone())
                .filter(|id| {
                    self.workspace.as_ref().is_some_and(|workspace| {
                        workspace.state.tasks.iter().any(|task| &task.id == id)
                    })
                });
            let mut actions = h_flex().gap_2().flex_wrap();
            if let Some(id) = task_id {
                let task = self
                    .workspace
                    .as_ref()
                    .and_then(|workspace| workspace.state.tasks.iter().find(|task| task.id == id));
                let requires_repair =
                    task.is_some_and(crate::task_ui::task_requires_service_repair);
                let can_retry = task.is_some_and(|task| {
                    task.handled_by.is_none()
                        && !matches!(
                            task.state,
                            workspace::TaskState::Running
                                | workspace::TaskState::Pausing
                                | workspace::TaskState::Queued
                                | workspace::TaskState::Uncertain
                        )
                        && !task
                            .blocked
                            .iter()
                            .any(|blocked| blocked.reason == course2md::dispatch::REASON_UNCERTAIN)
                        && task.artifact.as_ref() == Some(&preview.course.dir)
                });
                let components = preview
                    .processing_issues
                    .iter()
                    .filter_map(|issue| match issue.stage {
                        notes::ProcessingStage::Screenshots => Some("screenshots".to_owned()),
                        notes::ProcessingStage::Proofreading if !requires_repair => {
                            Some("proofreading".to_owned())
                        }
                        notes::ProcessingStage::Summary if !requires_repair => {
                            Some("summary".to_owned())
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                if can_retry && !components.is_empty() {
                    let retry = id.clone();
                    let label = if components.len() > 1 {
                        "补全未完成部分"
                    } else {
                        match components[0].as_str() {
                            "screenshots" => "补生成截图",
                            "proofreading" => "重试 AI 校对",
                            _ => "补生成摘要",
                        }
                    };
                    actions = actions.child(
                        outline_pill("reader-retry-incomplete")
                            .icon(icons::refresh())
                            .label(label)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.save_reading_position(cx);
                                this.reprocess_task(
                                    retry.clone(),
                                    components.clone(),
                                    Vec::new(),
                                    cx,
                                );
                            })),
                    );
                }
                if can_retry
                    && preview.processing_issues.iter().any(|issue| {
                        matches!(
                            issue.stage,
                            notes::ProcessingStage::Proofreading | notes::ProcessingStage::Summary
                        )
                    })
                {
                    let repair = id.clone();
                    actions = actions.child(
                        outline_pill("reader-repair-ai")
                            .icon(icons::settings())
                            .label("修复 AI 服务并补做")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.save_reading_position(cx);
                                this.repair_task_service(repair.clone(), window, cx);
                            })),
                    );
                }
                actions = actions.child(reveal(
                    "reveal-reader-view-incomplete-task",
                    (quiet("reader-view-incomplete-task")
                        .icon(icons::task())
                        .label("查看任务")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.save_reading_position(cx);
                            this.select_task(&id, cx);
                            this.page = Page::Task;
                            cx.notify();
                        })))
                    .into_any_element(),
                ));
            }
            let has_details = preview.processing_issues.iter().any(|issue| {
                issue
                    .outcome
                    .message
                    .as_ref()
                    .is_some_and(|detail| !detail.trim().is_empty())
            });
            if has_details {
                actions = actions.child(reveal(
                    "reveal-reader-processing-details",
                    (quiet("reader-processing-details")
                        .icon(IconName::Info)
                        .label(if self.reader_ui.processing_details_open {
                            "收起处理详情"
                        } else {
                            "查看处理详情"
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.reader_ui.processing_details_open =
                                !this.reader_ui.processing_details_open;
                            cx.notify();
                        })))
                    .into_any_element(),
                ));
            }
            problem = problem.child(actions);
            let mut processing_details = v_flex().gap_2();
            for (index, issue) in preview.processing_issues.iter().enumerate() {
                if let Some(detail) = issue
                    .outcome
                    .message
                    .as_ref()
                    .filter(|detail| !detail.trim().is_empty())
                {
                    processing_details = processing_details
                        .child(
                            theme::accessible_text(
                                ("reader-processing-detail-label", index),
                                format!("{}的技术详情", issue.stage.label()),
                            )
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM),
                        )
                        .child(
                            theme::accessible_text(
                                ("reader-processing-detail", index),
                                detail.clone(),
                            )
                            .text_sm()
                            .text_color(color(MUTED)),
                        );
                }
            }
            problem = problem.child(disclosure(
                "reader-processing-detail-panel",
                self.reader_ui.processing_details_open,
                processing_details,
                window,
                cx,
            ));
            notices.push(crate::motion::enter(
                "reader-processing-notice",
                problem));
        }
        if files_need_reload(&preview, &self.reader_ui.issues, &self.reader_ui.frames) {
            let course = preview.course.clone();
            let issues = preview
                .issues
                .iter()
                .chain(self.reader_ui.issues.iter())
                .collect::<Vec<_>>();
            let messages = v_flex()
                .flex_1()
                .min_w(px(240.))
                .gap_2()
                .when(issues.is_empty(), |view| {
                    view.child(
                        theme::accessible_text(
                            "reader-unreadable-images",
                            "部分截图暂时无法读取，笔记正文仍可阅读。",
                        )
                        .text_sm()
                        .text_color(color(WARNING)),
                    )
                })
                .children(issues.into_iter().enumerate().map(|(index, issue)| {
                    theme::accessible_text(("reader-issue", index), issue.clone())
                        .text_sm()
                        .text_color(color(WARNING))
                }));
            notices.push(crate::motion::enter(
                "reader-file-notice",
                h_flex()
                    .w_full()
                    .min_w_0()
                    .items_center()
                    .flex_wrap()
                    .gap_3()
                    .p_3()
                    .bg(color(WARNING_BG))
                    .rounded(RADIUS_CARD)
                    .child(
                        icons::warning()
                            .size(px(20.))
                            .flex_shrink_0()
                            .text_color(color(WARNING)),
                    )
                    .child(messages)
                    .child(reveal(
                        "reveal-reload-reader-files",
                        (outline_pill("reload-reader-files")
                            .icon(icons::refresh())
                            .label("重新读取本版文件")
                            .loading(self.reading)
                            .disabled(self.reading)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.load_reader_version(course.clone(), true, cx)
                            })))
                        .into_any_element(),
                    ))));
        }
        notices
    }

    /// Screenshot grid: loading, empty and no-match states plus measured cards.
    fn reader_gallery(
        &mut self,
        preview: &Rc<notes::Preview>,
        gallery_query: &str,
        reader_width: f32,
        rem_size: f32,
        item_layout: Rc<std::cell::RefCell<nav::ReadingLayout>>,
        scrolling_title: Option<AnyElement>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let gallery_indices = matching_frames(&self.reader_ui.frames, gallery_query);
        let measured_scroll = self.reader_scroll.clone();
        let measured = |index: usize, child: AnyElement| {
            let positions = item_layout.clone();
            let scroll = measured_scroll.clone();
            div()
                .w_full()
                .min_w_0()
                .flex_shrink_0()
                .on_children_prepainted(move |bounds, _, _| {
                    if let Some(bounds) = bounds.first() {
                        positions.borrow_mut().record(
                            index,
                            f32::from(bounds.top() - scroll.bounds().top() - scroll.offset().y),
                            f32::from(bounds.size.height),
                        );
                    }
                })
                .child(child)
        };
        let mut article = v_flex()
            .id("note-reader")
            .role(Role::Document)
            .aria_label(preview.course.title.clone())
            .flex_1()
            .min_h_0()
            .min_w_0()
            .w_full()
            .h_full()
            .overflow_y_scroll()
            .track_scroll(&self.reader_scroll)
            .gap_3()
            .py_3()
            .children(scrolling_title);
        let article_scroll = self.reader_scroll.clone();
        let reveal_article = |id: ElementId, child: AnyElement, full_width: bool| {
            let view = crate::focus_scroll::RevealFocus::new(id, child, article_scroll.clone());
            if full_width { view } else { view.inline() }
        };
        if self.reader_ui.data_loading && self.reader_ui.frames.is_empty() {
            article = article.child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .py_6()
                    .child(crate::motion::spinner("reader-images-spinner", cx))
                    .child(theme::accessible_text("loading-images", "正在读取截图…")),
            );
        } else if self.reader_ui.frames.is_empty() {
            article = article.child(crate::motion::enter(
                "reader-images-empty",
                v_flex()
                    .w_full()
                    .py_8()
                    .px_6()
                    .gap_4()
                    .items_center()
                    .bg(color(INSET))
                    .rounded(RADIUS_CARD)
                    .child(icons::image().size(px(32.)).text_color(color(MUTED)))
                    .child(
                        theme::accessible_text("no-note-images", "这份笔记没有截图")
                            .text_size(TEXT_TITLE)
                            .font_weight(FontWeight::SEMIBOLD),
                    )
                    .child(
                        theme::accessible_text(
                            "no-note-images-reason",
                            if preview.processing_issues.iter().any(|issue| {
                                matches!(issue.stage, notes::ProcessingStage::Screenshots)
                            }) {
                                "截图生成未完成，文字内容仍可阅读。"
                            } else {
                                "本次没有生成截图，文字内容已保存在笔记中。"
                            },
                        )
                        .text_color(color(MUTED)),
                    )
                    .child(
                        outline_pill("empty-images-to-note")
                            .icon(icons::article())
                            .label("阅读笔记")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.select_reader_view(0, window, cx)
                            })),
                    )));
        }
        if !self.reader_ui.frames.is_empty() && gallery_indices.is_empty() {
            article = article.child(
                v_flex()
                    .w_full()
                    .py_8()
                    .gap_3()
                    .items_center()
                    .child(semantic_label(
                        "gallery-no-matches",
                        "没有对应的截图",
                        icons::search(),
                    ))
                    .child(
                        theme::accessible_text(
                            "gallery-no-matches-help",
                            "搜索范围是截图说明和对应文字。试试更短的关键词。",
                        )
                        .text_color(color(MUTED)),
                    )
                    .child(
                        quiet("clear-gallery-search")
                            .icon(icons::close())
                            .label("清除搜索")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.reader_ui
                                    .gallery_find
                                    .update(cx, |input, cx| input.set_value("", window, cx));
                            })),
                    ),
            );
        }
        // Each screenshot needs enough width for both the slide and a readable
        // text excerpt. The final row keeps the same column widths as the rest.
        let gallery_columns = ((reader_width + 16.) / (rem_size * 20. + 16.))
            .floor()
            .clamp(1., 3.) as u16;
        // GPUI rounds each rendered line to whole pixels. Reserve that same
        // three-line height so enlarged text cannot run into the action row.
        let excerpt_line_height =
            px((f32::from(TEXT_BODY.to_pixels(window.rem_size())) * 1.6).round());
        let mut grid = div()
            .grid()
            .grid_cols(gallery_columns)
            .w_full()
            .min_w_0()
            .flex_shrink_0()
            .gap(px(16.));
        for index in gallery_indices.iter().copied() {
            let frame = &self.reader_ui.frames[index];
            let label = frame_label(&preview.course.title, frame, index);
            let mut card = v_flex()
                .id(SharedString::from(frame.anchor.clone()))
                .w_full()
                .h_full()
                .min_w_0()
                .bg(color(SURFACE))
                .border_1()
                .border_color(color(CARD_LINE))
                .rounded(RADIUS_CARD)
                .overflow_hidden();
            if let Some(path) = &frame.path {
                card = card.child(reveal_article(
                    ("reveal-screenshot", index).into(),
                    (control(("screenshot", index))
                        .ghost()
                        .p_0()
                        .w_full()
                        .h_auto()
                        .min_h(px(0.))
                        .rounded_t(RADIUS_CARD)
                        .rounded_b(px(0.))
                        .aspect_ratio(16. / 9.)
                        .accessibility_label(format!("放大{label}"))
                        .child(
                            img(path.clone())
                                .size_full()
                                .rounded_t(RADIUS_CARD)
                                .object_fit(ObjectFit::Contain)
                                .with_fallback(move || {
                                    theme::accessible_text(
                                        ("failed-reader-image", index),
                                        "这张截图无法读取；对应正文仍可阅读。",
                                    )
                                    .into_any_element()
                                }),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_reader_image(index, window, cx)
                        })))
                    .into_any_element(),
                    true,
                ));
            } else {
                card = card.child(
                    v_flex()
                        .w_full()
                        .aspect_ratio(16. / 9.)
                        .justify_center()
                        .p_4()
                        .bg(color(INSET))
                        .child(
                            theme::accessible_text(
                                ("missing-frame", index),
                                "这张图片无法读取；可继续阅读对应正文。",
                            )
                            .text_size(TEXT_AUX)
                            .text_color(color(WARNING)),
                        ),
                );
            }
            let body_index = frame.body_anchor.as_ref().and_then(|anchor| {
                preview
                    .blocks
                    .iter()
                    .enumerate()
                    .position(|(i, block)| block_anchor(block, i) == *anchor)
            });
            let mut card_body = v_flex().flex_1().min_w_0().p_3().gap_2().child(
                h_flex()
                    .items_center()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        theme::accessible_text(
                            ("frame-title", index),
                            frame
                                .seconds
                                .map(course2md::render::fmt_ts)
                                .unwrap_or_else(|| format!("第 {} 张图片", index + 1)),
                        )
                        .role(Role::Heading)
                        .flex_1()
                        .text_size(TEXT_BODY)
                        .font_weight(FontWeight::SEMIBOLD),
                    )
                    .when_some(body_index, |row, body_index| {
                        row.child(reveal_article(
                            ("reveal-frame-to-body", index).into(),
                            quiet(("frame-to-body", index))
                                .icon(icons::article())
                                .label("正文")
                                .tooltip("在笔记中查看")
                                .accessibility_label("在笔记中查看")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.jump_reader_block(body_index, 0., cx)
                                }))
                                .into_any_element(),
                            false,
                        ))
                    }),
            );
            let excerpt = frame_excerpt(frame, &gallery_query);
            let excerpt_marks = nav::text_matches(&excerpt, &gallery_query)
                .into_iter()
                .map(|range| {
                    (
                        range,
                        HighlightStyle {
                            background_color: Some(color(FIND_HIGHLIGHT).into()),
                            ..Default::default()
                        },
                    )
                })
                .collect::<Vec<_>>();
            // The three-line preview is a defined content slot. Full captions
            // and transcripts remain readable in the image viewer and article.
            // 无搜索高亮时用原生省略渲染（StyledText 不产出「…」，静默裁切像断句错误，M9）
            card_body = card_body.child(
                div()
                    .id(("frame-excerpt", index))
                    .role(Role::Paragraph)
                    .aria_label(excerpt.clone())
                    .w_full()
                    .min_w_0()
                    .max_h(excerpt_line_height * 3.)
                    .flex_shrink_0()
                    .whitespace_normal()
                    .text_size(TEXT_BODY)
                    .font_weight(FontWeight::NORMAL)
                    .line_height(excerpt_line_height)
                    .text_color(color(GRAY))
                    .when(excerpt_marks.is_empty(), |view| {
                        view.text_ellipsis()
                            .line_clamp(3)
                            .child(excerpt.clone())
                    })
                    .when(!excerpt_marks.is_empty(), |view| {
                        view.child(StyledText::new(excerpt).with_highlights(excerpt_marks))
                    }),
            );
            grid =
                grid.child(measured(index, card.child(card_body).into_any_element()).h_full());
        }
        article = article.child(grid);
        // 正文滚动容器也兑现「始终显示滚动条」偏好（review4#4）
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .w_full()
            .h_full()
            .child(article)
            .child(crate::backend::vertical_scrollbar(&self.reader_scroll))
            .into_any_element()
    }

    /// Contents are a reading rail; persistent selection belongs to the row
    /// surface so pointer feedback cannot erase the current chapter.
    fn reader_toc_panel(
        &self,
        headings: &[(usize, String, Option<f64>)],
        current: Option<usize>,
        side: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let mut panel = v_flex()
            .id("note-toc")
            .gap_1()
            .py_3()
            .overflow_y_scroll()
            .track_scroll(&self.reader_ui.toc_scroll);
        panel = if side {
            panel
                .w(TOC_PANEL)
                .flex_shrink_0()
                .h_full()
                .min_h_0()
                .pl_4()
                .border_l_1()
                .border_color(color(HAIRLINE))
        } else {
            panel.w_full().h_full().min_h_0().px_3()
        };
        panel = panel.child(
            h_flex()
                .items_center()
                .gap_2()
                .flex_shrink_0()
                .mb_2()
                .child(semantic_label("toc-title", "目录", icons::toc()).flex_1())
                .when(!side, |row| {
                    row.child(
                        quiet("close-note-toc")
                            .icon(icons::close())
                            .label("关闭")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.reader_ui.toc_open = Some(false);
                                cx.notify();
                            })),
                    )
                }),
        );
        for (index, label, seconds) in headings {
            let index = *index;
            let on = current == Some(index);
            let trailing_time =
                seconds.filter(|seconds| *label != course2md::render::fmt_ts(*seconds));
            panel = panel.child(crate::focus_scroll::RevealFocus::new(
                ("reveal-toc-item", index),
                div()
                    .w_full()
                    .min_w_0()
                    .rounded(RADIUS_PILL)
                    .when(on, |row| row.bg(color(ACCENT_SOFT)))
                    .child(
                        control(("toc-item", index))
                            .ghost()
                            .w_full()
                            .h_auto()
                            .min_h(CONTROL_HEIGHT)
                            .py(px(8.))
                            .px(px(8.))
                            .justify_start()
                            .rounded(RADIUS_PILL)
                            .tooltip(label.clone())
                            .accessibility_label(if on {
                                format!("当前章节：{label}")
                            } else {
                                format!("转到 {label}")
                            })
                            .child(
                                h_flex()
                                    .w_full()
                                    .min_w_0()
                                    .gap_2()
                                    // 时间戳固定首行右栏；标题只在左列换行（M8）
                                    .items_start()
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .whitespace_nowrap()
                                            .text_ellipsis()
                                            .line_height(relative(1.4))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(color(if on { ACCENT_STRONG } else { INK }))
                                            .child(label.clone()),
                                    )
                                    .when_some(trailing_time, |row, seconds| {
                                        row.child(
                                            div()
                                                .flex_shrink_0()
                                                .text_size(TEXT_AUX)
                                                .text_color(color(if on {
                                                    ACCENT_STRONG
                                                } else {
                                                    GRAY
                                                }))
                                                .child(course2md::render::fmt_ts(seconds)),
                                        )
                                    }),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !side {
                                    this.reader_ui.toc_open = Some(false);
                                }
                                this.jump_reader_block(index, 0., cx);
                            })),
                    ),
                self.reader_ui.toc_scroll.clone(),
            ));
        }
        panel
    }
    fn open_reader_image(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(preview) = &self.preview else {
            return;
        };
        if self
            .reader_ui
            .frames
            .get(index)
            .is_none_or(|frame| frame.path.is_none())
        {
            return;
        }
        let focus = cx.focus_handle();
        let visible = if self.result_tab == 1 {
            matching_frames(
                &self.reader_ui.frames,
                &self.reader_ui.gallery_find.read(cx).value(),
            )
        } else {
            (0..self.reader_ui.frames.len()).collect()
        };
        let Some(viewer_index) = visible.iter().position(|candidate| *candidate == index) else {
            return;
        };
        crate::import_ui::ConversionFollow::interrupt_for_reader_viewer(
            &mut self.following_conversion,
        );
        self.reader_ui.viewer = Some(ImageViewer {
            scroll: ScrollHandle::new(),
            viewport_scroll: ScrollHandle::new(),
            viewport_size: size(px(0.), px(0.)),
            frames: visible
                .iter()
                .map(|index| self.reader_ui.frames[*index].clone())
                .collect(),
            filtered: self.result_tab == 1
                && !self
                    .reader_ui
                    .gallery_find
                    .read(cx)
                    .value()
                    .trim()
                    .is_empty(),
            index: viewer_index,
            title: preview.course.title.clone(),
            source: self.reader_source(),
            source_available: self.reader_ui.source_available,
            version: preview.course.dir.clone(),
            zoom: None,
            details_open: None,
            return_focus: window.focused(cx),
            focus: focus.clone(),
        });
        let desktop = cx.entity();
        let content = cx.new(|cx| ImageDialog {
            _observation: cx.observe(&desktop, |_, _, cx| cx.notify()),
            desktop: desktop.clone(),
            footer: false,
        });
        let footer = cx.new(|cx| ImageDialog {
            _observation: cx.observe(&desktop, |_, _, cx| cx.notify()),
            desktop,
            footer: true,
        });
        let weak = cx.weak_entity();
        window.open_dialog(cx, move |dialog, window, _| {
            let width = (f32::from(window.bounds().size.width) - 48.)
                .min(1180.)
                .max(280.);
            let closed = weak.clone();
            let content = content.clone();
            let top = task_dialog_top(window);
            dialog
                .w(px(width))
                .h((window.bounds().size.height - top - px(24.)).max(px(0.)))
                .min_h_0()
                .margin_top(top)
                .overlay_closable(false)
                .close_button(false)
                .content(move |body, _, _| body.min_h_0().child(content.clone()))
                .footer(footer.clone())
                .on_close(move |_, window, cx| {
                    let _ =
                        closed.update(cx, |this, cx| this.restore_reader_image_focus(window, cx));
                })
        });
        focus.focus(window, cx);
        cx.notify();
    }
    fn restore_reader_image_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(viewer) = self.reader_ui.viewer.take() {
            if let Some(focus) = viewer.return_focus {
                focus.focus(window, cx);
            }
        }
        cx.notify();
    }
    fn close_reader_image(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.close_dialog(cx);
        self.restore_reader_image_focus(window, cx);
    }
    fn move_reader_image(&mut self, delta: isize, cx: &mut Context<Self>) {
        if let Some(viewer) = &mut self.reader_ui.viewer {
            viewer.index = (viewer.index as isize + delta)
                .clamp(0, viewer.frames.len().saturating_sub(1) as isize)
                as usize;
            viewer.zoom = None;
            viewer.scroll.set_offset(point(px(0.), px(0.)));
            viewer.viewport_scroll.set_offset(point(px(0.), px(0.)));
            cx.notify();
        }
    }
    fn image_fit(viewer: &ImageViewer, _window: &Window) -> f32 {
        let frame = &viewer.frames[viewer.index];
        // The first prepaint records the space that flex layout actually leaves
        // for media after the toolbar, transcript and naturally sized footer.
        // Until then, do not guess a fit from a presumed footer height.
        let width = f32::from(viewer.viewport_size.width);
        let height = f32::from(viewer.viewport_size.height);
        if width <= 0. || height <= 0. {
            return 0.;
        }
        (width / frame.width.max(1) as f32)
            .min(height / frame.height.max(1) as f32)
            .min(1.)
    }
    fn image_details_open(viewer: &ImageViewer, window: &Window) -> bool {
        viewer.details_open.unwrap_or_else(|| {
            f32::from(window.bounds().size.height) >= f32::from(window.rem_size()) * 32.
        })
    }
    fn image_separate_modes(window: &Window) -> bool {
        let available = window.bounds().size.height - task_dialog_top(window) - px(24.);
        f32::from(available) < f32::from(window.rem_size()) * 24.
    }
    fn zoom_reader_image(&mut self, factor: Option<f32>, window: &Window, cx: &mut Context<Self>) {
        if Self::image_separate_modes(window)
            && self
                .reader_ui
                .viewer
                .as_ref()
                .is_some_and(|viewer| Self::image_details_open(viewer, window))
        {
            return;
        }
        if let Some(viewer) = &mut self.reader_ui.viewer {
            let old_scale = viewer
                .zoom
                .unwrap_or_else(|| Self::image_fit(viewer, window));
            viewer.zoom = factor.map(|factor| (old_scale * factor).clamp(0.1, 4.));
            let new_scale = viewer
                .zoom
                .unwrap_or_else(|| Self::image_fit(viewer, window));
            let frame = &viewer.frames[viewer.index];
            let viewport = viewer.viewport_scroll.bounds().size;
            let offset = viewer.viewport_scroll.offset();
            viewer.viewport_scroll.set_offset(point(
                px(image_zoom_offset(
                    f32::from(viewport.width),
                    frame.width as f32 * old_scale,
                    frame.width as f32 * new_scale,
                    f32::from(offset.x),
                )),
                px(image_zoom_offset(
                    f32::from(viewport.height),
                    frame.height as f32 * old_scale,
                    frame.height as f32 * new_scale,
                    f32::from(offset.y),
                )),
            ));
            cx.notify();
        }
    }
    fn reader_image_content(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(viewer) = &self.reader_ui.viewer else {
            return div().into_any_element();
        };
        let frame = viewer.frames[viewer.index].clone();
        let index = viewer.index;
        let count = viewer.frames.len();
        let label = frame_label(&viewer.title, &frame, index);
        let scale = viewer
            .zoom
            .unwrap_or_else(|| Self::image_fit(viewer, window));
        let version = viewer.version.clone();
        let content_width = (f32::from(window.bounds().size.width) - 48.).min(1180.) - 34.;
        let compact = content_width < f32::from(window.rem_size()) * 52.;
        let text_only =
            Self::image_separate_modes(window) && Self::image_details_open(viewer, window);
        let timestamp = frame.seconds.map(course2md::render::fmt_ts);
        let heading = match timestamp {
            Some(time) if text_only => format!("文字 · {time}"),
            Some(time) if compact => time,
            Some(time) => format!("截图 · {time}"),
            None => if text_only { "截图文字" } else { "截图" }.into(),
        };
        let zoom_controls = if text_only {
            None
        } else if compact {
            let weak = cx.weak_entity();
            let fit = viewer.zoom.is_none();
            Some(
                quiet("image-zoom-menu")
                    .icon(icons::zoom_in())
                    .label(if scale > 0. {
                        format!("{}%", (scale * 100.).round() as u32)
                    } else {
                        "缩放".into()
                    })
                    .child(Icon::new(IconName::ChevronDown).size_4().flex_shrink_0())
                    .accessibility_label("缩放图片")
                    .tooltip("缩放图片")
                    .dropdown_menu(move |menu, _, _| {
                        let larger = weak.clone();
                        let smaller = weak.clone();
                        let fitted = weak.clone();
                        menu.item(
                            PopupMenuItem::new("放大 · +")
                                .icon(icons::zoom_in())
                                .disabled(scale >= 4.)
                                .on_click(move |_, window, cx| {
                                    let _ = larger.update(cx, |this, cx| {
                                        this.zoom_reader_image(Some(1.25), window, cx)
                                    });
                                }),
                        )
                        .item(
                            PopupMenuItem::new("缩小 · −")
                                .icon(icons::zoom_out())
                                .disabled(scale <= 0.1)
                                .on_click(move |_, window, cx| {
                                    let _ = smaller.update(cx, |this, cx| {
                                        this.zoom_reader_image(Some(0.8), window, cx)
                                    });
                                }),
                        )
                        .separator()
                        .item(
                            PopupMenuItem::new("适合窗口 · 0")
                                .icon(icons::fit_screen())
                                .checked(fit)
                                .on_click(move |_, window, cx| {
                                    let _ = fitted.update(cx, |this, cx| {
                                        this.zoom_reader_image(None, window, cx)
                                    });
                                }),
                        )
                    })
                    .into_any_element(),
            )
        } else {
            Some(
                (h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        control("image-zoom-out")
                            .icon(icons::zoom_out())
                            .w(CONTROL_HEIGHT)
                            .px_0()
                            .accessibility_label("缩小")
                            .tooltip("缩小 · −")
                            .disabled(scale <= 0.1)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.zoom_reader_image(Some(0.8), window, cx)
                            })),
                    )
                    .child(
                        theme::accessible_text(
                            "image-scale",
                            if scale > 0. {
                                format!("{}%", (scale * 100.).round() as u32)
                            } else {
                                "—".into()
                            },
                        )
                        .whitespace_nowrap()
                        .text_size(TEXT_BODY),
                    )
                    .child(
                        control("image-zoom-in")
                            .icon(icons::zoom_in())
                            .w(CONTROL_HEIGHT)
                            .px_0()
                            .accessibility_label("放大")
                            .tooltip("放大 · +")
                            .disabled(scale >= 4.)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.zoom_reader_image(Some(1.25), window, cx)
                            })),
                    )
                    .child(
                        control("image-fit")
                            .icon(icons::fit_screen())
                            .w(CONTROL_HEIGHT)
                            .px_0()
                            .accessibility_label("适合窗口")
                            .tooltip("适合窗口 · 0")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.zoom_reader_image(None, window, cx)
                            })),
                    ))
                .into_any_element(),
            )
        };
        let mut body = v_flex()
            .id("reader-image-dialog")
            .relative()
            .role(Role::Dialog)
            .aria_label(if text_only {
                format!("阅读{label}对应文字")
            } else {
                format!("查看{label}")
            })
            .track_focus(&viewer.focus)
            .key_context("ReaderImage")
            .on_action(cx.listener(|this, _: &PreviousImage, _, cx| this.move_reader_image(-1, cx)))
            .on_action(cx.listener(|this, _: &NextImage, _, cx| this.move_reader_image(1, cx)))
            .on_action(cx.listener(|this, _: &ZoomIn, window, cx| {
                this.zoom_reader_image(Some(1.25), window, cx)
            }))
            .on_action(cx.listener(|this, _: &ZoomOut, window, cx| {
                this.zoom_reader_image(Some(0.8), window, cx)
            }))
            .on_action(cx.listener(|this, _: &FitImage, window, cx| {
                this.zoom_reader_image(None, window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &CloseImage, window, cx| this.close_reader_image(window, cx)),
            )
            .gap_2()
            .h_full()
            .w_full()
            .min_h_0()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .flex_wrap()
                    .flex_shrink_0()
                    .child(
                        theme::accessible_text("image-title", heading)
                            .role(Role::Heading)
                            .aria_label(label.clone())
                            .whitespace_nowrap()
                            .text_size(if compact { TEXT_BODY } else { TEXT_TITLE })
                            .font_weight(FontWeight::SEMIBOLD),
                    )
                    .child(div().flex_1())
                    .child(
                        control("image-previous")
                            .icon(IconName::ChevronLeft)
                            .w(CONTROL_HEIGHT)
                            .px_0()
                            .accessibility_label("上一张")
                            .tooltip("上一张 · ←")
                            .disabled(index == 0)
                            .on_click(cx.listener(|this, _, _, cx| this.move_reader_image(-1, cx))),
                    )
                    .child(
                        theme::accessible_text(
                            "image-number",
                            if viewer.filtered {
                                format!("匹配截图 {} / {count}", index + 1)
                            } else {
                                format!("{} / {count}", index + 1)
                            },
                        )
                        .whitespace_nowrap()
                        .text_size(TEXT_BODY),
                    )
                    .child(
                        control("image-next")
                            .icon(IconName::ChevronRight)
                            .w(CONTROL_HEIGHT)
                            .px_0()
                            .accessibility_label("下一张")
                            .tooltip("下一张 · →")
                            .disabled(index + 1 >= count)
                            .on_click(cx.listener(|this, _, _, cx| this.move_reader_image(1, cx))),
                    )
                    .children(zoom_controls),
            );
        if !text_only {
            if let Some(path) = frame.path {
                let viewport_scroll = viewer.viewport_scroll.clone();
                let viewport_owner = cx.weak_entity();
                let viewport_version = version.clone();
                body = body.child(
                    div()
                        .on_children_prepainted(move |_, window, cx| {
                            let size = viewport_scroll.bounds().size;
                            let owner = viewport_owner.clone();
                            let version = viewport_version.clone();
                            window.defer(cx, move |_, cx| {
                                let _ = owner.update(cx, |this, cx| {
                                    if let Some(viewer) = &mut this.reader_ui.viewer
                                        && viewer.version == version
                                        && viewer.viewport_size != size
                                    {
                                        viewer.viewport_size = size;
                                        cx.notify();
                                    }
                                });
                            });
                        })
                        .id("image-viewport")
                        .bg(color(INSET))
                        .border_1()
                        .border_color(color(HAIRLINE))
                        .rounded(RADIUS_CARD)
                        .flex_1()
                        .min_h_0()
                        .w_full()
                        .overflow_x_scroll()
                        .overflow_y_scroll()
                        .track_scroll(&viewer.viewport_scroll)
                        .child(
                            div()
                                .id("reader-image-canvas")
                                .flex_none()
                                .w(px(frame.width as f32 * scale))
                                .h(px(frame.height as f32 * scale))
                                .min_w(relative(1.))
                                .min_h(relative(1.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    div()
                                        .id("enlarged-reader-image")
                                        .flex_none()
                                        .w(px(frame.width as f32 * scale))
                                        .h(px(frame.height as f32 * scale))
                                        .role(Role::Image)
                                        .aria_label(
                                            frame
                                                .caption
                                                .as_ref()
                                                .map(|caption| {
                                                    format!("{label}。原图说明：{caption}")
                                                })
                                                .unwrap_or(label),
                                        )
                                        .child(
                                            img(path)
                                                .size_full()
                                                .object_fit(ObjectFit::Contain)
                                                .with_fallback(|| {
                                                    // 放大查看器同时只存在一个，固定 id 无碰撞
                                                    theme::accessible_text(
                                                        "failed-reader-image-enlarged",
                                                        "这张截图无法读取；对应正文仍可阅读。",
                                                    )
                                                    .into_any_element()
                                                }),
                                        ),
                                ),
                        ),
                );
            } else {
                body = body.child(theme::accessible_text(
                    "image-unreadable",
                    "这张截图无法读取；对应正文仍可在笔记中查看。",
                ));
            }
        }
        let has_details = frame.caption.is_some() || !frame.transcript.is_empty();
        let mut details = v_flex()
            .id("image-details")
            .w_full()
            // Short windows show either the image or its text, giving the
            // chosen content the whole available viewport.
            .when(text_only, |view| view.flex_1())
            .when(!text_only, |view| view.flex_shrink_0().max_h(relative(0.4)))
            .min_h_0()
            .gap_2()
            .pr_3()
            .overflow_y_scroll()
            .track_scroll(&viewer.scroll);
        if let Some(caption) = frame.caption {
            details = details.child(
                paragraph(
                    "image-caption",
                    format!("原图说明：{caption}"),
                    0,
                    Vec::new(),
                    None,
                )
                .flex_shrink_0(),
            );
        }
        if !frame.transcript.is_empty() {
            details = details.child(
                paragraph(
                    "image-transcript",
                    format!(
                        "{}：{}",
                        if frame.seconds.is_some() {
                            "同期转录"
                        } else {
                            "相邻正文"
                        },
                        frame.transcript
                    ),
                    1,
                    Vec::new(),
                    None,
                )
                .flex_shrink_0(),
            );
        }
        if text_only && !has_details {
            details = details.child(
                theme::accessible_text(
                    "image-no-text",
                    "这张截图没有对应文字。可以返回图片，或在笔记中查看上下文。",
                )
                .text_size(TEXT_READER)
                .whitespace_normal(),
            );
        }
        if text_only || (has_details && Self::image_details_open(viewer, window)) {
            body = body
                .child(details)
                .child(Scrollbar::vertical(&viewer.scroll).mode(ScrollbarMode::Always));
        }
        body.into_any_element()
    }

    fn reader_image_actions(&mut self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(viewer) = &self.reader_ui.viewer else {
            return div().into_any_element();
        };
        let frame = &viewer.frames[viewer.index];
        let content_width = (f32::from(window.bounds().size.width) - 48.).min(1180.) - 34.;
        let compact = content_width < f32::from(window.rem_size()) * 52.;
        let has_details = frame.caption.is_some() || !frame.transcript.is_empty();
        let details_open = Self::image_details_open(viewer, window);
        let separate_modes = Self::image_separate_modes(window);
        let text_only = separate_modes && details_open;
        let version = viewer.version.clone();
        let body_anchor = frame.body_anchor.clone();
        let source_link = viewer
            .source
            .as_ref()
            .and_then(|source| frame.seconds.and_then(|time| nav::seek_url(source, time)));
        let original_source = viewer.source.clone();
        // Footer actions never need to scroll into the transcript viewport.
        let reveal = |_: &'static str, child: AnyElement| child;
        let mut actions = h_flex()
            .id("reader-image-actions")
            .key_context("ReaderImage")
            .on_action(cx.listener(|this, _: &PreviousImage, _, cx| this.move_reader_image(-1, cx)))
            .on_action(cx.listener(|this, _: &NextImage, _, cx| this.move_reader_image(1, cx)))
            .on_action(cx.listener(|this, _: &ZoomIn, window, cx| {
                this.zoom_reader_image(Some(1.25), window, cx)
            }))
            .on_action(cx.listener(|this, _: &ZoomOut, window, cx| {
                this.zoom_reader_image(Some(0.8), window, cx)
            }))
            .on_action(cx.listener(|this, _: &FitImage, window, cx| {
                this.zoom_reader_image(None, window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &CloseImage, window, cx| this.close_reader_image(window, cx)),
            )
            .gap_2()
            .w_full()
            .flex_wrap()
            .flex_shrink_0();
        if let Some(anchor) = body_anchor {
            actions =
                actions.child(reveal(
                    "reveal-image-to-body",
                    (control("image-to-body")
                        .icon(icons::article())
                        .label(if compact {
                            "正文"
                        } else {
                            "在笔记中查看"
                        })
                        .accessibility_label("在笔记中查看")
                        .tooltip("在笔记中查看")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            let index =
                                this.preview
                                    .as_ref()
                                    .filter(|preview| preview.course.dir == version)
                                    .and_then(|preview| {
                                        preview.blocks.iter().enumerate().position(
                                            |(index, block)| block_anchor(block, index) == anchor,
                                        )
                                    });
                            this.close_reader_image(window, cx);
                            if let Some(index) = index {
                                this.jump_reader_block(index, 0., cx);
                            }
                        })))
                    .into_any_element(),
                ));
        }
        if let Some(url) = source_link {
            actions = actions.child(reveal(
                "reveal-image-to-source",
                (control("image-to-source")
                    .icon(icons::play_arrow())
                    .ghost()
                    .label(if compact { "观看" } else { "从此处观看" })
                    .accessibility_label("从此处观看原视频")
                    .tooltip("从此处观看原视频")
                    .on_click(move |_, _, cx| cx.open_url(&url)))
                .into_any_element(),
            ));
        } else if let Some(source) = original_source.filter(|_| viewer.source_available) {
            actions = actions.child(reveal(
                "reveal-image-open-original",
                (control("image-open-original")
                    .icon(icons::movie())
                    .ghost()
                    .label(if compact {
                        "原视频"
                    } else {
                        "打开原视频"
                    })
                    .accessibility_label("打开原视频")
                    .tooltip("打开原视频")
                    .on_click(move |_, _, cx| match &source {
                        nav::SourceTarget::Web(url) => cx.open_url(url),
                        nav::SourceTarget::Local(path) => cx.open_with_system(path),
                    }))
                .into_any_element(),
            ));
        }
        if has_details || text_only {
            actions = actions.child(
                quiet("image-toggle-details")
                    .icon(if text_only {
                        icons::image()
                    } else {
                        icons::article()
                    })
                    .label(if text_only {
                        "返回图片"
                    } else if separate_modes {
                        "查看文字"
                    } else if details_open {
                        "收起文字"
                    } else {
                        "显示文字"
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(viewer) = &mut this.reader_ui.viewer {
                            viewer.details_open = Some(!details_open);
                            cx.notify();
                        }
                    })),
            );
        }
        actions
            .child(div().flex_1())
            .child(
                control("image-close")
                    .icon(IconName::Close)
                    .label("关闭")
                    .accessibility_label("关闭截图")
                    .on_click(
                        cx.listener(|this, _, window, cx| this.close_reader_image(window, cx)),
                    ),
            )
            .into_any_element()
    }
}

fn load_reader_data(preview: &notes::Preview) -> ReaderData {
    let mut data = ReaderData::default();
    let dir = &preview.course.dir;
    data.export_folder = existing_export_folder(dir, &preview.outputs);
    if let Some(manifest) = &preview.course.manifest {
        for (index, frame) in manifest.frames.iter().enumerate() {
            let section = preview.document.as_ref().and_then(|document| {
                document
                    .sections
                    .iter()
                    .enumerate()
                    .find(|(_, section)| section.image == frame.image)
            });
            data.frames.push(checked_frame(Frame {
                anchor: format!("frame:{index}:{}", frame.image),
                path: course2md::artifact::safe_asset_path(dir, &frame.image).ok(),
                seconds: Some(frame.t),
                caption: None,
                transcript: section
                    .map(|(_, section)| {
                        section
                            .speech
                            .iter()
                            .map(|speech| speech.text.as_str())
                            .collect::<Vec<_>>()
                            .join("\n\n")
                    })
                    .unwrap_or_default(),
                body_anchor: section
                    .map(|(index, _)| format!("section-{index}"))
                    .or_else(|| {
                        nav::nearest_time(heading_times(&preview.blocks), frame.t)
                            .map(|(i, _)| block_anchor(&preview.blocks[i], i))
                    }),
                width: 16,
                height: 9,
            }));
        }
        let parent = dir
            .parent()
            .filter(|parent| parent.file_name().is_some_and(|name| name == "versions"));
        if let Some(parent) = parent {
            match std::fs::read_dir(parent) {
                Ok(entries) => {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if !path.is_dir() || !path.join("manifest.json").is_file() {
                            continue;
                        }
                        match course2md::artifact::read_manifest(&path.join("manifest.json")) {
                            Ok(other) if other.course_id == manifest.course_id => {
                                let mut course = preview.course.clone();
                                course.dir = path;
                                course.title = other.title.clone();
                                course.slides = other.frames.len();
                                let label = format!(
                                    "第 {} 版 · {}{}",
                                    other.revision,
                                    nav::timestamp_local(other.created_at_ms),
                                    if notes::has_incomplete_content(&other) {
                                        " · 部分完成"
                                    } else {
                                        ""
                                    }
                                );
                                course.manifest = Some(other);
                                data.versions.push(Version { course, label });
                            }
                            Err(error) => data
                                .issues
                                .push(format!("有一个旧版本的清单无法读取：{error:#}")),
                            _ => {}
                        }
                    }
                    data.versions.sort_by_key(|version| {
                        std::cmp::Reverse(
                            version
                                .course
                                .manifest
                                .as_ref()
                                .map(|manifest| (manifest.revision, manifest.created_at_ms))
                                .unwrap_or_default(),
                        )
                    });
                }
                Err(error) => data
                    .issues
                    .push(format!("其他版本暂时无法读取：{error}。当前版本仍可阅读。")),
            }
        }
    }
    data
}

fn existing_export_folder(dir: &std::path::Path, outputs: &[String]) -> Option<PathBuf> {
    let folder = course2md::artifact::safe_asset_path(dir, "exports").ok()?;
    let has_file = outputs.iter().any(|output| {
        std::path::Path::new(output).starts_with("exports")
            && course2md::artifact::safe_asset_path(dir, output).is_ok_and(|path| path.is_file())
    });
    has_file.then_some(folder)
}
fn checked_frame(mut frame: Frame) -> Frame {
    if let Some((width, height)) = frame
        .path
        .as_ref()
        .and_then(|path| image::image_dimensions(path).ok())
        .filter(|(width, height)| *width > 0 && *height > 0)
    {
        frame.width = width;
        frame.height = height;
    } else {
        frame.path = None;
    }
    frame
}

fn processing_notice(issues: &[crate::notes::ProcessingIssue]) -> Option<String> {
    if issues.is_empty() {
        return None;
    }
    Some(format!(
        "正文已保存，{}未完成。",
        issues
            .iter()
            .map(|issue| issue.stage.label())
            .collect::<Vec<_>>()
            .join("、")
    ))
}
fn files_need_reload(preview: &crate::notes::Preview, issues: &[String], frames: &[Frame]) -> bool {
    !preview.issues.is_empty()
        || !issues.is_empty()
        || frames.iter().any(|frame| frame.path.is_none())
}

/// Preserve the image point under the viewport center while either axis grows
/// from a centered preview into scrollable content, or returns to a fitted image.
fn image_zoom_offset(viewport: f32, old_extent: f32, new_extent: f32, offset: f32) -> f32 {
    if viewport <= 0. || old_extent <= 0. {
        return 0.;
    }
    let old_margin = (viewport - old_extent).max(0.) * 0.5;
    let image_position = ((viewport * 0.5 - offset - old_margin) / old_extent).clamp(0., 1.);
    let new_margin = (viewport - new_extent).max(0.) * 0.5;
    (viewport * 0.5 - new_margin - image_position * new_extent)
        .clamp(-(new_extent - viewport).max(0.), 0.)
}

#[cfg(test)]
mod tests {
    use super::{
        ExportFeedback, ExportState, Frame, OfflineVideo, OfflineVideoRequest, PreviewBlock,
        block_anchor, block_time, capture_reader_position, existing_export_folder,
        exported_file_label, files_need_reload, frame_excerpt, image_zoom_offset, load_reader_data,
        matching_frames, nav, note_position_index, processing_notice, restored_reader_offset,
    };
    use crate::{ConversionOptions, notes::Course, source, workspace};

    #[test]
    fn saved_export_folder_requires_a_listed_file_inside_the_version() {
        let temp = tempfile::tempdir().unwrap();
        let exports = temp.path().join("exports");
        let outputs = vec!["exports/course.html".into()];
        assert!(existing_export_folder(temp.path(), &outputs).is_none());
        std::fs::create_dir(&exports).unwrap();
        assert!(existing_export_folder(temp.path(), &outputs).is_none());
        std::fs::write(exports.join("course.html"), "<p>笔记</p>").unwrap();
        assert_eq!(existing_export_folder(temp.path(), &outputs), Some(exports));
        assert!(existing_export_folder(temp.path(), &[]).is_none());
        assert!(existing_export_folder(temp.path(), &["exports/../course.html".into()]).is_none());
    }

    #[test]
    fn export_feedback_stays_with_its_version_and_ignores_cancelled_callbacks() {
        let old = std::path::PathBuf::from("/library/old-note/versions/one");
        let other = std::path::PathBuf::from("/library/other-note/versions/two");
        let path = std::path::PathBuf::from("/exports/old-note.html");
        let success = || ExportFeedback {
            title: "旧笔记".into(),
            format: course2md::config::OutputFormat::Html,
            result: Ok(path.clone()),
        };
        let mut state = ExportState::default();
        let cancelled = state.begin(old.clone()).unwrap();
        assert!(state.cancel(cancelled, &old));
        assert!(state.feedback.is_empty());
        let next = state.begin(other.clone()).unwrap();
        assert!(!state.finish(cancelled, &old, success()));
        assert!(state.is_pending(next, &other));
        assert!(state.finish(
            next,
            &other,
            ExportFeedback {
                title: "另一份笔记".into(),
                format: course2md::config::OutputFormat::Html,
                result: Err("保存位置不可写".into()),
            }
        ));
        let cancelled_retry = state.begin(other.clone()).unwrap();
        assert!(state.cancel(cancelled_retry, &other));
        assert!(state.feedback[&other].result.is_err());
        let completed = state.begin(old.clone()).unwrap();
        assert!(state.finish(completed, &old, success()));
        assert_eq!(state.feedback[&old].result.as_ref().unwrap(), &path);
        assert!(state.feedback[&other].result.is_err());
    }

    #[test]
    fn export_confirmation_uses_the_filename_without_the_private_directory() {
        let file = std::path::Path::new("/private/library/export/这份笔记.html");
        assert_eq!(exported_file_label(file), "已导出 · 这份笔记.html");
    }

    #[test]
    fn gallery_search_matches_its_own_text_and_preserves_frame_identity() {
        let frame = |index, transcript: &str, caption: Option<&str>| Frame {
            anchor: format!("frame-{index}"),
            path: None,
            seconds: Some(index as f64 * 10.),
            caption: caption.map(str::to_owned),
            transcript: transcript.into(),
            body_anchor: Some(format!("section-{index}")),
            width: 16,
            height: 9,
        };
        let frames = vec![
            frame(0, "结构与分组", None),
            frame(1, "OpenAI 与模型", Some("中文示意图")),
            frame(2, "结语", None),
        ];
        assert_eq!(matching_frames(&frames, "  OPENAI  "), vec![1]);
        assert_eq!(matching_frames(&frames, "示意图"), vec![1]);
        assert_eq!(frame_excerpt(&frames[1], "示意图"), "中文示意图");
        assert_eq!(matching_frames(&frames, "00:20"), vec![2]);
        assert!(matching_frames(&frames, "不存在的内容").is_empty());
        assert_eq!(matching_frames(&frames, ""), vec![0, 1, 2]);
        assert_eq!(frames[1].body_anchor.as_deref(), Some("section-1"));
        let phrase = frame(
            3,
            "Some introductory words. OpenAI said that Buckmaster offered terms.",
            None,
        );
        assert!(frame_excerpt(&phrase, "Buckmaster").starts_with("…OpenAI said that Buckmaster"));
    }

    fn reading_blocks() -> Vec<PreviewBlock> {
        vec![
            PreviewBlock::Heading {
                text: "摘要".into(),
                anchor: "summary".into(),
                seconds: None,
            },
            PreviewBlock::Paragraph {
                text: "摘要正文".into(),
                anchor: "summary-tldr".into(),
            },
            PreviewBlock::Heading {
                text: "第一节".into(),
                anchor: "section-1".into(),
                seconds: Some(5.),
            },
            PreviewBlock::Paragraph {
                text: "可在放大字号后继续阅读的长正文".into(),
                anchor: "section-1-text".into(),
            },
        ]
    }

    #[test]
    fn reading_top_survives_late_measurement_and_enlarged_text() {
        let blocks = reading_blocks();
        let mut initial = nav::ReadingLayout::default();
        initial.record(1, 48., 60.);
        let position = capture_reader_position(&initial, 0., |index| {
            (
                Some(block_anchor(&blocks[index], index)),
                block_time(&blocks, index),
            )
        })
        .unwrap();
        // Loading initially exposes only the paragraph. Its later measured
        // heading and a larger type scale must not replace the document top.
        let mut resized = nav::ReadingLayout::default();
        resized.record(0, 12., 36.);
        resized.record(1, 64., 120.);
        assert!(position.paragraph.is_none());
        assert_eq!(
            restored_reader_offset(&resized, note_position_index(&blocks, &position), &position),
            0.
        );
    }

    #[test]
    fn reading_summary_navigation_returns_to_the_heading_after_reflow() {
        let blocks = reading_blocks();
        let position = workspace::ReadingPosition {
            paragraph: Some("summary".into()),
            fraction: Some(0.),
            ..Default::default()
        };
        let index = note_position_index(&blocks, &position);
        assert_eq!(index, Some(0));
        for (top, height) in [(12., 22.), (16., 44.)] {
            let mut layout = nav::ReadingLayout::default();
            layout.record(0, top, height);
            layout.record(1, top + height + 12., 120.);
            let offset = restored_reader_offset(&layout, index, &position);
            assert_eq!(layout.top_item(offset).map(|(index, _, _)| index), Some(0));
        }
    }

    #[test]
    fn reading_resize_keeps_the_same_paragraph_and_progress() {
        let blocks = reading_blocks();
        let mut initial = nav::ReadingLayout::default();
        initial.record(3, 220., 200.);
        let position = capture_reader_position(&initial, -270., |index| {
            (
                Some(block_anchor(&blocks[index], index)),
                block_time(&blocks, index),
            )
        })
        .unwrap();
        let mut resized = nav::ReadingLayout::default();
        resized.record(3, 400., 400.);
        let offset =
            restored_reader_offset(&resized, note_position_index(&blocks, &position), &position);
        assert_eq!(offset, -500.);
        let (index, within, height) = resized.top_item(offset).unwrap();
        assert_eq!(index, 3);
        assert_eq!(nav::within_fraction(within, height), 0.25);
    }

    #[test]
    fn image_zoom_keeps_the_visible_center_and_all_edges_reachable() {
        // A centered small image grows around its midpoint, then fits again.
        assert_eq!(image_zoom_offset(1000., 640., 1280., 0.), -140.);
        assert_eq!(image_zoom_offset(1000., 1280., 640., -140.), 0.);
        // A panned image preserves the same content at the viewport center.
        assert_eq!(image_zoom_offset(1000., 2000., 4000., -400.), -1300.);
        // Shrinking near an edge clamps to reachable bounds without hiding it.
        assert_eq!(image_zoom_offset(1000., 2000., 1200., -1000.), -200.);
        assert_eq!(image_zoom_offset(1000., 2000., 1200., 0.), 0.);
    }

    #[test]
    fn an_untimed_heading_ends_the_preceding_time_scope() {
        let blocks = vec![
            PreviewBlock::Heading {
                text: "0:10".into(),
                anchor: "a".into(),
                seconds: Some(10.),
            },
            PreviewBlock::Paragraph {
                text: "第一段".into(),
                anchor: "b".into(),
            },
            PreviewBlock::Heading {
                text: "补充主题".into(),
                anchor: "c".into(),
                seconds: None,
            },
            PreviewBlock::Paragraph {
                text: "第二段".into(),
                anchor: "d".into(),
            },
        ];
        assert_eq!(block_time(&blocks, 1), Some(10.));
        assert_eq!(block_time(&blocks, 3), None);
    }
    fn fixture_version(
        root: &std::path::Path,
        revision: u64,
        times: &[f64],
    ) -> crate::notes::Course {
        fixture_with_outcomes(root, revision, times, Default::default())
    }
    fn fixture_with_outcomes(
        root: &std::path::Path,
        revision: u64,
        times: &[f64],
        outcomes: course2md::artifact::Outcomes,
    ) -> crate::notes::Course {
        use course2md::{
            artifact,
            fetch::VideoMeta,
            timeline::{Section, TranscriptEvent},
        };
        let work = root.join(format!("work-{revision}"));
        std::fs::create_dir_all(work.join("frames")).unwrap();
        let sections = times
            .iter()
            .enumerate()
            .map(|(index, time)| {
                let relative = format!("frames/frame-{index}.png");
                image::RgbImage::new(4 + revision as u32, 3)
                    .save(work.join(&relative))
                    .unwrap();
                Section {
                    t: *time,
                    end: *time + 10.,
                    image: relative,
                    speech: vec![TranscriptEvent {
                        start: *time,
                        end: *time + 10.,
                        text: format!("第 {revision} 版，{time} 秒的正文"),
                        raw: None,
                    }],
                }
            })
            .collect::<Vec<_>>();
        let target = artifact::Target {
            task_id: format!("task-{revision}"),
            course_id: "reader-fixture".into(),
            source_id: "source-fixture".into(),
            version_id: format!("v{revision}"),
            course_dir: root.join("course"),
        };
        let meta = VideoMeta {
            title: "测试课程".into(),
            uploader: "作者".into(),
            duration: 90.,
            webpage_url: "https://www.bilibili.com/video/BVfixture?p=2".into(),
            extractor: "bilibili".into(),
            id: "fixture".into(),
        };
        let manifest = smol::block_on(artifact::publish(
            &target,
            &work,
            &meta,
            &sections,
            None,
            &[],
            outcomes,
        ))
        .unwrap();
        crate::notes::Course {
            dir: target.version_dir(),
            title: meta.title,
            modified: std::time::SystemTime::now(),
            slides: times.len(),
            segments: times.len(),
            thumbnail: None,
            manifest: Some(manifest),
            warning: None,
        }
    }
    fn offline_task_fixture(
        root: &std::path::Path,
        course: &Course,
    ) -> (workspace::TaskRecord, workspace::LibraryLocation) {
        let manifest = course.manifest.as_ref().unwrap();
        let library = workspace::LibraryLocation {
            id: "offline-library".into(),
            name: "Offline library".into(),
            root: root.to_owned(),
            previous_roots: Vec::new(),
        };
        let task = workspace::TaskRecord {
            id: manifest.task_id.clone(),
            plan: workspace::TaskPlan {
                operation: Default::default(),
                source: source::Source {
                    input: "https://www.bilibili.com/video/BVfixture?p=2".into(),
                    identity: manifest.source_id.clone(),
                    online: true,
                    ..Default::default()
                },
                source_id: manifest.source_id.clone(),
                title: course.title.clone(),
                library_id: library.id.clone(),
                folder: None,
                options: ConversionOptions {
                    keep_video: true,
                    ..Default::default()
                },
                subtitle: None,
                config: Default::default(),
                asr_service: None,
                ai_service: None,
            },
            state: workspace::TaskState::Complete,
            intent: workspace::Intent::Run,
            created: 0,
            updated: 0,
            parent: None,
            handled_by: None,
            work_dir: root.join(".course2md/work").join(&manifest.task_id),
            stages: Default::default(),
            error: None,
            artifact: Some(course.dir.clone()),
            outcomes: serde_json::Value::Null,
            unread: false,
            logs: Vec::new(),
            blocked: Vec::new(),
            resend: Vec::new(),
        };
        (task, library)
    }

    #[test]
    fn offline_video_uses_its_version_task_and_rechecks_removed_media() {
        let root = tempfile::tempdir().unwrap();
        let first = fixture_version(root.path(), 1, &[10.]);
        let second = fixture_version(root.path(), 2, &[20.]);
        let (task, library) = offline_task_fixture(root.path(), &first);
        std::fs::create_dir_all(&task.work_dir).unwrap();
        let media = task.work_dir.join("media.mp4");
        std::fs::write(&media, "retained first video").unwrap();
        let request = OfflineVideoRequest::for_version(&first, &task, &library).unwrap();
        assert_eq!(
            request.inspect(),
            OfflineVideo::Available(media.canonicalize().unwrap())
        );
        assert!(OfflineVideoRequest::for_version(&second, &task, &library).is_none());
        let mut changed = task.clone();
        changed.artifact = Some(second.dir.clone());
        assert!(OfflineVideoRequest::for_version(&first, &changed, &library).is_none());
        changed = task.clone();
        changed.plan.source_id = "another-video".into();
        assert!(OfflineVideoRequest::for_version(&first, &changed, &library).is_none());
        changed = task.clone();
        changed.plan.options.keep_video = false;
        assert!(OfflineVideoRequest::for_version(&first, &changed, &library).is_none());
        std::fs::remove_file(media).unwrap();
        assert_eq!(request.inspect(), OfflineVideo::Missing);
    }

    #[test]
    fn offline_video_refuses_other_task_paths_and_files_outside_its_library() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let course = fixture_version(root.path(), 1, &[10.]);
        let (mut task, library) = offline_task_fixture(root.path(), &course);
        let expected_work = task.work_dir.clone();
        std::fs::write(outside.path().join("media.mp4"), "unrelated video").unwrap();
        task.work_dir = outside.path().to_owned();
        let request = OfflineVideoRequest::for_version(&course, &task, &library).unwrap();
        assert_eq!(request.inspect(), OfflineVideo::Missing);
        task.work_dir = expected_work;
        std::fs::create_dir_all(&task.work_dir).unwrap();
        let mut request = OfflineVideoRequest::for_version(&course, &task, &library).unwrap();
        std::fs::write(task.work_dir.join("media.mp4"), "retained video").unwrap();
        request.version_dir = outside.path().to_owned();
        assert_eq!(request.inspect(), OfflineVideo::Missing);

        #[cfg(unix)]
        {
            std::fs::remove_file(task.work_dir.join("media.mp4")).unwrap();
            std::os::unix::fs::symlink(
                outside.path().join("media.mp4"),
                task.work_dir.join("media.mp4"),
            )
            .unwrap();
            let request = OfflineVideoRequest::for_version(&course, &task, &library).unwrap();
            assert_eq!(request.inspect(), OfflineVideo::Missing);
        }
    }

    #[test]
    fn summary_failure_keeps_body_readable_and_does_not_offer_file_reload() {
        let root = tempfile::tempdir().unwrap();
        let detail = "摘要尚未完成，已保留正文 / Summary incomplete; body retained";
        let course = fixture_with_outcomes(
            root.path(),
            1,
            &[10.],
            course2md::artifact::Outcomes {
                transcript: course2md::artifact::Outcome::succeeded(),
                screenshots: course2md::artifact::Outcome::succeeded(),
                summary: course2md::artifact::Outcome::failed(detail),
                ..Default::default()
            },
        );
        let preview = crate::notes::read_preview(course.clone()).unwrap();
        let data = load_reader_data(&preview);
        assert!(preview.plain_text.contains("10 秒的正文"));
        assert!(preview.issues.is_empty());
        assert_eq!(
            processing_notice(&preview.processing_issues).as_deref(),
            Some("正文已保存，摘要未完成。")
        );
        assert_eq!(
            preview.processing_issues[0].outcome.message.as_deref(),
            Some(detail)
        );
        assert_eq!(preview.course.manifest.as_ref().unwrap().task_id, "task-1");
        assert!(!files_need_reload(&preview, &data.issues, &data.frames));

        std::fs::remove_file(course.dir.join("frames/frame-0.png")).unwrap();
        let damaged = crate::notes::read_preview(course).unwrap();
        let data = load_reader_data(&damaged);
        assert!(files_need_reload(&damaged, &data.issues, &data.frames));
        assert_eq!(
            processing_notice(&damaged.processing_issues).as_deref(),
            Some("正文已保存，摘要未完成。")
        );
    }
    #[test]
    fn a_missing_earlier_frame_does_not_shift_later_times_or_mix_versions() {
        let root = tempfile::tempdir().unwrap();
        let old = fixture_version(root.path(), 1, &[10., 20., 30.]);
        let new = fixture_version(root.path(), 2, &[40.]);
        std::fs::remove_file(old.dir.join("frames/frame-0.png")).unwrap();
        let old_preview = crate::notes::read_preview(old.clone()).unwrap();
        assert_eq!(old_preview.frames.len(), 2);
        let old_data = load_reader_data(&old_preview);
        assert_eq!(old_data.frames.len(), 3);
        assert!(old_data.frames[0].path.is_none());
        assert_eq!(old_data.frames[1].seconds, Some(20.));
        assert!(old_data.frames[1].transcript.contains("20 秒"));
        assert_eq!(old_data.frames[1].body_anchor.as_deref(), Some("section-1"));
        assert_eq!(old_data.versions.len(), 2);
        let new_data = load_reader_data(&crate::notes::read_preview(new.clone()).unwrap());
        assert_eq!(new_data.frames.len(), 1);
        assert_eq!(new_data.frames[0].seconds, Some(40.));
        assert!(
            new_data.frames[0]
                .path
                .as_ref()
                .unwrap()
                .starts_with(&new.dir)
        );
        assert!(
            !new_data.frames[0]
                .path
                .as_ref()
                .unwrap()
                .starts_with(&old.dir)
        );
    }
}

/// Tests for the virtualized note flow: item flattening, bounded rendering and/// reading-position round trips through the persistent list state.
#[cfg(test)]
mod flow_tests {
    use super::{
        NoteItem, nav, note_block_item, note_capture_position, note_item_block, note_items,
        note_restore_target, note_top_block,
    };
    use crate::notes::PreviewBlock;
    use gpui::{
        Context, InteractiveElement as _, IntoElement, ListAlignment, ListOffset, ListState,
        ParentElement as _, Render, Styled as _, TestAppContext, Window, div, list, px,
    };
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    fn blocks(count: usize) -> Vec<PreviewBlock> {
        (0..count)
            .map(|index| {
                if index % 10 == 0 {
                    PreviewBlock::Heading {
                        text: format!("章节 {index}"),
                        anchor: format!("h-{index}"),
                        seconds: Some(index as f64 * 30.),
                    }
                } else {
                    PreviewBlock::Paragraph {
                        text: format!("正文段落 {index}。").repeat(8),
                        anchor: format!("p-{index}"),
                    }
                }
            })
            .collect()
    }

    #[test]
    fn note_items_flatten_title_summary_and_blocks_in_order() {
        let mut note = vec![
            PreviewBlock::Heading {
                text: "摘要".into(),
                anchor: "summary".into(),
                seconds: None,
            },
            PreviewBlock::Paragraph {
                text: "总览".into(),
                anchor: "summary-tldr".into(),
            },
            PreviewBlock::Paragraph {
                text: "要点".into(),
                anchor: "key-point-0".into(),
            },
        ];
        note.extend(blocks(6));
        let items = note_items(&note, true);
        assert_eq!(items.len(), 1 + 3 + 6);
        assert_eq!(items[0], NoteItem::Title);
        assert_eq!(items[1], NoteItem::SummaryLabel(Some(0)));
        assert_eq!(items[2], NoteItem::Block(1));
        assert_eq!(items[3], NoteItem::Block(2));
        assert_eq!(items[4], NoteItem::Block(3));
        assert_eq!(note_block_item(&items, 0), Some(1));
        assert_eq!(note_block_item(&items, 8), Some(items.len() - 1));
        assert_eq!(note_top_block(&items, 0), Some(0));
        assert_eq!(note_top_block(&items, 2), Some(1));

        // A summary without its heading still leads with an unindexed label.
        let mut unheaded = vec![PreviewBlock::Paragraph {
            text: "总览".into(),
            anchor: "summary-tldr".into(),
        }];
        unheaded.extend(blocks(3));
        let items = note_items(&unheaded, false);
        assert_eq!(items[0], NoteItem::SummaryLabel(None));
        assert_eq!(note_top_block(&items, 0), Some(0));
        assert_eq!(note_item_block(items[0]), None);

        assert!(note_items(&[], false).is_empty());
        assert_eq!(note_items(&[], true), [NoteItem::Title]);
    }

    #[test]
    fn note_position_round_trips_through_the_list_state() {
        let note = blocks(200);
        let items = note_items(&note, false);
        let list = ListState::new(items.len(), ListAlignment::Top, px(1000.));
        let heights: Vec<f32> = (0..items.len())
            .map(|i| 40. + (i % 7) as f32 * 30.)
            .collect();

        // The document top is a boundary, not the first paragraph.
        assert_eq!(
            note_capture_position(&items, &note, &list, &nav::ReadingLayout::default()),
            Some(crate::workspace::ReadingPosition::default())
        );
        assert_eq!(
            note_restore_target(
                &items,
                &note,
                &crate::workspace::ReadingPosition::default(),
                None,
                &nav::ReadingLayout::default(),
            ),
            None
        );

        // Scroll into the note; the rendered window records its measurements.
        list.scroll_to(ListOffset {
            item_ix: 53,
            offset_in_item: px(13.),
        });
        let mut layout = nav::ReadingLayout::default();
        for (index, height) in heights.iter().enumerate().take(60).skip(50) {
            layout.record(index, 0., *height);
        }
        let position = note_capture_position(&items, &note, &list, &layout).unwrap();
        assert_eq!(position.paragraph.as_deref(), Some("p-53"));
        assert_eq!(position.seconds, Some(1500.));
        assert_eq!(position.within, -13.);
        assert_eq!(position.fraction, Some(13. / heights[53]));
        assert!(position.offset < 0.);

        // Scroll away: the next frame re-records only the new window, so the
        // restore first lands on the item, then refines once it is measured.
        list.scroll_to(ListOffset {
            item_ix: 0,
            offset_in_item: px(0.),
        });
        let mut layout = nav::ReadingLayout::default();
        for (index, height) in heights.iter().enumerate().take(10) {
            layout.record(index, 0., *height);
        }
        assert_eq!(
            note_restore_target(&items, &note, &position, None, &layout),
            Some((53, None))
        );
        list.scroll_to(ListOffset {
            item_ix: 53,
            offset_in_item: px((-position.within).max(0.)),
        });
        for (index, height) in heights.iter().enumerate().take(64).skip(50) {
            layout.record(index, 0., *height);
        }
        assert_eq!(
            note_restore_target(&items, &note, &position, None, &layout),
            Some((53, Some(13.)))
        );
        list.scroll_to(ListOffset {
            item_ix: 53,
            offset_in_item: px(13.),
        });
        let restored = note_capture_position(&items, &note, &list, &layout).unwrap();
        assert_eq!(restored, position);

        // A find match resolves to its recorded line, one line of context up.
        layout.record_search_line(53, 40, 100., 20.);
        assert_eq!(
            note_restore_target(&items, &note, &position, Some((53, 40)), &layout),
            Some((53, Some(80.)))
        );
    }

    /// The same list wiring the reader uses: variable-height items recording
    /// their viewport-relative geometry during prepaint.
    struct NoteListHarness {
        list: ListState,
        layout: Rc<RefCell<nav::ReadingLayout>>,
        renders: Rc<Cell<usize>>,
        blocks: Vec<PreviewBlock>,
        items: Rc<Vec<NoteItem>>,
    }

    impl NoteListHarness {
        fn new(count: usize) -> Self {
            let blocks = blocks(count);
            let items = note_items(&blocks, false);
            Self {
                list: ListState::new(items.len(), ListAlignment::Top, px(1000.)),
                layout: Rc::default(),
                renders: Rc::new(Cell::new(0)),
                blocks,
                items: Rc::new(items),
            }
        }
    }

    impl Render for NoteListHarness {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            // Like the app, each frame's records start empty and the visible
            // window re-records during prepaint.
            self.layout.replace(nav::ReadingLayout::default());
            let layout = self.layout.clone();
            let renders = self.renders.clone();
            let items = self.items.clone();
            div().size_full().child(
                list(self.list.clone(), move |index, _window, _cx| {
                    renders.set(renders.get() + 1);
                    let positions = layout.clone();
                    let items = items.clone();
                    div()
                        .w_full()
                        .on_children_prepainted(move |bounds, _, _| {
                            let Some(bounds) = bounds.first() else {
                                return;
                            };
                            if let Some(block) = note_item_block(items[index]) {
                                positions.borrow_mut().record(
                                    block,
                                    f32::from(bounds.top()),
                                    f32::from(bounds.size.height),
                                );
                            }
                        })
                        .child(
                            div()
                                .w_full()
                                .h(px(40. + (index % 7) as f32 * 30.))
                                .debug_selector(move || format!("note-item-{index}")),
                        )
                        .into_any_element()
                })
                .w_full()
                .h_full(),
            )
        }
    }

    #[gpui::test]
    fn note_list_builds_only_the_visible_window_and_reaches_the_end(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, _| NoteListHarness::new(500));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let first = cx.update(|_, cx| view.read(cx).renders.get());
        assert!(
            first > 0 && first < 60,
            "first screen built {first} of 500 items"
        );

        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.list.scroll_to_end();
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
        assert!(
            cx.debug_bounds("note-item-499").is_some(),
            "the last block renders after jumping to the end"
        );
        let total = cx.update(|_, cx| view.read(cx).renders.get());
        assert!(
            total < 160,
            "500 blocks built {total} elements across two frames"
        );

        // Reading position round trip with records produced by real prepaint.
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.list.scroll_to(ListOffset {
                    item_ix: 250,
                    offset_in_item: px(17.),
                });
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
        let position = cx.update(|_, cx| view.read(cx).capture());
        assert_eq!(position.paragraph.as_deref(), Some("h-250"));
        assert_eq!(position.within, -17.);

        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.list.scroll_to(ListOffset {
                    item_ix: 0,
                    offset_in_item: px(0.),
                });
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
        // First pass lands on the unmeasured item; second pass refines it.
        cx.update(|window, cx| {
            view.update(cx, |view, _| view.restore_rough(&position));
            window.draw(cx).clear(cx);
        });
        cx.update(|window, cx| {
            view.update(cx, |view, _| view.restore_precise(&position));
            window.draw(cx).clear(cx);
        });
        cx.update(|_, cx| assert_eq!(view.read(cx).capture(), position));
    }

    impl NoteListHarness {
        fn capture(&self) -> crate::workspace::ReadingPosition {
            note_capture_position(&self.items, &self.blocks, &self.list, &self.layout.borrow())
                .unwrap()
        }

        fn restore_rough(&mut self, position: &crate::workspace::ReadingPosition) {
            let (item_ix, offset) = note_restore_target(
                &self.items,
                &self.blocks,
                position,
                None,
                &self.layout.borrow(),
            )
            .unwrap();
            assert_eq!(offset, None, "the scrolled-away item is unmeasured");
            self.list.scroll_to(ListOffset {
                item_ix,
                offset_in_item: px((-position.within).max(0.)),
            });
        }

        fn restore_precise(&mut self, position: &crate::workspace::ReadingPosition) {
            let (item_ix, offset) = note_restore_target(
                &self.items,
                &self.blocks,
                position,
                None,
                &self.layout.borrow(),
            )
            .unwrap();
            let offset = offset.expect("the landed item is measured by now");
            self.list.scroll_to(ListOffset {
                item_ix,
                offset_in_item: px(offset.max(0.)),
            });
        }
    }
}
