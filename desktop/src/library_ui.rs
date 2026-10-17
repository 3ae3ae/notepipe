use super::*;
use crate::theme::*;
use gpui_component::{
    button::*,
    menu::{DropdownMenu, PopupMenu, PopupMenuItem},
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

const FOLDERS_UNAVAILABLE: &str = "文件夹暂不可用，请重新扫描笔记库。";

#[derive(Clone)]
pub struct FolderOrigin {
    pub root: PathBuf,
    pub draft_id: Option<String>,
    /// A rename or create is being written; further saves wait for it.
    pub saving: bool,
}

/// Resolved picker state shared by the full picker and the compact chip.
#[derive(Clone)]
pub(crate) struct FolderContext {
    pub(crate) storage: Option<PathBuf>,
    pub(crate) origin: FolderOrigin,
    pub(crate) folder: Option<u64>,
    pub(crate) load_error: Option<String>,
    pub(crate) label: String,
    pub(crate) folders: BTreeMap<u64, String>,
}

struct FolderDialog {
    desktop: Entity<Desktop>,
    title: &'static str,
    _observation: Subscription,
}

impl Render for FolderDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("folder-dialog-content")
            .role(Role::Dialog)
            .aria_label(self.title)
            .child(
                self.desktop
                    .update(cx, |desktop, cx| desktop.folder_editor_view(cx)),
            )
    }
}

