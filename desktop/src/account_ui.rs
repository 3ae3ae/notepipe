//! Bilibili account settings and cancellable QR login. No ticket is logged or stored in UI text.
use super::*;
use crate::theme::*;
use course2md::auth::{AccountProfile, AccountStatus, QrPoll, QrSession};
use gpui_component::button::*;
use std::sync::Arc;

#[derive(Default)]
pub(crate) struct AccountUi {
    generation: u64,
    status_generation: u64,
    dialog: Option<QrDialogState>,
    modules: Option<Arc<Vec<Vec<bool>>>>,
    status: Option<AccountStatus>,
    checking: bool,
    has_saved_login: bool,
    status_error: Option<String>,
    /// The current status_error is the logout-incomplete message; a flag so a
    /// copy change cannot silently break the status derivation.
    logout_failed: bool,
    retry_source: Option<(u64, String)>,
    return_focus: Option<FocusHandle>,
}
#[derive(Clone)]
enum QrDialogState {
    Generating,
    Waiting(u64),
    Confirming(u64),
    Expired,
    Error(String),
    Success(AccountProfile),
}

#[derive(Clone, Copy, Debug)]
struct QrLayout {
    dialog_width: f32,
    content_width: f32,
    content_height: f32,
    side_by_side: bool,
    code_size: f32,
    text_height: f32,
    gap: f32,
}

/// Match the dialog's 16px padding and 1px border, reserving its existing
/// title/footer budget. The QR itself never belongs to a scrolling region.
fn qr_layout(viewport_width: f32, viewport_height: f32, rem_size: f32) -> QrLayout {
    let scale = rem_size / 14.;
    let dialog_width = (420. * scale).min((viewport_width - 48.).max(0.));
    let content_width = (dialog_width - 34.).max(0.);
    let content_height = (viewport_height - rem_size * (180. / 14.) - 64.).max(80.);
    let gap = 16. * scale;
    let preferred_code = 260_f32.min(content_width);
    let text_reserve = rem_size * 4.5;
    let side_by_side = content_height < preferred_code + gap + text_reserve
        && content_width >= preferred_code.min(content_height) + gap + rem_size * 12.;
    let code_size = if side_by_side {
        preferred_code.min(content_height)
    } else {
        preferred_code.min((content_height - gap - text_reserve).max(0.))
    }
    .floor();
    let text_height = if side_by_side {
        content_height
    } else {
        (content_height - code_size - gap).max(0.)
    };
    QrLayout {
        dialog_width,
        content_width,
        content_height,
        side_by_side,
        code_size,
        text_height,
        gap,
    }
}

/// Whole physical pixels avoid blurry modules. Centering includes at least
/// four white modules on every side, even when the available size is fractional.
fn qr_raster(code_size: f32, module_count: usize, scale: f32) -> (f32, f32) {
    let total = (module_count + 8) as f32;
    // Keep a pixel of slack at each edge so snapping a fractional window origin
    // cannot take part of the four-module quiet zone away.
    let unit = ((code_size * scale - 2.).max(0.) / total).floor() / scale;
    let offset = (code_size - unit * total) / 2. + unit * 4.;
    (unit, offset)
}

impl AccountUi {
    fn current(&self, generation: u64) -> bool {
        self.generation == generation && self.dialog.is_some()
    }
    fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.dialog = None;
        self.modules = None;
    }

    fn close(&mut self) {
        self.invalidate();
        self.retry_source = None;
    }

    fn apply_status(
        &mut self,
        generation: u64,
        has_saved_login: bool,
        result: Result<AccountStatus, String>,
    ) -> bool {
        if self.status_generation != generation {
            return false;
        }
        self.checking = false;
        self.has_saved_login = has_saved_login;
        match result {
            Ok(status) => {
                // A removed credential file must not keep the previous "saved" badge.
                self.has_saved_login = !matches!(status, AccountStatus::Disconnected);
                self.status = Some(status);
                self.status_error = None;
                self.logout_failed = false;
            }
            Err(error) => {
                self.status_error = Some(error);
                self.logout_failed = false;
            }
        }
        true
    }

    fn login_action(&self) -> Option<&'static str> {
        if matches!(self.status, Some(AccountStatus::Connected(_)))
            && self.has_saved_login
            && self.status_error.is_none()
        {
            None
        } else if self.has_saved_login || matches!(self.status, Some(AccountStatus::Expired)) {
            Some("重新登录 Bilibili")
        } else {
            Some("登录 Bilibili")
        }
    }

    fn can_resume_source(&self, generation: u64, input: &str, in_source: bool) -> bool {
        in_source
            && self
                .retry_source
                .as_ref()
                .is_some_and(|(saved_generation, saved_input)| {
                    *saved_generation == generation && saved_input == input
                })
    }
}

