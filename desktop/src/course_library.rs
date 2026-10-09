//! Course organization and presentation are independent, persistent choices.
use super::*;
use crate::theme::*;
use gpui_component::{
    button::*,
    menu::{DropdownMenu, PopupMenuItem},
};
use std::rc::Rc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LibraryFilterOption {
    pub value: String,
    pub label: String,
    pub root: PathBuf,
    pub folder: Option<u64>,
}

pub(crate) fn library_filter_options(
    sections: &[LibrarySection],
    current_root: &std::path::Path,
    multi: bool,
) -> Vec<LibraryFilterOption> {
    let mut options = vec![LibraryFilterOption {
        value: "all".into(),
        label: "全部笔记".into(),
        root: current_root.to_path_buf(),
        folder: None,
    }];
    for (index, (root, library_name, folders)) in sections.iter().enumerate() {
        let unfiled = LibraryFilterOption {
            value: format!("f-{index}-0"),
            label: if multi {
                format!("{library_name} · 未分类")
            } else {
                "未分类".into()
            },
            root: root.clone(),
            folder: Some(0),
        };
        options.push(unfiled);
        for (id, name) in folders {
            options.push(LibraryFilterOption {
                value: format!("f-{index}-{id}"),
                label: if multi {
                    format!("{library_name} · {name}")
                } else {
                    name.clone()
                },
                root: root.clone(),
                folder: Some(*id),
            });
        }
    }
    options
}

pub(crate) fn library_filter_selected(
    options: &[LibraryFilterOption],
    current_root: &std::path::Path,
    folder: Option<u64>,
) -> String {
    options
        .iter()
        .find(|option| match folder {
            None => option.folder.is_none(),
            Some(id) => option.folder == Some(id) && option.root == current_root,
        })
        .map(|option| option.value.clone())
        .unwrap_or_else(|| "all".into())
}

pub(crate) fn library_layout_choice(cards: bool) -> &'static str {
    if cards { "cards" } else { "list" }
}

/// 文件夹分区：(保存位置根, 库名, [(文件夹 id, 名称)])。
pub(crate) type LibrarySection = (std::path::PathBuf, String, Vec<(u64, String)>);

/// Geometry shared by list and card layouts; rem-based spacing scales with text.
#[derive(Clone, Copy)]
struct LibraryLayout {
    columns: usize,
    compact: bool,
    stacked: bool,
}

/// One row of the virtualized library page. Grid rows carry their courses for
/// the current column count; folder groups stay one item per disclosure so the
/// expand motion is unchanged.
enum LibraryItem {
    Checking,
    Recovery,
    Coverage(String),
    Issues(Vec<String>),
    Loading,
    Empty { partial: bool },
    SearchScope(String),
    GroupHeader(Box<LibraryGroupHeader>),
    GroupBody(Box<LibraryGroupBody>),
    CardRow(Vec<(usize, Course)>),
    ListRow(usize, Box<Course>),
}

/// A folder group's header row.
struct LibraryGroupHeader {
    index: usize,
    key: String,
    name: String,
    count: usize,
    collapsed: bool,
}

/// A folder group's disclosure body: its card or list rows.
struct LibraryGroupBody {
    index: usize,
    key: String,
    collapsed: bool,
    entries: Vec<(usize, Course)>,
}

impl LibraryItem {
    /// Identity for the list splice: stable while the row means the same
    /// content, so measured heights and the scroll anchor survive updates.
    fn key(&self, columns: usize) -> String {
        match self {
            LibraryItem::Checking => "checking".to_owned(),
            LibraryItem::Recovery => "recovery".to_owned(),
            LibraryItem::Coverage(_) => "coverage".to_owned(),
            LibraryItem::Issues(_) => "issues".to_owned(),
            LibraryItem::Loading => "loading".to_owned(),
            LibraryItem::Empty { .. } => "empty".to_owned(),
            LibraryItem::SearchScope(_) => "search-scope".to_owned(),
            LibraryItem::GroupHeader(header) => format!("group-h-{}", header.key),
            LibraryItem::GroupBody(body) => format!("group-b-{}", body.key),
            LibraryItem::CardRow(row) => format!(
                "row-{columns}-{}-{}",
                row.len(),
                row.first()
                    .map(|(_, course)| course.dir.display().to_string())
                    .unwrap_or_default()
            ),
            LibraryItem::ListRow(_, course) => format!("card-{}", course.dir.display()),
        }
    }
}

/// Per-row metadata for the library list's render closure.
struct LibraryRowContext<'a> {
    index: usize,
    last: bool,
    focus: Option<&'a FocusHandle>,
    layout: LibraryLayout,
}

#[derive(IntoElement)]
struct LibraryDiagnostics {
    id: SharedString,
    messages: Vec<String>,
}

impl RenderOnce for LibraryDiagnostics {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state(
            SharedString::from(format!("library-diagnostics-state:{}", self.id)),
            cx,
            |_, _| false,
        );
        let open = *state.read(cx);
        v_flex()
            .w_full()
            .min_w_0()
            .gap_2()
            .child(
                quiet(SharedString::from(format!(
                    "library-diagnostics-toggle:{}",
                    self.id
                )))
                .self_start()
                .icon(if open {
                    IconName::ChevronUp
                } else {
                    IconName::Info
                })
                .label(if open { "收起诊断" } else { "查看诊断" })
                .on_click(move |_, _, cx| {
                    state.update(cx, |open, cx| {
                        *open = !*open;
                        cx.notify();
                    });
                }),
            )
            .child(disclosure(
                SharedString::from(format!("library-diagnostics-content:{}", self.id)),
                open,
                v_flex()
                    .w_full()
                    .min_w_0()
                    .gap_2()
                    .p_3()
                    .bg(color(INSET))
                    .rounded(RADIUS_SMALL)
                    .children(
                        self.messages
                            .into_iter()
                            .enumerate()
                            .map(|(index, message)| {
                                accessible_text(
                                    SharedString::from(format!(
                                        "library-diagnostic:{}:{index}",
                                        self.id
                                    )),
                                    message,
                                )
                                .w_full()
                                .min_w_0()
                                .whitespace_normal()
                                .text_size(TEXT_AUX)
                                .font_weight(FontWeight::NORMAL)
                                .text_color(color(GRAY))
                            }),
                    ),
                window,
                cx,
            ))
    }
}

#[derive(Clone)]
pub(super) struct CourseLocation {
    pub id: String,
    pub root: PathBuf,
    pub relative: PathBuf,
}

/// Filesystem facts are collected by the library worker and read by the UI.
#[derive(Default)]
pub(super) struct LibraryViewCache {
    pub locations: BTreeMap<PathBuf, CourseLocation>,
    pub recovery: BTreeMap<PathBuf, organize::Recovery>,
    pub title_recovery: BTreeMap<PathBuf, organize::Recovery>,
    aliases: BTreeMap<PathBuf, BTreeMap<PathBuf, String>>,
    alias_issues: Vec<String>,
}

impl LibraryViewCache {
    pub fn inspect(
        location: &workspace::LibraryLocation,
        scan: Option<&notes::LibraryScan>,
    ) -> Self {
        let mut cache = Self::default();
        if let Ok(Some(recovery)) = organize::recovery(&location.root) {
            cache.recovery.insert(location.root.clone(), recovery);
        }
        if let Ok(Some(recovery)) = organize::title_recovery(&location.root) {
            cache.title_recovery.insert(location.root.clone(), recovery);
        }
        match organize::title_aliases(&location.root) {
            Ok(names) => {
                cache.aliases.insert(location.root.clone(), names);
            }
            Err(error) => cache
                .alias_issues
                .push(format!("{}的显示名称尚未读取：{error:#}", location.name)),
        }
        if let Some(scan) = scan {
            for course in &scan.courses {
                let storage = course.storage_dir();
                let Ok(relative) = organize::relative_key(&location.root, &storage) else {
                    continue;
                };
                let membership = CourseLocation {
                    id: location.id.clone(),
                    root: location.root.clone(),
                    relative: relative.clone(),
                };
                // Imported versions can use canonical paths while a registered
                // root uses a symlink or /tmp alias. Resolve once in this worker.
                let mut paths = vec![
                    course.dir.clone(),
                    storage.clone(),
                    location.root.join(relative),
                ];
                paths.extend(storage.canonicalize().ok());
                paths.extend(course.dir.canonicalize().ok());
                for path in paths {
                    cache.locations.insert(path, membership.clone());
                }
            }
        }
        cache
    }

