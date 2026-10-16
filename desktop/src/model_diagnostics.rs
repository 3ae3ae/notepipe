//! Model evidence is loaded in the background; preparation uses the existing model job.
use super::{settings_detail_group, settings_detail_row, settings_status_row, settings_value};
use crate::theme::*;
use crate::*;
use course2md::{
    config::{AsrProvider, model_dir_from},
    models::status::{CacheState, LocalModelStatus},
};
use gpui_component::button::*;
use std::path::Path;

fn model_status_row(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    value: impl IntoElement,
    divider: bool,
) -> Div {
    settings_status_row(id, label, value)
        .min_h(px(56.))
        .when(divider, |row| row.border_b_1().border_color(color(HAIRLINE)))
}

#[derive(Clone)]
struct Request {
    provider: AsrProvider,
    model: String,
    root: PathBuf,
}
impl Request {
    fn new(provider: AsrProvider, model: Option<&str>, root: &Path) -> Self {
        Self {
            provider,
            model: model
                .filter(|model| !model.trim().is_empty())
                .unwrap_or(course2md::config::DEFAULT_ASR_MODEL)
                .into(),
            root: root.into(),
        }
    }
    fn key(&self) -> String {
        format!(
            "{}:{}:{}",
            self.provider.as_str(),
            self.model,
            self.root.display()
        )
    }
}
struct Entry {
    request: Request,
    generation: u64,
    checking: bool,
    result: Option<Result<LocalModelStatus, String>>,
    cache_directories: std::collections::BTreeSet<PathBuf>,
}
impl Entry {
    fn result(&self) -> Option<&Result<LocalModelStatus, String>> {
        self.result.as_ref()
    }
    fn checking(&self) -> bool {
        self.checking
    }
}
#[derive(Default)]
pub(super) struct State {
    entries: BTreeMap<String, Entry>,
    preparing: Option<Request>,
    result: Option<(String, bool)>,
    cancelled: bool,
    details: bool,
    cache_details: std::collections::BTreeSet<String>,
    /// 渲染期只登记 key，真正的 entries 写入与后台检查在 cx.defer 中执行
    pending_checks: std::collections::BTreeSet<String>,
}

/// A read-only view for setup. Checks and downloads keep the same ownership as Settings.
pub(crate) struct ModelSetupSnapshot {
    pub(crate) checking: bool,
    pub(crate) result: Option<Result<LocalModelStatus, String>>,
    pub(crate) device_issue: Option<String>,
    pub(crate) preparing: bool,
    pub(crate) notice: Option<(String, bool)>,
    pub(crate) cancelled: bool,
}
fn provider_name(provider: AsrProvider) -> &'static str {
    crate::provider_label(Some(provider))
}

/// 模型展示名的唯一权威来源（与设置/引导页的选择标签一致）。
pub(crate) fn model_display_name(model: &str) -> String {
    match model {
        "qwen3-1.7b" => "Qwen3 1.7B".to_owned(),
        "qwen3-0.6b" => "Qwen3 0.6B".to_owned(),
        "whisper" => "Whisper".to_owned(),
        _ => model.to_owned(),
    }
}

