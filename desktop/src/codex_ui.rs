//! Codex（ChatGPT 订阅）账号连接：导入 codex CLI 登录态或浏览器 PKCE 授权。
//! 凭据只存在于 CLI 令牌文件（{config_dir}/auth/codex.json）；桌面设置、日志与
//! 界面文本不接触令牌。设置的服务编辑器与首次引导的 AI 步骤共用此状态与组合。
use super::*;
use crate::preferences::{ServiceProtocol, ServicePurpose};
use crate::theme::*;
use course2md::login::codex::LoginStatus;

/// 识别根 crate 的登录缺失/失效标记，替换为界面内的连接引导。
fn login_required(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<course2md::login::codex::CodexLoginRequired>()
            .is_some()
    })
}

/// Codex 动作的归属界面：异步结果只落回发起时的界面。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum CodexSurface {
    Editor,
    Onboarding,
    /// 设置「服务与账号」页的账号卡片：只呈现连接状态，不填任何模型输入
    Settings,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CodexWork {
    Checking,
    Importing,
    Authorizing,
    Models,
    LoggingOut,
}

#[derive(Default)]
pub(crate) struct CodexUi {
    generation: u64,
    pub(crate) status: Option<LoginStatus>,
    working: Option<CodexWork>,
    /// 最近一次动作的可见结果；bool = 是否为失败（失败用危险色呈现）
    pub(crate) notice: Option<(String, bool)>,
    surface: Option<CodexSurface>,
}

impl CodexUi {
    fn begin(&mut self, surface: CodexSurface, work: CodexWork) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.surface = Some(surface);
        self.working = Some(work);
        // A status check follows other actions and must not hide their result.
        if work != CodexWork::Checking {
            self.notice = None;
        }
        self.generation
    }
    fn current(&self, generation: u64, work: CodexWork) -> bool {
        self.generation == generation && self.working == Some(work)
    }
    pub(crate) fn busy(&self) -> bool {
        self.working.is_some()
    }
    fn fail(&mut self, message: String) {
        self.notice = Some((message, true));
    }
    fn inform(&mut self, message: String) {
        self.notice = Some((message, false));
    }
}

/// 目录到达时为空的模型输入填入首个候选（受支持默认值，仍可修改）。
/// set_value 需要 Window：经共享的 defer + update_window 途径回到窗口上下文。
pub(crate) fn autofill_input(
    handle: AnyWindowHandle,
    input: Entity<InputState>,
    value: Option<String>,
    cx: &mut Context<Desktop>,
) {
    let Some(value) = value else { return };
    if !input.read(cx).value().trim().is_empty() {
        return;
    }
    cx.defer(move |cx| {
        let _ = cx.update_window(handle, |_, window, cx| {
            input.update(cx, |input, cx| {
                // 动作完成到落窗之间用户可能已开始输入；空输入才填默认值
                if input.value().trim().is_empty() {
                    input.set_value(value, window, cx);
                }
            });
        });
    });
}