impl Desktop {
    pub fn begin_add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.import_video_from_action(window, cx);
        if self.online && self.value(Field::Source, cx).is_empty() {
            self.inputs[&Field::Source].update(cx, |state, cx| state.focus(window, cx));
        }
    }

    pub fn invalidate_source(&mut self) {
        self.pending_conversion = None;
        if let Some(cancel) = self.preview_cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        if let Some(cancel) = self.subtitle_cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        if let Some(cancel) = self.source_covers_cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.preview_generation = self.preview_generation.wrapping_add(1);
        self.subtitle_generation = self.subtitle_generation.wrapping_add(1);
        self.source_preview = None;
        self.source_editor_open = false;
        self.generation_options_open = false;
        self.source_candidates.clear();
        self.source_candidate_covers.clear();
        self.source_collection_title = None;
        self.source_collection_name = None;
        self.selected_candidates.clear();
        self.subtitle_loading = false;
        self.subtitle_error = None;
        self.preview_error = None;
        self.source_validation = None;
        self.show_preview_details = false;
        self.expanded_subtitle_issue = None;
    }

    pub fn inspect_source(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut input = self.value(Field::Source, cx);
        if !self.prepare_next_import(&input, window, cx) {
            return;
        }
        if self.preview_cancel.is_some() && self.last_source_input == input {
            return;
        }
        let previous_source = self.source_preview.clone();
        self.completed_source = None;
        self.invalidate_source();
        if input.is_empty() {
            self.source_validation = Some(
                if self.online {
                    "先粘贴视频链接"
                } else {
                    "先选择一个视频"
                }
                .into(),
            );
            cx.notify();
            return;
        }
        if self.online {
            let links = source::video_links(&input);
            if links.len() > 1 {
                self.source_candidates = links
                    .into_iter()
                    .map(|input| {
                        let mut candidate = source::SourceCandidate {
                            title: input.clone(),
                            input,
                            identity: None,
                            duration: None,
                            thumbnail: None,
                        };
                        candidate.identity = course2md::fetch::candidate_identity(&candidate);
                        candidate
                    })
                    .collect();
                self.source_collection_title = Some("分享内容中有多个链接，请选择一个视频".into());
                self.save_current_draft(cx);
                cx.notify();
                return;
            }
            if let Some(link) = links.into_iter().next()
                && input != link
            {
                self.inputs[&Field::Source].update(cx, |state, cx| {
                    state.set_value(link.clone(), window, cx);
                });
                input = link;
            }
            if let Err(error) = source::validate_url(&input) {
                self.source_validation = Some(error.to_string());
                cx.notify();
                return;
            }
        }
        self.last_source_input = input.clone();
        if !self.save_current_draft(cx) {
            return;
        }
        let token = self
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.state.draft())
            .map(|draft| (draft.id.clone(), draft.revision));
        let generation = self.preview_generation;
        let request_input = input.clone();
        self.preview_workers += 1;
        let cancel = Arc::new(AtomicBool::new(false));
        self.preview_cancel = Some(cancel.clone());
        let online = self.online;
        let handle = window.window_handle();
        // probe 是同步网络/子进程工作；见 crate::spawn_blocking_io 的说明
        let task = crate::spawn_blocking_io(move || source::probe(input, online, cancel));
        cx.spawn(async move |this, cx| {
            let result = task
                .recv()
                .await
                .unwrap_or_else(|_| Err(anyhow::anyhow!(crate::INTERRUPTED)));
            // worker 计数无论窗口存亡都必须归还（request_close 等待它归零）
            let _ = this.update(cx, |this, _| {
                this.preview_workers = this.preview_workers.saturating_sub(1);
            });
            {
                let _ = cx.update_window(handle, |_, window, cx| this.update(cx, |this, cx| {
                    if this.preview_generation != generation || this.value(Field::Source, cx) != request_input { return; }
                    if let Some((id, revision)) = &token
                        && !this.workspace.as_ref().is_some_and(|workspace| workspace.state.matches_input(id, *revision))
                    {
                        return;
                    }
                    this.preview_cancel = None;
                    match result {
                        Ok(source::SourceProbe::Single(source)) => {
                            let mut source = *source;
                            if let course2md::subtitle::SubtitleEvidence::Found { tracks, .. } = &mut source.subtitles {
                                course2md::subtitle::sort_tracks(tracks, &this.preferences.generation().preferred_subtitle_languages, "zh-Hans", source.original_language.as_deref());
                            }
                            let title = this.workspace.as_ref().and_then(|workspace| workspace.state.draft())
                                .filter(|draft| draft.custom_title).map(|draft| draft.title.clone()).unwrap_or_else(|| source.title.clone());
                            this.draft_loading = true;
                            this.inputs[&Field::Title].update(cx, |state, cx| state.set_value(title, window, cx));
                            this.draft_loading = false;
                            let previous_source = previous_source.as_ref().filter(|previous| previous.identity == source.identity);
                            let wanted = previous_source.and_then(|previous| previous.subtitle_request.as_ref().map(|track| track.id.clone()).or_else(|| previous.selected_subtitle.as_ref().map(|subtitle| subtitle.track_id.clone())));
                            let default_track = match &wanted {
                                Some(id) => source.subtitles.tracks().iter().find(|track| &track.id == id).cloned().or_else(|| previous_source.and_then(|previous| previous.subtitles.tracks().iter().find(|track| &track.id == id && matches!(track.origin, course2md::subtitle::SubtitleOrigin::File { .. })).cloned())),
                                None => source.subtitles.tracks().first().cloned(),
                            };
                            if let Some(previous) = previous_source { source.selected_subtitle = previous.selected_subtitle.clone(); }
                            if wanted.is_some() && default_track.is_none() {
                                let message = "原来选择的字幕无法读取，请重新选择文字来源".to_owned();
                                this.subtitle_error = Some(message.clone());
                                source.subtitle_read_error = Some(course2md::subtitle::SubtitleReadError::Failed { message });
                            }
                            this.source_preview = Some(source);
                            this.save_current_draft(cx);
                            if this.task_options.source_mode != course2md::config::TranscriptSource::Asr && let Some(track) = default_track {
                                this.confirm_subtitle(track, false, cx);
                            }
                        }
                        Ok(source::SourceProbe::Collection { title, candidates, unavailable_entries }) => {
                            this.source_collection_name = (!title.is_empty()).then(|| title.clone());
                            this.source_collection_title = Some(if title.is_empty() { "请选择本次处理的视频".into() } else { title });
                            this.start_source_cover_fetch(&candidates, generation, window, cx);
                            this.source_candidates = candidates
                                .into_iter()
                                .map(|mut candidate| {
                                    candidate.identity = course2md::fetch::candidate_identity(&candidate);
                                    candidate
                                })
                                .collect();
                            if this.source_candidates.is_empty() {
                                this.preview_error = Some("还无法确定要处理哪个视频。请复制具体视频的链接。".into());
                            } else if unavailable_entries > 0 {
                                this.preview_error = Some(format!("另有 {unavailable_entries} 个条目无法确认"));
                            }
                        }
                        Ok(source::SourceProbe::Unresolved { message }) => this.preview_error = Some(message),
                        Err(error) => this.preview_error = Some(format!("{error:#}")),
                    }
                    this.advance_conversion(window, cx);
                    cx.notify();
                }));
            }
        }).detach();
        cx.notify();
    }

    /// 后台逐张下载候选行预览图：同一封面 URL 只下载一次（首帧缺失的分集共享
    /// 视频封面），完成一张刷新一行。过期结果由 preview_generation 守卫丢弃，
    /// 与探测结果同一纪律。
    fn start_source_cover_fetch(
        &mut self,
        candidates: &[source::SourceCandidate],
        generation: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut groups: Vec<(String, Arc<Vec<String>>)> = Vec::new();
        for candidate in candidates {
            let Some(url) = candidate.thumbnail.clone() else {
                continue;
            };
            if let Some((_, inputs)) = groups.iter_mut().find(|(known, _)| *known == url) {
                Arc::make_mut(inputs).push(candidate.input.clone());
            } else {
                groups.push((url, Arc::new(vec![candidate.input.clone()])));
            }
        }
        if groups.is_empty() {
            return;
        }
        if let Some(cancel) = self.source_covers_cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.source_covers_cancel = Some(cancel.clone());
        let workers = groups.len().min(4);
        let queue = Arc::new(std::sync::Mutex::new(
            groups
                .into_iter()
                .collect::<std::collections::VecDeque<_>>(),
        ));
        let (tx, rx) = smol::channel::unbounded::<(Arc<Vec<String>>, PathBuf)>();
        for _ in 0..workers {
            let queue = queue.clone();
            let tx = tx.clone();
            let cancel = cancel.clone();
            // 阻塞网络下载不进 GPUI 执行器；见 crate::spawn_blocking_io 的说明
            std::thread::spawn(move || {
                loop {
                    let next = queue.lock().unwrap_or_else(|p| p.into_inner()).pop_front();
                    let Some((url, inputs)) = next else { break };
                    if cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    if let Ok(path) = source::cache_remote_cover(&url, &cancel) {
                        let _ = tx.send_blocking((inputs, path));
                    }
                }
            });
        }
        drop(tx);
        let handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            // 通道在所有工作线程结束后关闭，循环自然退出
            while let Ok((inputs, path)) = rx.recv().await {
                let keep = cx
                    .update_window(handle, |_, _window, cx| {
                        this.update(cx, |this, cx| {
                            if this.preview_generation != generation {
                                return false;
                            }
                            for input in inputs.iter() {
                                this.source_candidate_covers
                                    .insert(input.clone(), path.clone());
                            }
                            cx.notify();
                            true
                        })
                        .unwrap_or(false)
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
        })
        .detach();
    }
    fn folder_name(&self, id: Option<u64>) -> String {
        match id.filter(|id| *id != 0) {
            None => "未分类".into(),
            Some(id) => self
                .library
                .folders
                .get(&id)
                .cloned()
                .unwrap_or_else(|| "已删除的文件夹".into()),
        }
    }

    fn current_folder_origin(&self) -> FolderOrigin {
        if self.page == Page::New
            && let Some(workspace) = &self.workspace
            && let Some(draft) = workspace.state.draft()
            && let Some(library) = workspace.state.library(&draft.library_id)
        {
            return FolderOrigin {
                root: library.root.clone(),
                draft_id: Some(draft.id.clone()),
                saving: false,
            };
        }
        FolderOrigin {
            root: self.library_root.clone(),
            draft_id: None,
            saving: false,
        }
    }

    pub fn begin_folder(&mut self, id: Option<u64>, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.page, Page::Library | Page::New) {
            self.navigate(Page::Library, cx);
        }
        let origin = self.current_folder_origin();
        let Some(library) = self.library_indexes.get(&origin.root) else {
            self.message = Some(FOLDERS_UNAVAILABLE.into());
            cx.notify();
            return;
        };
        let name = id
            .and_then(|id| library.folders.get(&id))
            .cloned()
            .unwrap_or_default();
        self.folder_origin = Some(origin);
        self.folder_editor = Some(id);
        self.folder_error = None;
        self.inputs[&Field::FolderName].update(cx, |state, cx| {
            state.set_value(name, window, cx);
        });
        let desktop = cx.entity();
        let title = if id.is_some() {
            "重命名文件夹"
        } else {
            "新建文件夹"
        };
        let content = cx.new(|cx| FolderDialog {
            _observation: cx.observe(&desktop, |_, _, cx| cx.notify()),
            desktop,
            title,
        });
        let weak = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _, _| {
            let weak = weak.clone();
            dialog
                .title(title)
                .w(px(420.))
                .overlay_closable(false)
                .close_button(false)
                .child(content.clone())
                .on_close(move |_, _, cx| {
                    let _ = weak.update(cx, |this, cx| {
                        this.folder_editor = None;
                        this.folder_origin = None;
                        this.folder_error = None;
                        cx.notify();
                    });
                })
        });
        let input = self.inputs[&Field::FolderName].clone();
        window.defer(cx, move |window, cx| {
            input.update(cx, |input, cx| input.focus(window, cx))
        });
        cx.notify();
    }

    /// Writes the folder record on a blocking thread, then stores the result in
    /// the folder cache before `done` sees the outcome.
    pub(crate) fn edit_folders<T: Send + 'static>(
        &mut self,
        root: PathBuf,
        change: impl FnOnce(&mut organize::Library) -> anyhow::Result<T> + Send + 'static,
        done: impl FnOnce(&mut Self, anyhow::Result<T>, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) {
        let worker_root = root.clone();
        let task = crate::spawn_blocking_io(move || {
            let mut value = None;
            let library = organize::Library::edit(&worker_root, |library| {
                value = Some(change(library)?);
                Ok(())
            })?;
            Ok((
                library,
                anyhow::Context::context(value, "文件夹记录未更新")?,
            ))
        });
        cx.spawn(async move |this, cx| {
            let result = task
                .recv()
                .await
                .unwrap_or_else(|_| Err(anyhow::anyhow!(crate::INTERRUPTED)));
            let _ = this.update(cx, |this, cx| {
                let result = result.map(|(library, value)| {
                    this.store_folders(&root, library);
                    value
                });
                done(this, result, cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn store_folders(&mut self, root: &std::path::Path, library: organize::Library) {
        if self.library_root == root {
            self.library = library.clone();
            self.library_error = None;
        }
        self.library_indexes.insert(root.to_owned(), library);
    }

    pub fn save_folder(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.folder_editor else {
            return;
        };
        if self.folder_origin.is_none() {
            self.folder_origin = Some(self.current_folder_origin());
        }
        let Some(origin) = self.folder_origin.as_mut().filter(|origin| !origin.saving) else {
            return;
        };
        origin.saving = true;
        let root = origin.root.clone();
        let name = self.value(Field::FolderName, cx);
        self.edit_folders(
            root,
            move |library| library.rename(id, &name),
            |this, result, cx| {
                // A closed dialog keeps the saved folder but no longer navigates.
                let Some(origin) = this.folder_origin.as_mut() else {
                    return;
                };
                origin.saving = false;
                let origin = origin.clone();
                match result {
                    Ok(saved) => this.folder_saved(origin, saved, cx),
                    Err(error) => this.folder_error = Some(format!("{error:#}")),
                }
            },
            cx,
        );
        cx.notify();
    }

    fn folder_saved(&mut self, origin: FolderOrigin, saved: u64, cx: &mut Context<Self>) {
        if let Some(draft_id) = &origin.draft_id {
            if let Some(workspace) = &mut self.workspace {
                if let Err(error) = workspace.transaction(|state| {
                    if let Some(draft) = state.drafts.iter_mut().find(|draft| &draft.id == draft_id)
                    {
                        draft.folder = Some(saved);
                    }
                    Ok(())
                }) {
                    self.folder_error = Some(format!("文件夹已创建，保存位置尚未更新：{error:#}"));
                    self.folder_editor = Some(Some(saved));
                    return;
                }
                if workspace.state.current_draft == *draft_id {
                    self.target_folder = Some(saved);
                }
            }
        } else {
            self.library_root = origin.root;
            self.folder_filter = Some(saved);
            if self.page != Page::Library {
                self.navigate(Page::Library, cx);
            }
        }
        self.folder_editor = None;
        self.folder_origin = None;
        self.folder_error = None;
        let handle = self.window;
        cx.defer(move |cx| {
            let _ = cx.update_window(handle, |_, window, cx| window.close_dialog(cx));
        });
    }

    pub fn folder_editor_view(&self, cx: &mut Context<Self>) -> Div {
        let mut view = v_flex().gap_3();
        if let Some(id) = self.folder_editor {
            view = view
                .child(
                    accessible_text("folder-name-label", "文件夹名称")
                        .font_weight(FontWeight::MEDIUM),
                )
                .child(
                    text_input(&self.inputs[&Field::FolderName])
                        .aria_label("文件夹名称")
                        .when(self.folder_error.is_some(), |v| {
                            v.border_color(color(DANGER))
                        }),
                )
                .when_some(self.folder_error.clone(), |v, error| {
                    v.child(
                        accessible_text("folder-editor-error", error)
                            .text_sm()
                            .text_color(color(DANGER)),
                    )
                })
                .child(
                    h_flex()
                        .gap_2()
                        .justify_end()
                        .child(
                            control("cancel-folder")
                                .icon(crate::icons::close())
                                .h_auto()
                                .min_h(rems(2.6))
                                .ghost()
                                .label("取消")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.folder_editor = None;
                                    this.folder_origin = None;
                                    this.folder_error = None;
                                    window.close_dialog(cx);
                                    cx.notify();
                                })),
                        )
                        .child(
                            control("save-folder")
                                .icon(if id.is_some() {
                                    icons::edit()
                                } else {
                                    icons::create_new_folder()
                                })
                                .h_auto()
                                .min_h(rems(2.6))
                                .primary()
                                .label(if id.is_some() {
                                    "保存名称"
                                } else {
                                    "创建文件夹"
                                })
                                .on_click(cx.listener(|this, _, _, cx| this.save_folder(cx))),
                        ),
                );
        }
        view
    }

    pub fn begin_delete_folder(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let origin = self.current_folder_origin();
        let name = self
            .library_indexes
            .get(&origin.root)
            .and_then(|library| library.folders.get(&id))
            .cloned()
            .unwrap_or_else(|| self.folder_name(Some(id)));
        let answer = window.prompt(
            PromptLevel::Warning,
            &format!("删除「{name}」文件夹？"),
            Some("其中的笔记会移到未分类"),
            &["删除文件夹", "保留文件夹"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await.ok() != Some(0) {
                return;
            }
            let _ = this.update(cx, |this, cx| {
                this.edit_folders(
                    origin.root.clone(),
                    move |library| {
                        library.remove(id);
                        Ok(())
                    },
                    move |this, result, _| match result {
                        // Drafts keep the deleted folder so their next submission
                        // asks for a deliberate destination.
                        Ok(()) => {
                            if this.library_root == origin.root && this.folder_filter == Some(id) {
                                this.folder_filter = Some(0);
                            }
                        }
                        Err(error) => this.message = Some(format!("无法删除文件夹：{error:#}")),
                    },
                    cx,
                );
            });
        })
        .detach();
    }

    pub(crate) fn folder_context(&self, course: Option<PathBuf>) -> FolderContext {
        let storage = course.as_ref().map(|path| {
            self.courses
                .iter()
                .find(|course| &course.dir == path || &course.storage_dir() == path)
                .map(Course::storage_dir)
                .unwrap_or_else(|| path.clone())
        });
        let origin = if let Some(path) = &storage {
            let root = self
                .library_view_cache
                .locations
                .get(path)
                .map(|location| location.root.clone())
                .unwrap_or_else(|| self.library_root.clone());
            FolderOrigin {
                root,
                draft_id: None,
                saving: false,
            }
        } else {
            self.current_folder_origin()
        };
        let loaded = self.library_indexes.get(&origin.root);
        let checking = loaded.is_none() && self.loading;
        let load_error = (loaded.is_none() && !checking).then(|| FOLDERS_UNAVAILABLE.to_owned());
        let folder = storage
            .as_ref()
            .and_then(|path| self.library_view_cache.locations.get(path))
            .filter(|location| location.root == origin.root)
            .and_then(|location| loaded?.folder_key(&location.relative))
            .or_else(|| {
                if storage.is_none() {
                    self.target_folder
                } else {
                    None
                }
            });
        let label = if checking {
            "正在读取文件夹…".to_owned()
        } else if load_error.is_some() {
            "文件夹暂不可用".to_owned()
        } else if let Some(id) = folder {
            loaded
                .and_then(|organization| organization.folders.get(&id))
                .cloned()
                .unwrap_or_else(|| "文件夹已删除，请重新选择".into())
        } else {
            "未分类".into()
        };
        FolderContext {
            storage,
            origin,
            folder,
            load_error,
            label,
            folders: loaded
                .map(|organization| organization.folders.clone())
                .unwrap_or_default(),
        }
    }

    pub(crate) fn folder_menu(
        entity: WeakEntity<Desktop>,
        origin: FolderOrigin,
        storage: Option<PathBuf>,
        folders: BTreeMap<u64, String>,
        current: Option<u64>,
    ) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
        move |menu, _, _| {
            let mut entries = vec![(None, "未分类".to_owned())];
            entries.extend(folders.iter().map(|(id, name)| (Some(*id), name.clone())));
            entries.into_iter().fold(menu, |menu, (id, name)| {
                let entity = entity.clone();
                let storage = storage.clone();
                let origin = origin.clone();
                menu.item(
                    PopupMenuItem::new(name)
                        .icon(crate::icons::folder())
                        .checked(id == current)
                        .on_click(move |_, _, cx| {
                            let _ = entity.update(cx, |this, cx| {
                                if let Some(path) = storage.clone() {
                                    let root = origin.root.clone();
                                    this.edit_folders(
                                        origin.root.clone(),
                                        move |library| library.assign(&root, &path, id),
                                        |this, result, _| {
                                            if let Err(error) = result {
                                                this.message =
                                                    Some(format!("无法移动笔记：{error:#}"))
                                            }
                                        },
                                        cx,
                                    );
                                } else if this
                                    .workspace
                                    .as_ref()
                                    .and_then(|workspace| workspace.state.draft())
                                    .is_some_and(|draft| {
                                        Some(&draft.id) == origin.draft_id.as_ref()
                                    })
                                {
                                    this.target_folder = id;
                                    this.save_current_draft(cx);
                                }
                                cx.notify();
                            });
                        }),
                )
            })
        }
    }

    pub fn folder_picker(
        &self,
        course: Option<PathBuf>,
        index: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let has_course = course.is_some();
        let context = self.folder_context(course);
        let load_error = context.load_error.clone();
        let label = context.label.clone();
        let picker = control(("folder-picker", index))
            .w_full()
            .min_w_0()
            .when(has_course, |button| button.ghost())
            .h_auto()
            .min_h(rems(2.6))
            .when(!has_course, |button| {
                button.min_h(rems(56. / 14.)).rounded(RADIUS_SMALL)
            })
            .py_2()
            .disabled(self.loading || load_error.is_some())
            .icon(crate::icons::folder())
            .accessibility_label(format!("保存到文件夹：{label}"))
            .tooltip(label.clone())
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .whitespace_normal()
                    .text_ellipsis()
                    .line_clamp(2)
                    .child(label),
            )
            .child(crate::icons::chevron_down().size_4().flex_shrink_0())
            .dropdown_menu(Self::folder_menu(
                cx.entity().downgrade(),
                context.origin,
                context.storage,
                context.folders,
                context.folder,
            ));
        v_flex()
            .gap_1()
            .child(picker)
            .when_some(load_error, |view, error| {
                view.child(
                    accessible_text(("folder-picker-error", index), error)
                        .text_sm()
                        .text_color(color(DANGER)),
                )
            })
    }
}