/// 模型准备按钮文案的唯一权威来源（设置页与引导页共用；此前两处实现已漂移）。
pub(crate) fn model_action_label(
    provider: AsrProvider,
    state: Option<&CacheState>,
) -> &'static str {
    match state {
        Some(CacheState::Missing) => "下载并准备模型",
        Some(CacheState::Partial) => "继续准备模型",
        Some(CacheState::Loaded) => "重新验证加载",
        Some(CacheState::Cached) if matches!(provider, AsrProvider::Cpu | AsrProvider::Gpu) => {
            "检查模型文件"
        }
        Some(CacheState::Cached) => "验证模型加载",
        _ => "继续准备模型",
    }
}
/// Single classification of the device facts behind a chosen recognition
/// provider. `model_device_issue` (message) and `model_runtime_repair` (repair
/// UI) must stay in sync; classifying once keeps both derived from one tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeviceIssue {
    Engine,
    AppleRuntime,
    LlamaMissing,
    GpuAbsent,
    NpuDevice,
    NpuRuntime,
}
fn device_issue(
    provider: AsrProvider,
    environment: &crate::backend::Environment,
) -> Option<DeviceIssue> {
    if !environment.engine {
        return Some(DeviceIssue::Engine);
    }
    match provider {
        AsrProvider::Coreml if !environment.apple => Some(DeviceIssue::AppleRuntime),
        AsrProvider::Gpu if !environment.llama => Some(DeviceIssue::LlamaMissing),
        AsrProvider::Gpu if environment.gpu.is_none() => Some(DeviceIssue::GpuAbsent),
        AsrProvider::Cpu if !environment.llama => Some(DeviceIssue::LlamaMissing),
        AsrProvider::Npu if !environment.npu_device => Some(DeviceIssue::NpuDevice),
        AsrProvider::Npu if !environment.npu_runtime => Some(DeviceIssue::NpuRuntime),
        _ => None,
    }
}
impl Desktop {
    pub(crate) fn setup_model_snapshot(
        &self,
        provider: AsrProvider,
        model: Option<&str>,
        root: &Path,
    ) -> ModelSetupSnapshot {
        let request = Request::new(provider, model, root);
        let state = &self.settings_ui.model_diagnostics;
        let entry = state.entries.get(&request.key());
        let matching = state
            .preparing
            .as_ref()
            .is_some_and(|active| active.key() == request.key());
        ModelSetupSnapshot {
            checking: entry.is_none_or(Entry::checking),
            result: entry.and_then(|entry| entry.result.clone()),
            device_issue: self.model_device_issue(provider),
            preparing: matching && self.kind == Kind::Models && self.job.is_some(),
            notice: matching.then(|| state.result.clone()).flatten(),
            cancelled: matching && state.cancelled,
        }
    }

    pub(crate) fn prepare_setup_model(
        &mut self,
        provider: AsrProvider,
        model: Option<&str>,
        root: &Path,
        cx: &mut Context<Self>,
    ) {
        self.begin_model_preparation(Request::new(provider, model, root), cx);
    }

