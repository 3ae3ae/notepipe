//! The note library's folder tree. Each library is a root whose unfiled notes
//! sit at its top level; folders nest inside it, and notes and folders move
//! by dragging onto a folder or through the same commands on right-click.
use super::*;
use crate::theme::*;
use gpui_component::menu::{ContextMenuExt, DropdownMenu, PopupMenu, PopupMenuItem};
use std::{collections::HashMap, path::Path};

/// Width of the tree beside the notes.
pub(crate) const FOLDER_SIDEBAR: Rems = rems(232. / 14.);
/// Space between the tree and the notes.
pub(crate) const FOLDER_SIDEBAR_GAP: Rems = rems(24. / 14.);
/// Below this content width the tree folds into a menu above the notes.
const FOLDER_SIDEBAR_MIN_CONTENT: f32 = 720.;
const TREE_ROW: Rems = rems(32. / 14.);
const TREE_INDENT: Rems = rems(16. / 14.);
const TREE_GLYPH: Rems = rems(16. / 14.);

/// A note being dragged onto a folder.
#[derive(Clone)]
pub(crate) struct NoteDrag {
    pub(crate) storage: PathBuf,
    pub(crate) title: SharedString,
}

/// A folder being dragged into another folder or its library's top level.
#[derive(Clone)]
pub(crate) struct FolderDrag {
    root: PathBuf,
    id: u64,
    name: SharedString,
}

fn drag_chip(icon: gpui_component::Icon, label: SharedString) -> Div {
    h_flex()
        .max_w(rems(20.))
        .gap_2()
        .px_3()
        .py_2()
        .rounded(RADIUS_SMALL)
        .bg(color(SURFACE))
        .shadow(shadow_popover())
        .text_size(TEXT_BODY)
        .font_weight(FontWeight::SEMIBOLD)
        .child(icon.size(ICON_SLOT).text_color(color(GRAY)))
        .child(div().min_w_0().truncate().child(label))
}

impl Render for NoteDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        drag_chip(icons::article(), self.title.clone())
    }
}

impl Render for FolderDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        drag_chip(icons::folder(), self.name.clone())
    }
}

/// A place in the tree: every note, a library's top level, or a folder.
#[derive(Clone, PartialEq)]
enum TreeTarget {
    All,
    Root(PathBuf),
    Folder(PathBuf, u64),
}

