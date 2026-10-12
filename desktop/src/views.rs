//! Native pages: stable navigation and actions surround independently scrolling content.
use super::*;
use crate::theme::*;
use gpui_component::button::*;

const SHELL_GUTTER: f32 = 24.;
pub(super) const SETTINGS_SIDEBAR_WIDTH: f32 = 200.;
pub(super) const SETTINGS_COLUMN_GAP: f32 = 32.;
const SETTINGS_CONTENT_MAX_WIDTH: f32 = 800.;
const SETTINGS_SHELL_WIDTH: f32 =
    SETTINGS_SIDEBAR_WIDTH + SETTINGS_COLUMN_GAP + SETTINGS_CONTENT_MAX_WIDTH + SHELL_GUTTER * 2.;
const SETTINGS_SIDEBAR_BREAKPOINT: f32 = 1100.;

/// The app bar marks the current destination with a tonal pill; TitleBar's
/// default bottom hairline would be unused chrome.
pub(crate) fn apply_shell_title_bar_chrome(bar: TitleBar) -> TitleBar {
    bar.pl_0().bg(color(CANVAS)).border_b_0()
}

fn shell_column_for(page: Page) -> Div {
    shell_column_at(shell_column_width(page))
}

fn shell_column_width(page: Page) -> AbsoluteLength {
    match page {
        // Each task gets a useful content width: a gallery needs room for covers.
        Page::Settings => rems(SETTINGS_SHELL_WIDTH / 14.).into(),
        Page::Library => rems(1168. / 14.).into(),
        _ => COLUMN.into(),
    }
}

/// The max width includes both gutters; layout calculations use the same box.
pub(super) fn shell_content_width(page: Page, window: &Window) -> f32 {
    (f32::from(window.bounds().size.width).min(f32::from(
        shell_column_width(page).to_pixels(window.rem_size()),
    )) - SHELL_GUTTER * 2.)
        .max(0.)
}

pub(super) fn settings_uses_sidebar(window: &Window) -> bool {
    f32::from(window.bounds().size.width)
        >= SETTINGS_SIDEBAR_BREAKPOINT * (f32::from(window.rem_size()) / 14.)
}

pub(super) fn settings_sidebar_width(window: &Window) -> f32 {
    SETTINGS_SIDEBAR_WIDTH * f32::from(window.rem_size()) / 14.
}

/// Shared by the settings panel and its grids: exclude the real navigation and gutters.
pub(super) fn settings_content_width(window: &Window) -> f32 {
    let available_width = shell_content_width(Page::Settings, window);
    if settings_uses_sidebar(window) {
        (available_width - settings_sidebar_width(window) - SETTINGS_COLUMN_GAP).max(0.)
    } else {
        available_width
    }
}

fn shell_column_at(width: AbsoluteLength) -> Div {
    div()
        .w_full()
        .min_w_0()
        .max_w(width)
        .mx_auto()
        .px(px(SHELL_GUTTER))
}

impl Desktop {
    /// Keep task results reachable while the user is on another page.
    fn shell_topbar(&self, _window: &mut Window, cx: &mut Context<Self>) -> TitleBar {
        let tasks_need_attention = self.workspace.as_ref().is_some_and(|workspace| {
            workspace.state.tasks.iter().any(|task| {
                task.handled_by.is_none()
                    && matches!(
                        task.state,
                        workspace::TaskState::NeedsAttention
                            | workspace::TaskState::Uncertain
                            | workspace::TaskState::Partial
                    )
            })
        });
        let task_count = self.workspace.as_ref().map_or(0, |workspace| {
            crate::task_ui::actionable_task_count(&workspace.state.tasks)
        });
        let scale = self.preferences.application().font_scale;
        // macOS draws the traffic lights inside the title bar's leading edge.
        let leading = if cfg!(target_os = "macos") { 80. } else { 12. };
        let page = self.page;
        let brand = div()
            .id("brand-home")
            .flex()
            .items_center()
            .gap(rems(10. / 14.))
            .px(rems(6. / 14.))
            .h(CONTROL_HEIGHT)
            .rounded(RADIUS_PILL)
            .cursor_pointer()
            // A brand link, not a destination: no pill, so it never reads as selected.
            .hover(|style| style.opacity(0.8))
            .role(Role::Link)
            .aria_label("course2md 工作台")
            .child(
                div()
                    .text_size(TEXT_TITLE)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(color(INK))
                    .child("course2md"),
            )
            .on_click(cx.listener(|this, _, window, cx| {
                if this.prepare_workbench_input(window, cx) {
                    this.navigate(Page::New, cx);
                }
            }));
        let library = nav_action("nav-library", matches!(page, Page::Library | Page::Result), cx)
            .icon(icons::book_open())
            .label("笔记库")
            .on_click(cx.listener(|this, _, _, cx| {
                this.folder_filter = None;
                this.navigate(Page::Library, cx);
            }));
        // Tasks stay reachable, but only claim space once there is work to show.
        let tasks = (task_count > 0 || tasks_need_attention || page == Page::Task).then(|| {
            nav_action("nav-tasks", page == Page::Task, cx)
                .icon(if tasks_need_attention { icons::warning() } else { icons::task() })
                .label(if task_count > 0 { format!("任务 · {task_count}") } else { "任务".to_owned() })
                .on_click(cx.listener(|this, _, _, cx| this.navigate(Page::Task, cx)))
        });
        let settings = nav_action("nav-settings", page == Page::Settings, cx)
            .icon(if self.settings_have_problem() { icons::warning() } else { icons::settings() })
            .accessibility_label("设置")
            .on_click(cx.listener(|this, _, window, cx| this.open_settings(window, cx)));
        apply_shell_title_bar_chrome(TitleBar::new().h(px(40. * scale + 16.))).child(
            h_flex()
                .w_full()
                .min_w_0()
                .h_full()
                .pl(px(leading))
                .pr(px(16.))
                .gap(px(4.))
                .items_center()
                .child(brand)
                .child(div().flex_1())
                .child(library)
                .children(tasks)
                .child(settings),
        )
    }