struct AccountDialog {
    desktop: Entity<Desktop>,
    _observation: Subscription,
    footer: bool,
}
impl Render for AccountDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.desktop.update(cx, |desktop, cx| {
            if self.footer {
                desktop.account_dialog_actions(cx).into_any_element()
            } else {
                desktop.account_dialog_content(window, cx)
            }
        })
    }
}

impl Desktop {
    pub(crate) fn account_connected(&self) -> bool {
        matches!(self.account.status, Some(AccountStatus::Connected(_)))
    }

    pub fn refresh_account(&mut self, cx: &mut Context<Self>) {
        if self.account.checking {
            return;
        }
        self.account.status_generation = self.account.status_generation.wrapping_add(1);
        let generation = self.account.status_generation;
        self.account.checking = true;
        self.account.status_error = None;
        self.account.logout_failed = false;
        // 账号状态是同步网络请求；见 crate::spawn_blocking_io 的说明
        let task = crate::spawn_blocking_io(|| {
            let saved = course2md::auth::cookie_path().is_file();
            let status =
                course2md::auth::bilibili_account_status().map_err(|error| error.to_string());
            (saved, status)
        });
        cx.spawn(async move |this, cx| {
            let Ok((saved, result)) = task.recv().await else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                if this.account.apply_status(generation, saved, result) {
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    pub fn account_settings_page(&self, cx: &mut Context<Self>) -> AnyElement {
        self.account_summary(false, cx).into_any_element()
    }

    /// Shared account state and actions only; the guide owns its heading and footer.
    pub(crate) fn account_onboarding_page(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        self.account_summary(true, cx)
    }

    fn account_summary(&self, onboarding: bool, cx: &mut Context<Self>) -> Div {
        let status = self.account_status_text();
        let saved = self.account.has_saved_login;
        let expired = matches!(self.account.status, Some(AccountStatus::Expired));
        let login_action = self.account.login_action();
        let connected = matches!(self.account.status, Some(AccountStatus::Connected(_)));
        let (kind, label) = if self.account.checking {
            (BadgeKind::Progress, "验证中")
        } else if self.account.status_error.is_some() {
            (BadgeKind::Warning, "暂时无法验证")
        } else if expired {
            (BadgeKind::Warning, "登录已失效")
        } else if connected {
            (BadgeKind::Success, "已登录")
        } else if saved {
            (BadgeKind::Neutral, "已保留登录")
        } else {
            (BadgeKind::Neutral, "未登录")
        };
        let detail = status.trim_start_matches("已登录 · ").to_owned();
        let show_detail = !self.account.checking && detail != label;
        v_flex()
            .w_full()
            .min_w_0()
            .gap_3()
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .gap_2()
                    .items_center()
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_1()
                            .child(semantic_label(
                                "bilibili-account-identity",
                                if onboarding {
                                    "账号状态"
                                } else {
                                    "Bilibili 账号"
                                },
                                icons::bilibili(),
                            ))
                            .when(show_detail, |view| {
                                view.child(
                                    accessible_text("bilibili-account-status", detail.clone())
                                        .min_w_0()
                                        .whitespace_normal()
                                        .pl(rems(28. / 14.))
                                        .text_size(TEXT_AUX)
                                        .text_color(color(MUTED)),
                                )
                            }),
                    )
                    .when(self.account.checking, |row| {
                        row.child(crate::motion::spinner("account-checking", cx))
                    })
                    .child(badge(kind).child(label)),
            )
            .child(
                supporting_info(
                    "bilibili-account-policy",
                    if saved {
                        "读取字幕和视频时使用此账号的权限。退出登录不会删除课程和笔记。"
                    } else if onboarding {
                        "登录后可读取账号有权访问的视频与字幕，也可以跳过，之后在设置中登录。"
                    } else {
                        "公开内容可以先尝试读取；遇到账号权限限制时，登录后继续。"
                    },
                )
                .w_full()
                .min_w_0()
                .whitespace_normal(),
            )
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .justify_end()
                    .flex_wrap()
                    .when_some(login_action, |view, label| {
                        view.child(
                            outline_pill("account-login")
                                .icon(icons::bilibili())
                                .label(label)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_account_dialog(window, cx)
                                })),
                        )
                    })
                    .child(
                        control("account-refresh")
                            .ghost()
                            .icon(icons::refresh())
                            .label("重新检查")
                            .disabled(self.account.checking)
                            .on_click(cx.listener(|this, _, _, cx| this.refresh_account(cx))),
                    )
                    .when(saved, |view| {
                        view.child(
                            control("account-logout")
                                .ghost()
                                .icon(icons::logout())
                                .label("退出登录")
                                .on_click(cx.listener(|this, _, _, cx| this.clear_account(cx))),
                        )
                    }),
            )
    }

    /// Status text without the「Bilibili：」prefix and the「已登录 · 」name
    /// separator; the single source for the summary row, the compact source
    /// row and the detail line, which each format their own wrapper.
    fn account_status_text(&self) -> String {
        if self.account.checking {
            return "正在验证登录状态…".into();
        }
        if self.account.logout_failed {
            return "退出登录尚未完成，原登录状态已保留".into();
        }
        if self.account.status_error.is_some() {
            return if self.account.has_saved_login {
                "暂时无法验证，已保留登录".into()
            } else {
                "暂时无法检查登录状态".into()
            };
        }
        match &self.account.status {
            Some(AccountStatus::Connected(profile)) if !profile.name.trim().is_empty() => {
                format!("已登录 · {}", profile.name)
            }
            Some(AccountStatus::Connected(_)) => "已登录".into(),
            Some(AccountStatus::Expired) => "登录已失效".into(),
            Some(AccountStatus::Disconnected) => "未登录".into(),
            None if self.account.has_saved_login => "已保留登录，尚未验证".into(),
            None => "未登录".into(),
        }
    }

    /// Only rendered inside source details or a permission repair, never a permanent status row.
    pub fn source_account_row(&self, cx: &mut Context<Self>) -> Div {
        let status = format!("Bilibili：{}", self.account_status_text());
        let login_action = self.account.login_action();
        h_flex()
            .w_full()
            .items_center()
            .gap_3()
            .flex_wrap()
            .child(
                div()
                    .id("source-bilibili-account-status")
                    .role(Role::Label)
                    .aria_label(status.clone())
                    .child(status)
                    .flex_1()
                    .min_w_0()
                    .whitespace_normal()
                    .text_sm()
                    .text_color(color(MUTED)),
            )
            .child(
                control("source-account-refresh")
                    .ghost()
                    .icon(icons::refresh())
                    .label("重新检查")
                    .disabled(self.account.checking)
                    .on_click(cx.listener(|this, _, _, cx| this.refresh_account(cx))),
            )
            .when_some(login_action, |view, label| {
                view.child(
                    control("source-account-login")
                        .icon(icons::login())
                        .ghost()
                        .label(label)
                        .on_click(
                            cx.listener(|this, _, window, cx| this.open_account_dialog(window, cx)),
                        ),
                )
            })
    }

    fn can_retry_account_source(&self, cx: &App) -> bool {
        self.account.can_resume_source(
            self.preview_generation,
            &self.value(Field::Source, cx),
            !self.onboarding.active
                && matches!(self.page, Page::New)
                && self.online
                && (self.preview_error.is_some() || self.subtitle_attention_required()),
        )
    }

    fn clear_account(&mut self, cx: &mut Context<Self>) {
        self.account.invalidate();
        self.account.status_generation = self.account.status_generation.wrapping_add(1);
        self.account.checking = false;
        match course2md::auth::clear_bilibili_login() {
            Ok(()) => {
                self.account.has_saved_login = false;
                self.account.status = Some(AccountStatus::Disconnected);
                self.account.status_error = None;
                self.account.logout_failed = false;
            }
            Err(_) => {
                self.account.status_error = Some("退出登录尚未完成，原登录状态已保留。".into());
                self.account.logout_failed = true;
            }
        }
        cx.notify();
    }

    pub fn close_account_dialog(&mut self, cx: &mut Context<Self>) {
        self.account.close();
        cx.notify();
    }

    pub fn open_account_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.account.dialog.is_some() {
            return;
        }
        self.account.retry_source = if !self.onboarding.active
            && matches!(self.page, Page::New)
            && self.online
            && (self.preview_error.is_some() || self.subtitle_attention_required())
            && course2md::auth::is_bilibili_url(&self.value(Field::Source, cx))
        {
            Some((self.preview_generation, self.value(Field::Source, cx)))
        } else {
            None
        };
        self.account.return_focus = window.focused(cx);
        self.start_account_qr(cx);
        let desktop = cx.entity();
        let content = cx.new(|cx| AccountDialog {
            _observation: cx.observe(&desktop, |_, _, cx| cx.notify()),
            desktop: desktop.clone(),
            footer: false,
        });
        let footer = cx.new(|cx| AccountDialog {
            _observation: cx.observe(&desktop, |_, _, cx| cx.notify()),
            desktop,
            footer: true,
        });
        let weak = cx.weak_entity();
        window.open_dialog(cx, move |dialog, window, _| {
            let weak = weak.clone();
            let closed = weak.clone();
            let layout = qr_layout(
                f32::from(window.bounds().size.width),
                f32::from(window.bounds().size.height),
                f32::from(window.rem_size()),
            );
            dialog
                .title(
                    crate::settings_ui::settings_value("bilibili-dialog-title", "登录 Bilibili")
                        .text_size(TEXT_TITLE)
                        .font_weight(FontWeight::SEMIBOLD),
                )
                .w(px(layout.dialog_width))
                .margin_top(task_dialog_top(window))
                .overlay_closable(false)
                .child(content.clone())
                .footer(footer.clone())
                .on_close(move |_, window, cx| {
                    let _ = closed.update(cx, |this, cx| {
                        this.close_account_dialog(cx);
                        if let Some(focus) = this.account.return_focus.take() {
                            focus.focus(window, cx);
                        }
                    });
                })
                .on_cancel(move |_, _, cx| {
                    let _ = weak.update(cx, |this, cx| this.close_account_dialog(cx));
                    true
                })
        });
    }

    fn start_account_qr(&mut self, cx: &mut Context<Self>) {
        self.account.invalidate();
        self.account.dialog = Some(QrDialogState::Generating);
        let generation = self.account.generation;
        let task = crate::spawn_blocking_io(QrSession::generate);
        cx.spawn(async move |this, cx| {
            let Ok(result) = task.recv().await else {
                return;
            };
            let mut session = match result {
                Ok(session) => session,
                Err(_) => {
                    let _ = this.update(cx, |this, cx| {
                        if this.account.current(generation) {
                            this.account.dialog = Some(QrDialogState::Error("获取二维码失败，请检查网络后重试。".into()));
                            cx.notify();
                        }
                    });
                    return;
                }
            };
            let modules = Arc::new(session.modules());
            let active = this.update(cx, |this, cx| {
                if !this.account.current(generation) { return false; }
                this.account.modules = Some(modules);
                this.account.dialog = Some(QrDialogState::Waiting(session.remaining().as_secs()));
                cx.notify();
                true
            }).unwrap_or(false);
            if !active { return; }
            loop {
                smol::Timer::after(Duration::from_secs(2)).await;
                if !this.update(cx, |this, _| this.account.current(generation)).unwrap_or(false) { break; }
                let task = crate::spawn_blocking_io(move || {
                    let result = session.poll();
                    (session, result)
                });
                let Ok((next, result)) = task.recv().await else { break; };
                session = next;
                let remaining = session.remaining().as_secs();
                let keep_polling = this.update(cx, |this, cx| {
                    if !this.account.current(generation) { return false; }
                    let (state, keep_polling) = match result {
                        Ok(QrPoll::Waiting) => (QrDialogState::Waiting(remaining), true),
                        Ok(QrPoll::AwaitingConfirmation) => (QrDialogState::Confirming(remaining), true),
                        Ok(QrPoll::Expired) => (QrDialogState::Expired, false),
                        Ok(QrPoll::Authenticated(login)) => {
                            // The generation check and atomic credential commit happen together on
                            // the UI thread. A cancelled request can never save a late result.
                            match login.save() {
                                Ok(profile) => {
                                    this.account.status_generation = this.account.status_generation.wrapping_add(1);
                                    this.account.checking = false;
                                    this.account.has_saved_login = true;
                                    this.account.status = Some(AccountStatus::Connected(profile.clone()));
                                    this.account.status_error = None;
                                    this.account.logout_failed = false;
                                    (QrDialogState::Success(profile), false)
                                }
                                Err(_) => (QrDialogState::Error("账号已确认，但保存登录状态失败。请检查本地权限后重新扫码。".into()), false),
                            }
                        }
                        Err(_) => (QrDialogState::Error("登录请求失败，请检查网络后重新获取二维码。".into()), false),
                    };
                    if !keep_polling { this.account.modules = None; }
                    this.account.dialog = Some(state);
                    cx.notify();
                    keep_polling
                }).unwrap_or(false);
                if !keep_polling { break; }
            }
        }).detach();
        cx.notify();
    }

    fn account_dialog_content(&mut self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let layout = qr_layout(
            f32::from(window.bounds().size.width),
            f32::from(window.bounds().size.height),
            f32::from(window.rem_size()),
        );
        let state = self
            .account
            .dialog
            .clone()
            .unwrap_or(QrDialogState::Generating);
        let (message, hint): (String, String) = match &state {
            QrDialogState::Generating => ("正在获取二维码".into(), String::new()),
            QrDialogState::Waiting(seconds) => (
                "请使用哔哩哔哩 App 扫码".into(),
                format!("二维码约 {seconds} 秒后过期"),
            ),
            QrDialogState::Confirming(seconds) => (
                "已扫码，请在手机上确认登录".into(),
                format!("请在约 {seconds} 秒内确认"),
            ),
            QrDialogState::Expired => ("二维码已过期".into(), "请刷新二维码后重新扫码。".into()),
            QrDialogState::Error(error) => ("登录未完成".into(), error.clone()),
            QrDialogState::Success(profile) => (
                "已登录 Bilibili".into(),
                if profile.name.trim().is_empty() {
                    "登录状态已保存。".into()
                } else {
                    format!("{}，登录状态已保存。", profile.name)
                },
            ),
        };
        let retry = matches!(state, QrDialogState::Expired | QrDialogState::Error(_));
        let success = matches!(state, QrDialogState::Success(_));
        let mut visual = v_flex()
            .w(px(layout.code_size))
            .h(px(layout.code_size))
            .flex_shrink_0()
            .items_center()
            .justify_center()
            .rounded(RADIUS_CARD)
            .bg(color(if success {
                SUCCESS_BG
            } else if retry {
                WARNING_BG
            } else {
                INSET
            }));
        if let Some(modules) = &self.account.modules {
            let modules = modules.clone();
            // A real QR code intentionally keeps its high-contrast white scanning surface.
            visual = visual.rounded(px(0.)).bg(gpui::rgb(0xffffff)).child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| {
                        // Whole physical-pixel modules and a >=4-module quiet zone keep QR edges crisp.
                        let scale = window.scale_factor();
                        let (unit, offset) = qr_raster(
                            bounds.size.width.as_f32().min(bounds.size.height.as_f32()),
                            modules.len(),
                            scale,
                        );
                        for (y, row) in modules.iter().enumerate() {
                            for (x, dark) in row.iter().enumerate() {
                                if *dark {
                                    window.paint_quad(fill(
                                        Bounds::new(
                                            point(
                                                px(((bounds.origin.x.as_f32()
                                                    + offset
                                                    + x as f32 * unit)
                                                    * scale)
                                                    .round()
                                                    / scale),
                                                px(((bounds.origin.y.as_f32()
                                                    + offset
                                                    + y as f32 * unit)
                                                    * scale)
                                                    .round()
                                                    / scale),
                                            ),
                                            size(px(unit), px(unit)),
                                        ),
                                        gpui::rgb(0x000000),
                                    ));
                                }
                            }
                        }
                    },
                )
                .w_full()
                .h_full(),
            );
        } else if success || retry {
            visual = visual.child(
                if success {
                    icons::check_circle()
                } else {
                    icons::warning()
                }
                .size_8()
                .text_color(color(if success { SUCCESS } else { WARNING })),
            );
        } else {
            visual = visual.child(crate::motion::spinner("qr-code-generating", cx));
        }
        let phase = match state {
            QrDialogState::Generating => 0,
            QrDialogState::Waiting(_) => 1,
            QrDialogState::Confirming(_) => 2,
            QrDialogState::Expired => 3,
            QrDialogState::Error(_) => 4,
            QrDialogState::Success(_) => 5,
        };
        let explanation = v_flex()
            .id("account-login-explanation")
            .min_w_0()
            .min_h_0()
            .max_h(px(layout.text_height))
            .overflow_y_scroll()
            .gap_2()
            .when(layout.side_by_side, |view| view.flex_1())
            .when(!layout.side_by_side, |view| view.w_full().items_center())
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .when(matches!(state, QrDialogState::Confirming(_)), |row| {
                        row.child(crate::motion::spinner("qr-awaiting-confirmation", cx))
                    })
                    .child(
                        accessible_text("bilibili-login-step", message)
                            .min_w_0()
                            .whitespace_normal()
                            .when(!layout.side_by_side, |text| text.text_center())
                            .font_weight(FontWeight::SEMIBOLD),
                    ),
            )
            .when(!hint.is_empty(), |view| {
                view.child(
                    accessible_text("bilibili-login-detail", hint)
                        .w_full()
                        .min_w_0()
                        .whitespace_normal()
                        .when(!layout.side_by_side, |text| text.text_center())
                        .text_sm()
                        .text_color(color(MUTED)),
                )
            });
        v_flex()
            .id("account-login-body")
            .w_full()
            .max_w(px(layout.content_width))
            .min_w_0()
            .min_h_0()
            .max_h(px(layout.content_height))
            .gap(px(layout.gap))
            .items_center()
            .when(layout.side_by_side, |view| view.flex_row())
            .child(crate::motion::enter(
                SharedString::from(format!("qr-visual-{}-{phase}", self.account.generation)),
                visual))
            .child(explanation)
            .into_any_element()
    }

    fn account_dialog_actions(&self, cx: &mut Context<Self>) -> Div {
        let retry = matches!(
            self.account.dialog,
            Some(QrDialogState::Expired | QrDialogState::Error(_))
        );
        let success = matches!(self.account.dialog, Some(QrDialogState::Success(_)));
        let retry_source = success && self.can_retry_account_source(cx);
        h_flex()
            .w_full()
            .justify_end()
            .gap_3()
            .flex_wrap()
            .child(
                control("account-qr-close")
                    .when(success, |button| button.primary())
                    .icon(if success {
                        icons::check_circle()
                    } else {
                        icons::close()
                    })
                    .label(if retry_source {
                        "继续转换"
                    } else if success {
                        "完成"
                    } else {
                        "取消"
                    })
                    .on_click(cx.listener(|this, _, window, cx| {
                        let retry = matches!(this.account.dialog, Some(QrDialogState::Success(_)))
                            && this.can_retry_account_source(cx);
                        this.close_account_dialog(cx);
                        window.close_dialog(cx);
                        if retry {
                            this.inspect_source(window, cx);
                            this.start_conversion(window, cx);
                        }
                    })),
            )
            .when(retry, |view| {
                view.child(
                    primary_pill("account-qr-retry")
                        .icon(icons::refresh())
                        .label("刷新二维码")
                        .on_click(cx.listener(|this, _, _, cx| this.start_account_qr(cx))),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{AccountUi, QrDialogState, qr_layout, qr_raster};
    use course2md::auth::{AccountProfile, AccountStatus};

    #[test]
    fn qr_layout_keeps_the_complete_code_inside_short_and_regular_dialogs() {
        // The rejected native frame had 196px of body height and a 260px code.
        let short = qr_layout(860., 620., 28.);
        assert!(short.side_by_side);
        assert!(short.code_size <= 196.);
        assert!(short.code_size + short.gap + 28. * 12. <= short.content_width);

        let regular = qr_layout(1068., 768., 14.);
        assert!(!regular.side_by_side);
        assert_eq!(regular.dialog_width, 420.);
        assert!(regular.code_size >= 240.);

        for (width, height) in [(860., 620.), (1068., 768.), (1600., 1000.)] {
            for scale in [1., 1.25, 1.5, 2.] {
                let layout = qr_layout(width, height, 14. * scale);
                assert!(layout.code_size > 0.);
                assert!(layout.code_size <= layout.content_width);
                assert!(layout.code_size <= layout.content_height);
                if layout.side_by_side {
                    assert!(layout.code_size + layout.gap < layout.content_width);
                    assert!(layout.text_height <= layout.content_height);
                } else {
                    assert!(
                        layout.code_size + layout.gap + layout.text_height
                            <= layout.content_height + 0.001,
                    );
                }
            }
        }
    }

    #[test]
    fn qr_physical_pixels_retain_four_white_modules_after_origin_snapping() {
        for code_size in [196., 195., 260.] {
            for module_count in [21, 41, 57, 77] {
                for scale in [1., 2.] {
                    let (unit, offset) = qr_raster(code_size, module_count, scale);
                    assert!(unit > 0.);
                    assert_eq!((unit * scale).fract(), 0.);
                    for origin in [0., 0.125, 0.25, 0.5, 0.75] {
                        let first = ((origin + offset) * scale).round() / scale;
                        let last = ((origin + offset + (module_count - 1) as f32 * unit) * scale)
                            .round()
                            / scale
                            + unit;
                        assert!(first - origin >= unit * 4. - 0.001);
                        assert!(origin + code_size - last >= unit * 4. - 0.001);
                    }
                }
            }
        }
    }

    #[test]
    fn closing_and_refreshing_reject_late_qr_results() {
        let mut account = AccountUi {
            dialog: Some(QrDialogState::Generating),
            ..Default::default()
        };
        let first = account.generation;
        assert!(account.current(first));
        account.invalidate();
        assert!(!account.current(first));
        account.dialog = Some(QrDialogState::Generating);
        assert!(!account.current(first));
        assert!(account.current(account.generation));
    }

    #[test]
    fn temporary_validation_failure_keeps_login_and_allows_repair() {
        let connected = AccountStatus::Connected(AccountProfile {
            name: "Fixture account".into(),
        });
        let mut account = AccountUi {
            status_generation: 7,
            status: Some(connected.clone()),
            has_saved_login: true,
            checking: true,
            ..Default::default()
        };
        assert!(account.apply_status(7, true, Err("Network unavailable".into())));
        assert_eq!(account.status, Some(connected.clone()));
        assert!(account.has_saved_login);
        assert!(!account.checking);
        assert_eq!(account.login_action(), Some("重新登录 Bilibili"));

        assert!(account.apply_status(7, true, Ok(connected)));
        assert!(account.status_error.is_none());
        assert_eq!(account.login_action(), None);

        assert!(account.apply_status(7, true, Ok(AccountStatus::Expired)));
        assert_eq!(account.login_action(), Some("重新登录 Bilibili"));
        assert!(account.apply_status(7, true, Ok(AccountStatus::Disconnected)));
        assert!(!account.has_saved_login);
        assert_eq!(account.login_action(), Some("登录 Bilibili"));
    }

    #[test]
    fn late_account_check_cannot_replace_a_newly_authenticated_account() {
        let connected = AccountStatus::Connected(AccountProfile {
            name: "New account".into(),
        });
        let mut account = AccountUi {
            status_generation: 4,
            status: Some(connected.clone()),
            has_saved_login: true,
            ..Default::default()
        };
        assert!(!account.apply_status(3, false, Ok(AccountStatus::Disconnected)));
        assert_eq!(account.status, Some(connected));
        assert!(account.has_saved_login);
        assert!(account.status_error.is_none());
    }

    #[test]
    fn source_continuation_requires_the_same_input_and_origin_after_login() {
        let input = "https://www.bilibili.com/video/BVfixture";
        let mut account = AccountUi {
            retry_source: Some((5, input.into())),
            dialog: Some(QrDialogState::Waiting(120)),
            ..Default::default()
        };
        assert!(account.can_resume_source(5, input, true));
        assert!(!account.can_resume_source(6, input, true));
        assert!(!account.can_resume_source(5, "https://b23.tv/changed", true));
        // Settings and an active guide are not the originating source view.
        assert!(!account.can_resume_source(5, input, false));
        account.invalidate();
        account.dialog = Some(QrDialogState::Waiting(180));
        assert!(account.can_resume_source(5, input, true));
        account.close();
        assert!(!account.can_resume_source(5, input, true));
    }
}