    pub fn merge(&mut self, other: Self) {
        for (path, location) in other.locations {
            let replace = self.locations.get(&path).is_none_or(|previous| {
                location.root.components().count() > previous.root.components().count()
            });
            if replace {
                self.locations.insert(path, location);
            }
        }
        self.recovery.extend(other.recovery);
        self.title_recovery.extend(other.title_recovery);
        self.aliases.extend(other.aliases);
        self.alias_issues.extend(other.alias_issues);
    }
}

struct CourseRenameDialog {
    desktop: Entity<Desktop>,
    input: Entity<InputState>,
    course: Course,
    root: PathBuf,
    error: Option<String>,
    _subscription: Subscription,
}

impl CourseRenameDialog {
    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.input.read(cx).value().to_string();
        match organize::rename_course(&self.root, &self.course.storage_dir(), &name) {
            Ok(()) => {
                self.desktop.update(cx, |desktop, cx| {
                    desktop.refresh_library(cx);
                    desktop.message = Some(format!("笔记已更名为「{}」。", name.trim()));
                    cx.notify();
                });
                window.close_dialog(cx);
            }
            Err(error) => {
                self.error = Some(format!("{error:#}"));
                self.input.update(cx, |state, cx| state.focus(window, cx));
                cx.notify();
            }
        }
    }
}

impl Render for CourseRenameDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("rename-note-dialog")
            .role(Role::Dialog)
            .aria_label("重命名笔记")
            .gap_3()
            .child(accessible_text("rename-note-label", "笔记名称"))
            .child(text_input(&self.input).aria_label("笔记名称"))
            .when_some(self.error.clone(), |view, error| {
                view.child(
                    accessible_text("rename-note-error", error)
                        .role(Role::Alert)
                        .text_color(color(DANGER)),
                )
            })
            .child(
                accessible_text("rename-note-scope", "仅修改笔记名称。")
                    .text_sm()
                    .text_color(color(MUTED)),
            )
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        control("cancel-note-rename")
                            .icon(IconName::Close)
                            .label("取消")
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        control("save-note-rename")
                            .icon(icons::edit())
                            .primary()
                            .label("保存名称")
                            .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                    ),
            )
    }
}

impl Desktop {
    pub fn course_display_title(&self, course: &Course) -> String {
        self.courses
            .iter()
            .find(|item| item.storage_dir() == course.storage_dir())
            .map(|item| item.title.clone())
            .unwrap_or_else(|| course.title.clone())
    }

    /// Apply the names already read by the library worker. The UI does no I/O.
    pub fn apply_course_title_aliases(&mut self) {
        // 幂等合并：不重复追加同一条诊断（打开笔记路径会再次调用本函数）
        for issue in self.library_view_cache.alias_issues.clone() {
            if !self.library_issues.contains(&issue) {
                self.library_issues.push(issue);
            }
        }
        for course in &mut self.courses {
            if let Some(location) = self.library_view_cache.locations.get(&course.storage_dir())
                && let Some(name) = self
                    .library_view_cache
                    .aliases
                    .get(&location.root)
                    .and_then(|names| names.get(&location.relative))
            {
                course.title = name.clone();
            }
        }
        let title = self
            .preview
            .as_ref()
            .map(|preview| self.course_display_title(&preview.course));
        if let (Some(preview), Some(title)) = (&mut self.preview, title) {
            preview.course.title = title;
        }
    }