    fn task_result_notice(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        // A new result can briefly announce itself. Historical unresolved work
        // remains in Tasks; it must not consume every unrelated page's height.
        let (announced_id, shown) = self.transient_task_result.as_ref()?;
        if shown.elapsed() >= std::time::Duration::from_secs(8) {
            return None;
        }
        let visible_task = if self.page == Page::New {
            self.current_input_task(cx).map(|task| task.id.as_str())
        } else {
            None
        };
        let task = self
            .workspace
            .as_ref()?
            .state
            .tasks
            .iter()
            .filter(|task| {
                &task.id == announced_id
                    && task.unread
                    && task.handled_by.is_none()
                    && visible_task != Some(task.id.as_str())
                    && !(self.reading
                        && self.page == Page::New
                        && self.following_conversion.as_ref().is_some_and(|follow| {
                            follow.follows(&task.id, self.preview_generation)
                        }))
            })
            .max_by_key(|task| task.updated)?;
        let id = task.id.clone();
        let dismiss_id = id.clone();
        let exports_only = task.exports_only() && task.state == workspace::TaskState::Complete;
        let result = if matches!(
            task.state,
            workspace::TaskState::Complete | workspace::TaskState::Partial
        ) {
            task.artifact.clone().map(|out_dir| Completed {
                out_dir,
                title: task.plan.title.clone(),
                ..Default::default()
            })
        } else {
            None
        };
        let action = match task.state {
            workspace::TaskState::Complete if exports_only => "打开导出位置",
            workspace::TaskState::Complete => "阅读笔记",
            workspace::TaskState::Partial if result.is_some() => "阅读笔记",
            workspace::TaskState::Partial => "查看未完成部分",
            workspace::TaskState::Uncertain => "确认请求结果",
            _ => "查看任务",
        };
        Some(crate::motion::enter(
            SharedString::from(format!("task-result-notice-{}", task.id)),
            shell_column_for(self.page).py_2().flex_shrink_0().child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .items_center()
                    .gap_2()
                    .p_3()
                    .rounded_md()
                    .bg(color(TINT))
                    .child(icons::info().text_color(color(ACCENT)))
                    .child(
                        accessible_text(
                            "background-task-result",
                            format!(
                                "《{}》：{}",
                                task.plan.title,
                                if exports_only {
                                    "文件已导出"
                                } else {
                                    task.state.label()
                                },
                            ),
                        )
                        .flex_1()
                        .min_w_0()
                        .line_clamp(2)
                        .text_ellipsis(),
                    )
                    .child(
                        control("open-task-result")
                            .icon(icons::arrow_forward())
                            .label(action)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if exports_only {
                                    this.open_task_export_location(id.clone(), cx);
                                } else if let Some(done) = &result {
                                    this.open_course(Course::from_completed(done), cx);
                                } else {
                                    this.select_task(&id, cx);
                                    this.navigate(Page::Task, cx);
                                }
                            })),
                    )
                    .child(
                        control("dismiss-task-result")
                            .ghost()
                            .icon(IconName::Close)
                            .accessibility_label("关闭这条任务提示")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.transient_task_result = None;
                                if let Some(workspace) = &mut this.workspace {
                                    if let Err(error) = workspace.transaction(|state| {
                                        if let Some(task) = state.task_mut(&dismiss_id) {
                                            task.unread = false;
                                        }
                                        Ok(())
                                    }) {
                                        this.workspace_error =
                                            Some(format!("任务提示状态尚未保存：{error:#}"));
                                    }
                                }
                                cx.notify();
                            })),
                    ),
            )))
    }
    fn page_title(&self) -> String {
        match self.page {
            Page::New => "生成笔记".into(),
            Page::Library => "笔记库".into(),
            Page::Task => "任务".into(),
            Page::Settings => "设置".into(),
            Page::Result => self
                .preview
                .as_ref()
                .map(|p| p.course.title.clone())
                .unwrap_or_else(|| "笔记".into()),
        }
    }
    fn page_header(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let row = h_flex()
            .w_full()
            .min_w_0()
            .gap_3()
            .flex_wrap()
            .h_auto()
            .min_h(rems(3.5))
            .items_center()
            .justify_between()
            .child(
                theme::page_heading(
                    "page-title",
                    self.page_title(),
                )
                .flex_1()
                .min_w_0(),
            )
            .when(self.library_controls_visible(cx), |row| {
                let available = shell_content_width(Page::Library, window);
                let scale = f32::from(window.rem_size()) / 14.;
                row.child(self.library_search(window, cx).w(px(
                    if available < 600. * scale { available } else { 360. * scale },
                )))
                .child(primary_pill("library-new-note")
                    .icon(icons::plus()).label("新笔记")
                    .on_click(cx.listener(|this, _, window, cx| {
                        if this.prepare_workbench_input(window, cx) {
                            this.navigate(Page::New, cx);
                        }
                    })))
            });
        v_flex().pt(px(40.)).pb(px(16.)).child(row)
    }
}