impl Desktop {
    /// Every library with its folder record, when that record has loaded.
    fn tree_libraries(&self) -> Vec<(PathBuf, String, Option<organize::Library>)> {
        let libraries: Vec<_> = self
            .workspace
            .as_ref()
            .map(|workspace| {
                workspace
                    .state
                    .libraries
                    .iter()
                    .map(|location| {
                        (
                            location.root.clone(),
                            location.name.clone(),
                            self.library_indexes.get(&location.root).cloned(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        if libraries.is_empty() {
            vec![(
                self.library_root.clone(),
                course2md::tr!("笔记库").to_owned(),
                Some(self.library.clone()),
            )]
        } else {
            libraries
        }
    }

    pub(crate) fn folder_sidebar_visible(&self, window: &Window, cx: &App) -> bool {
        let scale = f32::from(window.rem_size()) / 14.;
        self.library_controls_visible(cx)
            && crate::views::shell_content_width(Page::Library, window)
                >= FOLDER_SIDEBAR_MIN_CONTENT * scale
    }

    fn selected_target(&self) -> TreeTarget {
        match self.folder_filter {
            FolderFilter::All => TreeTarget::All,
            FolderFilter::Unfiled => TreeTarget::Root(self.library_root.clone()),
            FolderFilter::Folder(id) => TreeTarget::Folder(self.library_root.clone(), id),
        }
    }

    pub(crate) fn select_library_scope(
        &mut self,
        root: PathBuf,
        filter: FolderFilter,
        cx: &mut Context<Self>,
    ) {
        if let Some(organization) = self.library_indexes.get(&root) {
            self.library = organization.clone();
        }
        // The tree keeps the selected folder visible: its library top and
        // every folder above it open.
        if let (FolderFilter::Folder(id), Some(organization)) =
            (filter, self.library_indexes.get(&root))
        {
            let mut parent = organization.parent(id);
            while let Some(folder) = parent {
                self.folder_tree_open.insert((root.clone(), folder));
                parent = organization.parent(folder);
            }
            self.folder_tree_open.remove(&(root.clone(), 0));
        }
        self.library_root = root;
        self.folder_filter = filter;
        self.library_list.scroll_to(ListOffset {
            item_ix: 0,
            offset_in_item: px(0.),
        });
        cx.notify();
    }

    fn select_target(&mut self, target: TreeTarget, cx: &mut Context<Self>) {
        match target {
            TreeTarget::All => {
                let root = self.library_root.clone();
                self.select_library_scope(root, FolderFilter::All, cx)
            }
            TreeTarget::Root(root) => self.select_library_scope(root, FolderFilter::Unfiled, cx),
            TreeTarget::Folder(root, id) => {
                self.select_library_scope(root, FolderFilter::Folder(id), cx)
            }
        }
    }

    fn toggle_tree_node(&mut self, root: &Path, id: u64, cx: &mut Context<Self>) {
        let key = (root.to_path_buf(), id);
        if !self.folder_tree_open.remove(&key) {
            self.folder_tree_open.insert(key);
        }
        cx.notify();
    }

    /// Library tops start open; folders open when the person opens them.
    fn tree_node_open(&self, root: &Path, id: u64) -> bool {
        let open = self.folder_tree_open.contains(&(root.to_path_buf(), id));
        if id == 0 { !open } else { open }
    }

    /// File a note into a folder of its own library, or unfile it.
    pub(crate) fn file_note(
        &mut self,
        root: PathBuf,
        storage: PathBuf,
        folder: Option<u64>,
        cx: &mut Context<Self>,
    ) {
        let own_root = self
            .library_view_cache
            .locations
            .get(&storage)
            .map(|location| location.root.clone());
        if own_root.as_ref().is_some_and(|own| *own != root) {
            self.message = Some(course2md::tr!("笔记只能移到它所在保存位置里的文件夹").into());
            cx.notify();
            return;
        }
        let target = root.clone();
        self.edit_folders(
            root,
            move |library| library.assign(&target, &storage, folder),
            |this, result, _| {
                if let Err(error) = result {
                    this.message = Some(course2md::tr!("无法移动笔记：{}", format!("{error:#}")))
                }
            },
            cx,
        );
    }

    pub(crate) fn move_folder_into(
        &mut self,
        root: PathBuf,
        id: u64,
        parent: Option<u64>,
        cx: &mut Context<Self>,
    ) {
        let moved = root.clone();
        self.edit_folders(
            root,
            move |library| library.move_folder(id, parent),
            move |this, result, cx| match result {
                Ok(()) => {
                    if let Some(parent) = parent {
                        this.folder_tree_open.insert((moved, parent));
                    }
                }
                Err(error) => {
                    this.message = Some(course2md::tr!("无法移动文件夹：{}", format!("{error:#}")));
                    cx.notify();
                }
            },
            cx,
        );
    }

    /// The commands for a folder, shared by the tree, folder tiles and the path bar.
    pub(crate) fn folder_node_menu(
        &self,
        root: PathBuf,
        id: u64,
        cx: &mut Context<Self>,
    ) -> MenuBuilder {
        let entity = cx.weak_entity();
        Rc::new(move |menu, window, cx| {
            let Some(desktop) = entity.upgrade() else {
                return menu;
            };
            let Some(library) = desktop.read(cx).library_indexes.get(&root).cloned() else {
                return menu;
            };
            let act =
                |action: fn(&mut Desktop, PathBuf, u64, &mut Window, &mut Context<Desktop>)| {
                    let entity = entity.clone();
                    let root = root.clone();
                    move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                        let _ = entity
                            .update(cx, |this, cx| action(this, root.clone(), id, window, cx));
                    }
                };
            let destinations = {
                let entity = entity.clone();
                let root = root.clone();
                let library = library.clone();
                move |menu: PopupMenu, _: &mut Window, _: &mut Context<PopupMenu>| {
                    let inside = library.subtree(id);
                    let current = library.parent(id);
                    let top = (None, course2md::tr!("顶层").to_owned());
                    let folders = library
                        .tree()
                        .into_iter()
                        .filter(|(folder, _)| !inside.contains(folder))
                        .map(|(folder, _)| (Some(folder), library.path(folder).join(" / ")));
                    std::iter::once(top)
                        .chain(folders)
                        .fold(menu, |menu, (parent, label)| {
                            let entity = entity.clone();
                            let root = root.clone();
                            menu.item(
                                PopupMenuItem::new(label)
                                    .icon(icons::folder())
                                    .checked(parent == current)
                                    .disabled(parent == current)
                                    .on_click(move |_, _, cx| {
                                        let _ = entity.update(cx, |this, cx| {
                                            this.move_folder_into(root.clone(), id, parent, cx)
                                        });
                                    }),
                            )
                        })
                }
            };
            menu.item(
                PopupMenuItem::new(course2md::tr!("打开"))
                    .icon(icons::folder_open())
                    .on_click(act(|this, root, id, _, cx| {
                        this.select_library_scope(root, FolderFilter::Folder(id), cx)
                    })),
            )
            .item(
                PopupMenuItem::new(course2md::tr!("新建子文件夹…"))
                    .icon(icons::folder_plus())
                    .on_click(act(|this, root, id, window, cx| {
                        this.begin_folder_at(root, None, Some(id), window, cx)
                    })),
            )
            .separator()
            .item(
                PopupMenuItem::new(course2md::tr!("重命名…"))
                    .icon(icons::edit())
                    .on_click(act(|this, root, id, window, cx| {
                        this.begin_folder_at(root, Some(id), None, window, cx)
                    })),
            )
            .submenu_with_icon(
                Some(icons::folder_input()),
                course2md::tr!("移动到"),
                window,
                cx,
                destinations,
            )
            .separator()
            .item(
                PopupMenuItem::new(course2md::tr!("删除文件夹…"))
                    .icon(icons::delete())
                    .danger(true)
                    .on_click(act(|this, root, id, window, cx| {
                        this.begin_delete_folder_at(root, id, window, cx)
                    })),
            )
        })
    }

    fn root_node_menu(&self, root: PathBuf, cx: &mut Context<Self>) -> MenuBuilder {
        let entity = cx.weak_entity();
        Rc::new(move |menu, _, _| {
            let create = entity.clone();
            let root_for_create = root.clone();
            let reveal = root.clone();
            menu.item(
                PopupMenuItem::new(course2md::tr!("新建文件夹…"))
                    .icon(icons::folder_plus())
                    .on_click(move |_, window, cx| {
                        let _ = create.update(cx, |this, cx| {
                            this.begin_folder_at(root_for_create.clone(), None, None, window, cx)
                        });
                    }),
            )
            .item(
                PopupMenuItem::new(course2md::tr!("打开保存位置"))
                    .icon(icons::folder_open())
                    .on_click(move |_, _, cx| cx.reveal_path(&reveal)),
            )
        })
    }

    /// One row of the tree: disclosure, icon and name, indented by depth.
    #[allow(clippy::too_many_arguments)]
    fn tree_row(
        &self,
        target: TreeTarget,
        depth: usize,
        icon: gpui_component::Icon,
        label: SharedString,
        disclosure: Option<bool>,
        menu: Option<MenuBuilder>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = self.selected_target() == target;
        let key = match &target {
            TreeTarget::All => "folder-tree-all".to_owned(),
            TreeTarget::Root(root) => format!("folder-tree-root-{}", root.display()),
            TreeTarget::Folder(root, id) => format!("folder-tree-{}-{id}", root.display()),
        };
        let expander = match (disclosure, &target) {
            (Some(open), TreeTarget::Root(root) | TreeTarget::Folder(root, _)) => {
                let id = match &target {
                    TreeTarget::Folder(_, id) => *id,
                    _ => 0,
                };
                let root = root.clone();
                div()
                    .id(SharedString::from(format!("{key}-disclosure")))
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(TREE_GLYPH)
                    .flex_shrink_0()
                    .rounded(px(4.))
                    .cursor_pointer()
                    .hover(|style| style.bg(blend(color(SURFACE), color(INK), 0.08)))
                    .child(
                        if open {
                            icons::chevron_down()
                        } else {
                            icons::chevron_right()
                        }
                        .size(rems(12. / 14.))
                        .text_color(color(GRAY)),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.toggle_tree_node(&root, id, cx);
                    }))
                    .into_any_element()
            }
            _ => div().size(TREE_GLYPH).flex_shrink_0().into_any_element(),
        };
        let mut row = h_flex()
            .id(SharedString::from(key))
            .role(Role::TreeItem)
            .aria_label(label.clone())
            .when_some(disclosure, |row, open| row.aria_expanded(open))
            .aria_selected(selected)
            .w_full()
            .min_w_0()
            .h(TREE_ROW)
            .flex_shrink_0()
            .pl(rems(4. / 14. + TREE_INDENT.0 * depth as f32))
            .pr_2()
            .gap_1()
            .items_center()
            .rounded_full()
            .cursor_pointer()
            .text_size(TEXT_BODY)
            .when(selected, |row| {
                row.bg(color(SECONDARY_CONTAINER))
                    .text_color(color(ON_SECONDARY_CONTAINER))
                    .font_weight(FontWeight::SEMIBOLD)
            })
            .when(!selected, |row| {
                row.hover(|style| style.bg(blend(color(CANVAS), color(INK), 0.05)))
            })
            .child(expander)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(ICON_SLOT)
                    .flex_shrink_0()
                    .child(icon.size(TREE_GLYPH).text_color(if selected {
                        color(ON_SECONDARY_CONTAINER)
                    } else {
                        color(GRAY)
                    })),
            )
            .child(div().flex_1().min_w_0().truncate().child(label.clone()))
            .on_click(cx.listener({
                let target = target.clone();
                move |this, _, _, cx| this.select_target(target.clone(), cx)
            }));
        // Libraries and folders take notes and folders dropped on them.
        if let TreeTarget::Root(root) | TreeTarget::Folder(root, _) = &target {
            let folder = match &target {
                TreeTarget::Folder(_, id) => Some(*id),
                _ => None,
            };
            let note_root = root.clone();
            let folder_root = root.clone();
            row = row
                .drag_over::<NoteDrag>(|style, _, _, _| style.bg(color(SECONDARY_CONTAINER)))
                .drag_over::<FolderDrag>(|style, _, _, _| style.bg(color(SECONDARY_CONTAINER)))
                .on_drop(cx.listener(move |this, drag: &NoteDrag, _, cx| {
                    this.file_note(note_root.clone(), drag.storage.clone(), folder, cx)
                }))
                .on_drop(cx.listener(move |this, drag: &FolderDrag, _, cx| {
                    if drag.root != folder_root {
                        this.message =
                            Some(course2md::tr!("文件夹只能在同一个保存位置里移动").into());
                        cx.notify();
                    } else if Some(drag.id) != folder {
                        this.move_folder_into(folder_root.clone(), drag.id, folder, cx)
                    }
                }));
        }
        if let TreeTarget::Folder(root, id) = &target {
            let drag = FolderDrag {
                root: root.clone(),
                id: *id,
                name: label.clone(),
            };
            row = row.on_drag(drag, |drag, _, _, cx| cx.new(|_| drag.clone()));
        }
        match menu {
            Some(menu) => row
                .context_menu(move |popup, window, cx| menu(popup, window, cx))
                .into_any_element(),
            None => row.into_any_element(),
        }
    }

    /// The folder tree beside the notes.
    pub(crate) fn folder_sidebar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let mut tree = v_flex()
            .id("folder-tree")
            .role(Role::Tree)
            .aria_label(course2md::tr!("文件夹"))
            .w_full()
            .gap(px(2.))
            .child(self.tree_row(
                TreeTarget::All,
                0,
                icons::book_open(),
                course2md::tr!("全部笔记").into(),
                None,
                None,
                cx,
            ));
        for (root, name, library) in self.tree_libraries() {
            let open = self.tree_node_open(&root, 0);
            let has_folders = library
                .as_ref()
                .is_some_and(|library| !library.folders.is_empty());
            let menu = self.root_node_menu(root.clone(), cx);
            tree = tree.child(self.tree_row(
                TreeTarget::Root(root.clone()),
                0,
                icons::storage(),
                name.into(),
                has_folders.then_some(open),
                Some(menu),
                cx,
            ));
            let Some(library) = library.filter(|_| open) else {
                continue;
            };
            // A closed folder hides everything deeper than itself.
            let mut hidden_below: Option<usize> = None;
            for (id, depth) in library.tree() {
                if hidden_below.is_some_and(|limit| depth > limit) {
                    continue;
                }
                hidden_below = None;
                let has_children = !library.children(Some(id)).is_empty();
                let open = self.tree_node_open(&root, id);
                if has_children && !open {
                    hidden_below = Some(depth);
                }
                let menu = self.folder_node_menu(root.clone(), id, cx);
                tree = tree.child(self.tree_row(
                    TreeTarget::Folder(root.clone(), id),
                    depth + 1,
                    if open && has_children {
                        icons::folder_open()
                    } else {
                        icons::folder()
                    },
                    library.folders[&id].clone().into(),
                    has_children.then_some(open),
                    Some(menu),
                    cx,
                ));
            }
        }
        let create_root = self.library_root.clone();
        let create_parent = self.folder_filter.folder_id();
        v_flex()
            .w(FOLDER_SIDEBAR)
            .flex_shrink_0()
            .h_full()
            .min_h_0()
            .gap_2()
            .child(
                div()
                    .id("folder-tree-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(tree),
            )
            .child(
                quiet("folder-tree-new")
                    .self_start()
                    .icon(icons::folder_plus())
                    .label(course2md::tr!("新建文件夹"))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.begin_folder_at(create_root.clone(), None, create_parent, window, cx)
                    })),
            )
            .into_any_element()
    }

    /// Where the notes come from: the selected library and folder path, each
    /// part a way back up, or a menu of every place when the tree is folded.
    pub(crate) fn library_path_bar(&self, tree_visible: bool, cx: &mut Context<Self>) -> Div {
        let mut path = h_flex().flex_1().min_w_0().gap_1().items_center();
        let crumb = |id: SharedString,
                     label: String,
                     target: Option<TreeTarget>,
                     cx: &mut Context<Self>| {
            let current = target.is_none();
            quiet(id)
                .label(label)
                .when(current, |button| button.text_color(color(INK)))
                .when_some(target, |button, target| {
                    button.on_click(
                        cx.listener(move |this, _, _, cx| this.select_target(target.clone(), cx)),
                    )
                })
        };
        let separator = || {
            icons::chevron_right()
                .size(rems(12. / 14.))
                .text_color(color(GRAY))
                .flex_shrink_0()
        };
        if !tree_visible {
            let entity = cx.weak_entity();
            let libraries = self.tree_libraries();
            path = path.child(
                quiet("library-places")
                    .icon(icons::folder())
                    .child(icons::chevron_down().size_4())
                    .tooltip(course2md::tr!("文件夹"))
                    .accessibility_label(course2md::tr!("文件夹"))
                    .dropdown_menu(move |menu, _, _| {
                        let pick = |target: TreeTarget| {
                            let entity = entity.clone();
                            move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                                let _ = entity
                                    .update(cx, |this, cx| this.select_target(target.clone(), cx));
                            }
                        };
                        let mut menu = menu.item(
                            PopupMenuItem::new(course2md::tr!("全部笔记"))
                                .icon(icons::book_open())
                                .on_click(pick(TreeTarget::All)),
                        );
                        for (root, name, library) in &libraries {
                            menu = menu.separator().item(
                                PopupMenuItem::new(name.clone())
                                    .icon(icons::storage())
                                    .on_click(pick(TreeTarget::Root(root.clone()))),
                            );
                            for (id, _) in library.iter().flat_map(organize::Library::tree) {
                                let label = library
                                    .as_ref()
                                    .map(|library| library.path(id).join(" / "))
                                    .unwrap_or_default();
                                menu = menu.item(
                                    PopupMenuItem::new(label)
                                        .icon(icons::folder())
                                        .on_click(pick(TreeTarget::Folder(root.clone(), id))),
                                );
                            }
                        }
                        menu
                    }),
            );
        }
        match self.selected_target() {
            TreeTarget::All => path.child(crumb(
                "library-path-all".into(),
                course2md::tr!("全部笔记").into(),
                None,
                cx,
            )),
            TreeTarget::Root(root) | TreeTarget::Folder(root, _) => {
                let name = self
                    .tree_libraries()
                    .into_iter()
                    .find(|(candidate, _, _)| *candidate == root)
                    .map(|(_, name, _)| name)
                    .unwrap_or_else(|| course2md::tr!("笔记库").into());
                let folder = self.folder_filter.folder_id();
                path = path.child(crumb(
                    "library-path-root".into(),
                    name,
                    folder.map(|_| TreeTarget::Root(root.clone())),
                    cx,
                ));
                if let (Some(id), Some(library)) = (folder, self.library_indexes.get(&root)) {
                    let mut chain = vec![id];
                    let mut parent = library.parent(id);
                    while let Some(folder) = parent.filter(|_| chain.len() <= library.folders.len())
                    {
                        chain.push(folder);
                        parent = library.parent(folder);
                    }
                    for folder in chain.into_iter().rev() {
                        let name = library.folders.get(&folder).cloned().unwrap_or_default();
                        path = path.child(separator()).child(crumb(
                            SharedString::from(format!("library-path-{folder}")),
                            name,
                            (folder != id).then(|| TreeTarget::Folder(root.clone(), folder)),
                            cx,
                        ));
                    }
                }
                path
            }
        }
    }

    /// The folders inside the selected place, before its notes.
    pub(crate) fn folder_tiles(
        &self,
        root: &Path,
        parent: Option<u64>,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        let library = self.library_indexes.get(root)?;
        let children = library.children(parent);
        if children.is_empty() {
            return None;
        }
        let mut counts: HashMap<u64, usize> = HashMap::new();
        for course in &self.courses {
            if self
                .course_location(course)
                .is_some_and(|location| location.root == root)
                && let Some(folder) = self.course_folder(course)
            {
                *counts.entry(folder).or_default() += 1;
            }
        }
        let tiles = children.into_iter().map(|id| {
            let notes: usize = library
                .subtree(id)
                .iter()
                .map(|folder| counts.get(folder).copied().unwrap_or(0))
                .sum();
            let name: SharedString = library.folders[&id].clone().into();
            let menu = self.folder_node_menu(root.to_path_buf(), id, cx);
            let open_root = root.to_path_buf();
            let note_root = root.to_path_buf();
            let folder_root = root.to_path_buf();
            let drag = FolderDrag {
                root: root.to_path_buf(),
                id,
                name: name.clone(),
            };
            h_flex()
                .id(SharedString::from(format!("folder-tile-{id}")))
                .role(Role::Button)
                .aria_label(name.clone())
                .w(rems(15.))
                .min_w_0()
                .gap_3()
                .px_3()
                .py_2()
                .items_center()
                .rounded(RADIUS_CARD)
                .bg(color(SURFACE))
                .cursor_pointer()
                .hover(|style| style.bg(blend(color(SURFACE), color(INK), 0.05)))
                .child(
                    icons::folder()
                        .size(rems(24. / 14.))
                        .text_color(color(ACCENT)),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .truncate()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(name.clone()),
                        )
                        .child(
                            div()
                                .text_size(TEXT_AUX)
                                .text_color(color(GRAY))
                                .child(course2md::tr!("{} 篇笔记", notes)),
                        ),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.select_library_scope(open_root.clone(), FolderFilter::Folder(id), cx)
                }))
                .on_drag(drag, |drag, _, _, cx| cx.new(|_| drag.clone()))
                .drag_over::<NoteDrag>(|style, _, _, _| style.bg(color(SECONDARY_CONTAINER)))
                .drag_over::<FolderDrag>(|style, _, _, _| style.bg(color(SECONDARY_CONTAINER)))
                .on_drop(cx.listener(move |this, drag: &NoteDrag, _, cx| {
                    this.file_note(note_root.clone(), drag.storage.clone(), Some(id), cx)
                }))
                .on_drop(cx.listener(move |this, drag: &FolderDrag, _, cx| {
                    if drag.root == folder_root && drag.id != id {
                        this.move_folder_into(folder_root.clone(), drag.id, Some(id), cx)
                    }
                }))
                .context_menu(move |popup, window, cx| menu(popup, window, cx))
        });
        Some(h_flex().w_full().gap_3().flex_wrap().children(tiles))
    }
}