    pub fn begin_course_rename(
        &mut self,
        course: Course,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(root) = self
            .course_location(&course)
            .map(|library| library.root.clone())
        else {
            self.message = Some("这份笔记的保存位置暂时无法确认，请重新检查课程库。".into());
            cx.notify();
            return;
        };
        let name = self.course_display_title(&course);
        let input = cx.new(|cx| InputState::new(window, cx));
        input.update(cx, |state, cx| state.set_value(name, window, cx));
        let desktop = cx.entity();
        let focus = input.clone();
        let content = cx.new(|cx| {
            let subscription = cx.subscribe_in(
                &input,
                window,
                |this: &mut CourseRenameDialog, _, event, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        this.save(window, cx);
                    }
                    if matches!(event, InputEvent::Change) {
                        this.error = None;
                        cx.notify();
                    }
                },
            );
            CourseRenameDialog {
                desktop,
                input,
                course,
                root,
                error: None,
                _subscription: subscription,
            }
        });
        window.open_dialog(cx, move |dialog, _, _| {
            dialog
                .title("重命名笔记")
                .w(px(480.))
                .overlay_closable(false)
                .child(content.clone())
        });
        window.defer(cx, move |window, cx| {
            focus.update(cx, |state, cx| state.focus(window, cx))
        });
    }

    fn course_actions(&self, course: Course, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let entity = cx.entity().downgrade();
        let folder_context = self.folder_context(Some(course.dir.clone()));
        control(("course-actions", index))
            .ghost()
            .icon(IconName::Ellipsis)
            .min_h(rems(2.))
            .tooltip("笔记操作")
            .accessibility_label(format!("《{}》的笔记操作", course.title))
            .dropdown_menu(move |menu, window, cx| {
                let rename = entity.clone();
                let selected = course.clone();
                let path = course.storage_dir();
                let mut menu = menu
                    .item(
                        PopupMenuItem::new("重命名笔记…")
                            .icon(icons::edit())
                            .on_click(move |_, window, cx| {
                                let _ = rename.update(cx, |this, cx| {
                                    this.begin_course_rename(selected.clone(), window, cx)
                                });
                            }),
                    )
                    .item(
                        PopupMenuItem::new("打开笔记保存位置")
                            .icon(IconName::FolderOpen)
                            .on_click(move |_, _, cx| cx.open_with_system(&path)),
                    )
                    .separator();
                for (format, label) in [
                    (course2md::config::OutputFormat::Md, "导出 Markdown 包…"),
                    (course2md::config::OutputFormat::Html, "导出网页文件…"),
                    (course2md::config::OutputFormat::Json, "导出结构化数据…"),
                ] {
                    let entity = entity.clone();
                    let course = course.clone();
                    menu = menu.item(PopupMenuItem::new(label).icon(icons::download()).on_click(
                        move |_, window, cx| {
                            let _ = entity.update(cx, |this, cx| {
                                this.export_course(course.clone(), format, window, cx)
                            });
                        },
                    ));
                }
                // 归属修改收入溢出菜单：不与分组标题/筛选轨重复（design D3）
                let context = folder_context.clone();
                Desktop::folder_menu(
                    entity.clone(),
                    context.origin,
                    context.storage,
                    context.folders,
                    context.folder,
                )(menu.separator(), window, cx)
            })
            .into_any_element()
    }

    fn recover_library_folders(&mut self, root: PathBuf, rebuild: bool, cx: &mut Context<Self>) {
        match organize::recover(&root, rebuild) {
            Ok((library, preserved)) => {
                self.library_indexes.insert(root.clone(), library.clone());
                if self.library_root == root {
                    self.library = library;
                    self.library_error = None;
                }
                self.message = Some(format!(
                    "分类已{}，原损坏记录保留在 {}。笔记正文和显示名称仍保留。",
                    if rebuild { "重建" } else { "恢复" },
                    preserved.display()
                ));
                self.refresh_library(cx);
            }
            Err(error) => self.message = Some(format!("分类尚未恢复：{error:#}")),
        }
        cx.notify();
    }

    fn library_recovery_view(
        &self,
        access: &crate::storage::LibraryAccess,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        let mut view = v_flex().gap_3();
        let workspace = self.workspace.as_ref()?;
        let mut has_notice = false;
        for (index, location) in workspace.state.libraries.iter().enumerate() {
            if access.unavailable.contains(&location.root) {
                has_notice = true;
                let name = location.name.clone();
                let root = location.root.clone();
                view = view.child(
                    v_flex()
                        .gap_3()
                        .p_4()
                        .bg(color(WARNING_BG))
                        .rounded(RADIUS_CARD)
                        .child(badge(BadgeKind::Warning).child("保存位置不可用"))
                        .child(accessible_text(
                            ("library-unavailable", index),
                            format!("{name} 暂时无法访问。连接后可继续浏览笔记。"),
                        ))
                        .child(
                            control(("retry-library-location", index))
                                .icon(icons::refresh())
                                .label("重新检查保存位置")
                                .tooltip(root.display().to_string())
                                .self_start()
                                .loading(self.loading)
                                .on_click(cx.listener(|this, _, _, cx| this.refresh_library(cx))),
                        ),
                );
                continue;
            }
            if let Some(recovery) = self.library_view_cache.recovery.get(&location.root) {
                has_notice = true;
                let restore = location.root.clone();
                let rebuild = location.root.clone();
                let folder = location.root.clone();
                let prefix = format!("{}的文件夹记录暂时无法读取：", location.name);
                let diagnostics: Vec<_> = self
                    .library_issues
                    .iter()
                    .filter(|issue| issue.starts_with(&prefix))
                    .cloned()
                    .collect();
                let mut row = h_flex().gap_2().flex_wrap();
                if recovery.has_backup {
                    row = row.child(
                        control(("restore-classification", index))
                            .primary()
                            .icon(icons::history())
                            .label("恢复最近分类备份")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.recover_library_folders(restore.clone(), false, cx)
                            })),
                    );
                }
                row = row
                    .child(
                        control(("rebuild-classification", index))
                            .when(!recovery.has_backup, |button| button.primary())
                            .icon(icons::refresh())
                            .label("保留损坏记录并重建分类")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.recover_library_folders(rebuild.clone(), true, cx)
                            })),
                    )
                    .child(
                        control(("show-classification-files", index))
                            .ghost()
                            .icon(IconName::FolderOpen)
                            .label("打开保存位置")
                            .on_click(move |_, _, cx| cx.open_with_system(&folder)),
                    );
                view = view.child(
                    v_flex()
                        .gap_3()
                        .p_4()
                        .bg(color(WARNING_BG))
                        .rounded(RADIUS_CARD)
                        .child(badge(BadgeKind::Warning).child("分类需要恢复"))
                        .child(accessible_text(
                            ("classification-recovery", index),
                            format!(
                                "{} 的分类记录无法读取。已读取的笔记仍可阅读，文件夹分类暂不可用。",
                                location.name
                            ),
                        ))
                        .child(row)
                        .when(!diagnostics.is_empty(), |view| {
                            view.child(LibraryDiagnostics {
                                id: SharedString::from(format!("classification:{}", location.id)),
                                messages: diagnostics,
                            })
                        }),
                );
            }
            if let Some(recovery) = self.library_view_cache.title_recovery.get(&location.root) {
                has_notice = true;
                let prefix = format!("{}的显示名称尚未读取：", location.root.display());
                let diagnostics: Vec<_> = self
                    .library_issues
                    .iter()
                    .filter(|issue| issue.starts_with(&prefix))
                    .cloned()
                    .collect();
                let mut row = h_flex().gap_2().flex_wrap();
                for reset in [false, true] {
                    if !reset && !recovery.has_backup {
                        continue;
                    }
                    let root = location.root.clone();
                    row = row.child(
                        control((
                            if reset {
                                "reset-note-names"
                            } else {
                                "restore-note-names"
                            },
                            index,
                        ))
                        .icon(if reset {
                            icons::refresh()
                        } else {
                            icons::history()
                        })
                        .label(if reset {
                            "保留损坏记录并恢复原名称"
                        } else {
                            "恢复最近名称备份"
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            match organize::recover_titles(&root, reset) {
                                Ok(path) => {
                                    this.message = Some(format!(
                                        "笔记名称已恢复，损坏记录保留在 {}。正文和分类保持完整。",
                                        path.display()
                                    ));
                                    this.refresh_library(cx);
                                }
                                Err(error) => {
                                    this.message = Some(format!("名称尚未恢复：{error:#}"))
                                }
                            }
                            cx.notify();
                        })),
                    );
                }
                view = view.child(
                    v_flex()
                        .gap_3()
                        .p_4()
                        .bg(color(WARNING_BG))
                        .rounded(RADIUS_CARD)
                        .child(badge(BadgeKind::Warning).child("笔记名称需要恢复"))
                        .child(accessible_text(
                            ("title-recovery", index),
                            format!(
                                "{} 的笔记名称记录无法读取，现有正文文件会保留。",
                                location.name
                            ),
                        ))
                        .child(row)
                        .when(!diagnostics.is_empty(), |view| {
                            view.child(LibraryDiagnostics {
                                id: SharedString::from(format!("names:{}", location.id)),
                                messages: diagnostics,
                            })
                        }),
                );
            }
        }
        has_notice.then_some(view)
    }

    pub fn library_page(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let scale = self.preferences.application().font_scale;
        let rem = f32::from(window.rem_size());
        let content = crate::views::shell_content_width(Page::Library, window);
        // gap_4 is one rem, and card padding/menu spacing scales with that rem.
        let columns = ((content + rem) / (20. * rem + rem)).floor().max(1.) as usize;
        let layout = LibraryLayout {
            columns,
            compact: content < 336. * scale,
            stacked: content < 56. * rem + 24.,
        };
        let query = self.value(Field::Search, cx).to_lowercase();
        let mut items = Vec::new();
        let Some(all_access) = self.cached_library_access() else {
            items.push(LibraryItem::Checking);
            return self.library_list_page(items, layout, rem, cx);
        };
        let in_scope =
            |root: &&PathBuf| self.folder_filter.is_none() || **root == self.library_root;
        let scope = crate::storage::LibraryAccess {
            available: all_access
                .available
                .iter()
                .filter(in_scope)
                .cloned()
                .collect(),
            unavailable: all_access
                .unavailable
                .iter()
                .filter(in_scope)
                .cloned()
                .collect(),
        };
        let coverage = scope.coverage();
        let courses: Vec<_> = self
            .courses
            .iter()
            .enumerate()
            .filter(|(_, course)| {
                self.course_location(course)
                    .map(|location| scope.available.contains(&location.root))
                    .unwrap_or_else(|| {
                        self.workspace.is_none()
                            && self
                                .library_view_cache
                                .locations
                                .get(&course.storage_dir())
                                .is_some_and(|location| scope.available.contains(&location.root))
                    })
                    && course.title.to_lowercase().contains(&query)
                    && self.folder_filter.is_none_or(|id| {
                        self.course_location(course)
                            .is_some_and(|location| location.root == self.library_root)
                            && (!self.library_indexes.contains_key(&self.library_root)
                                || self.course_folder(course).unwrap_or(0) == id)
                    })
            })
            .map(|(index, course)| (index, course.clone()))
            .collect();
        if self.library_recovery_view(&all_access, cx).is_some() {
            items.push(LibraryItem::Recovery);
        }
        if coverage != crate::storage::LibraryCoverage::Complete {
            let message = if coverage == crate::storage::LibraryCoverage::Unavailable {
                if query.is_empty() {
                    "当前范围的保存位置暂时无法访问，笔记列表尚未读取。重新连接后可继续浏览。"
                        .to_owned()
                } else {
                    "当前范围的保存位置暂时无法访问，尚未搜索笔记。关键词已保留，重新连接后可继续搜索。".to_owned()
                }
            } else if query.is_empty() {
                format!(
                    "当前仅显示 {} 个可访问位置中的笔记；另外 {} 个位置尚未读取。",
                    scope.available.len(),
                    scope.unavailable.len()
                )
            } else {
                format!(
                    "当前仅搜索 {} 个可访问的保存位置；另外 {} 个位置尚未纳入搜索结果。",
                    scope.available.len(),
                    scope.unavailable.len()
                )
            };
            items.push(LibraryItem::Coverage(message));
        }
        // Recovery cards own their matching diagnostics. Keep unrelated read
        // failures visible without repeating the same classification warning.
        let remaining_issues: Vec<_> = self
            .library_issues
            .iter()
            .filter(|issue| {
                !self.workspace.as_ref().is_some_and(|workspace| {
                    workspace.state.libraries.iter().any(|location| {
                        !all_access.unavailable.contains(&location.root)
                            && ((self
                                .library_view_cache
                                .recovery
                                .contains_key(&location.root)
                                && issue.starts_with(&format!(
                                    "{}的文件夹记录暂时无法读取：",
                                    location.name
                                )))
                                || (self
                                    .library_view_cache
                                    .title_recovery
                                    .contains_key(&location.root)
                                    && issue.starts_with(&format!(
                                        "{}的显示名称尚未读取：",
                                        location.root.display()
                                    ))))
                    })
                })
            })
            .cloned()
            .collect();
        if !remaining_issues.is_empty() && coverage != crate::storage::LibraryCoverage::Unavailable
        {
            items.push(LibraryItem::Issues(remaining_issues));
        }
        // 刷新期间保留已加载的列表：Loading 只是一行状态，不再整页替换；
        // 只有首次加载（没有任何已加载内容）时才让 Loading 独占页面
        if self.loading {
            items.push(LibraryItem::Loading);
        }
        if self.loading && courses.is_empty() {
            return self.library_list_page(items, layout, rem, cx);
        }
        if coverage == crate::storage::LibraryCoverage::Unavailable {
            return self.library_list_page(items, layout, rem, cx);
        }
        if courses.is_empty() {
            if query.is_empty()
                && (coverage == crate::storage::LibraryCoverage::Partial
                    || !self.library_issues.is_empty())
            {
                return self.library_list_page(items, layout, rem, cx);
            }
            items.push(LibraryItem::Empty {
                partial: coverage == crate::storage::LibraryCoverage::Partial,
            });
            return self.library_list_page(items, layout, rem, cx);
        }
        if !query.is_empty() {
            let raw = self.value(Field::Search, cx);
            let scoped = if self.folder_filter.is_some() {
                format!(
                    "在当前文件夹中搜索「{raw}」，找到 {} 篇笔记。",
                    courses.len()
                )
            } else {
                format!("在全部笔记中搜索「{raw}」，找到 {} 篇笔记。", courses.len())
            };
            items.push(LibraryItem::SearchScope(scoped));
        }
        if self.desktop_settings.library_group_folders
            && self.folder_filter.is_none()
            && self.library_error.is_none()
        {
            let mut groups: BTreeMap<(PathBuf, u64), Vec<(usize, Course)>> = BTreeMap::new();
            for entry in courses {
                let root = self
                    .course_location(&entry.1)
                    .map(|lib| lib.root.clone())
                    .unwrap_or_else(|| self.library_root.clone());
                groups
                    .entry((root, self.course_folder(&entry.1).unwrap_or(0)))
                    .or_default()
                    .push(entry);
            }
            for (group_index, ((root, id), entries)) in groups.into_iter().enumerate() {
                let location = self
                    .workspace
                    .as_ref()
                    .and_then(|w| w.state.libraries.iter().find(|lib| lib.root == root));
                let key = format!(
                    "{}:{id}",
                    location
                        .map(|lib| lib.id.clone())
                        .unwrap_or_else(|| root.display().to_string())
                );
                let collapsed = query.is_empty()
                    && self
                        .workspace
                        .as_ref()
                        .is_some_and(|w| w.state.collapsed.contains(&key));
                let folder_name = self
                    .library_indexes
                    .get(&root)
                    .and_then(|lib| lib.folders.get(&id))
                    .cloned()
                    .unwrap_or_else(|| {
                        if self.library_indexes.contains_key(&root) {
                            "未分类"
                        } else {
                            "分类记录暂不可用"
                        }
                        .into()
                    });
                let name = if self
                    .workspace
                    .as_ref()
                    .is_some_and(|w| w.state.libraries.len() > 1)
                {
                    format!(
                        "{} · {folder_name}",
                        location.map(|lib| lib.name.as_str()).unwrap_or("课程库")
                    )
                } else {
                    folder_name
                };
                items.push(LibraryItem::GroupHeader(Box::new(LibraryGroupHeader {
                    index: group_index,
                    key: key.clone(),
                    name,
                    count: entries.len(),
                    collapsed,
                })));
                items.push(LibraryItem::GroupBody(Box::new(LibraryGroupBody {
                    index: group_index,
                    key,
                    collapsed,
                    entries,
                })));
            }
        } else if !self.desktop_settings.library_cards || layout.stacked {
            items.extend(
                courses
                    .into_iter()
                    .map(|(index, course)| LibraryItem::ListRow(index, Box::new(course))),
            );
        } else {
            items.extend(
                courses
                    .chunks(layout.columns)
                    .map(|row| LibraryItem::CardRow(row.to_vec())),
            );
        }
        if !items
            .iter()
            .any(|item| matches!(item, LibraryItem::Empty { .. }))
        {
            self.entered.remove("library-empty-state");
        }
        self.library_list_page(items, layout, rem, cx)
    }

    /// The library's variable-height list: notices, folder groups and card
    /// rows share one scroll region, spliced by identity so measured heights
    /// and the scroll anchor survive content updates.
    fn library_list_page(
        &mut self,
        items: Vec<LibraryItem>,
        layout: LibraryLayout,
        rem: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let flat = items.iter().find_map(|item| match item {
            LibraryItem::CardRow(row) => {
                Some((true, row.first().map(|(index, _)| *index).unwrap_or(0)))
            }
            LibraryItem::ListRow(index, _) => Some((false, *index)),
            _ => None,
        });
        let keys = items.iter().map(|item| item.key(layout.columns)).collect();
        Self::reconcile_list_items(
            &self.library_list,
            &mut self.library_keys,
            &mut self.library_focus,
            keys,
            cx,
        );
        if self.library_rem != rem {
            self.library_list.remeasure();
            self.library_rem = rem;
        }
        let focus: Rc<Vec<FocusHandle>> = Rc::new(self.library_focus.clone());
        let state = self.library_list.clone();
        let items = Rc::new(items);
        let desktop = cx.weak_entity();
        let element = list(state, move |index, window, cx| {
            let Some(item) = items.get(index) else {
                return div().into_any_element();
            };
            let last = index + 1 == items.len();
            let row = LibraryRowContext {
                index,
                last,
                focus: focus.get(index),
                layout,
            };
            desktop
                .update(cx, |this, cx| this.library_item(item, row, window, cx))
                .unwrap_or_else(|_| div().into_any_element())
        })
        .w_full()
        .h_full()
        .flex_1()
        .min_h_0()
        .pb_6();
        if let Some((grid, id)) = flat {
            crate::motion::enter(
                if grid {
                    ("library-grid", id)
                } else {
                    ("library-list", id)
                },
                element)
        } else {
            element.into_any_element()
        }
    }

    fn library_item(
        &mut self,
        item: &LibraryItem,
        row: LibraryRowContext,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (index, last, focus, layout) = (row.index, row.last, row.focus, row.layout);
        let content = match item {
            LibraryItem::Checking => h_flex()
                .w_full()
                .gap_2()
                .items_center()
                .py_6()
                .child(crate::motion::spinner("library-location-check-spinner", cx))
                .child(accessible_text(
                    "library-location-checking-label",
                    "正在检查保存位置…",
                ))
                .into_any_element(),
            LibraryItem::Recovery => self
                .cached_library_access()
                .and_then(|access| self.library_recovery_view(&access, cx))
                .unwrap_or_else(div)
                .into_any_element(),
            LibraryItem::Coverage(message) => {
                accessible_text("library-search-coverage", message.clone())
                    .text_sm()
                    .text_color(color(MUTED))
                    .into_any_element()
            }
            LibraryItem::Issues(messages) => v_flex()
                .gap_2()
                .p_3()
                .bg(color(WARNING_BG))
                .border_1()
                .border_color(color(WARNING_BG))
                .rounded(RADIUS_CARD)
                .child(badge(BadgeKind::Warning).child("部分内容暂未读取"))
                .child(accessible_text(
                    "library-issues-title",
                    "已读取的笔记仍可阅读。请检查保存位置后重新检查；当前列表和搜索仅包含已读取的内容。",
                ))
                .child(
                    control("retry-unread-library-content")
                        .self_start()
                        .icon(icons::refresh())
                        .label("重新检查")
                        .loading(self.loading)
                        .on_click(cx.listener(|this, _, _, cx| this.refresh_library(cx))),
                )
                .child(LibraryDiagnostics {
                    id: "unread-content".into(),
                    messages: messages.clone(),
                })
                .into_any_element(),
            LibraryItem::Loading => h_flex()
                .gap_2()
                .items_center()
                .py_6()
                .child(crate::motion::spinner("library-scan-spinner", cx))
                .child(accessible_text("library-loading", "正在读取笔记…"))
                .into_any_element(),
            LibraryItem::Empty { partial } => {
                let partial = *partial;
                let query = self.value(Field::Search, cx).to_lowercase();
                let content = v_flex()
                    .py_12()
                    .px_6()
                    .gap_4()
                    .items_center()
                    .text_center()
                    .child(
                        Icon::new(IconName::BookOpen)
                            .size(px(32.))
                            .text_color(color(MUTED)),
                    )
                    .child(
                        accessible_text(
                            "library-empty-heading",
                            if !query.is_empty() {
                                if partial || !self.library_issues.is_empty() {
                                    "已读取的笔记中没有匹配的笔记。"
                                } else {
                                    "没有匹配的笔记"
                                }
                            } else if self.folder_filter.is_some() {
                                "这个文件夹还没有笔记"
                            } else {
                                "还没有笔记"
                            },
                        )
                        .text_lg()
                        .font_weight(FontWeight::SEMIBOLD),
                    )
                    .child(
                        accessible_text(
                            "library-empty-description",
                            if !query.is_empty() {
                                if partial {
                                    "未连接的位置尚未搜索。可以重新连接保存位置，或调整关键词。"
                                } else {
                                    "试试更短的关键词。"
                                }
                            } else if self.folder_filter.is_some() {
                                "从全部笔记中选择内容，移到这个文件夹。"
                            } else {
                                "导入视频，生成的内容会保存在这里"
                            },
                        )
                        .text_color(color(MUTED)),
                    )
                    .when(!query.is_empty(), |empty| {
                        empty.child(
                            outline_pill("clear-search")
                                .icon(IconName::Close)
                                .label("清除搜索")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.inputs[&Field::Search].update(cx, |state, cx| {
                                        state.set_value("", window, cx)
                                    });
                                    cx.notify();
                                })),
                        )
                    })
                    .when(query.is_empty() && self.folder_filter.is_none(), |empty| {
                        empty.child(
                            primary_pill("empty-library-add")
                                .icon(IconName::Plus)
                                .label("导入视频")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.begin_add(window, cx)
                                })),
                        )
                    })
                    .when(query.is_empty() && self.folder_filter.is_some(), |empty| {
                        empty.child(
                            outline_pill("empty-folder-all-notes")
                                .icon(IconName::BookOpen)
                                .label("浏览全部笔记")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.folder_filter = None;
                                    this.library_list.scroll_to(ListOffset {
                                        item_ix: 0,
                                        offset_in_item: px(0.),
                                    });
                                    cx.notify();
                                })),
                        )
                    });
                if self.enter_once("library-empty-state".to_owned()) {
                    crate::motion::enter("library-empty-state", content)
                } else {
                    content.into_any_element()
                }
            }
            LibraryItem::SearchScope(scoped) => h_flex()
                .items_baseline()
                .gap_3()
                .flex_wrap()
                .child(
                    accessible_text("library-search-scope", scoped.clone())
                        .text_size(TEXT_AUX)
                        .text_color(color(GRAY)),
                )
                .child(
                    quiet("clear-search-scope")
                        .icon(IconName::Close)
                        .label("清空搜索")
                        .min_h(rems(1.6))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.inputs[&Field::Search]
                                .update(cx, |state, cx| state.set_value("", window, cx));
                            cx.notify();
                        })),
                )
                .into_any_element(),
            LibraryItem::GroupHeader(header) => {
                let LibraryGroupHeader {
                    index,
                    key,
                    name,
                    count,
                    collapsed,
                } = header.as_ref();
                let key = key.clone();
                let collapsed = *collapsed;
                let index = *index;
                control(("library-group", index))
                    .ghost()
                    .w_full()
                    .justify_start()
                    .min_h(rems(1.6))
                    .p_0()
                    .accessibility_label(format!(
                        "{} {name}，{} 篇笔记",
                        if collapsed { "展开" } else { "收起" },
                        count
                    ))
                    .child(
                        h_flex()
                            .w_full()
                            .gap_2()
                            .items_baseline()
                            .child(
                                Icon::new(if collapsed {
                                    IconName::ChevronRight
                                } else {
                                    IconName::ChevronDown
                                })
                                .size(px(12.))
                                .text_color(color(GRAY))
                                .flex_shrink_0(),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(name.clone()),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_size(TEXT_AUX)
                                    .text_color(color(GRAY))
                                    .child(count.to_string()),
                            ),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(workspace) = &mut this.workspace {
                            match workspace.transaction(|state| {
                                let became = !state.collapsed.remove(&key);
                                if became {
                                    state.collapsed.insert(key.clone());
                                }
                                Ok(became)
                            }) {
                                Ok(true) => {
                                    this.entered.remove(&format!("folder-disclosure-{index}"));
                                }
                                Ok(false) => {}
                                Err(error) => {
                                    this.workspace_error =
                                        Some(format!("分组展开状态尚未保存：{error:#}"));
                                }
                            }
                        }
                        cx.notify();
                    }))
                    .into_any_element()
            }
            LibraryItem::GroupBody(body) => {
                let collapsed = body.collapsed;
                let index = body.index;
                let animate = !collapsed && self.enter_once(format!("folder-disclosure-{index}"));
                let collection = self.course_collection(&body.entries, layout, animate, cx);
                if animate {
                    disclosure(("folder-disclosure", index), true, collection, window, cx)
                } else if collapsed {
                    div().hidden().into_any_element()
                } else {
                    collection.into_any_element()
                }
            }
            LibraryItem::CardRow(row) => self.library_card_row(row, layout, cx).into_any_element(),
            LibraryItem::ListRow(index, course) => self
                .library_list_row(index, course.as_ref(), layout, cx)
                .into_any_element(),
        };
        // The old single scroll column spaced its children with gap_6, folder
        // groups with gap_2 and 20px between groups, card rows with gap_4 and
        // list rows with gap_2; each item carries the same trailing spacing.
        let mut wrapper = v_flex().w_full().min_w_0();
        if !last {
            wrapper = match item {
                LibraryItem::GroupHeader { .. } => wrapper.mb_2(),
                LibraryItem::GroupBody { .. } => wrapper.mb(px(20.)),
                LibraryItem::CardRow(_) => wrapper.mb_4(),
                LibraryItem::ListRow(..) => wrapper.mb_2(),
                _ => wrapper.mb_6(),
            };
        }
        let mut wrapper = wrapper
            .child(content)
            .id(("library-item", index))
            .tab_stop(false);
        if let Some(focus) = focus {
            wrapper = wrapper.track_focus(focus);
        }
        wrapper.into_any_element()
    }

    /// Sidebar successor: folder filter + creation entry, final form with M5.
    fn folder_filter_control(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let multi = self
            .workspace
            .as_ref()
            .is_some_and(|w| w.state.libraries.len() > 1);
        let mut sections: Vec<LibrarySection> = self
            .workspace
            .as_ref()
            .map(|w| w.state.libraries.clone())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|location| {
                self.library_indexes
                    .get(&location.root)
                    .map(|organization| {
                        (
                            location.root.clone(),
                            location.name.clone(),
                            organization
                                .folders
                                .iter()
                                .map(|(id, name)| (*id, name.clone()))
                                .collect(),
                        )
                    })
            })
            .collect();
        if sections.is_empty() {
            sections.push((
                self.library_root.clone(),
                "课程库".to_owned(),
                self.library
                    .folders
                    .iter()
                    .map(|(id, name)| (*id, name.clone()))
                    .collect(),
            ));
        }
        let options = library_filter_options(&sections, &self.library_root, multi);
        let selected = library_filter_selected(&options, &self.library_root, self.folder_filter);
        let choices: Vec<(String, String)> = options
            .iter()
            .map(|option| (option.value.clone(), option.label.clone()))
            .collect();
        let mut group = SingleChoiceGroup::new("folder-filter", "文件夹筛选")
            .options(choices)
            .selected(selected);
        // 真实文件夹用对象图标，与「全部/未分类」的集合过滤分开（review2#5）
        for option in &options {
            if option.folder.is_some_and(|id| id != 0) {
                group = group.icon(option.value.clone(), icons::folder());
            }
        }
        group
            .on_change(cx.listener({
                let options = options.clone();
                move |this, value: &SharedString, _, cx| {
                    let Some(option) = options.iter().find(|option| option.value == value.as_ref())
                    else {
                        return;
                    };
                    this.library_root = option.root.clone();
                    if let Some(organization) = this.library_indexes.get(&option.root) {
                        this.library = organization.clone();
                    }
                    this.folder_filter = option.folder;
                    this.scrolls[Page::Library as usize].set_offset(point(px(0.), px(0.)));
                    cx.notify();
                }
            }))
    }

    pub(super) fn library_controls_visible(&self, cx: &App) -> bool {
        !(self.courses.is_empty()
            && self.folder_filter.is_none()
            && self.value(Field::Search, cx).is_empty()
            && self.library_error.is_none()
            && self.library_issues.is_empty()
            && !self.loading)
    }

    /// Library controls share Material action icons and theme-aware state surfaces.
    pub fn library_toolbar(&self, cx: &mut Context<Self>) -> Div {
        if !self.library_controls_visible(cx) {
            return div();
        }
        let group_on = self.desktop_settings.library_group_folders;
        let cards_on = self.desktop_settings.library_cards;
        let can_group = self.folder_filter.is_none() && self.library_error.is_none();
        let controls = h_flex()
            .gap_2()
            .flex_wrap()
            .min_w_0()
            .max_w_full()
            .flex_shrink_0()
            .items_center()
            .child(self.folder_filter_control(cx))
            .when_some(self.folder_filter.filter(|id| *id != 0), |row, id| {
                let entity = cx.entity().downgrade();
                row.child(
                    control("manage-folder")
                        .rounded(RADIUS_PILL)
                        .px(px(12.))
                        .icon(icons::edit())
                        .label("管理文件夹")
                        .dropdown_menu(move |menu, _, _| {
                            let rename = entity.clone();
                            let remove = entity.clone();
                            menu.item(PopupMenuItem::new("重命名").icon(icons::edit()).on_click(
                                move |_, window, cx| {
                                    let _ = rename.update(cx, |this, cx| {
                                        this.begin_folder(Some(id), window, cx)
                                    });
                                },
                            ))
                            .item(
                                PopupMenuItem::new("删除文件夹…")
                                    .icon(icons::delete())
                                    .on_click(move |_, window, cx| {
                                        let _ = remove.update(cx, |this, cx| {
                                            this.begin_delete_folder(id, window, cx)
                                        });
                                    }),
                            )
                        }),
                )
            })
            .child(
                SingleChoiceGroup::new("library-layout", "笔记显示方式")
                    .options([("list", "列表"), ("cards", "卡片")])
                    .selected(library_layout_choice(cards_on))
                    .on_change(cx.listener(|this, value: &SharedString, _, cx| {
                        this.desktop_settings.library_cards = value.as_ref() == "cards";
                        this.save_library_presentation(cx);
                    })),
            )
            .when(can_group, |row| {
                // 与「列表/卡片」拉开：两组选择不读成一段多选（review2#5）
                row.child(
                    div().ml_4().child(
                        SingleChoiceGroup::new("library-grouping", "笔记分组方式")
                        .options([("flat", "平铺"), ("group", "分组")])
                        .selected(if group_on { "group" } else { "flat" })
                        .on_change(cx.listener(|this, value: &SharedString, _, cx| {
                            this.desktop_settings.library_group_folders =
                                value.as_ref() == "group";
                            this.save_library_presentation(cx);
                        })),
                    ),
                )
            });
        h_flex()
            .w_full()
            .min_w_0()
            .flex_wrap()
            .gap_2()
            .pb_4()
            .child(
                div()
                    .flex_1()
                    .flex_basis(rems(10.))
                    .min_w_0()
                    .max_w_full()
                    .child(
                        text_input(&self.inputs[&Field::Search])
                            .aria_label("搜索笔记标题")
                            .w_full()
                            .border_color(color(CONTROL))
                            .text_size(TEXT_BODY)
                            .prefix(
                                icons::search()
                                    .size(rems(18. / 14.))
                                    .text_color(color(GRAY)),
                            )
                            .cleanable(true),
                    ),
            )
            .child(controls)
    }

    /// Source identity helps readers recognize a note; routine revision numbers
    /// remain in the reader's version history. This uses already-cached task data.
    fn course_meta(&self, course: &Course) -> String {
        let ms = course
            .manifest
            .as_ref()
            .map(|manifest| manifest.created_at_ms)
            .unwrap_or_else(|| {
                course
                    .modified
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|duration| duration.as_secs() * 1000)
                    .unwrap_or(0)
            });
        let stamp = crate::reader_navigation::timestamp_local(ms);
        let mut parts = Vec::new();
        // 「全部笔记」下的对象归属：文件夹在元信息里可见（review2#5）；
        // 分组视图已由组头表达，不重复
        if !self.desktop_settings.library_group_folders
            && let Some(folder_id) = self.course_folder(course).filter(|id| *id != 0)
            && let Some(name) = self
                .course_location(course)
                .and_then(|location| self.library_indexes.get(&location.root))
                .and_then(|library| library.folders.get(&folder_id))
        {
            parts.push(name.clone());
        }
        if let Some(source) = course.manifest.as_ref().and_then(|manifest| {
            self.workspace
                .as_ref()?
                .state
                .tasks
                .iter()
                .find(|task| task.id == manifest.task_id)
                .map(|task| &task.plan.source)
        }) {
            if !source.author.trim().is_empty() {
                parts.push(source.author.clone());
            }
            if source.duration.is_finite() && source.duration > 0. {
                parts.push(course2md::render::fmt_ts(source.duration));
            }
            if !source.online {
                parts.push("本地视频".into());
            }
        }
        parts.push(stamp.get(..10).unwrap_or(&stamp).to_owned());
        parts.join(" · ")
    }

    fn course_has_reading_position(&self, course: &Course) -> bool {
        course.manifest.as_ref().is_some_and(|manifest| {
            let key = crate::reader_ui::manifest_reading_key(
                &manifest.course_id,
                &manifest.version_id,
                0,
            );
            self.workspace
                .as_ref()
                .and_then(|workspace| workspace.state.positions.get(&key))
                .is_some_and(|position| {
                    position.offset < -8. || position.seconds.is_some_and(|seconds| seconds > 0.)
                })
        })
    }

    fn course_read_error(
        &self,
        course: &Course,
        index: usize,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        let (_, message) = self
            .reader_course_error
            .as_ref()
            .filter(|(path, _)| path == &course.dir)?;
        let retry = course.clone();
        let path = course.storage_dir();
        Some(
            v_flex()
                .w_full()
                .gap_2()
                .px_4()
                .pb_3()
                .child(
                    theme::accessible_text(("course-read-error", index), message.clone())
                        .text_size(TEXT_AUX)
                        .text_color(color(WARNING)),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .flex_wrap()
                        .child(
                            quiet(("retry-course-read", index))
                                .icon(icons::refresh())
                                .label("重试打开")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.open_course(retry.clone(), cx)
                                })),
                        )
                        .child(
                            quiet(("reveal-unread-course", index))
                                .icon(icons::folder_open())
                                .label("检查保存位置")
                                .on_click(move |_, _, cx| cx.reveal_path(&path)),
                        ),
                ),
        )
    }

    /// Note collections: white SURFACE
    /// cards with a CARD_LINE hairline and RADIUS_CARD corners; covers render only
    /// when real thumbnail data exists (no placeholder block).
    fn course_collection(
        &self,
        courses: &[(usize, Course)],
        layout: LibraryLayout,
        animate: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let collection_id = courses.first().map(|(index, _)| *index).unwrap_or(0);
        let collection = v_flex().w_full();
        if !self.desktop_settings.library_cards || layout.stacked {
            let rows = v_flex()
                .gap_2()
                .children(courses.iter().map(|(index, course)| {
                    self.library_list_row(index, course, layout, cx)
                }));
            return if animate {
                collection.child(crate::motion::enter(
                    ("library-list", collection_id),
                    rows))
            } else {
                collection.child(rows)
            };
        }
        let cards = v_flex().gap_4().children(
            courses
                .chunks(layout.columns)
                .map(|row| self.library_card_row(row, layout, cx)),
        );
        if animate {
            collection.child(crate::motion::enter(
                ("library-grid", collection_id),
                cards))
        } else {
            collection.child(cards)
        }
    }

    /// One row of course cards: equal-height columns, and trailing spacers keep
    /// the final row's column widths aligned with the rest.
    fn library_card_row(
        &self,
        row: &[(usize, Course)],
        layout: LibraryLayout,
        cx: &mut Context<Self>,
    ) -> Div {
        h_flex()
            .gap_4()
            .items_stretch()
            .children(row.iter().map(|(index, course)| {
                let mut card = v_flex()
                    .flex_1()
                    .min_w_0()
                    .bg(color(SURFACE))
                    .border_1()
                    .border_color(color(CARD_LINE))
                    .rounded(RADIUS_CARD)
                    .overflow_hidden();
                if let Some(thumbnail) = &course.thumbnail {
                    card = card.child(
                        control(("read-course", *index))
                            .ghost()
                            .w_full()
                            .h_auto()
                            .p_0()
                            .rounded_t(RADIUS_CARD)
                            .rounded_b(px(0.))
                            .aspect_ratio(16. / 9.)
                            .accessibility_label(format!("阅读 {}", course.title))
                            .child(
                                img(thumbnail.clone())
                                    .size_full()
                                    .rounded_t(RADIUS_CARD)
                                    .object_fit(ObjectFit::Cover),
                            )
                            .on_click({
                                let course = course.clone();
                                cx.listener(move |this, _, _, cx| {
                                    this.open_course(course.clone(), cx)
                                })
                            }),
                    );
                }
                card.child(
                    v_flex()
                        .flex_1()
                        .w_full()
                        .min_w_0()
                        .p_4()
                        .gap_2()
                        .child(
                            control(("read-title", *index))
                                .accessibility_label(format!("阅读 {}", course.title))
                                .ghost()
                                .w_full()
                                .h_auto()
                                .p_0()
                                .justify_start()
                                .child(
                                    div()
                                        .w_full()
                                        .whitespace_normal()
                                        .when(!layout.stacked, |title| {
                                            title.text_ellipsis().line_clamp(2)
                                        })
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(course.title.clone()),
                                )
                                .on_click({
                                    let course = course.clone();
                                    cx.listener(move |this, _, _, cx| {
                                        this.open_course(course.clone(), cx)
                                    })
                                }),
                        )
                        .child(
                            v_flex()
                                .w_full()
                                .min_w_0()
                                .gap_1()
                                .child(
                                    div()
                                        .w_full()
                                        .whitespace_normal()
                                        .text_size(TEXT_AUX)
                                        .font_weight(FontWeight::NORMAL)
                                        .text_color(color(GRAY))
                                        .child(self.course_meta(course)),
                                )
                                .child(
                                    div()
                                        .w_full()
                                        .whitespace_normal()
                                        .text_size(TEXT_AUX)
                                        .font_weight(FontWeight::NORMAL)
                                        .text_color(color(GRAY))
                                        .child(course.content_description()),
                                )
                                .when(course.incomplete(), |view| {
                                    view.child(badge(BadgeKind::Warning).child("部分内容待补全"))
                                })
                                .child(
                                    h_flex()
                                        .w_full()
                                        .min_w_0()
                                        .mt_auto()
                                        .gap_2()
                                        .items_center()
                                        .flex_wrap()
                                        .child(div().flex_1())
                                        .child(self.course_actions(course.clone(), *index, cx))
                                        .child(
                                            outline_pill(("read-card-action", *index))
                                                .icon(IconName::BookOpen)
                                                .label(if self.course_has_reading_position(course) {
                                                    "继续阅读"
                                                } else {
                                                    "阅读笔记"
                                                })
                                                .loading(self.opening_course.as_ref() == Some(&course.dir))
                                                .disabled(self.opening_course.as_ref() == Some(&course.dir))
                                                .on_click({
                                                    let course = course.clone();
                                                    cx.listener(move |this, _, _, cx| {
                                                        this.open_course(course.clone(), cx)
                                                    })
                                                }),
                                        ),
                                ),
                        )
                        .children(self.course_read_error(course, *index, cx))
                        .children(self.course_export_feedback(course, cx)),
                )
            }))
            .children((row.len()..layout.columns).map(|_| div().flex_1()))
    }

    /// 16:9 row thumbnail at the shared course-row size; the card cover is a
    /// different full-width variant and keeps its own sizing.
    fn course_row_thumbnail(thumbnail: &std::path::Path) -> gpui::Img {
        img(thumbnail.to_path_buf())
            .w(rems(6.857))
            .h(rems(3.857))
            .object_fit(ObjectFit::Cover)
            .rounded(RADIUS_SMALL)
            .flex_shrink_0()
    }

    /// Title, caller-built meta line and the incomplete badge, shared by the
    /// list row and the recent-note row.
    fn course_row_text(
        &self,
        course: &Course,
        clamp_title: bool,
        meta: Div,
    ) -> Div {
        v_flex()
            .flex_1()
            .min_w_0()
            .gap_2()
            .child(
                div()
                    .w_full()
                    .whitespace_normal()
                    .when(clamp_title, |title| title.text_ellipsis().line_clamp(2))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(course.title.clone()),
            )
            .child(meta)
            .when(course.incomplete(), |view| {
                view.child(badge(BadgeKind::Warning).child("部分内容待补全"))
            })
    }

    /// One course row in the list presentation.
    fn library_list_row(
        &self,
        index: &usize,
        course: &Course,
        layout: LibraryLayout,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut read = h_flex().w_full().min_w_0().items_center().gap(px(12.));
        if !layout.stacked && let Some(thumbnail) = &course.thumbnail {
            read = read.child(Self::course_row_thumbnail(thumbnail));
        }
        read = read.child(self.course_row_text(
            course,
            !layout.stacked,
            div()
                .w_full()
                .whitespace_normal()
                .text_size(TEXT_AUX)
                .font_weight(FontWeight::NORMAL)
                .text_color(color(GRAY))
                .child(format!(
                    "{} · {}",
                    self.course_meta(course),
                    course.content_description()
                )),
        ));
        let read = control(("read-course", *index))
            .ghost()
            .flex_1()
            .min_w_0()
            .h_auto()
            .p_0()
            .justify_start()
            .when(layout.stacked, |button| button.w_full().flex_none())
            .accessibility_label(format!("阅读 {}", course.title))
            .tooltip("阅读笔记")
            .child(read)
            .on_click({
                let course = course.clone();
                cx.listener(move |this, _, _, cx| this.open_course(course.clone(), cx))
            });
        let actions = h_flex()
            .min_w_0()
            .flex_shrink_0()
            .items_center()
            .gap_2()
            .when(layout.stacked, |row| row.w_full().flex_wrap())
            .when(layout.stacked, |row| row.child(div().flex_1()))
            .child(
                outline_pill(("read-course-action", *index))
                    .icon(IconName::BookOpen)
                    .when(!layout.compact || layout.stacked, |button| {
                        button.label(if self.course_has_reading_position(course) {
                            "继续阅读"
                        } else {
                            "阅读笔记"
                        })
                    })
                    .loading(self.opening_course.as_ref() == Some(&course.dir))
                    .disabled(self.opening_course.as_ref() == Some(&course.dir))
                    .accessibility_label(format!("阅读 {}", course.title))
                    .tooltip("阅读笔记")
                    .on_click({
                        let course = course.clone();
                        cx.listener(move |this, _, _, cx| this.open_course(course.clone(), cx))
                    }),
            )
            .child(self.course_actions(course.clone(), *index, cx));
        v_flex()
            .w_full()
            .min_w_0()
            .bg(color(SURFACE))
            .border_1()
            .border_color(color(CARD_LINE))
            .rounded(RADIUS_CARD)
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .items_center()
                    .p_4()
                    .gap_4()
                    .when(layout.stacked, |row| row.flex_col().items_start())
                    .child(read)
                    .child(actions),
            )
            .children(self.course_read_error(course, *index, cx))
            .children(
                self.course_export_feedback(course, cx)
                    .map(|feedback| feedback.px_4().pb_3()),
            )
    }

    /// Workbench "最近笔记": attention tasks first, then recent readable notes.
    /// An empty library renders nothing at all (the hero plus box are the empty state).
    pub fn recent_notes_section(&self, cx: &mut Context<Self>) -> Option<Div> {
        let linked = self.current_input_task(cx).map(|task| task.id.clone());
        let mut attention: Vec<_> = self
            .workspace
            .as_ref()
            .into_iter()
            .flat_map(|w| w.state.tasks.iter())
            .filter(|task| {
                Some(&task.id) != linked.as_ref()
                    && task.handled_by.is_none()
                    && (matches!(
                        task.state,
                        crate::workspace::TaskState::NeedsAttention
                            | crate::workspace::TaskState::Uncertain
                            | crate::workspace::TaskState::Paused
                    ) || (task.state == crate::workspace::TaskState::Partial
                        && task.artifact.as_ref().is_some_and(|path| {
                            !crate::task_ui::task_component_failures(task, path).is_empty()
                        })))
            })
            .cloned()
            .collect();
        attention.sort_by_key(|task| std::cmp::Reverse(task.updated));
        attention.truncate(2);
        let notes: Vec<Course> = self
            .courses
            .iter()
            .filter(|course| {
                !attention
                    .iter()
                    .any(|task| task.artifact.as_ref() == Some(&course.dir))
            })
            .take(5)
            .cloned()
            .collect();
        if attention.is_empty() && notes.is_empty() {
            return None;
        }
        let mut section = v_flex().w_full().min_w_0().gap_2();
        section = section.child(
            h_flex()
                .w_full()
                .items_center()
                .gap_2()
                .child(
                    icons::history()
                        .size(px(20.))
                        .text_color(color(ACCENT_STRONG)),
                )
                .child(
                    accessible_text("recent-title", "最近笔记")
                        .text_size(TEXT_TITLE)
                        .font_weight(FontWeight::SEMIBOLD),
                )
                .child(div().flex_1())
                .child(
                    quiet("recent-all")
                        .label("查看全部")
                        .icon(icons::arrow_forward())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.folder_filter = None;
                            this.navigate(Page::Library, cx);
                        })),
                ),
        );
        for task in attention {
            let id = task.id.clone();
            let uncertain = task.state == crate::workspace::TaskState::Uncertain
                || task
                    .blocked
                    .iter()
                    .any(|request| request.reason == "uncertain");
            let requires_repair = !uncertain && crate::task_ui::task_requires_service_repair(&task);
            let partial_components = if task.state == crate::workspace::TaskState::Partial {
                task.artifact
                    .as_ref()
                    .map(|path| {
                        crate::task_ui::task_component_failures(&task, path)
                            .into_iter()
                            .map(|(component, _, _)| component)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            let mut actions = h_flex().gap_2().flex_wrap();
            if let Some(course) = task.artifact.as_ref().and_then(|path| {
                self.courses
                    .iter()
                    .find(|course| &course.dir == path)
                    .cloned()
            }) {
                actions = actions.child(
                    outline_pill(SharedString::from(format!("recent-attention-read-{id}")))
                        .icon(icons::book_open())
                        .label(if self.course_has_reading_position(&course) {
                            "继续阅读"
                        } else {
                            "阅读笔记"
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.open_course(course.clone(), cx);
                        })),
                );
            }
            if requires_repair {
                let repair_id = id.clone();
                actions = actions.child(
                    quiet(SharedString::from(format!("recent-repair-{id}")))
                        .icon(icons::settings())
                        .label("修复 AI 服务并补做")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.repair_task_service(repair_id.clone(), window, cx);
                        })),
                );
            } else if !uncertain && !partial_components.is_empty() {
                let retry_id = id.clone();
                let label = if partial_components.len() > 1 {
                    "补全未完成部分"
                } else {
                    match partial_components[0].as_str() {
                        "screenshots" => "补生成截图",
                        "proofreading" => "重试 AI 校对",
                        "summary" => "补生成摘要",
                        "exports" => "仅补导出",
                        _ => "补全未完成部分",
                    }
                };
                actions = actions.child(
                    quiet(SharedString::from(format!("recent-retry-{id}")))
                        .icon(icons::refresh())
                        .label(label)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.reprocess_task(
                                retry_id.clone(),
                                partial_components.clone(),
                                Vec::new(),
                                cx,
                            );
                        })),
                );
            } else if !uncertain && task.state == crate::workspace::TaskState::Paused {
                let resume_id = id.clone();
                actions = actions.child(
                    outline_pill(SharedString::from(format!("recent-resume-{id}")))
                        .icon(icons::play_arrow())
                        .label("继续处理")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.set_task_intent(
                                resume_id.clone(),
                                crate::workspace::Intent::Run,
                                cx,
                            );
                        })),
                );
            } else {
                actions = actions.child(
                    outline_pill(SharedString::from(format!("recent-task-{id}")))
                        .icon(icons::task())
                        .label("查看任务")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            // 统一走 navigate()：保存 New 草稿、阅读位置、跟随清理
                            this.select_task(&id, cx);
                            this.navigate(Page::Task, cx);
                        })),
                );
            }
            section = section.child(
                v_flex()
                    .w_full()
                    .min_w_0()
                    .p_3()
                    .gap_2()
                    .bg(color(SURFACE))
                    .border_1()
                    .border_color(color(CARD_LINE))
                    .rounded(RADIUS_CARD)
                    .child(
                        h_flex()
                            .gap_2()
                            .items_start()
                            .child(badge(BadgeKind::Warning).child(task.state.label()))
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .whitespace_normal()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(task.plan.title.clone()),
                            ),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(color(GRAY))
                            .whitespace_normal()
                            .child(crate::task_ui::task_attention_summary(&task)),
                    )
                    .child(actions),
            );
        }
        for (index, course) in notes.into_iter().enumerate() {
            section = section.child(self.recent_note_row(course, index, cx));
        }
        Some(section)
    }

    fn recent_note_row(&self, course: Course, index: usize, cx: &mut Context<Self>) -> Div {
        let mut content = h_flex().w_full().min_w_0().gap_3().items_center();
        if let Some(thumbnail) = &course.thumbnail {
            content = content.child(Self::course_row_thumbnail(thumbnail));
        }
        content = content.child(self.course_row_text(
            &course,
            true,
            div()
                .w_full()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_size(TEXT_AUX)
                .text_color(color(GRAY))
                .child(format!(
                    "{} · {}",
                    course.content_description(),
                    self.course_meta(&course)
                )),
        ));
        let open = course.clone();
        h_flex()
            .w_full()
            .min_w_0()
            .p_4()
            .gap_4()
            .items_center()
            .bg(color(SURFACE))
            .border_1()
            .border_color(color(CARD_LINE))
            .rounded(RADIUS_CARD)
            .child(
                control(("recent-note-content", index))
                    .ghost()
                    .flex_1()
                    .min_w_0()
                    .h_auto()
                    .p_0()
                    .justify_start()
                    .accessibility_label(format!("阅读 {}", course.title))
                    .tooltip("阅读笔记")
                    .child(content)
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.open_course(open.clone(), cx)),
                    ),
            )
            .child(
                outline_pill(("recent-read", index))
                    .icon(IconName::BookOpen)
                    .label("阅读笔记")
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.open_course(course.clone(), cx)),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{LibraryGroupBody, LibraryGroupHeader, LibraryItem};
    use crate::notes::Course;
    use std::time::SystemTime;

    fn course(dir: &str) -> Course {
        Course {
            dir: dir.into(),
            title: dir.into(),
            modified: SystemTime::UNIX_EPOCH,
            slides: 0,
            segments: 0,
            thumbnail: None,
            manifest: None,
            warning: None,
        }
    }

    #[test]
    fn library_item_keys_follow_content_identity_and_columns() {
        let row = vec![(0, course("/lib/a")), (1, course("/lib/b"))];
        let items = [
            LibraryItem::Coverage("m".into()),
            LibraryItem::CardRow(row.clone()),
            LibraryItem::ListRow(0, Box::new(course("/lib/a"))),
            LibraryItem::GroupHeader(Box::new(LibraryGroupHeader {
                index: 0,
                key: "lib:0".into(),
                name: "未分类".into(),
                count: 2,
                collapsed: false,
            })),
            LibraryItem::GroupBody(Box::new(LibraryGroupBody {
                index: 0,
                key: "lib:0".into(),
                collapsed: false,
                entries: row.clone(),
            })),
        ];
        let keys: Vec<String> = items.iter().map(|item| item.key(3)).collect();
        let unique: std::collections::HashSet<_> = keys.iter().collect();
        assert_eq!(unique.len(), keys.len(), "library rows need distinct keys");
        // Regrouping for a new column count replaces the affected rows.
        assert_ne!(
            LibraryItem::CardRow(row.clone()).key(3),
            LibraryItem::CardRow(row.clone()).key(2)
        );
        // Content edits keep the row identity keyed by its first course; the
        // row re-measures when it is visible.
        let mut changed = row.clone();
        changed[1] = (1, course("/lib/c"));
        assert_eq!(
            LibraryItem::CardRow(changed).key(3),
            LibraryItem::CardRow(row).key(3)
        );
    }

    #[test]
    fn library_filters_are_surfaced_as_one_level_choices() {
        let root = std::path::PathBuf::from("/notes");
        let options = super::library_filter_options(
            &[(root.clone(), "课程库".into(), vec![(2, "讲座".into())])],
            &root,
            false,
        );
        assert_eq!(options[0].label, "全部笔记");
        assert_eq!(options[1].folder, Some(0));
        assert_eq!(options[2].label, "讲座");
        assert_eq!(super::library_filter_selected(&options, &root, None), "all");
        assert_eq!(
            super::library_filter_selected(&options, &root, Some(2)),
            "f-0-2"
        );
        assert_eq!(super::library_layout_choice(false), "list");
        assert_eq!(super::library_layout_choice(true), "cards");
    }
}