impl Render for Desktop {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        theme::apply_preference(&self.preferences.application().appearance, window, cx);
        theme::apply_scale(self.preferences.application().font_scale, window, cx);
        if self.onboarding.active {
            let content = self.onboarding_page(window, cx);
            return div()
                .id("onboarding-action-root")
                .track_focus(&self.root_focus)
                .tab_stop(false)
                .size_full()
                .child(a11y::ModalBackground::new(
                    v_flex()
                        .size_full()
                        .bg(color(CANVAS))
                        .text_color(color(INK))
                        .text_size(TEXT_BODY)
                        .child(
                            TitleBar::new()
                                .h(px(40. * self.preferences.application().font_scale + 16.))
                                .pl_0()
                                .bg(color(CANVAS))
                                .border_color(color(HAIRLINE))
                                .child(
                                    h_flex()
                                        .w_full()
                                        .justify_center()
                                        .gap_2()
                                        .items_center()
                                        .child(icons::book_open().size_4().text_color(color(GRAY)))
                                        .child("course2md"),
                                ),
                        )
                        .child(content),
                    window.has_active_dialog(cx),
                ))
                .children(Root::render_dialog_layer(window, cx))
                .into_any_element();
        }
        let content = match self.page {
            Page::New => self.new_page(window, cx),
            Page::Task => self.queue_page(window, cx),
            Page::Library => self.library_page(window, cx),
            Page::Settings => self.settings_page(window, cx),
            Page::Result => self.reader_page(window, cx),
        };
        // Task and library content virtualize their long lists: the list owns
        // scrolling for the page instead of the shared page-scroll container.
        let self_scrolling = matches!(self.page, Page::Task | Page::Library);
        let content = v_flex()
            .gap_4()
            .when(
                matches!(self.page, Page::Result | Page::Settings) || self_scrolling,
                |v| v.h_full().min_h_0(),
            )
            .when(
                // The workbench card already shows a followed result opening.
                self.reading
                    && !matches!(self.page, Page::Library | Page::Result)
                    && !(self.page == Page::New && self.following_conversion.is_some()),
                |v| {
                    v.child(
                        h_flex()
                            .items_center()
                            .gap(px(8.))
                            .child(crate::motion::spinner("opening-note-spinner", cx))
                            .child(
                                accessible_text("opening-note", "正在打开笔记…")
                                    .role(Role::Status)
                                    .text_color(color(MUTED)),
                            ),
                    )
                },
            )
            .child(content);
        let topbar = self.shell_topbar(window, cx);
        let task_notice = self.task_result_notice(cx);
        let model_notice = self.onboarding_background_notice(window, cx);
        let body = v_flex()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .w_full()
            .when_some(task_notice, |body, notice| body.child(notice))
            .when_some(model_notice, |body, notice| {
                body.child(shell_column_for(self.page).py_2().child(notice))
            })
            .when(self.page == Page::Library, |v| {
                v.child(
                    shell_column_for(self.page)
                        .flex_shrink_0()
                        .child(self.page_header(window, cx)),
                )
            })
            .when(self.page == Page::Library, |v| {
                v.child(shell_column_for(self.page).child(self.library_toolbar(window, cx)))
            })
            .when(
                self.page == Page::Library
                    && self
                        .settings_group_notice(crate::preferences::PreferenceGroup::Application)
                        .is_some_and(|(_, error)| error),
                |v| {
                    v.child(
                        shell_column_for(self.page).pb_3().flex_shrink_0().child(
                            div()
                                .id("library-settings-feedback")
                                .w_full()
                                .min_w_0()
                                .p_3()
                                .rounded(RADIUS_CARD)
                                .bg(color(DANGER_BG))
                                .child(self.group_feedback(
                                    crate::preferences::PreferenceGroup::Application,
                                    cx,
                                )),
                        ),
                    )
                },
            )
            .when_some(self.workspace_error.clone(), |v, message| {
                v.child(crate::motion::enter(
                    SharedString::from(format!("workspace-error-{message}")),
                    shell_column_for(self.page)
                        .pb_3()
                        .text_color(color(DANGER))
                        .child(accessible_text("workspace-error", message).role(Role::Alert))
                        .child(
                            h_flex()
                                .flex_wrap()
                                .gap_2()
                                .mt_2()
                                .child(
                                    control("retry-workspace-records")
                                        .icon(icons::refresh())
                                        .label("重新读取并保存记录")
                                        .disabled(self.job.is_some() || self.storage_ui.busy)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.retry_workspace_records(window, cx)
                                        })),
                                )
                                .when(self.workspace.is_none(), |row| {
                                    row.child(
                                        control("rebuild-workspace-records")
                                            .icon(icons::restart())
                                            .label("保全原文件并重建记录")
                                            .disabled(self.job.is_some() || self.storage_ui.busy)
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.rebuild_workspace_records(window, cx)
                                            })),
                                    )
                                }),
                        )))
            })
            .when_some(self.message.clone(), |v, message| {
                let completed = message.starts_with("笔记已生成");
                v.child(crate::motion::state_enter(
                    SharedString::from(format!("app-message-{message}")),
                    shell_column_for(self.page).pb_3().child(
                        h_flex()
                            .min_w_0()
                            .items_center()
                            .gap_3()
                            .p_3()
                            .rounded_md()
                            .bg(color(if completed { SUCCESS_BG } else { TINT }))
                            .child(if completed {
                                icons::check_circle().text_color(color(SUCCESS))
                            } else {
                                icons::info().text_color(color(ACCENT))
                            })
                            .child(
                                accessible_text("app-message", message)
                                    .role(Role::Status)
                                    .flex_1()
                                    .min_w_0()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .whitespace_normal(),
                            )
                            .child(
                                control("dismiss-message")
                                    .ghost()
                                    .icon(IconName::Close)
                                    .accessibility_label("关闭提示")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.message = None;
                                        cx.notify();
                                    })),
                            ),
                    )))
            })
            .child(
                div()
                    .id(("page-scroll", self.page as usize))
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .w_full()
                    .when(
                        !matches!(self.page, Page::Result | Page::Settings) && !self_scrolling,
                        |view| {
                            view.overflow_y_scroll()
                                .track_scroll(&self.scrolls[self.page as usize])
                        },
                    )
                    .child(
                        shell_column_for(self.page)
                            .when(
                                matches!(self.page, Page::Result | Page::Settings)
                                    || self_scrolling,
                                |v| v.h_full().min_h_0(),
                            )
                            .when(self.page != Page::Settings && !self_scrolling, |v| v.pb_6())
                            .child(content),
                    ),
            );
        let background = v_flex()
            .size_full()
            .bg(color(CANVAS))
            .text_color(color(INK))
            .text_size(rems(1.))
            .child(topbar)
            .child(body);
        div()
            .id("desktop-action-root")
            .track_focus(&self.root_focus)
            .tab_stop(false)
            .size_full()
            .on_action(cx.listener(|this, _: &ImportVideo, window, cx| {
                if !window.has_active_dialog(cx) {
                    this.import_video_from_action(window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &SearchContent, window, cx| {
                if window.has_active_dialog(cx) {
                    return;
                }
                if this.page == Page::Result {
                    this.open_reader_find(window, cx);
                } else {
                    this.navigate(Page::Library, cx);
                    this.inputs[&Field::Search].update(cx, |input, cx| input.focus(window, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &OpenSettings, window, cx| {
                if !window.has_active_dialog(cx) {
                    this.open_settings(window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &OpenAbout, window, cx| {
                if !window.has_active_dialog(cx) {
                    this.open_settings(window, cx);
                    this.settings_tab = 3;
                    this.prepare_settings_view(window, cx);
                }
            }))
            .child(a11y::ModalBackground::new(
                background,
                window.has_active_dialog(cx),
            ))
            .children(Root::render_dialog_layer(window, cx))
            .into_any_element()
    }
}