    fn default_model_request(&self) -> Request {
        let defaults = &self.preferences.generation().options;
        Request::new(
            defaults
                .provider
                .unwrap_or_else(|| self.recommended_local_provider()),
            defaults.asr_model.as_deref(),
            &model_dir_from(defaults.model_dir.as_deref()),
        )
    }
    pub fn ensure_model_diagnostic(
        &mut self,
        provider: AsrProvider,
        model: Option<&str>,
        root: &Path,
        cx: &mut Context<Self>,
    ) {
        // 调用方多在渲染路径：这里只登记意图并 defer，真正的 entries 写入与
        // 后台文件系统检查不在渲染期发生（SKILL.md：渲染期不做文件/网络工作）
        let request = Request::new(provider, model, root);
        if request.provider == AsrProvider::Api {
            return;
        }
        let key = request.key();
        let state = &mut self.settings_ui.model_diagnostics;
        if state.entries.contains_key(&key) || !state.pending_checks.insert(key) {
            return;
        }
        let desktop = cx.entity().downgrade();
        cx.defer(move |cx| {
            let _ = desktop.update(cx, |this, cx| {
                this.settings_ui
                    .model_diagnostics
                    .pending_checks
                    .remove(&request.key());
                this.check_model_request(request, false, cx);
            });
        });
    }
    fn check_model_request(&mut self, request: Request, force: bool, cx: &mut Context<Self>) {
        if request.provider == AsrProvider::Api {
            return;
        }
        let key = request.key();
        let state = &mut self.settings_ui.model_diagnostics;
        if !force && state.entries.contains_key(&key) {
            return;
        }
        let generation = state
            .entries
            .get(&key)
            .map_or(1, |entry| entry.generation + 1);
        state.entries.insert(
            key.clone(),
            Entry {
                request: request.clone(),
                generation,
                checking: true,
                result: None,
                cache_directories: Default::default(),
            },
        );
        // inspect() 会对数 GB 的模型文件做同步 SHA-256，必须离开 executor；
        // 见 crate::spawn_blocking_io 的说明
        let work = crate::spawn_blocking_io(move || {
            let result =
                course2md::models::status::inspect(request.provider, &request.model, &request.root)
                    .map_err(|error| format!("{error:#}"));
            let directories = result
                .as_ref()
                .ok()
                .into_iter()
                .flat_map(|status| &status.parts)
                .filter(|part| part.path.is_dir())
                .map(|part| part.path.clone())
                .collect();
            (result, directories)
        });
        cx.spawn(async move |this, cx| {
            let Ok((result, cache_directories)) = work.recv().await else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                if let Some(entry) = this.settings_ui.model_diagnostics.entries.get_mut(&key)
                    && entry.generation == generation
                {
                    entry.checking = false;
                    entry.result = Some(result);
                    entry.cache_directories = cache_directories;
                    cx.notify();
                }
            });
        })
        .detach();
    }
    pub fn refresh_model_diagnostics(&mut self, cx: &mut Context<Self>) {
        let mut requests = self
            .settings_ui
            .model_diagnostics
            .entries
            .values()
            .map(|entry| (entry.request.key(), entry.request.clone()))
            .collect::<BTreeMap<_, _>>();
        let default = self.default_model_request();
        requests.insert(default.key(), default);
        if let Some(request) = self.settings_ui.model_diagnostics.preparing.clone() {
            requests.insert(request.key(), request);
        }
        for request in requests.into_values() {
            self.check_model_request(request, true, cx);
        }
    }
    pub(super) fn ensure_settings_model_diagnostic(&mut self, cx: &mut Context<Self>) {
        self.check_model_request(self.default_model_request(), false, cx);
    }
    pub fn model_preparation_args(&self) -> Vec<String> {
        let request = self
            .settings_ui
            .model_diagnostics
            .preparing
            .clone()
            .unwrap_or_else(|| self.default_model_request());
        vec![
            "models".into(),
            "prepare".into(),
            "--provider".into(),
            request.provider.as_str().into(),
            "--model".into(),
            request.model,
            "--dir".into(),
            request.root.display().to_string(),
            "--json".into(),
        ]
    }
    fn begin_model_preparation(&mut self, request: Request, cx: &mut Context<Self>) {
        if self.job.is_some() {
            self.message = Some("当前处理结束后才能准备模型".into());
            cx.notify();
            return;
        }
        self.settings_ui.model_diagnostics.preparing = Some(request);
        self.settings_ui.model_diagnostics.result = None;
        self.settings_ui.model_diagnostics.cancelled = false;
        self.settings_ui.model_diagnostics.details = false;
        self.start_model_preparation(cx);
    }
    pub fn model_preparation_finished(
        &mut self,
        success: bool,
        cancelled: bool,
        cx: &mut Context<Self>,
    ) {
        self.settings_ui.model_diagnostics.cancelled = cancelled && !success;
        let request = self
            .settings_ui
            .model_diagnostics
            .preparing
            .clone()
            .unwrap_or_else(|| self.default_model_request());
        let result = (
            if success {
                format!("{} 已准备完成", request.model)
            } else if cancelled {
                format!("{} 的准备已停止", request.model)
            } else {
                format!(
                    "{} 的准备未完成：{}",
                    request.model,
                    self.task_error.as_deref().unwrap_or("准备失败")
                )
            },
            !success && !cancelled,
        );
        self.settings_ui.model_diagnostics.result = Some(result.clone());
        // 同步到引导页的准备记录（取代渲染期的 retain_setup_model_result）
        self.onboarding.apply_model_preparation_result(
            &request.key(),
            result,
            cancelled && !success,
        );
        self.refresh_model_diagnostics(cx);
        cx.notify();
    }
    fn model_device_issue(&self, provider: AsrProvider) -> Option<String> {
        let environment = self.environment.as_ref()?;
        Some(match device_issue(provider, environment)? {
            DeviceIssue::Engine => "转换程序无法运行，请重新安装应用".into(),
            DeviceIssue::AppleRuntime
                if !cfg!(all(target_os = "macos", target_arch = "aarch64")) =>
            {
                "当前系统不支持 Apple 原生识别，请改用其他方式".into()
            }
            DeviceIssue::AppleRuntime => "缺少 Apple 识别组件，请重新安装应用或改用其他方式".into(),
            DeviceIssue::LlamaMissing if provider == AsrProvider::Gpu => {
                "未检测到 llama-server，安装后重新检查".into()
            }
            DeviceIssue::LlamaMissing => "未检测到 llama-server，安装后重新检查".into(),
            DeviceIssue::GpuAbsent => "未检测到可用 GPU，可改用 CPU".into(),
            DeviceIssue::NpuDevice => "未检测到 Intel NPU，请改用其他方式".into(),
            DeviceIssue::NpuRuntime => "缺少 uv 或 Python，安装后重新检查".into(),
        })
    }
    fn model_runtime_repair(&self, provider: AsrProvider, key: &str) -> Div {
        let mut view = v_flex().w_full().min_w_0().gap_2();
        let Some(environment) = &self.environment else {
            return view;
        };
        let reinstall = match device_issue(provider, environment) {
            Some(DeviceIssue::Engine) => true,
            Some(DeviceIssue::AppleRuntime) => {
                cfg!(all(target_os = "macos", target_arch = "aarch64"))
            }
            _ => false,
        };
        if reinstall {
            return view.child(
                control(SharedString::from(format!("model-reinstall-{key}"))).settings_text()
                    .icon(icons::download())
                    .label("下载完整应用")
                    .self_start()
                    .on_click(|_, _, cx| {
                        cx.open_url("https://github.com/mizorewww/course2md/releases")
                    }),
            );
        }
        if device_issue(provider, environment) == Some(DeviceIssue::LlamaMissing) {
            view = view.child(
                control(SharedString::from(format!(
                    "model-llama-install-help-{key}"
                ))).settings_text()
                .icon(icons::external_link())
                .label("查看 llama.cpp 安装说明")
                .self_start()
                .on_click(|_, _, cx| {
                    cx.open_url("https://github.com/ggml-org/llama.cpp#quick-start")
                }),
            );
            if cfg!(target_os = "macos") {
                view = view
                    .child(
                        settings_value(
                            SharedString::from(format!("model-llama-install-command-{key}")),
                            "需要 Homebrew；安装后重新打开应用",
                        )
                        .text_sm(),
                    )
                    .child(
                        control(SharedString::from(format!(
                            "model-copy-llama-install-{key}"
                        ))).settings_text()
                        .icon(icons::content_copy())
                        .label("复制安装命令")
                        .self_start()
                        .on_click(|_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(
                                "brew install llama.cpp".into(),
                            ))
                        }),
                    );
            } else {
                view = view.child(
                    settings_value(
                        SharedString::from(format!("model-llama-path-help-{key}")),
                        "安装 llama-server 并加入 PATH，然后重新打开应用",
                    )
                    .text_sm(),
                );
            }
        }
        if device_issue(provider, environment) == Some(DeviceIssue::NpuRuntime) {
            view = view.child(
                control(SharedString::from(format!(
                    "model-python-install-help-{key}"
                ))).settings_text()
                .icon(icons::external_link())
                .label("查看 uv 安装说明")
                .self_start()
                .on_click(|_, _, cx| {
                    cx.open_url("https://docs.astral.sh/uv/getting-started/installation/")
                }),
            );
        }
        view
    }
    pub fn model_readiness_panel(
        &self,
        provider: AsrProvider,
        model: Option<&str>,
        root: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        self.model_readiness_panel_with(provider, model, root, true, window, cx)
    }
    /// `provider_row`：是否展示「本机引擎」行。工作台的选择器已是方式事实来源，传 false；
    /// 设置页的模型管理面板传 true（面板独立承担状态摘要）。
    pub fn model_readiness_panel_with(
        &self,
        provider: AsrProvider,
        model: Option<&str>,
        root: &Path,
        provider_row: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let request = Request::new(provider, model, root);
        let key = request.key();
        let entry = self.settings_ui.model_diagnostics.entries.get(&key);
        let device_issue = self.model_device_issue(provider);
        let checking = entry.is_none_or(Entry::checking);
        let status = entry
            .and_then(Entry::result)
            .and_then(|result| result.as_ref().ok());
        let (mut view, cache_action, cache_details) =
            self.model_status_section(provider, &request, provider_row, cx);
        let (actions, extras) = self.model_prepare_actions(
            provider,
            &request,
            &key,
            checking,
            status,
            &device_issue,
            window,
            cx,
        );
        for extra in extras {
            view = view.child(extra);
        }
        view.child(h_flex().w_full().min_w_0().gap_2().items_center().flex_wrap()
            .child(actions.w_auto()).children(cache_action))
            .when_some(cache_details, |view, details| view.child(details))
    }

    /// Status rows: engine, model name, device issue with repair guidance,
    /// check progress, cache state summary and expandable cache details.
    fn model_status_section(
        &self,
        provider: AsrProvider,
        request: &Request,
        provider_row: bool,
        cx: &mut Context<Self>,
    ) -> (Div, Option<Div>, Option<AnyElement>) {
        let key = request.key();
        let entry = self.settings_ui.model_diagnostics.entries.get(&key);
        let device_issue = self.model_device_issue(provider);
        let mut cache_details = None;
        let mut cache_action = None;
        let mut view = v_flex().w_full().min_w_0();
        if provider_row {
            view = view.child(model_status_row(
                SharedString::from(format!("model-provider-label-{key}")),
                "本机引擎",
                settings_value(
                    SharedString::from(format!("model-provider-{key}")),
                    provider_name(provider),
                )
                .w_auto(),
                true,
            ));
        }
        view = view.child(model_status_row(
            SharedString::from(format!("model-name-label-{key}")),
            "模型",
            settings_value(
                SharedString::from(format!("model-name-{key}")),
                model_display_name(&request.model),
            )
            .w_auto(),
            true,
        ));
        if let Some(issue) = &device_issue {
            view = view
                .child(settings_detail_row(
                    SharedString::from(format!("model-device-label-{key}")),
                    "设备状态",
                    settings_value(
                        SharedString::from(format!("model-device-problem-{key}")),
                        issue.clone(),
                    )
                    .text_color(color(DANGER)),
                ))
                .child(self.model_runtime_repair(provider, &key));
        }
        let checking = entry.is_none_or(Entry::checking);
        let status = entry
            .and_then(Entry::result)
            .and_then(|result| result.as_ref().ok());
        if checking {
            view = view.child(model_status_row(
                SharedString::from(format!("model-checking-label-{key}")),
                "模型状态",
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(crate::motion::spinner(
                        SharedString::from(format!("model-check-spinner-{key}")),
                        cx,
                    ))
                    .child(
                        settings_value(
                            SharedString::from(format!("model-checking-{key}")),
                            "正在检查模型…",
                        )
                        .w_auto()
                        .role(Role::Status),
                    ),
                false,
            ));
        }
        if let Some(Err(error)) = entry.and_then(Entry::result) {
            view = view.child(settings_detail_row(
                SharedString::from(format!("model-check-error-label-{key}")),
                "检查结果",
                settings_value(
                    SharedString::from(format!("model-check-error-{key}")),
                    format!("检查失败：{error}"),
                )
                .text_color(color(DANGER)),
            ));
        }
        if let Some(status) = status {
            // 准备进行中必须展示真实阶段：缓存检查还停留在「待下载」会与进度条矛盾（review4#5）
            let being_prepared = self
                .settings_ui
                .model_diagnostics
                .preparing
                .as_ref()
                .is_some_and(|request| request.key() == key);
            let (kind, label, description) = if being_prepared {
                (BadgeKind::Progress, "准备中", None)
            } else {
                match status.state {
                    CacheState::Missing => (BadgeKind::Neutral, "待下载", None),
                    CacheState::Partial => (BadgeKind::Warning, "待准备", None),
                    CacheState::Cached => (BadgeKind::Success, "已下载", None),
                    CacheState::Loaded => (BadgeKind::Success, "已验证可加载", None),
                    CacheState::Unsupported => (
                        BadgeKind::Warning,
                        "不支持",
                        Some("请选择这种识别方式支持的模型"),
                    ),
                }
            };
            view = view.child(model_status_row(
                SharedString::from(format!("model-state-label-{key}")),
                "模型状态",
                badge(kind).text_size(TEXT_SETTINGS_AUX).child(label),
                false,
            ));
            if let Some(description) = description {
                view = view.child(
                    theme::supporting_info(
                        SharedString::from(format!("model-state-{key}")),
                        description,
                    )
                    .text_size(TEXT_SETTINGS_AUX),
                );
            }
            let detail_key = key.clone();
            let cache_open = self
                .settings_ui
                .model_diagnostics
                .cache_details
                .contains(&key);
            let mut cache = v_flex().w_full().min_w_0().gap_3();
            for (index, part) in status.parts.iter().enumerate() {
                let path = part.path.clone();
                let mut part_details = v_flex()
                    .w_full()
                    .min_w_0()
                    .gap_2()
                    .child(
                        settings_value(
                            SharedString::from(format!("model-cache-path-{key}-{index}")),
                            part.path.display().to_string(),
                        )
                        .text_size(TEXT_SETTINGS_AUX)
                        .text_color(color(GRAY)),
                    )
                    .when(
                        entry.is_some_and(|entry| entry.cache_directories.contains(&path)),
                        |view| {
                            view.child(
                                quiet(SharedString::from(format!(
                                    "open-model-cache-{key}-{index}"
                                ))).settings_text()
                                .icon(icons::folder_open())
                                .label("打开缓存位置")
                                .accessibility_label(format!(
                                    "打开 {} 的缓存位置：{}",
                                    part.name,
                                    part.path.display()
                                ))
                                .self_end()
                                .on_click(move |_, _, cx| cx.open_with_system(&path)),
                            )
                        },
                    );
                if !part.missing.is_empty() {
                    part_details = part_details.child(settings_value(
                        SharedString::from(format!("model-missing-files-{key}-{index}")),
                        format!("缺少或尚未验证：{}", part.missing.join("、")),
                    ));
                }
                cache = cache.child(
                    v_flex()
                        .w_full()
                        .min_w_0()
                        .gap_2()
                        .p_4()
                        .bg(color(SURFACE))
                        .border_1()
                        .border_color(color(CARD_LINE))
                        .rounded(RADIUS_CARD)
                        .child(
                            semantic_label(
                                SharedString::from(format!("model-cache-part-label-{key}-{index}")),
                                part.name.clone(),
                                icons::storage(),
                            )
                            .w_full()
                            .min_w_0(),
                        )
                        .child(part_details),
                );
            }
            cache_action = Some(
                        h_flex()
                            .w_auto()
                            .min_w_0()
                            .gap_3()
                            .flex_wrap()
                            .child(
                                quiet(SharedString::from(format!("model-cache-details-{key}"))).settings_text()
                                    .ml(-crate::theme::QUIET_INSET)
                                    .icon(icons::folder_open())
                                    .label(if cache_open {
                                        "收起缓存详情"
                                    } else {
                                        "查看缓存详情"
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if !this
                                            .settings_ui
                                            .model_diagnostics
                                            .cache_details
                                            .remove(&detail_key)
                                        {
                                            this.settings_ui
                                                .model_diagnostics
                                                .cache_details
                                                .insert(detail_key.clone());
                                        }
                                        cx.notify();
                                    })),
                            )
                            .when(status.bytes > 0, |row| {
                                row.child(
                                    settings_value(
                                        SharedString::from(format!("model-cached-size-{key}")),
                                        format!(
                                            "缓存占用 {}",
                                            crate::activity::bytes(status.bytes)
                                        ),
                                    )
                                    .text_size(TEXT_SETTINGS_AUX)
                                    .text_color(color(GRAY)),
                                )
                            })
            );
            cache_details = Some(crate::motion::disclosure(
                        SharedString::from(format!("model-cache-content-{key}")),
                        cache_open,
                        cache));
            if let Some(error) = &status.last_error {
                view = view.child(settings_detail_row(
                    SharedString::from(format!("model-last-error-label-{key}")),
                    "上次准备",
                    settings_value(
                        SharedString::from(format!("model-last-error-{key}")),
                        format!("未完成：{error}"),
                    )
                    .text_color(color(DANGER)),
                ));
            }
        }
        (view, cache_action, cache_details)
    }

    /// Action row (recheck / pause / prepare) and the transient rows it owns:
    /// prepare progress, download transfers, and queue/network-scope notes.
    fn model_prepare_actions(
        &self,
        provider: AsrProvider,
        request: &Request,
        key: &str,
        checking: bool,
        status: Option<&LocalModelStatus>,
        device_issue: &Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Div, Vec<AnyElement>) {
        let mut extras: Vec<AnyElement> = Vec::new();
        let request = request.clone();
        let active = self.kind == Kind::Models
            && self.job.is_some()
            && self
                .settings_ui
                .model_diagnostics
                .preparing
                .as_ref()
                .is_some_and(|active| active.key() == key);
        let request_for_check = request.clone();
        let recheck = outline_pill(SharedString::from(format!("recheck-model-{key}"))).settings_text()
            .icon(icons::refresh())
            .label("重新检查模型")
            .disabled(checking || active)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.check_model_request(request_for_check.clone(), true, cx)
            }));
        let mut actions = h_flex().w_full().min_w_0().gap_2().flex_wrap();

        let prepare_allowed = self.environment.is_some()
            && device_issue.is_none()
            && status.is_some_and(|status| status.can_prepare);
        if active {
            extras.push(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(crate::motion::spinner("model-prepare-spinner", cx))
                    .child(
                        settings_value(
                            "active-model-prepare",
                            // 对外展示用显示名，不外露缓存目录里的技术 ID（review4 可选）
                            format!("正在准备 {}", model_display_name(&request.model)),
                        )
                        .role(Role::Status),
                    )
                    .into_any_element(),
            );
            for (index, (stage, progress)) in self
                .progress
                .iter()
                .filter(|(stage, progress)| {
                    crate::activity::is_model_transfer_stage(stage)
                        && progress.has_samples()
                        && !progress.done
                })
                .enumerate()
            {
                extras.push(
                    crate::motion::transfer_status(
                        ("model-download-progress", index),
                        crate::activity::model_transfer_phase(stage, &progress.message),
                        &progress.transfer_metrics(stage, true),
                        progress.fraction(),
                        window,
                        cx,
                    )
                    .into_any_element(),
                );
            }
            actions = actions.child(
                control("stop-model-preparation").settings_text()
                    .icon(icons::pause())
                    .label(if self.cancelling {
                        "正在停止…"
                    } else {
                        "暂停准备"
                    })
                    .disabled(self.cancelling)
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(job) = &this.job {
                            job.cancel();
                            this.cancelling = true;
                            cx.notify();
                        }
                    })),
            );
        } else if prepare_allowed {
            let loaded = status.is_some_and(|status| status.state == CacheState::Loaded);
            let label = model_action_label(provider, status.map(|status| &status.state));
            actions = actions.child(
                control(SharedString::from(format!("prepare-model-{key}"))).settings_text()
                    .icon(if loaded {
                        icons::refresh()
                    } else {
                        icons::download()
                    })
                    .when(loaded, |button| button.ghost())
                    .when(!loaded, |button| button.primary())
                    .label(label)
                    .accessibility_label(format!(
                        "{label}：{} · {}",
                        provider_name(provider),
                        request.model
                    ))
                    .disabled(self.job.is_some())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.begin_model_preparation(request.clone(), cx)
                    })),
            );
            if self.job.is_some() && !loaded {
                extras.push(
                    settings_value(
                        SharedString::from(format!("model-waits-for-job-{key}")),
                        "当前处理结束后可准备",
                    )
                    .text_sm()
                    .into_any_element(),
                );
            }
        }
        (actions.child(recheck), extras)
    }

    pub(super) fn model_diagnostics_panel(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let request = self.default_model_request();
        // 使用语音服务识别时没有本地模型可检查：整组省略，而不是重述识别方式
        let mut view = v_flex().w_full().min_w_0().gap_6();
        if request.provider != AsrProvider::Api {
            view = view.child(
                crate::settings_ui::SettingsGroup::new("model-diagnostic-default", "默认识别模型")
                    .child(crate::settings_ui::setting_surface().child(
                        self.model_readiness_panel(
                            request.provider,
                            Some(&request.model),
                            &request.root,
                            window,
                            cx,
                        ),
                    )),
            );
        }
        if self.kind == Kind::Models
            && self.job.is_some()
            && let Some(active) = &self.settings_ui.model_diagnostics.preparing
            && active.key() != request.key()
        {
            view = view.child(
                settings_detail_group("other-active-model", icons::storage(), "正在准备的模型")
                    .child(self.model_readiness_panel(
                        active.provider,
                        Some(&active.model),
                        &active.root,
                        window,
                        cx,
                    )),
            );
        }
        if let Some((message, error)) = &self.settings_ui.model_diagnostics.result {
            view = view.child(
                settings_value("model-prepare-result", message.clone())
                    .role(Role::Status)
                    .text_color(color(if *error { DANGER } else { INK })),
            );
        }
        if self.settings_ui.model_diagnostics.preparing.is_some()
            && !self.logs.is_empty()
            && self.kind == Kind::Models
        {
            view = view.child(
                control("model-preparation-details").settings_text()
                    .icon(icons::info())
                    .ghost()
                    .label(if self.settings_ui.model_diagnostics.details {
                        "收起准备详情"
                    } else {
                        "查看准备详情"
                    })
                    .self_start()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.settings_ui.model_diagnostics.details =
                            !this.settings_ui.model_diagnostics.details;
                        cx.notify();
                    })),
            );
            if self.settings_ui.model_diagnostics.details {
                view = view.child(
                    settings_detail_group(
                        "model-preparation-log-heading",
                        icons::task(),
                        "准备日志",
                    )
                    .child(
                        settings_value(
                            "model-preparation-log",
                            self.logs.iter().cloned().collect::<Vec<_>>().join("\n"),
                        )
                        .text_size(TEXT_SETTINGS_AUX),
                    ),
                );
            }
        }
        view
    }
    pub(super) fn model_hardware_details(&self, cx: &mut Context<Self>) -> Div {
        let request = self.default_model_request();
        let Some(environment) = &self.environment else {
            return v_flex();
        };
        let mut hardware =
            settings_detail_group("model-hardware-heading", icons::computer(), "设备与运行时");
        for (id, label, value) in [
            ("cpu-device", "CPU 架构", std::env::consts::ARCH),
            (
                "apple-device",
                "Apple 平台",
                if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
                    "原生运行于 Apple 芯片"
                } else {
                    "当前平台不适用"
                },
            ),
            (
                "gpu-device",
                "GPU",
                environment.gpu.as_deref().unwrap_or(if environment.llama {
                    "未检测到"
                } else {
                    "无法检查（缺少 llama-server）"
                }),
            ),
            (
                "npu-device",
                "Intel NPU",
                if environment.npu_device {
                    "已检测到设备"
                } else {
                    "未检测到设备"
                },
            ),
            (
                "npu-runtime",
                "NPU 启动器",
                if environment.npu_runtime {
                    "已找到"
                } else {
                    "未找到 uv 或 Python"
                },
            ),
            (
                "apple-runtime",
                "Apple 识别程序",
                if environment.apple {
                    "已检测到"
                } else {
                    "不完整"
                },
            ),
        ]
        .into_iter()
        .filter(|(id, _, _)| match *id {
            "apple-device" | "apple-runtime" => cfg!(target_os = "macos"),
            "npu-device" | "npu-runtime" => {
                !cfg!(target_os = "macos") || request.provider == AsrProvider::Npu
            }
            _ => true,
        }) {
            hardware = hardware.child(settings_detail_row(
                SharedString::from(format!("{id}-label")),
                label,
                settings_value(id, value.to_owned()),
            ));
        }
        let alternatives = [
            (AsrProvider::Coreml, environment.apple),
            (
                AsrProvider::Gpu,
                environment.gpu.is_some() && environment.llama,
            ),
            (AsrProvider::Cpu, environment.llama),
            (AsrProvider::Npu, environment.npu),
        ];
        let available = alternatives
            .into_iter()
            .filter(|(_, available)| *available)
            .map(|(provider, _)| provider_name(provider))
            .collect::<Vec<_>>();
        if !available.is_empty() {
            hardware = hardware.child(settings_detail_row(
                "available-local-model-providers-label",
                "可用识别方式",
                settings_value("available-local-model-providers", available.join("、")),
            ));
        }
        if self.model_device_issue(request.provider).is_some() {
            hardware = hardware.child(
                h_flex().gap_2().flex_wrap().children(
                    alternatives
                        .into_iter()
                        .filter(|(provider, available)| *available && *provider != request.provider)
                        .map(|(provider, _)| {
                            control(SharedString::from(format!(
                                "choose-available-model-{}",
                                provider.as_str()
                            ))).settings_text()
                            .icon(icons::mic())
                            .label(format!("默认改用 {}", provider_name(provider)))
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    let mut value = this.generation_edit_base();
                                    value.select_provider(Some(provider));
                                    value.options.asr_model =
                                        Some(course2md::config::DEFAULT_ASR_MODEL.into());
                                    if this.commit_generation(value, cx) {
                                        this.refresh_model_diagnostics(cx);
                                    }
                                },
                            ))
                        }),
                ),
            );
        }
        hardware
    }
    pub(super) fn default_model_readiness_panel(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let request = self.default_model_request();
        self.model_readiness_panel(
            request.provider,
            Some(&request.model),
            &request.root,
            window,
            cx,
        )
    }

    /// One-line readiness conclusion for the generation group main flow; the full
    /// panel stays behind 技术详情.
    pub(super) fn default_model_conclusion(&self) -> (bool, String) {
        let request = self.default_model_request();
        if request.provider == AsrProvider::Api {
            return (true, "使用语音服务识别".into());
        }
        if self.model_device_issue(request.provider).is_some() {
            return (false, "所选识别方式暂不可用，可在模型管理中查看原因".into());
        }
        let entry = self
            .settings_ui
            .model_diagnostics
            .entries
            .get(&request.key());
        if entry.is_none_or(Entry::checking) {
            return (true, "正在检查识别模型…".into());
        }
        if entry.and_then(Entry::result).is_some_and(Result::is_err) {
            return (false, "模型检查未完成，可在模型管理中查看原因并重试".into());
        }
        match entry
            .and_then(Entry::result)
            .and_then(|result| result.as_ref().ok())
            .map(|status| &status.state)
        {
            Some(CacheState::Loaded) => (true, "识别模型已就绪".into()),
            Some(CacheState::Cached) => (true, "模型已下载".into()),
            Some(CacheState::Missing | CacheState::Partial) => {
                (true, "首次识别时会自动下载所选模型".into())
            }
            Some(CacheState::Unsupported) => {
                (false, "当前识别方式不支持所选模型，请选择其他模型".into())
            }
            None => (true, "尚未检查识别模型".into()),
        }
    }
}