impl Desktop {
    /// 读本地令牌文件的账号状态（不触网）；仍在后台线程执行，不占用渲染。
    pub(crate) fn codex_refresh_status(&mut self, surface: CodexSurface, cx: &mut Context<Self>) {
        if self.codex.busy() {
            return;
        }
        let generation = self.codex.begin(surface, CodexWork::Checking);
        let task = crate::spawn_blocking_io(|| {
            course2md::login::codex::status().map_err(|error| error.to_string())
        });
        cx.spawn(async move |this, cx| {
            let result = task
                .recv()
                .await
                .unwrap_or_else(|_| Err(crate::interrupted().into()));
            let _ = this.update(cx, |this, cx| {
                if !this.codex.current(generation, CodexWork::Checking) {
                    return;
                }
                this.codex.working = None;
                match result {
                    Ok(status) => this.codex.status = Some(status),
                    Err(error) => this
                        .codex
                        .fail(course2md::tr!("暂时无法读取 Codex 登录状态：{}", error)),
                }
                // 已连接的 Codex 界面没有目录时，顺接拉取账号目录（空模型输入自动填首个候选）
                let needs_catalog = matches!(this.codex.status, Some(LoginStatus::Connected(_)))
                    && this.codex_form_needs_catalog(surface);
                if needs_catalog {
                    this.codex_refresh_models(surface, cx);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn codex_import(&mut self, surface: CodexSurface, cx: &mut Context<Self>) {
        self.codex_login(
            surface,
            CodexWork::Importing,
            course2md::login::codex::import_for_desktop,
            cx,
        );
    }

    /// 浏览器 PKCE 授权：阻塞最长 180 秒并打开系统浏览器，必须离开 UI 线程。
    pub(crate) fn codex_authorize(&mut self, surface: CodexSurface, cx: &mut Context<Self>) {
        self.codex_login(
            surface,
            CodexWork::Authorizing,
            course2md::login::codex::authorize_for_desktop,
            cx,
        );
    }

    fn codex_login(
        &mut self,
        surface: CodexSurface,
        work: CodexWork,
        action: fn() -> anyhow::Result<course2md::login::codex::DesktopCatalog>,
        cx: &mut Context<Self>,
    ) {
        if self.codex.busy() {
            return;
        }
        let generation = self.codex.begin(surface, work);
        let task = crate::spawn_blocking_io(action);
        cx.spawn(async move |this, cx| {
            let result = task
                .recv()
                .await
                .unwrap_or_else(|_| Err(anyhow::anyhow!(crate::interrupted())));
            let _ = this.update(cx, |this, cx| {
                if !this.codex.current(generation, work) {
                    return;
                }
                this.codex.working = None;
                match result {
                    Ok(catalog) => {
                        this.codex.status = Some(LoginStatus::Connected("Codex".into()));
                        if catalog.catalog_is_fallback {
                            this.codex.inform(course2md::tr!("模型目录暂不可用，已使用默认模型").into());
                        } else {
                            this.codex.notice = None;
                        }
                        this.codex_apply_catalog(surface, catalog.models, cx);
                        // 重读登录状态，展示账号详情而非通用占位
                        this.codex_refresh_status(surface, cx);
                    }
                    Err(error) => {
                        let message = if login_required(&error) {
                            // 登录缺失/失效：引导使用界面内的连接动作，而非 CLI 命令
                            match work {
                                CodexWork::Importing => {
                                    course2md::tr!("命令行 Codex 的登录已失效，请先在 Codex 中重新登录再导入，或改用浏览器授权")
                                        .into()
                                }
                                _ => course2md::tr!("Codex 登录已失效，请重新连接").into(),
                            }
                        } else {
                            match work {
                                CodexWork::Importing => course2md::tr!("导入 Codex 登录未完成：{}", format!("{error:#}")),
                                _ => course2md::tr!("浏览器授权未完成：{}", format!("{error:#}")),
                            }
                        };
                        this.codex.fail(message);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// 用已保存的登录态重新拉取账号可用的模型目录。
    pub(crate) fn codex_refresh_models(&mut self, surface: CodexSurface, cx: &mut Context<Self>) {
        // 先让模型字段进入加载状态：即使随后被忙碌守卫拦下，点击也不是静默无响应
        // 设置页账号卡片没有模型输入，无目录可刷新
        if !self.codex_form_models_loading(surface) {
            return;
        }
        if self.codex.busy() {
            let message = course2md::tr!("上一个 Codex 请求尚未完成").to_string();
            self.codex_form_models_failed(surface, message);
            cx.notify();
            return;
        }
        let generation = self.codex.begin(surface, CodexWork::Models);
        let task = crate::spawn_blocking_io(course2md::login::codex::refresh_models_for_desktop);
        cx.spawn(async move |this, cx| {
            let result = task
                .recv()
                .await
                .unwrap_or_else(|_| Err(anyhow::anyhow!(crate::interrupted())));
            let _ = this.update(cx, |this, cx| {
                if !this.codex.current(generation, CodexWork::Models) {
                    return;
                }
                this.codex.working = None;
                match result {
                    Ok(catalog) => {
                        this.codex.notice = None;
                        this.codex_apply_catalog(surface, catalog, cx);
                    }
                    Err(error) => {
                        let message = if login_required(&error) {
                            course2md::tr!("Codex 登录已失效，请先连接 Codex 账号").to_string()
                        } else {
                            course2md::tr!("暂时无法获取 Codex 模型目录：{}", format!("{error:#}"))
                        };
                        this.codex_form_models_failed(surface, message);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// 退出 Codex 登录：删除 CLI 令牌副本，并解除指向 Codex 服务的默认绑定。
    pub(crate) fn codex_logout(&mut self, surface: CodexSurface, cx: &mut Context<Self>) {
        if self.codex.busy() {
            return;
        }
        let generation = self.codex.begin(surface, CodexWork::LoggingOut);
        let task = crate::spawn_blocking_io(course2md::login::codex::remove_credentials);
        cx.spawn(async move |this, cx| {
            let result = task
                .recv()
                .await
                .unwrap_or_else(|_| Err(anyhow::anyhow!(crate::interrupted())));
            let _ = this.update(cx, |this, cx| {
                if !this.codex.current(generation, CodexWork::LoggingOut) {
                    return;
                }
                this.codex.working = None;
                match result {
                    Ok(_) => {
                        this.codex.status = Some(LoginStatus::Disconnected);
                        if this.codex_detach_default_service() {
                            this.codex.inform(
                                course2md::tr!("已退出 Codex 登录；默认 AI 服务已解除绑定").into(),
                            );
                        }
                        this.codex_forms_logged_out();
                    }
                    Err(error) => {
                        this.codex
                            .fail(course2md::tr!("退出登录尚未完成：{}", format!("{error:#}")));
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// 默认 AI 服务绑定指向 Codex 服务时解除绑定；返回是否发生了变更。
    fn codex_detach_default_service(&mut self) -> bool {
        let is_codex = self
            .preferences
            .default_refs()
            .llm
            .as_deref()
            .and_then(|id| self.preferences.version(id))
            .is_some_and(|version| version.config.protocol == ServiceProtocol::CodexResponses);
        if !is_codex {
            return false;
        }
        match self
            .preferences
            .set_default_service(ServicePurpose::Ai, None)
        {
            Ok(_) => true,
            Err(error) => {
                self.codex.fail(course2md::tr!(
                    "已退出 Codex 登录，默认 AI 服务尚未解除绑定：{}",
                    format!("{error:#}")
                ));
                false
            }
        }
    }

    /// 账号目录只落到仍在编辑 Codex 服务的发起界面。
    fn codex_apply_catalog(
        &mut self,
        surface: CodexSurface,
        catalog: Vec<(String, String)>,
        cx: &mut Context<Self>,
    ) {
        let models: Vec<String> = catalog.into_iter().map(|(slug, _)| slug).collect();
        let suggested = models.first().cloned();
        self.codex_form_catalog(surface, models, suggested, cx);
    }

    /// 编辑器/引导共用的 Codex 账号区：状态、说明与登录动作。
    pub(crate) fn codex_account_section(
        &self,
        surface: CodexSurface,
        disabled: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let id = match surface {
            CodexSurface::Editor => "codex-account",
            CodexSurface::Onboarding => "setup-codex-account",
            CodexSurface::Settings => "settings-codex-account",
        };
        let working = self.codex.working;
        let busy = working.is_some();
        let (kind, label) = match (working, &self.codex.status) {
            (Some(CodexWork::Checking), _) => (BadgeKind::Progress, course2md::tr!("正在检查")),
            (Some(CodexWork::LoggingOut), _) => (BadgeKind::Progress, course2md::tr!("正在退出")),
            (Some(_), _) => (BadgeKind::Progress, course2md::tr!("正在连接")),
            (_, Some(LoginStatus::Connected(_))) => (BadgeKind::Success, course2md::tr!("已连接")),
            (_, Some(LoginStatus::Expired)) => (BadgeKind::Warning, course2md::tr!("登录已失效")),
            _ => (BadgeKind::Neutral, course2md::tr!("未连接")),
        };
        let detail = match &self.codex.status {
            Some(LoginStatus::Connected(detail)) if !detail.trim().is_empty() => {
                Some(detail.clone())
            }
            _ => None,
        };
        let connected = matches!(self.codex.status, Some(LoginStatus::Connected(_)))
            && !matches!(working, Some(CodexWork::LoggingOut));
        let notice = self.codex.notice.clone();
        let authorizing = working == Some(CodexWork::Authorizing);
        let mut lines = vec![(
            "policy",
            course2md::tr!("使用你的 ChatGPT 订阅").to_owned(),
            false,
        )];
        if authorizing {
            lines.push((
                "authorizing",
                course2md::tr!("请在浏览器中完成授权（约 3 分钟内）").into(),
                false,
            ));
        }
        lines.extend(notice.map(|(message, is_error)| ("notice", message, is_error)));
        crate::settings_ui::AccountCard {
            id,
            icon: icons::sparkles(),
            title: course2md::tr!("Codex 账号"),
            status: (kind, label),
            detail,
            busy,
            lines,
            actions: if !connected {
                vec![
                    quiet(SharedString::from(format!("{id}-authorize")))
                        .when(
                            surface == CodexSurface::Settings,
                            SettingsText::settings_text,
                        )
                        .icon(icons::external_link())
                        .label(course2md::tr!("浏览器授权"))
                        .disabled(disabled || busy)
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.codex_authorize(surface, cx)),
                        )
                        .into_any_element(),
                    tonal_pill(SharedString::from(format!("{id}-import")), cx)
                        .when(
                            surface == CodexSurface::Settings,
                            SettingsText::settings_text,
                        )
                        .icon(icons::login())
                        .label(if matches!(self.codex.status, Some(LoginStatus::Expired)) {
                            course2md::tr!("重新导入 Codex 登录")
                        } else {
                            course2md::tr!("导入 Codex 登录")
                        })
                        .disabled(disabled || busy)
                        .on_click(cx.listener(move |this, _, _, cx| this.codex_import(surface, cx)))
                        .into_any_element(),
                ]
            } else {
                // 与 Bilibili 账号同序：重新检查在前、退出登录在后
                vec![
                    quiet(SharedString::from(format!("{id}-recheck")))
                        .when(
                            surface == CodexSurface::Settings,
                            SettingsText::settings_text,
                        )
                        .icon(icons::refresh())
                        .label(course2md::tr!("重新检查"))
                        .disabled(disabled || busy)
                        .on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.codex_refresh_status(surface, cx)
                            }),
                        )
                        .into_any_element(),
                    quiet(SharedString::from(format!("{id}-logout")))
                        .when(
                            surface == CodexSurface::Settings,
                            SettingsText::settings_text,
                        )
                        .icon(icons::logout())
                        .label(course2md::tr!("退出登录"))
                        .disabled(disabled || busy)
                        .on_click(cx.listener(move |this, _, _, cx| this.codex_logout(surface, cx)))
                        .into_any_element(),
                ]
            },
        }
        .render(cx)
    }
}
