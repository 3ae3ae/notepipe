//! A resumable first-use guide. Configuration, contract tests and model jobs
//! use the same persistence and execution paths as Settings.
use crate::credentials::Secret;
use crate::preferences::{
    Authentication, BindingScope, GenerationPreferences, PreferenceGroup, ServiceConfiguration,
    ServiceDraft, ServicePurpose, ServiceTestEvidence, ServiceVersion, TestOutcome,
};
use crate::service_test::{self, TestKind};
use crate::settings_ui::{field_label, settings_detail_row, settings_value};
use crate::theme::*;
use crate::*;
use course2md::{
    config::{AsrProvider, model_dir_from},
    models::status::CacheState,
};
use gpui_component::switch::Switch;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Engine,
    Ai,
    Account,
    Model,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum InputField {
    Address,
    Model,
    Key,
}

pub(crate) fn saved_api_key_placeholder(has_saved_credential: bool) -> &'static str {
    if has_saved_credential {
        "••••••••"
    } else {
        ""
    }
}

struct ServiceSetup {
    inputs: BTreeMap<InputField, Entity<InputState>>,
    draft: ServiceDraft,
    original: Option<ServiceVersion>,
    pending_version: Option<ServiceVersion>,
    errors: Vec<preferences::FieldError>,
    evidence: Vec<(TestKind, ServiceTestEvidence)>,
    running: Option<Arc<AtomicBool>>,
    test_serial: u64,
    details_open: bool,
    show_key: bool,
    models: crate::model_discovery::State,
    _subscriptions: Vec<Subscription>,
}

#[derive(Clone, PartialEq, Eq)]
struct ModelRequest {
    provider: AsrProvider,
    model: String,
    root: PathBuf,
}

impl ModelRequest {
    // 与 model_diagnostics::Request::key 同一格式，跨模块匹配用
    fn key(&self) -> String {
        format!(
            "{}:{}:{}",
            self.provider.as_str(),
            self.model,
            self.root.display()
        )
    }
}

impl State {
    /// model_preparation_finished 事件调用：请求匹配时同步准备结果
    pub(crate) fn apply_model_preparation_result(
        &mut self,
        request_key: &str,
        result: (String, bool),
        cancelled: bool,
    ) {
        if let Some(preparation) = &mut self.model_preparation
            && preparation.request.key() == request_key
        {
            preparation.result = Some(result);
            preparation.cancelled = cancelled;
        }
    }
}

struct ModelPreparation {
    request: ModelRequest,
    result: Option<(String, bool)>,
    cancelled: bool,
}

impl ServiceSetup {
    fn new(purpose: ServicePurpose, window: &mut Window, cx: &mut Context<Desktop>) -> Self {
        let inputs: BTreeMap<_, _> = [InputField::Address, InputField::Model, InputField::Key]
            .into_iter()
            .map(|field| {
                let placeholder = match field {
                    InputField::Address => "https://api.example.com/v1",
                    InputField::Model => "",
                    InputField::Key => "",
                };
                (
                    field,
                    cx.new(|cx| {
                        InputState::new(window, cx)
                            .placeholder(placeholder)
                            .masked(field == InputField::Key)
                    }),
                )
            })
            .collect();
        let subscriptions = inputs
            .iter()
            .map(|(field, input)| {
                let field = *field;
                cx.subscribe_in(input, window, move |this, _, event, _, cx| {
                    if matches!(event, InputEvent::Change) && this.onboarding.active {
                        let service = this.onboarding.service_mut(purpose);
                        service.errors.clear();
                        service.evidence.clear();
                        if matches!(field, InputField::Address | InputField::Key) {
                            service.models.invalidate();
                        }
                        this.onboarding.notice = None;
                        cx.notify();
                    }
                })
            })
            .collect();
        Self {
            inputs,
            draft: ServiceDraft::new(purpose),
            original: None,
            pending_version: None,
            errors: Vec::new(),
            evidence: Vec::new(),
            running: None,
            test_serial: 0,
            details_open: false,
            show_key: false,
            models: crate::model_discovery::State::default(),
            _subscriptions: subscriptions,
        }
    }
    fn cancel(&self) {
        if let Some(cancel) = &self.running {
            cancel.store(true, Ordering::Release);
        }
    }
    fn value(&self, field: InputField, cx: &App) -> String {
        self.inputs[&field].read(cx).value().to_string()
    }
    fn current_draft(&self, cx: &App) -> ServiceDraft {
        let mut draft = self.draft.clone();
        draft.address = self.value(InputField::Address, cx);
        draft.model = self.value(InputField::Model, cx);
        draft
    }
    fn unchanged(&self, cx: &App) -> bool {
        self.value(InputField::Key, cx).is_empty()
            && self.original.as_ref().is_some_and(|version| {
                self.current_draft(cx)
                    .configuration()
                    .ok()
                    .is_some_and(|config| {
                        config.fingerprint("configuration")
                            == version.config.fingerprint("configuration")
                    })
            })
    }
}

pub(crate) struct State {
    pub(crate) active: bool,
    step: Step,
    session: u64,
    provider: Option<AsrProvider>,
    local_provider: Option<AsrProvider>,
    model: String,
    ai_proofread: bool,
    ai_summary: bool,
    speech: ServiceSetup,
    ai: ServiceSetup,
    notice: Option<(String, bool)>,
    finish_failed: bool,
    model_details_open: bool,
    model_choices_open: bool,
    model_preparation: Option<ModelPreparation>,
    model_status_request: Option<ModelRequest>,
    model_return_page: Option<Page>,
    scroll: ScrollHandle,
}

impl State {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Desktop>) -> Self {
        Self {
            active: false,
            step: Step::Engine,
            session: 0,
            provider: None,
            local_provider: None,
            model: course2md::config::DEFAULT_ASR_MODEL.into(),
            ai_proofread: true,
            ai_summary: false,
            speech: ServiceSetup::new(ServicePurpose::Speech, window, cx),
            ai: ServiceSetup::new(ServicePurpose::Ai, window, cx),
            notice: None,
            finish_failed: false,
            model_details_open: false,
            model_choices_open: false,
            model_preparation: None,
            model_status_request: None,
            model_return_page: None,
            scroll: ScrollHandle::new(),
        }
    }
    fn service(&self, purpose: ServicePurpose) -> &ServiceSetup {
        match purpose {
            ServicePurpose::Speech => &self.speech,
            ServicePurpose::Ai => &self.ai,
        }
    }
    fn service_mut(&mut self, purpose: ServicePurpose) -> &mut ServiceSetup {
        match purpose {
            ServicePurpose::Speech => &mut self.speech,
            ServicePurpose::Ai => &mut self.ai,
        }
    }
}

#[derive(Clone, Copy)]
enum ProviderChoice {
    Local,
    Online,
    Engine(Option<AsrProvider>),
}

/// Ask the same validator used before transcription. Explicit scratch paths and
/// a provider keep this check independent of user files, devices and environment.
fn setup_model_supported(provider: AsrProvider, model: &str) -> bool {
    if provider == AsrProvider::Api || model.trim().is_empty() {
        return false;
    }
    let mut file = course2md::settings::ConfigFile::default();
    file.defaults.provider = Some(provider);
    file.defaults.asr_model = Some(model.to_owned());
    file.defaults.out = Some(PathBuf::from("."));
    file.defaults.model_dir = Some(PathBuf::from("."));
    course2md::options::resolve(String::new(), &Default::default(), &file)
        .is_ok_and(|config| config.validate_asr_with_auth(false, false).is_ok())
}

/// Cache inspection uses canonical names, while the saved selection can retain
/// a compatible alias. Never normalize an unsupported value into another model.
fn setup_cache_model(provider: AsrProvider, model: &str) -> String {
    if !setup_model_supported(provider, model) {
        return model.to_owned();
    }
    match provider {
        AsrProvider::Cpu | AsrProvider::Gpu => course2md::config::DEFAULT_ASR_MODEL.into(),
        AsrProvider::Coreml => {
            course2md::models::normalize_apple_model(model).unwrap_or_else(|_| model.to_owned())
        }
        _ => model.to_owned(),
    }
}

fn apply_provider_choice(
    provider: &mut Option<AsrProvider>,
    local_provider: &mut Option<AsrProvider>,
    model: &mut String,
    choice: ProviderChoice,
    recommended: AsrProvider,
) -> Option<String> {
    let next = match choice {
        ProviderChoice::Online | ProviderChoice::Engine(Some(AsrProvider::Api)) => {
            if *provider != Some(AsrProvider::Api) {
                *local_provider = *provider;
            }
            Some(AsrProvider::Api)
        }
        ProviderChoice::Local => *local_provider,
        ProviderChoice::Engine(selected) => {
            *local_provider = selected;
            selected
        }
    };
    *provider = next;
    if next == Some(AsrProvider::Api) {
        return None;
    }
    let resolved = next.unwrap_or(recommended);
    if setup_model_supported(resolved, model) {
        return None;
    }
    let previous = std::mem::replace(model, course2md::config::DEFAULT_ASR_MODEL.into());
    Some(format!(
        "{} 不支持“{}”，已改用 Qwen3 1.7B。",
        crate::provider_label(Some(resolved)),
        previous
    ))
}

/// A pre-existing alias or repository remains an explicit, visible choice.
fn local_model_choices(provider: AsrProvider, current: &str) -> Vec<(String, String, String)> {
    let mut choices = vec![(
        course2md::config::DEFAULT_ASR_MODEL.to_owned(),
        "Qwen3 1.7B".to_owned(),
        "默认 · 占用较高".to_owned(),
    )];
    if matches!(provider, AsrProvider::Coreml | AsrProvider::Npu) {
        choices.extend([
            (
                "qwen3-0.6b".into(),
                "Qwen3 0.6B".into(),
                "轻量".into(),
            ),
            (
                "whisper".into(),
                "Whisper".into(),
                String::new(),
            ),
        ]);
    }
    if provider == AsrProvider::Npu {
        choices.extend([
            (
                "whisper-tiny".into(),
                "Whisper Tiny".into(),
                String::new(),
            ),
            (
                "whisper-base".into(),
                "Whisper Base".into(),
                String::new(),
            ),
            (
                "whisper-small".into(),
                "Whisper Small".into(),
                String::new(),
            ),
        ]);
    }
    if !choices.iter().any(|(id, _, _)| id == current) {
        choices.push((
            current.to_owned(),
            current.to_owned(),
            if setup_model_supported(provider, current) {
                "当前配置"
            } else {
                "当前配置不适用于此引擎，请选择其他模型"
            }
            .into(),
        ));
    }
    choices
}

fn engine_preferences(
    current: &GenerationPreferences,
    provider: Option<AsrProvider>,
    model: &str,
) -> GenerationPreferences {
    let mut next = current.clone();
    next.select_provider(provider);
    if provider != Some(AsrProvider::Api) {
        next.options.asr_model = Some(model.to_owned());
        next.local_model_draft = None;
    }
    next
}

fn requested_tests(
    purpose: ServicePurpose,
    proofread: bool,
    summary: bool,
    vision: bool,
) -> Vec<TestKind> {
    if purpose == ServicePurpose::Speech {
        return vec![TestKind::Speech];
    }
    let mut tests = enabled_ai_tests(proofread, summary, vision);
    if tests.is_empty() {
        tests.push(TestKind::Proofread);
    }
    tests
}

fn enabled_ai_tests(proofread: bool, summary: bool, vision: bool) -> Vec<TestKind> {
    let mut tests = Vec::new();
    if proofread {
        tests.push(TestKind::Proofread);
    }
    if summary {
        tests.push(TestKind::Summary);
    }
    if proofread && vision {
        tests.push(TestKind::Vision);
    }
    tests
}

fn newly_enabled_ai_tests(
    current: &GenerationPreferences,
    proofread: bool,
    summary: bool,
) -> Vec<TestKind> {
    let existing = enabled_ai_tests(current.ai_proofread, current.ai_summary, current.vision);
    enabled_ai_tests(proofread, summary, current.vision)
        .into_iter()
        .filter(|kind| !existing.contains(kind))
        .collect()
}

fn evidence_covers(
    config: &ServiceConfiguration,
    required: &[TestKind],
    evidence: &[(TestKind, ServiceTestEvidence)],
) -> bool {
    required.iter().all(|kind| {
        evidence.iter().any(|(tested, evidence)| {
            tested == kind
                && evidence.outcome == TestOutcome::Passed
                && evidence.contract == kind.contract()
                && evidence.fingerprint == config.fingerprint(kind.contract())
        })
    })
}

/// 共享 ⓘ 辅助信息：设置/引导/导入页同一处理（共享图标 + 常规字重，review4#2）。
pub(crate) fn help(id: impl Into<ElementId>, value: impl Into<SharedString>) -> Div {
    theme::supporting_info(id, value)
}


impl Desktop {
    pub(crate) fn start_onboarding(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.setup_model_job_running() {
            self.open_setup_model_status(window, cx);
            return;
        }
        if self.page == Page::New && !self.save_current_draft(cx) {
            return;
        }
        self.onboarding.speech.cancel();
        self.onboarding.ai.cancel();
        for purpose in [ServicePurpose::Speech, ServicePurpose::Ai] {
            let id = self.onboarding.service(purpose).draft.id.clone();
            let _ = self.preferences.discard_service_draft(&id);
        }
        self.onboarding.active = false;
        self.onboarding.session += 1;
        let generation = self.preferences.generation();
        self.onboarding.provider = generation.options.provider;
        self.onboarding.local_provider = if generation.options.provider == Some(AsrProvider::Api) {
            generation.last_local_provider
        } else {
            generation.options.provider
        };
        self.onboarding.model = generation
            .options
            .asr_model
            .clone()
            .unwrap_or(course2md::config::DEFAULT_ASR_MODEL.into());
        self.onboarding.ai_proofread = generation.ai_proofread;
        self.onboarding.ai_summary = generation.ai_summary;
        let refs = self.preferences.default_refs();
        self.hydrate_setup_service(ServicePurpose::Speech, refs.asr.as_deref(), window, cx);
        self.hydrate_setup_service(ServicePurpose::Ai, refs.llm.as_deref(), window, cx);
        if self.onboarding.ai.original.is_none() {
            self.onboarding.ai_proofread = true;
        }
        self.onboarding.model_details_open = false;
        self.onboarding.model_choices_open = false;
        self.onboarding.finish_failed = false;
        self.onboarding.model_status_request = None;
        self.onboarding.model_return_page = None;
        self.onboarding.active = true;
        self.setup_step(Step::Engine, cx);
        self.root_focus.focus(window, cx);
    }

    fn select_setup_route(&mut self, local: bool, cx: &mut Context<Self>) {
        self.apply_setup_provider_choice(
            if local {
                ProviderChoice::Local
            } else {
                ProviderChoice::Online
            },
            cx,
        );
    }

    fn select_setup_provider(&mut self, provider: Option<AsrProvider>, cx: &mut Context<Self>) {
        self.apply_setup_provider_choice(ProviderChoice::Engine(provider), cx);
    }

    fn apply_setup_provider_choice(&mut self, choice: ProviderChoice, cx: &mut Context<Self>) {
        let recommended = self.recommended_local_provider();
        let state = &mut self.onboarding;
        let notice = apply_provider_choice(
            &mut state.provider,
            &mut state.local_provider,
            &mut state.model,
            choice,
            recommended,
        );
        state.notice = notice.map(|message| (message, false));
        cx.notify();
    }

    fn select_setup_model(&mut self, model: &str, cx: &mut Context<Self>) {
        let provider = self
            .onboarding
            .provider
            .unwrap_or_else(|| self.recommended_local_provider());
        if setup_model_supported(provider, model) {
            self.onboarding.model = model.to_owned();
            self.onboarding.notice = None;
        } else {
            self.onboarding.notice =
                Some(("此模型不适用于当前引擎，请选择其他模型。".into(), true));
        }
        cx.notify();
    }

    fn hydrate_setup_service(
        &mut self,
        purpose: ServicePurpose,
        reference: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let original = reference
            .and_then(|id| self.preferences.version(id))
            .cloned();
        let draft = original
            .as_ref()
            .map(ServiceDraft::from_version)
            .unwrap_or_else(|| ServiceDraft::new(purpose));
        let service = self.onboarding.service_mut(purpose);
        service.cancel();
        service.running = None;
        service.draft = draft.clone();
        service.original = original;
        service.pending_version = None;
        service.models.invalidate();
        service.errors.clear();
        service.evidence.clear();
        service.details_open = false;
        service.show_key = false;
        for (field, value) in [
            (InputField::Address, draft.address),
            (InputField::Model, draft.model),
            (InputField::Key, String::new()),
        ] {
            service.inputs[&field].update(cx, |input, cx| input.set_value(value, window, cx));
        }
        service.inputs[&InputField::Key].update(cx, |input, cx| {
            input.set_placeholder(
                saved_api_key_placeholder(draft.credential.is_some()),
                window,
                cx,
            );
            input.set_masked(true, window, cx);
        });
    }

    fn setup_step(&mut self, step: Step, cx: &mut Context<Self>) {
        if step == Step::Engine && self.setup_model_job_running() {
            return;
        }
        if step != Step::Model {
            self.onboarding.model_status_request = None;
            self.onboarding.model_return_page = None;
        }
        if step == Step::Account {
            self.refresh_account(cx);
        }
        if step == Step::Ai
            && self.onboarding.ai.draft.protocol == preferences::ServiceProtocol::CodexResponses
        {
            self.codex_refresh_status(crate::codex_ui::CodexSurface::Onboarding, cx);
        }
        self.onboarding.step = step;
        self.onboarding.notice = None;
        self.onboarding.scroll.set_offset(point(px(0.), px(0.)));
        cx.notify();
    }

    fn setup_tests(&self, purpose: ServicePurpose) -> Vec<TestKind> {
        requested_tests(
            purpose,
            self.onboarding.ai_proofread,
            self.onboarding.ai_summary,
            self.preferences.generation().vision,
        )
    }

    fn setup_required_tests(&self, purpose: ServicePurpose, cx: &App) -> Vec<TestKind> {
        if !self.onboarding.service(purpose).unchanged(cx) {
            return self.setup_tests(purpose);
        }
        match purpose {
            ServicePurpose::Speech => Vec::new(),
            ServicePurpose::Ai => newly_enabled_ai_tests(
                self.preferences.generation(),
                self.onboarding.ai_proofread,
                self.onboarding.ai_summary,
            ),
        }
    }

    fn setup_service_passed(&self, purpose: ServicePurpose, cx: &App) -> bool {
        let service = self.onboarding.service(purpose);
        service.value(InputField::Key, cx).is_empty()
            && service
                .current_draft(cx)
                .configuration()
                .ok()
                .is_some_and(|config| {
                    evidence_covers(
                        &config,
                        &self.setup_required_tests(purpose, cx),
                        &service.evidence,
                    )
                })
    }

    fn stage_setup_service(
        &mut self,
        purpose: ServicePurpose,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<ServiceConfiguration> {
        let service = self.onboarding.service(purpose);
        let draft = service.current_draft(cx);
        let key = service.value(InputField::Key, cx);
        match self.preferences.save_service_draft(
            draft,
            (!key.trim().is_empty()).then(|| Secret::new(key.clone())),
        ) {
            Ok(draft) => {
                let service = self.onboarding.service_mut(purpose);
                service.draft = draft;
                if !key.is_empty() {
                    service.inputs[&InputField::Key]
                        .update(cx, |input, cx| input.set_value("", window, cx));
                }
                service.errors = service.draft.validate();
                if let Some(error) = service.errors.first() {
                    let field = match error.field {
                        "address" => InputField::Address,
                        "model" => InputField::Model,
                        _ => InputField::Key,
                    };
                    service.inputs[&field].update(cx, |input, cx| input.focus(window, cx));
                    cx.notify();
                    return None;
                }
                service.draft.configuration().ok()
            }
            Err(error) => {
                self.onboarding.notice = Some((
                    preferences::save_failure_message(PreferenceGroup::Services, &error),
                    true,
                ));
                cx.notify();
                None
            }
        }
    }

    fn start_setup_test(
        &mut self,
        purpose: ServicePurpose,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.onboarding.service(purpose).running.is_some() {
            return;
        }
        let Some(config) = self.stage_setup_service(purpose, window, cx) else {
            return;
        };
        let required = self.setup_required_tests(purpose, cx);
        let required = if required.is_empty() {
            self.setup_tests(purpose)
        } else {
            required
        };
        self.root_focus.focus(window, cx);
        let cancel = Arc::new(AtomicBool::new(false));
        let session = self.onboarding.session;
        let service = self.onboarding.service_mut(purpose);
        service.test_serial += 1;
        let serial = service.test_serial;
        service.running = Some(cancel.clone());
        service.evidence.clear();
        self.onboarding.notice = None;
        let vault = self.preferences.vault();
        // 服务测试含同步 Keychain 读取与受限 HTTP；见 crate::spawn_blocking_io 的说明
        let task = crate::spawn_blocking_io(move || {
            smol::block_on(async move {
                let mut results = Vec::new();
                for kind in required {
                    // Codex 订阅服务走 CLI 方言（Responses/SSE）检查登录态与模型
                    let evidence = if config.protocol == preferences::ServiceProtocol::CodexResponses
                    {
                        service_test::test_codex_blocking(&config, kind, &cancel)
                    } else {
                        service_test::test_service(config.clone(), kind, vault.clone(), cancel.clone())
                            .await
                    };
                    let passed = evidence.outcome == TestOutcome::Passed;
                    results.push((kind, evidence));
                    if !passed || cancel.load(Ordering::Acquire) {
                        break;
                    }
                }
                results
            })
        });
        cx.spawn(async move |this, cx| {
            let Ok(results) = task.recv().await else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                let mut persisted = true;
                for (_, evidence) in &results {
                    persisted &= this.preferences.record_test(evidence.clone()).is_ok();
                }
                if this.onboarding.session != session
                    || this.onboarding.service(purpose).test_serial != serial
                {
                    return;
                }
                let service = this.onboarding.service(purpose);
                let unchanged = service.value(InputField::Key, cx).is_empty()
                    && service
                        .current_draft(cx)
                        .configuration()
                        .ok()
                        .is_some_and(|config| {
                            results.iter().all(|(_, evidence)| {
                                evidence.fingerprint == config.fingerprint(&evidence.contract)
                            })
                        });
                let service = this.onboarding.service_mut(purpose);
                service.running = None;
                if unchanged {
                    service.evidence = results;
                    if !persisted {
                        this.onboarding.notice = Some((
                            "检查结果尚未保存".into(),
                            true,
                        ));
                    }
                } else {
                    this.onboarding.notice =
                        Some(("配置已改变，请检查当前填写的配置。".into(), false));
                }
                // Keep newly arrived feedback above the fixed action row.
                if this.onboarding.active
                    && matches!(
                        (this.onboarding.step, purpose),
                        (Step::Ai, ServicePurpose::Ai) | (Step::Engine, ServicePurpose::Speech)
                    )
                {
                    this.onboarding.scroll.scroll_to_bottom();
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn continue_setup_service(
        &mut self,
        purpose: ServicePurpose,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let unchanged = self.onboarding.service(purpose).unchanged(cx);
        let pending = self.onboarding.service(purpose).pending_version.clone();
        if pending.is_none() && !self.setup_service_passed(purpose, cx) {
            self.start_setup_test(purpose, window, cx);
            return;
        }
        let version = if let Some(version) = pending {
            version
        } else if unchanged {
            let Some(version) = self.onboarding.service(purpose).original.clone() else {
                return;
            };
            if let Err(error) = self.preferences.check_dispatch(&version.id) {
                self.onboarding.notice = Some((format!("当前服务暂不可用：{error:#}"), true));
                cx.notify();
                return;
            }
            version
        } else {
            if self.stage_setup_service(purpose, window, cx).is_none() {
                return;
            }
            let id = self.onboarding.service(purpose).draft.id.clone();
            match self
                .preferences
                .publish_service(&id, BindingScope::Defaults)
            {
                Ok(version) => {
                    self.onboarding.service_mut(purpose).pending_version = Some(version.clone());
                    version
                }
                Err(error) => {
                    self.onboarding.notice = Some((
                        preferences::save_failure_message(PreferenceGroup::Services, &error),
                        true,
                    ));
                    cx.notify();
                    return;
                }
            }
        };
        let mut next = self.preferences.generation().clone();
        match purpose {
            ServicePurpose::Speech => next.select_provider(Some(AsrProvider::Api)),
            ServicePurpose::Ai => {
                next.ai_proofread = self.onboarding.ai_proofread;
                next.ai_summary = self.onboarding.ai_summary;
            }
        }
        if !self.commit_generation(next, cx) {
            self.onboarding.notice = Some((
                "默认选项尚未保存，可重试".into(),
                true,
            ));
            return;
        }
        self.refresh_dispatch_controls(cx);
        self.hydrate_setup_service(purpose, Some(&version.id), window, cx);
        self.setup_step(
            if purpose == ServicePurpose::Speech {
                Step::Ai
            } else {
                Step::Account
            },
            cx,
        );
    }

    fn save_setup_engine(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.onboarding.provider == Some(AsrProvider::Api) {
            self.continue_setup_service(ServicePurpose::Speech, window, cx);
            return;
        }
        let provider = self
            .onboarding
            .provider
            .unwrap_or_else(|| self.recommended_local_provider());
        if !setup_model_supported(provider, &self.onboarding.model) {
            self.onboarding.notice =
                Some(("当前模型不适用于所选引擎，请选择其他模型。".into(), true));
            cx.notify();
            return;
        }
        let next = engine_preferences(
            self.preferences.generation(),
            self.onboarding.provider,
            &self.onboarding.model,
        );
        if self.commit_generation(next, cx) {
            self.setup_step(Step::Ai, cx);
        } else {
            self.onboarding.notice =
                Some(("默认引擎尚未保存，当前选择保留。请重试保存。".into(), true));
        }
    }

    fn skip_setup_ai(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.onboarding.ai.cancel();
        // Keep this private edit valid when the user returns within the guide.
        // Only finishing the guide discards unpublished service drafts.
        self.setup_step(Step::Account, cx);
    }

    fn setup_model_request(&self) -> ModelRequest {
        self.onboarding
            .model_status_request
            .clone()
            .unwrap_or_else(|| ModelRequest {
                provider: self
                    .onboarding
                    .provider
                    .unwrap_or_else(|| self.recommended_local_provider()),
                model: setup_cache_model(
                    self.onboarding
                        .provider
                        .unwrap_or_else(|| self.recommended_local_provider()),
                    &self.onboarding.model,
                ),
                root: model_dir_from(self.preferences.generation().options.model_dir.as_deref()),
            })
    }

    fn setup_model_job_running(&self) -> bool {
        self.onboarding
            .model_preparation
            .as_ref()
            .is_some_and(|preparation| {
                let request = &preparation.request;
                self.setup_model_snapshot(request.provider, Some(&request.model), &request.root)
                    .preparing
            })
    }

    fn open_setup_model_status(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.onboarding.active && self.page == Page::New && !self.save_current_draft(cx) {
            return;
        }
        let Some(preparation) = &self.onboarding.model_preparation else {
            return;
        };
        if !self.onboarding.active {
            self.onboarding.model_return_page = Some(self.page);
        }
        self.onboarding.model_status_request = Some(preparation.request.clone());
        self.onboarding.active = true;
        self.setup_step(Step::Model, cx);
        self.root_focus.focus(window, cx);
    }

    fn finish_onboarding(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut application = self.preferences.application().clone();
        application.desktop.setup_completed = true;
        if let Err(error) = self.preferences.save_application(application) {
            self.onboarding.finish_failed = true;
            self.onboarding.notice = Some((
                preferences::save_failure_message(PreferenceGroup::Application, &error),
                true,
            ));
            cx.notify();
            return;
        }
        self.leave_onboarding(window, cx);
    }

    fn leave_onboarding(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for purpose in [ServicePurpose::Speech, ServicePurpose::Ai] {
            self.onboarding.service(purpose).cancel();
            self.onboarding.service_mut(purpose).models.invalidate();
            let id = self.onboarding.service(purpose).draft.id.clone();
            let _ = self.preferences.discard_service_draft(&id);
            self.onboarding.service(purpose).inputs[&InputField::Key]
                .update(cx, |input, cx| input.set_value("", window, cx));
        }
        self.onboarding.active = false;
        self.onboarding.finish_failed = false;
        self.refresh_preference_defaults(cx);
        let return_page = self
            .onboarding
            .model_return_page
            .take()
            .unwrap_or(Page::New);
        self.navigate(return_page, cx);
        self.root_focus.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn onboarding_page(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // 渲染不收敛状态：model_preparation 的结果在 model_preparation_finished 事件里同步
        let step = self.onboarding.step;
        let (number, title, description, icon) = match step {
            Step::Engine => (
                1,
                "默认识别方式",
                Some("仅用于没有字幕的视频"),
                icons::microphone(),
            ),
            Step::Ai => (
                2,
                "连接 AI 服务",
                None,
                icons::auto_fix(),
            ),
            Step::Account => (
                3,
                "连接 Bilibili",
                None,
                icons::bilibili(),
            ),
            Step::Model if self.onboarding.provider == Some(AsrProvider::Api) => (
                4,
                "准备就绪",
                None,
                icons::circle_check(),
            ),
            Step::Model => (
                4,
                "准备识别模型",
                None,
                icons::download(),
            ),
        };
        let content = match step {
            Step::Engine => self.setup_engine_content(window, cx),
            Step::Ai => self.setup_service_content(ServicePurpose::Ai, window, cx),
            Step::Account => self.account_onboarding_page(window, cx),
            Step::Model => self.setup_model_content(window, cx),
        };
        let scale = f32::from(window.rem_size()) / 14.;
        let width = (760. * scale.min(1.5))
            .min(f32::from(window.viewport_size().width) - 48.)
            .max(280.);
        let available_height =
            (f32::from(window.viewport_size().height) - (40. * scale + 16.) - 48.).max(160.);
        let compact = available_height < 540. * scale;
        let steps = ["识别方式", "AI 服务", "Bilibili", "模型准备"];
        let heading = v_flex()
            .w_full()
            .min_w_0()
            .gap_3()
            .when(!compact, |heading| {
                heading.child(h_flex().w_full().min_w_0().items_center().gap_2().children(
                    steps.into_iter().enumerate().map(|(index, label)| {
                        let current = index + 1 == number;
                        let finished = index + 1 < number;
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .size(rems(24. / 14.))
                                    .flex_shrink_0()
                                    .rounded_full()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .bg(color(if current {
                                        ACCENT
                                    } else if finished {
                                        SUCCESS_BG
                                    } else {
                                        INSET
                                    }))
                                    .text_color(color(if current { ON_PRIMARY } else { GRAY }))
                                    .text_size(TEXT_AUX)
                                    .when(finished, |v| {
                                        v.child(
                                            icons::check()
                                                .size(rems(16. / 14.))
                                                .text_color(color(SUCCESS)),
                                        )
                                    })
                                    .when(!finished, |v| v.child((index + 1).to_string())),
                            )
                            .when(!compact, |v| {
                                v.child(
                                    div()
                                        .min_w_0()
                                        .text_size(TEXT_AUX)
                                        .font_weight(if current {
                                            FontWeight::SEMIBOLD
                                        } else {
                                            FontWeight::MEDIUM
                                        })
                                        .text_color(color(if current { INK } else { GRAY }))
                                        .child(label),
                                )
                            })
                    }),
                ))
            })
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .gap_3()
                    .items_center()
                    .child(
                        icon.size(rems(if compact { 20. } else { 28. } / 14.))
                            .flex_shrink_0()
                            .when(step != Step::Account, |icon| icon.text_color(color(ACCENT))),
                    )
                    .child(
                        settings_value("setup-title", title)
                            .role(Role::Heading)
                            .flex_1()
                            .min_w_0()
                            .text_size(if compact { TEXT_TITLE } else { TEXT_DISPLAY })
                            .font_weight(FontWeight::SEMIBOLD),
                    )
                    .when(compact, |row| {
                        row.child(
                            div()
                                .flex_shrink_0()
                                .text_size(TEXT_AUX)
                                .text_color(color(GRAY))
                                .child(format!("{number} / 4")),
                        )
                    }),
            )
            .when_some(description.filter(|_| !compact), |view, description| {
                view.child(info_callout("setup-description", description))
            });
        let scrolling_content = v_flex()
            .w_full()
            .min_w_0()
            .gap(px(if compact { 16. } else { 24. }))
            .child(content)
            .when_some(self.onboarding.notice.clone(), |view, (message, error)| {
                view.child(
                    settings_value("setup-notice", message)
                        .role(Role::Status)
                        .text_color(color(if error { DANGER } else { MUTED })),
                )
            });
        // The scroll viewport owns generous inner gutters, including the input's
        // exterior focus ring. Natural height keeps the footer near the task.
        let footer = self.setup_footer(compact, cx);
        let panel = v_flex()
            .relative()
            .w(px(width))
            .max_w_full()
            .min_w_0()
            .max_h(px(available_height))
            .bg(color(SURFACE))
            .rounded(RADIUS_HERO)
            .border_1()
            .border_color(color(HAIRLINE))
            .shadow(shadow_popover())
            .child(
                heading
                    .flex_shrink_0()
                    .px(px(if compact { 20. } else { 32. }))
                    .pt(px(if compact { 20. } else { 32. }))
                    .pb(px(if compact { 16. } else { 24. })),
            )
            .child(
                v_flex()
                    .relative()
                    .flex_1()
                    .w_full()
                    .min_h_0()
                    .child(
                        v_flex()
                            .id("setup-scroll")
                            .flex_1()
                            .w_full()
                            .min_w_0()
                            .min_h_0()
                            .overflow_y_scroll()
                            .track_scroll(&self.onboarding.scroll)
                            .px(px(if compact { 20. } else { 32. }))
                            .pb(px(if compact { 20. } else { 32. }))
                            .child(motion::state_enter(
                                ("setup-step", number),
                                scrolling_content)),
                    )
                    .child(
                        crate::backend::vertical_scrollbar(&self.onboarding.scroll),
                    ),
            )
            .child(
                footer
                    .px(px(if compact { 20. } else { 32. }))
                    .py_4()
                    .border_t_1()
                    .border_color(color(HAIRLINE)),
            );
        v_flex()
            .flex_1()
            .w_full()
            .min_h_0()
            .items_center()
            .justify_start()
            .p(px(24.))
            .child(panel)
            .into_any_element()
    }

    fn setup_engine_content(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let selected = self.onboarding.provider;
        let scale = f32::from(window.rem_size()) / 14.;
        let width =
            (760. * scale.min(1.5)).min(f32::from(window.viewport_size().width) - 48.) - 64.;
        let columns = if width >= 630. * scale {
            3
        } else if width >= 430. * scale {
            2
        } else {
            1
        };
        let recommended = self.recommended_local_provider();
        let local = selected != Some(AsrProvider::Api);
        let routes = div()
            .grid()
            .grid_cols(if width >= 430. * scale { 2 } else { 1 })
            .gap_3()
            .w_full()
            .min_w_0()
            .child(
                self.setup_reveal(
                    "setup-local-route-reveal",
                    described_choice(
                        "setup-local-route",
                        "本机识别",
                        "推荐 · 音频不离开本机",
                        icons::computer(),
                        local,
                        window,
                        cx,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.select_setup_route(true, cx);
                    })),
                ),
            )
            .child(
                self.setup_reveal(
                    "setup-cloud-route-reveal",
                    described_choice(
                        "setup-cloud-route",
                        "在线语音服务",
                        "音频会发送到所选服务",
                        icons::cloud(),
                        !local,
                        window,
                        cx,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.select_setup_route(false, cx);
                    })),
                ),
            );
        let mut body = v_flex().w_full().min_w_0().gap_4().child(routes);
        if !local {
            return body.child(self.setup_service_content(ServicePurpose::Speech, window, cx));
        }
        let engines = [
            (
                Some(AsrProvider::Coreml),
                "Apple 原生",
                "",
                icons::computer(),
            ),
            (
                Some(AsrProvider::Gpu),
                "GPU",
                "",
                icons::computer(),
            ),
            (
                Some(AsrProvider::Cpu),
                "CPU",
                "",
                icons::computer(),
            ),
            (
                Some(AsrProvider::Npu),
                "Intel NPU",
                "",
                icons::computer(),
            ),
        ];
        // Hardware this platform cannot have is not offered as a disabled choice.
        let engines = engines.into_iter().filter(|(provider, ..)| {
            *provider == selected
                || match provider {
                    Some(AsrProvider::Npu) => !cfg!(target_os = "macos"),
                    Some(AsrProvider::Coreml) => cfg!(target_os = "macos"),
                    _ => true,
                }
        });
        let mut choices = div().grid().grid_cols(columns).gap_3().w_full().min_w_0();
        for (index, (provider, label, description, icon)) in engines.enumerate() {
            let capability = provider.unwrap_or(recommended);
            // Api 不在本页提供；supports(Npu) 即 npu_device && npu_runtime（backend::detect 合一）。
            let ready = self.environment.as_ref().map(|e| {
                e.engine && !matches!(capability, AsrProvider::Api) && e.supports(capability)
            });
            let status = match ready {
                Some(true) => "已就绪",
                Some(false) if capability == AsrProvider::Npu => "未检测到 Intel NPU",
                Some(false) if capability == AsrProvider::Coreml => "需要 Apple 芯片与运行环境",
                Some(false) if capability == AsrProvider::Gpu => "未检测到 GPU 运行环境",
                Some(false) => "需要安装运行环境",
                None => "正在检测",
            };
            choices = choices.child(
                self.setup_reveal(
                    ("setup-engine-choice-reveal", index),
                    described_choice(
                        ("setup-engine-choice", index),
                        label,
                        description,
                        icon,
                        selected == provider,
                        window,
                        cx,
                    )
                    // A ready engine needs no badge; only checking or unavailable is status.
                    .when(ready != Some(true), |choice| {
                        choice.child(
                            h_flex()
                                .pl(rems(28. / 14.))
                                .child(badge(BadgeKind::Neutral).child(status)),
                        )
                    })
                    .disabled(provider.is_some() && ready == Some(false))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select_setup_provider(provider, cx);
                    })),
                ),
            );
        }
        body = body.child(
            v_flex()
                .w_full()
                .min_w_0()
                .gap_3()
                .child(
                    described_choice(
                        "setup-engine-auto",
                        "自动选择引擎",
                        format!("目前使用 {}", crate::provider_label(Some(recommended))),
                        icons::auto_fix(),
                        selected.is_none(),
                        window,
                        cx,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.select_setup_provider(None, cx);
                    })),
                )
                .child(field_label("setup-fixed-engine-label", "固定引擎"))
                .child(choices),
        );
        let provider = selected.unwrap_or(recommended);
        let models = local_model_choices(provider, &self.onboarding.model);
        let current_model = models
            .iter()
            .find(|(id, _, _)| id == &self.onboarding.model)
            .map(|(_, label, _)| label.clone())
            .unwrap_or_else(|| self.onboarding.model.clone());
        let request = self.setup_model_request();
        self.ensure_model_diagnostic(request.provider, Some(&request.model), &request.root, cx);
        let snapshot =
            self.setup_model_snapshot(request.provider, Some(&request.model), &request.root);
        let model_ready = !snapshot.checking
            && snapshot.device_issue.is_none()
            && matches!(
                snapshot
                    .result
                    .as_ref()
                    .and_then(|result| result.as_ref().ok())
                    .map(|status| &status.state),
                Some(CacheState::Loaded | CacheState::Cached)
            );
        let model_status = if snapshot.checking {
            "正在检查模型".to_owned()
        } else if snapshot.device_issue.is_some() {
            "运行环境未就绪".to_owned()
        } else {
            match snapshot
                .result
                .as_ref()
                .and_then(|result| result.as_ref().ok())
                .map(|status| &status.state)
            {
                Some(CacheState::Loaded) => "已就绪".into(),
                Some(CacheState::Cached) => "已下载".into(),
                Some(CacheState::Partial) => {
                    "未下载完整".into()
                }
                Some(CacheState::Missing) => "未下载".into(),
                Some(CacheState::Unsupported) => "此模型不适用于当前引擎，请更换模型".into(),
                None => "检查未完成".into(),
            }
        };
        let mut model_grid = div()
            .grid()
            .grid_cols(columns.min(models.len() as u16))
            .gap_3()
            .w_full()
            .min_w_0();
        for (index, (id, label, description)) in models.into_iter().enumerate() {
            model_grid = model_grid.child(
                self.setup_reveal(
                    ("setup-model-choice-reveal", index),
                    described_choice(
                        ("setup-model-choice", index),
                        label,
                        description,
                        icons::microphone(),
                        self.onboarding.model == id,
                        window,
                        cx,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select_setup_model(&id, cx);
                    })),
                ),
            );
        }
        body = body.child(
            v_flex()
                .w_full()
                .min_w_0()
                .gap_3()
                .child(
                    h_flex()
                        .w_full()
                        .min_w_0()
                        .gap_3()
                        .items_center()
                        .flex_wrap()
                        .child(semantic_label(
                            "setup-model-label",
                            "识别模型",
                            icons::microphone(),
                        ))
                        .child(settings_value("setup-current-model", current_model).w_auto())
                        // Readiness reads as the model's status, next to its name.
                        .child(
                            h_flex().flex_1().min_w_0().child(
                                badge(if model_ready {
                                    BadgeKind::Success
                                } else {
                                    BadgeKind::Neutral
                                })
                                .child(model_status),
                            ),
                        )
                        .child(
                            quiet("setup-change-model")
                                .mr(-crate::theme::QUIET_INSET)
                                .icon(if self.onboarding.model_choices_open {
                                    icons::chevron_up()
                                } else {
                                    icons::tune()
                                })
                                .label(if self.onboarding.model_choices_open {
                                    "收起模型"
                                } else {
                                    "更换模型"
                                })
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.onboarding.model_choices_open =
                                        !this.onboarding.model_choices_open;
                                    cx.notify();
                                })),
                        ),
                )
                .child(motion::disclosure(
                    "setup-model-choices",
                    self.onboarding.model_choices_open,
                    model_grid,
                    window,
                    cx,
                )),
        );
        body
    }

    fn setup_reveal(&self, id: impl Into<ElementId>, child: impl IntoElement) -> AnyElement {
        crate::focus_scroll::RevealFocus::new(id, child, self.onboarding.scroll.clone())
            .into_any_element()
    }

    /// 引导的 AI 服务类型切换：无密钥协议收起认证编辑，并按协议预填/替换固定地址。
    fn setup_ai_kind_switched(
        &mut self,
        protocol: preferences::ServiceProtocol,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        {
            let current = self.onboarding.ai.value(InputField::Address, cx);
            let service = self.onboarding.service_mut(ServicePurpose::Ai);
            // 派生名称（空或与旧地址主机一致）跟随新类型重新生成；手动命名保留
            let previous_host = preferences::service_host(&current, service.draft.protocol);
            if service.draft.name.trim().is_empty()
                || Some(service.draft.name.trim()) == previous_host.as_deref()
            {
                service.draft.name.clear();
            }
            let kind_changed = service.draft.protocol != protocol;
            service.draft.protocol = protocol;
            service.evidence.clear();
            service.errors.clear();
            service.models.invalidate();
            if kind_changed {
                // 模型 ID 不跨服务类型携带；Codex 连接成功后由账号目录自动填充
                service.draft.model.clear();
                service.inputs[&InputField::Model].update(cx, |input, cx| {
                    input.set_value("", window, cx);
                });
            }
            if protocol.keyless() {
                service.draft.authentication = Authentication::None;
                service.inputs[&InputField::Key].update(cx, |input, cx| {
                    input.set_value("", window, cx);
                });
            }
        }
        let codex_addresses = [
            preferences::CODEX_ADDRESS,
            course2md::provider::CODEX_RESPONSES_URL,
        ];
        let current = self.onboarding.ai.value(InputField::Address, cx);
        let current = current.trim().trim_end_matches('/').to_string();
        let is_codex_address = codex_addresses.contains(&current.as_str());
        let next = match protocol {
            preferences::ServiceProtocol::CodexResponses => Some(preferences::CODEX_ADDRESS),
            preferences::ServiceProtocol::OllamaChat if current.is_empty() || is_codex_address => {
                Some(preferences::OLLAMA_DEFAULT_ADDRESS)
            }
            // 离开 Codex 时清掉固定地址（非用户数据），其余输入保留
            _ if is_codex_address => Some(""),
            _ => None,
        };
        if let Some(next) = next {
            let service = self.onboarding.service_mut(ServicePurpose::Ai);
            // set_value 不触发 Change 事件，草稿在此同步
            service.inputs[&InputField::Address].update(cx, |input, cx| {
                input.set_value(next, window, cx);
            });
            service.draft.address = next.to_string();
        }
        if protocol == preferences::ServiceProtocol::CodexResponses {
            self.codex_refresh_status(crate::codex_ui::CodexSurface::Onboarding, cx);
        }
        cx.notify();
    }

    /// codex_ui 的引导钩子：仅在 AI 步骤仍在编辑 Codex 服务时落目录/错误。
    pub(super) fn onboarding_codex_needs_catalog(&self) -> bool {
        if !self.onboarding.active {
            return false;
        }
        let service = self.onboarding.service(ServicePurpose::Ai);
        service.draft.protocol == preferences::ServiceProtocol::CodexResponses
            && service.models.models().is_empty()
            && !service.models.loading()
    }
    pub(super) fn onboarding_codex_models_loading(&mut self) -> bool {
        if !self.onboarding.active {
            return false;
        }
        let service = self.onboarding.service_mut(ServicePurpose::Ai);
        if service.draft.protocol != preferences::ServiceProtocol::CodexResponses {
            return false;
        }
        service.models.begin_external();
        true
    }
    pub(super) fn onboarding_codex_models_failed(&mut self, message: String) {
        let service = self.onboarding.service_mut(ServicePurpose::Ai);
        if service.draft.protocol == preferences::ServiceProtocol::CodexResponses {
            service
                .models
                .reject(crate::model_discovery::Error::Custom(message));
        }
    }
    pub(super) fn onboarding_codex_catalog(
        &mut self,
        models: Vec<String>,
        suggested: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if !self.onboarding.active {
            return;
        }
        let service = self.onboarding.service_mut(ServicePurpose::Ai);
        if service.draft.protocol != preferences::ServiceProtocol::CodexResponses {
            return;
        }
        service.models.prime(models);
        let input = service.inputs[&InputField::Model].clone();
        crate::codex_ui::autofill_input(input, suggested, cx);
    }
    pub(super) fn onboarding_codex_logged_out(&mut self) {
        let service = self.onboarding.service_mut(ServicePurpose::Ai);
        if service.draft.protocol == preferences::ServiceProtocol::CodexResponses {
            service.models.invalidate();
        }
    }

    fn fetch_setup_models(&mut self, purpose: ServicePurpose, cx: &mut Context<Self>) {
        if self.onboarding.service(purpose).draft.protocol
            == preferences::ServiceProtocol::CodexResponses
        {
            self.codex_refresh_models(crate::codex_ui::CodexSurface::Onboarding, cx);
            return;
        }
        let service = self.onboarding.service(purpose);
        let draft = service.current_draft(cx);
        let typed_key = Secret::new(service.value(InputField::Key, cx));
        let request = match crate::model_discovery::Request::from_draft(&draft, typed_key) {
            Ok(request) => request,
            Err(error) => {
                self.onboarding.service_mut(purpose).models.reject(error);
                cx.notify();
                return;
            }
        };
        let session = self.onboarding.session;
        let ticket = self.onboarding.service_mut(purpose).models.begin(&request);
        let vault = self.preferences.vault();
        // discover() 内部是同步 Keychain/HTTP；见 crate::spawn_blocking_io 的说明
        let task = crate::spawn_blocking_io(move || {
            smol::block_on(crate::model_discovery::discover(request, vault))
        });
        cx.spawn(async move |this, cx| {
            let result = task
                .recv()
                .await
                .unwrap_or(Err(crate::model_discovery::Error::Network));
            let _ = this.update(cx, |this, cx| {
                if !this.onboarding.active || this.onboarding.session != session {
                    return;
                }
                let service = this.onboarding.service(purpose);
                let current = crate::model_discovery::RequestKey::from_draft(
                    &service.current_draft(cx),
                    &service.value(InputField::Key, cx),
                )
                .ok();
                this.onboarding.service_mut(purpose).models.complete(
                    ticket,
                    current.as_ref(),
                    result,
                );
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn setup_input_row(
        &self,
        purpose: ServicePurpose,
        field: InputField,
        label: &'static str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let service = self.onboarding.service(purpose);
        let id = format!("setup-{}-{field:?}", purpose as usize);
        let input = text_input(&service.inputs[&field])
            .w_full()
            .aria_label(label)
            .disabled(service.running.is_some() || service.pending_version.is_some());
        let key = match field {
            InputField::Address => "address",
            InputField::Model => "model",
            InputField::Key => "api_key",
        };
        let mut field_column = v_flex().w_full().min_w_0().gap_2();
        if field == InputField::Key {
            field_column = field_column.child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .gap_2()
                    .items_center()
                    .child(div().flex_1().min_w_0().child(input))
                    .child(
                        quiet(format!("setup-key-visibility-{}", purpose as usize))
                            .mr(-crate::theme::QUIET_INSET)
                            .icon(if service.show_key {
                                icons::eye_off()
                            } else {
                                icons::eye()
                            })
                            .label(if service.show_key { "隐藏" } else { "显示" })
                            .tooltip(if service.show_key {
                                "隐藏密钥"
                            } else {
                                "显示密钥"
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                let service = this.onboarding.service_mut(purpose);
                                service.show_key = !service.show_key;
                                let masked = !service.show_key;
                                service.inputs[&InputField::Key]
                                    .update(cx, |input, cx| input.set_masked(masked, window, cx));
                                cx.notify();
                            })),
                    ),
            );
            if service.draft.credential.is_some() {
                field_column = field_column.child(
                    help(
                        format!("setup-kept-key-{}", purpose as usize),
                        "已保存密钥，留空则保留",
                    )
                    .text_size(TEXT_AUX),
                );
            }
        } else if field == InputField::Model {
            field_column = field_column.child(crate::model_discovery::model_field(
                if purpose == ServicePurpose::Ai {
                    "setup-ai-model"
                } else {
                    "setup-speech-model"
                },
                &service.inputs[&field],
                &service.models,
                service.running.is_some() || service.pending_version.is_some(),
                cx.listener(move |this, _, _, cx| this.fetch_setup_models(purpose, cx)),
                cx,
            ));
        } else {
            field_column = field_column.child(input);
        }
        field_column = field_column.children(
            service
                .errors
                .iter()
                .filter(|error| error.field == key)
                .enumerate()
                .map(|(index, error)| {
                    settings_value(
                        SharedString::from(format!("{id}-error-{index}")),
                        error.message.clone(),
                    )
                    .text_color(color(DANGER))
                }),
        );
        let icon = match field {
            InputField::Address => icons::link(),
            InputField::Model => icons::storage(),
            InputField::Key => icons::shield(),
        };
        let row = stacked_field(SharedString::from(id.clone()), label, icon, field_column);
        crate::focus_scroll::RevealFocus::new(
            SharedString::from(format!("{id}-reveal")),
            row,
            self.onboarding.scroll.clone(),
        )
        .into_any_element()
    }

    fn setup_service_content(
        &self,
        purpose: ServicePurpose,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let service = self.onboarding.service(purpose);
        let busy = service.running.is_some() || service.pending_version.is_some();
        let mut body = v_flex().w_full().min_w_0().gap_4();
        if purpose == ServicePurpose::Speech {
            body = body.child(
                self.setup_reveal(
                    "setup-speech-protocol-reveal",
                    stacked_field(
                        "setup-speech-protocol-label",
                        "接口类型",
                        icons::cloud(),
                        SingleChoiceGroup::new("setup-speech-protocol", "语音接口类型")
                            .full_width()
                            .options([("transcriptions", "语音转录"), ("chat", "音频聊天")])
                            .icon("transcriptions", icons::microphone())
                            .icon("chat", icons::article())
                            .selected(
                                if service.draft.protocol
                                    == preferences::ServiceProtocol::SpeechChat
                                {
                                    "chat"
                                } else {
                                    "transcriptions"
                                },
                            )
                            .disabled(busy)
                            .on_change(cx.listener(|this, value: &SharedString, _, cx| {
                                this.onboarding.speech.draft.protocol = if value.as_ref() == "chat"
                                {
                                    preferences::ServiceProtocol::SpeechChat
                                } else {
                                    preferences::ServiceProtocol::SpeechTranscriptions
                                };
                                this.onboarding.speech.evidence.clear();
                                this.onboarding.speech.models.invalidate();
                                cx.notify();
                            })),
                    ),
                ),
            );
        }
        if purpose == ServicePurpose::Ai {
            body = body.child(
                self.setup_reveal(
                    "setup-ai-kind-reveal",
                    stacked_field(
                        "setup-ai-kind-label",
                        "服务类型",
                        icons::cloud(),
                        SingleChoiceGroup::new("setup-ai-kind", "AI 服务类型")
                            .full_width()
                            .options(
                                [
                                    preferences::ServiceProtocol::AiChat,
                                    preferences::ServiceProtocol::OllamaChat,
                                    preferences::ServiceProtocol::CodexResponses,
                                ]
                                .into_iter()
                                .map(|candidate| (candidate.label(), candidate.ai_kind_label())),
                            )
                            .icon(preferences::ServiceProtocol::AiChat.label(), icons::cloud())
                            .icon(preferences::ServiceProtocol::OllamaChat.label(), icons::server())
                            .icon(preferences::ServiceProtocol::CodexResponses.label(), icons::sparkles())
                            .selected(service.draft.protocol.label())
                            .disabled(busy)
                            .on_change(cx.listener(
                                |this, value: &SharedString, window, cx| {
                                    let Some(protocol) = [
                                        preferences::ServiceProtocol::AiChat,
                                        preferences::ServiceProtocol::OllamaChat,
                                        preferences::ServiceProtocol::CodexResponses,
                                    ]
                                    .into_iter()
                                    .find(|candidate| candidate.label() == value.as_ref())
                                    else {
                                        return;
                                    };
                                    this.setup_ai_kind_switched(protocol, window, cx)
                                },
                            )),
                    ),
                ),
            );
        }
        let codex = service.draft.protocol == preferences::ServiceProtocol::CodexResponses;
        if !codex {
            body = body.child(self.setup_input_row(purpose, InputField::Address, "服务地址", cx));
        }
        if !service.draft.protocol.keyless() {
            body = body.child(
                self.setup_reveal(
                    format!("setup-auth-reveal-{}", purpose as usize),
                    stacked_field(
                        format!("setup-auth-label-{}", purpose as usize),
                        "认证方式",
                        icons::shield(),
                        SingleChoiceGroup::new(
                            format!("setup-auth-{}", purpose as usize),
                            "服务认证",
                        )
                        .full_width()
                        .options([("key", "API Key"), ("none", "无需认证")])
                        .icon("key", icons::shield())
                        .icon("none", icons::web())
                        .selected(if service.draft.authentication == Authentication::ApiKey {
                            "key"
                        } else {
                            "none"
                        })
                        .disabled(busy)
                        .on_change(cx.listener(
                            move |this, value: &SharedString, _, cx| {
                                let service = this.onboarding.service_mut(purpose);
                                service.draft.authentication = if value.as_ref() == "none" {
                                    Authentication::None
                                } else {
                                    Authentication::ApiKey
                                };
                                service.evidence.clear();
                                service.errors.clear();
                                service.models.invalidate();
                                cx.notify();
                            },
                        )),
                    ),
                ),
            );
        }
        if !service.draft.protocol.keyless() && service.draft.authentication == Authentication::ApiKey {
            body = body.child(self.setup_input_row(purpose, InputField::Key, "API Key", cx));
        }
        if codex {
            body = body.child(self.codex_account_section(
                crate::codex_ui::CodexSurface::Onboarding,
                busy,
                cx,
            ));
        }
        body = body.child(self.setup_input_row(purpose, InputField::Model, "模型", cx));
        if purpose == ServicePurpose::Ai {
            let mut uses = h_flex().w_full().min_w_0().flex_wrap().gap_4();
            for (id, label, checked, icon) in [
                (
                    "proofread",
                    "自动校对文字",
                    self.onboarding.ai_proofread,
                    icons::auto_fix(),
                ),
                (
                    "summary",
                    "生成课程摘要",
                    self.onboarding.ai_summary,
                    icons::summarize(),
                ),
            ] {
                uses = uses.child(
                    h_flex()
                        .flex_1()
                        .flex_basis(rems(240. / 14.))
                        .min_w_0()
                        .min_h(CONTROL_HEIGHT)
                        .items_center()
                        .gap_2()
                        .child(icon.size_4().flex_shrink_0().text_color(color(GRAY)))
                        .child(field_label(format!("setup-ai-{id}-label"), label).min_w_0())
                        .child(crate::focus_scroll::FocusRing::new(
                            format!("setup-ai-{id}-focus"),
                            coral_switch(
                                Switch::new(format!("setup-ai-{id}"))
                                    .checked(checked)
                                    .disabled(busy)
                                    .on_click(cx.listener(move |this, enabled, _, cx| {
                                        if id == "proofread" {
                                            this.onboarding.ai_proofread = *enabled;
                                        } else {
                                            this.onboarding.ai_summary = *enabled;
                                        }
                                        cx.notify();
                                    })),
                            ),
                        )),
                );
            }
            body = body
                .child(self.setup_reveal("setup-ai-uses-reveal", uses))
                .child(
                    help(
                        "setup-ai-use-scope",
                        "开启后转录文字会发送到所选 AI 服务",
                    )
                    .text_size(TEXT_AUX),
                );
        }
        if let Some(cancel) = &service.running {
            let stopping = cancel.load(Ordering::Acquire);
            body = body.child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .items_center()
                    .flex_wrap()
                    .gap_2()
                    .p_3()
                    .rounded(RADIUS_CARD)
                    .bg(color(INSET))
                    .child(motion::spinner(
                        format!("setup-service-check-{}", purpose as usize),
                        cx,
                    ))
                    .child(
                        settings_value(
                            format!("setup-service-check-label-{}", purpose as usize),
                            if stopping {
                                "正在停止检查"
                            } else {
                                "正在检查配置"
                            },
                        )
                        .flex_1()
                        .min_w_0()
                        .font_weight(FontWeight::SEMIBOLD),
                    )
                    .child(
                        quiet(format!("setup-cancel-check-{}", purpose as usize))
                            .icon(icons::close())
                            .label("停止检查")
                            .disabled(stopping)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.onboarding.service(purpose).cancel();
                                cx.notify();
                            })),
                    ),
            );
        }
        if !service.evidence.is_empty() {
            let mut feedback = v_flex()
                .w_full()
                .min_w_0()
                .gap_2()
                .p_3()
                .bg(color(INSET))
                .rounded(RADIUS_CARD);
            for (index, (kind, evidence)) in service.evidence.iter().enumerate() {
                let passed = evidence.outcome == TestOutcome::Passed;
                feedback =
                    feedback.child(
                        h_flex()
                            .w_full()
                            .min_w_0()
                            .items_start()
                            .gap_2()
                            .child(
                                if passed {
                                    icons::circle_check()
                                } else {
                                    icons::warning()
                                }
                                .size_4()
                                .mt_1()
                                .flex_shrink_0()
                                .text_color(color(if passed { SUCCESS } else { DANGER })),
                            )
                            .child(
                                settings_value(
                                    format!("setup-test-message-{}-{index}", purpose as usize),
                                    if passed {
                                        format!("{}测试通过", kind.label())
                                    } else {
                                        evidence.message.clone()
                                    },
                                )
                                .flex_1()
                                .min_w_0()
                                .text_color(color(if passed { INK } else { DANGER })),
                            ),
                    );
            }
            let details = service
                .evidence
                .iter()
                .flat_map(|(_, evidence)| evidence.details.iter().cloned())
                .collect::<Vec<_>>()
                .join("\n");
            feedback = feedback
                .child(
                    h_flex()
                        .w_full()
                        .min_w_0()
                        .gap_2()
                        .flex_wrap()
                        .when(self.setup_service_passed(purpose, cx), |row| {
                            row.child(
                                quiet(format!("setup-recheck-service-{}", purpose as usize))
                                    .icon(icons::refresh())
                                    .label("重新检查")
                                    .disabled(busy)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.start_setup_test(purpose, window, cx)
                                    })),
                            )
                        })
                        .child(
                            quiet(format!("setup-test-details-{}", purpose as usize))
                                .self_start()
                                .icon(icons::info())
                                .label(if service.details_open {
                                    "收起检查详情"
                                } else {
                                    "检查详情"
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let service = this.onboarding.service_mut(purpose);
                                    service.details_open = !service.details_open;
                                    cx.notify();
                                })),
                        ),
                )
                .child(motion::disclosure(
                    format!("setup-test-detail-content-{}", purpose as usize),
                    service.details_open,
                    v_flex().w_full().min_w_0().child(
                        help(
                            format!("setup-test-details-text-{}", purpose as usize),
                            details,
                        )
                        .text_size(TEXT_AUX),
                    ),
                    window,
                    cx,
                ));
            body = body.child(motion::state_enter(
                ("setup-service-feedback", service.test_serial as usize),
                feedback));
        }
        body
    }

    fn setup_model_content(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let request = self.setup_model_request();
        let provider = request.provider;
        if provider == AsrProvider::Api {
            let version = self
                .preferences
                .default_refs()
                .asr
                .and_then(|id| self.preferences.version(&id))
                .cloned();
            return v_flex()
                .w_full()
                .min_w_0()
                .gap_4()
                .child(help(
                    "setup-cloud-model-description",
                    version
                        .map(|version| {
                            format!(
                                "识别时使用 {} · {}",
                                version.config.name, version.config.model
                            )
                        })
                        .unwrap_or_else(|| "当前没有默认语音服务，请返回第一步完成配置。".into()),
                ));
        }
        let preparation_result = self
            .onboarding
            .model_preparation
            .as_ref()
            .filter(|preparation| preparation.request == request)
            .and_then(|preparation| preparation.result.clone());
        let previously_cancelled = self
            .onboarding
            .model_preparation
            .as_ref()
            .is_some_and(|preparation| preparation.request == request && preparation.cancelled);
        let model = request.model;
        let root = request.root;
        self.ensure_model_diagnostic(provider, Some(&model), &root, cx);
        let snapshot = self.setup_model_snapshot(provider, Some(&model), &root);
        let cancelled = if snapshot.notice.is_some() {
            snapshot.cancelled
        } else {
            previously_cancelled
        };
        let status = snapshot
            .result
            .as_ref()
            .and_then(|result| result.as_ref().ok());
        let mut body = v_flex().w_full().min_w_0().gap_4();
        let mut actions = h_flex()
            .w_full()
            .min_w_0()
            .gap_2()
            .items_center()
            .flex_wrap();
        let (label, hint) = if snapshot.preparing {
            ("正在准备模型", "")
        } else if cancelled {
            ("模型准备已暂停", "")
        } else if snapshot.checking {
            ("正在检查模型", "")
        } else if snapshot.device_issue.is_some() {
            ("运行环境需要准备", "可返回第一步更换引擎")
        } else {
            match status.map(|status| &status.state) {
                Some(CacheState::Loaded) => ("模型已就绪", ""),
                Some(CacheState::Cached) => ("模型文件已下载", ""),
                Some(CacheState::Missing) => ("模型尚未下载", ""),
                Some(CacheState::Partial) => ("模型尚未下载完整", ""),
                Some(CacheState::Unsupported) => {
                    ("引擎不支持当前模型", "请返回第一步选择其他模型。")
                }
                None => ("模型检查未完成", ""),
            }
        };
        let model_title = crate::settings_ui::model_diagnostics::model_display_name(&model);
        body = body.child(
            v_flex()
                .w_full()
                .min_w_0()
                .gap_3()
                .child(
                    h_flex()
                        .w_full()
                        .min_w_0()
                        .items_start()
                        .gap_2()
                        .child(
                            icons::microphone()
                                .size(rems(20. / 14.))
                                .mt_1()
                                .flex_shrink_0()
                                .text_color(color(ACCENT)),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .gap_1()
                                .child(
                                    settings_value("setup-model-identity", model_title)
                                        .text_size(TEXT_TITLE)
                                        .font_weight(FontWeight::SEMIBOLD),
                                )
                                .child(
                                    help("setup-model-engine", crate::provider_label(Some(provider)))
                                        .text_size(TEXT_AUX),
                                ),
                        ),
                )
                .child(
                    v_flex()
                        .w_full()
                        .min_w_0()
                        .gap_1()
                        .child(
                            h_flex()
                                .w_full()
                                .min_w_0()
                                .gap_2()
                                .items_center()
                                .child(
                                    div()
                                        .size(rems(20. / 14.))
                                        .flex_shrink_0()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .when(
                                            (snapshot.checking && !cancelled) || snapshot.preparing,
                                            |v| v.child(motion::spinner("setup-model-spinner", cx)),
                                        )
                                        .when(
                                            !((snapshot.checking && !cancelled)
                                                || snapshot.preparing),
                                            |v| {
                                                v.child(
                                                    icons::storage()
                                                        .size(rems(20. / 14.))
                                                        .text_color(color(GRAY)),
                                                )
                                            },
                                        ),
                                )
                                .child(
                                    settings_value("setup-model-state", label)
                                        .font_weight(FontWeight::SEMIBOLD),
                                ),
                        )
                        .when(!hint.is_empty(), |v| {
                            v.child(info_callout("setup-model-state-hint", hint))
                        }),
                ),
        );
        if snapshot.preparing {
            body = body.child(self.setup_model_progress(window, cx));
            actions = actions.child(
                outline_pill("setup-pause-model")
                    .self_start()
                    .icon(icons::pause())
                    .label(if self.cancelling {
                        "正在暂停…"
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
        } else {
            let allowed = self.environment.is_some()
                && snapshot.device_issue.is_none()
                && status.is_some_and(|status| status.can_prepare);
            let needs_download = status.is_some_and(|status| {
                matches!(status.state, CacheState::Missing | CacheState::Partial)
            });
            if allowed {
                let button = if needs_download {
                    primary_pill("setup-prepare-model")
                } else if status.is_some_and(|status| status.state == CacheState::Loaded) {
                    quiet("setup-prepare-model")
                } else {
                    outline_pill("setup-prepare-model")
                };
                let target = root.clone();
                actions = actions.child(
                    button
                        .self_start()
                        .icon(
                            if status.is_some_and(|status| status.state == CacheState::Loaded) {
                                icons::refresh()
                            } else {
                                icons::download()
                            },
                        )
                        .label(if cancelled {
                            "继续准备"
                        } else {
                            crate::settings_ui::model_diagnostics::model_action_label(
                                provider,
                                status.map(|status| &status.state),
                            )
                        })
                        .disabled(self.job.is_some() || snapshot.checking)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if this.job.is_some() {
                                return;
                            }
                            let request = ModelRequest {
                                provider,
                                model: model.clone(),
                                root: target.clone(),
                            };
                            this.onboarding.model_status_request = Some(request.clone());
                            this.onboarding.model_preparation = Some(ModelPreparation {
                                request,
                                result: None,
                                cancelled: false,
                            });
                            this.prepare_setup_model(provider, Some(&model), &target, cx);
                        })),
                );
            }
            actions = actions.child(
                quiet("setup-recheck-model")
                    .self_start()
                    .icon(icons::refresh())
                    .label("重新检查")
                    .disabled(snapshot.checking || self.job.is_some())
                    .on_click(cx.listener(|this, _, _, cx| this.refresh_environment(cx))),
            );
            if self.job.is_some() {
                body = body.child(help(
                    "setup-model-job-busy",
                    "当前任务完成后才能准备模型",
                ));
            }
        }
        let mut details = v_flex().w_full().min_w_0().gap_3();
        if let Some((message, error)) = snapshot.notice.or(preparation_result) {
            if error && !cancelled && !snapshot.preparing {
                body = body.child(
                    settings_value(
                        "setup-model-result",
                        "准备未完成，原因见模型详情",
                    )
                    .text_color(color(DANGER))
                    .role(Role::Status),
                );
            }
            details = details.child(help("setup-model-preparation-detail", message));
        }
        if let Some(issue) = snapshot.device_issue {
            details = details.child(help("setup-model-device-detail", issue));
        }
        if let Some(Err(error)) = &snapshot.result {
            details = details.child(help("setup-model-error-detail", error.clone()));
        }
        if let Some(status) = status {
            if status.bytes > 0 {
                details = details.child(settings_detail_row(
                    "setup-model-size-label",
                    "已下载",
                    settings_value("setup-model-size", crate::activity::bytes(status.bytes)),
                ));
            }
            for (index, part) in status.parts.iter().enumerate() {
                details = details
                    .child(field_label(("setup-model-part", index), part.name.clone()))
                    .child(
                        help(("setup-model-path", index), part.path.display().to_string())
                            .text_size(TEXT_AUX),
                    );
            }
            if let Some(error) = &status.last_error {
                details = details.child(help(
                    "setup-model-last-error",
                    format!("上次准备记录：{error}"),
                ));
            }
        }
        actions = actions.child(
            quiet("setup-model-details")
                .icon(icons::info())
                .label(if self.onboarding.model_details_open {
                    "收起详情"
                } else {
                    "模型详情"
                })
                .on_click(cx.listener(|this, _, _, cx| {
                    this.onboarding.model_details_open = !this.onboarding.model_details_open;
                    cx.notify();
                })),
        );
        body.child(
            v_flex()
                .w_full()
                .min_w_0()
                .gap_2()
                .child(self.setup_reveal("setup-model-actions-reveal", actions))
                .child(motion::disclosure(
                    "setup-model-detail-content",
                    self.onboarding.model_details_open,
                    details,
                    window,
                    cx,
                )),
        )
    }

    fn setup_model_progress(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        // Reserve the phase, detail and bar region before the first worker sample.
        // Incoming progress must not move the pause or continue actions.
        let mut view = v_flex().w_full().min_w_0().min_h(rems(80. / 14.)).gap_3();
        let mut has_phase = false;
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
            has_phase = true;
            let phase = activity::model_transfer_phase(stage, &progress.message);
            view = view.child(motion::transfer_status(
                ("setup-model-progress", index),
                phase,
                &progress.transfer_metrics(stage, true),
                progress.fraction(),
                window,
                cx,
            ));
        }
        if !has_phase {
            view = view.child(semantic_label(
                "setup-model-awaiting-progress",
                "等待准备进度",
                icons::download(),
            ));
        }
        view
    }

    fn setup_service_action_label(&self, purpose: ServicePurpose, cx: &App) -> &'static str {
        let service = self.onboarding.service(purpose);
        if service.running.is_some() {
            "保存并继续"
        } else if service.pending_version.is_some() {
            "重试保存"
        } else if service.unchanged(cx) && self.setup_required_tests(purpose, cx).is_empty() {
            "保留并继续"
        } else if self.setup_service_passed(purpose, cx) {
            "保存并继续"
        } else if service.evidence.is_empty() {
            "检查配置"
        } else {
            "重新检查"
        }
    }

    /// footer 的服务动作落在哪个服务上：Engine 步选了云端识别时按 Speech
    /// 服务检查，AI 步按 AI 服务；其余步骤没有服务动作。
    fn footer_service_purpose(&self) -> Option<ServicePurpose> {
        if self.onboarding.step == Step::Engine
            && self.onboarding.provider == Some(AsrProvider::Api)
        {
            Some(ServicePurpose::Speech)
        } else if self.onboarding.step == Step::Ai {
            Some(ServicePurpose::Ai)
        } else {
            None
        }
    }

    fn setup_footer(&self, _compact: bool, cx: &mut Context<Self>) -> Div {
        if self.onboarding.finish_failed {
            return v_flex()
                .w_full()
                .flex_shrink_0()
                .gap_2()
                .child(
                    h_flex()
                        .w_full()
                        .min_w_0()
                        .flex_wrap()
                        .gap_2()
                        .child(
                            quiet("setup-enter-temporarily")
                                .label("暂时进入应用")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.leave_onboarding(window, cx)
                                })),
                        )
                        .child(div().flex_1())
                        .child(
                            primary_pill("setup-retry-finish")
                                .label("重试保存")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.finish_onboarding(window, cx)
                                })),
                        ),
                )
                .child(
                    help(
                        "setup-finish-failure-hint",
                        "暂时进入不会保存未完成的选择",
                    )
                    .text_size(TEXT_AUX),
                );
        }
        let step = self.onboarding.step;
        let model_review = self.onboarding.model_return_page.is_some();
        let model_running = self.setup_model_job_running();
        let emphasize_model_preparation = if step == Step::Model
            && !model_running
            && self.job.is_none()
        {
            let request = self.setup_model_request();
            let snapshot =
                self.setup_model_snapshot(request.provider, Some(&request.model), &request.root);
            request.provider != AsrProvider::Api
                && self.environment.is_some()
                && !snapshot.checking
                && snapshot.device_issue.is_none()
                && snapshot.result.as_ref().is_some_and(|result| {
                    result.as_ref().is_ok_and(|status| {
                        status.can_prepare
                            && matches!(status.state, CacheState::Missing | CacheState::Partial)
                    })
                })
        } else {
            false
        };
        let busy = self
            .footer_service_purpose()
            .is_some_and(|purpose| self.onboarding.service(purpose).running.is_some());
        let label = match self.footer_service_purpose() {
            Some(purpose) => self.setup_service_action_label(purpose, cx),
            None => match step {
                Step::Account => {
                    if self.account_connected() {
                        "继续"
                    } else {
                        "跳过登录"
                    }
                }
                Step::Model if model_review => "返回应用",
                Step::Model if model_running => "继续使用",
                Step::Model if emphasize_model_preparation => "稍后准备",
                Step::Model => "开始使用",
                _ => "保存并继续",
            },
        };
        let mut row = h_flex()
            .w_full()
            .min_w_0()
            .gap_2()
            .items_center()
            .flex_wrap();
        if step == Step::Engine && !model_review {
            row = row.child(
                quiet("setup-later")
                    .ml(-crate::theme::QUIET_INSET)
                    .icon(icons::arrow_forward())
                    .label("直接开始使用")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.save_setup_engine(window, cx);
                        if this.onboarding.step == Step::Ai {
                            this.finish_onboarding(window, cx);
                        }
                    })),
            );
        }
        if step != Step::Engine && !model_review {
            row = row.child(
                quiet("setup-back")
                    .ml(-crate::theme::QUIET_INSET)
                    .icon(icons::arrow_left())
                    .label("上一步")
                    .disabled(step == Step::Model && model_running)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.setup_step(
                            match step {
                                Step::Ai => Step::Engine,
                                Step::Account => Step::Ai,
                                _ => Step::Account,
                            },
                            cx,
                        )
                    })),
            );
        }
        row = row.child(div().flex_1());
        if step == Step::Ai {
            row = row.child(
                outline_pill("setup-skip-ai")
                    .label("稍后配置 AI")
                    .on_click(cx.listener(|this, _, window, cx| this.skip_setup_ai(window, cx))),
            );
        }
        let next = if emphasize_model_preparation {
            outline_pill("setup-next")
        } else {
            primary_pill("setup-next")
        };
        row = row.child(
            next.label(label)
                .icon(icons::arrow_forward())
                .disabled(busy)
                .on_click(cx.listener(move |this, _, window, cx| match step {
                    Step::Engine => this.save_setup_engine(window, cx),
                    Step::Ai => this.continue_setup_service(ServicePurpose::Ai, window, cx),
                    Step::Account => this.setup_step(Step::Model, cx),
                    Step::Model if model_review => this.leave_onboarding(window, cx),
                    Step::Model => this.finish_onboarding(window, cx),
                })),
        );
        v_flex()
            .w_full()
            .flex_shrink_0()
            .gap_2()
            .when(
                self.footer_service_purpose().is_some_and(|purpose| {
                    matches!(
                        self.setup_service_action_label(purpose, cx),
                        "检查配置" | "重新检查" | "重试保存"
                    )
                }),
                |v| {
                    // 后果说明与发起它的检查动作同现同隐（review4#6）：
                    // 服务已通过检查、动作是「保留并继续」时不展示
                    v.child(
                        help("setup-test-notice", "检查仅发送内置示例，服务商可能计费")
                            .text_size(TEXT_AUX),
                    )
                },
            )
            .child(row)
            .when(model_running, |v| {
                v.child(
                    h_flex().w_full().justify_end().child(
                        help("setup-return-hint", "模型会继续在后台准备")
                            .text_size(TEXT_AUX)
                            .text_right(),
                    ),
                )
            })
    }

    pub(crate) fn onboarding_background_notice(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        if self.onboarding.active {
            return None;
        }
        let preparation = self.onboarding.model_preparation.as_ref()?;
        let running = self.setup_model_job_running();
        let result_error = preparation.result.as_ref().is_some_and(|(_, error)| *error);
        let label = if running {
            if self.cancelling {
                "正在暂停".to_owned()
            } else {
                self.progress
                    .iter()
                    .find(|(stage, progress)| {
                        crate::activity::is_model_transfer_stage(stage)
                            && progress.has_samples()
                            && !progress.done
                    })
                    .map(|(stage, progress)| {
                        let phase = activity::model_transfer_phase(stage, &progress.message);
                        progress
                            .fraction()
                            .map(|fraction| {
                                format!("{phase} · {:.0}%", fraction.clamp(0., 1.) * 100.)
                            })
                            .unwrap_or(phase)
                    })
                    .unwrap_or_else(|| "正在准备".into())
            }
        } else if preparation.cancelled {
            "准备已暂停".into()
        } else if result_error {
            "准备未完成".into()
        } else if preparation.result.is_some() {
            "准备已结束".into()
        } else {
            "准备已结束，请查看结果".into()
        };
        Some(
            v_flex()
                .w_full()
                .min_w_0()
                .px_6()
                .py_2()
                .gap_2()
                .bg(color(INSET))
                .child(
                    h_flex()
                        .w_full()
                        .min_w_0()
                        .gap_2()
                        .items_center()
                        .flex_wrap()
                        .child(
                            h_flex()
                                .flex_1()
                                .flex_basis(rems(240. / 14.))
                                .min_w_0()
                                .gap_2()
                                .items_center()
                                .when(running, |row| {
                                    row.child(div().flex_shrink_0().child(motion::spinner(
                                        "setup-background-model-spinner",
                                        cx,
                                    )))
                                })
                                .child(
                                    settings_value(
                                        "setup-background-model-status",
                                        format!("{} · {label}", preparation.request.model),
                                    )
                                    .w_auto()
                                    .flex_1()
                                    .text_color(color(
                                        if result_error && !preparation.cancelled && !running {
                                            DANGER
                                        } else {
                                            INK
                                        },
                                    )),
                                ),
                        )
                        .child(
                            quiet("setup-background-model-open")
                                .label(if running {
                                    "查看进度"
                                } else {
                                    "查看结果"
                                })
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_setup_model_status(window, cx)
                                })),
                        )
                        .when(!running, |row| {
                            row.child(
                                quiet("setup-background-model-dismiss")
                                    .label("关闭提示")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if this.setup_model_job_running() {
                                            return;
                                        }
                                        this.onboarding.model_preparation = None;
                                        cx.notify();
                                    })),
                            )
                        }),
                ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ProviderChoice, apply_provider_choice, engine_preferences, evidence_covers,
        local_model_choices, newly_enabled_ai_tests, requested_tests, saved_api_key_placeholder,
        setup_cache_model, setup_model_supported,
    };
    use crate::preferences::{
        Authentication, GenerationPreferences, ServiceConfiguration, ServiceProtocol,
        ServicePurpose, ServiceTestEvidence, TestOutcome,
    };
    use crate::service_test::TestKind;
    use course2md::config::AsrProvider;

    #[test]
    fn model_progress_distinguishes_download_from_loading() {
        assert_eq!(
            super::activity::model_transfer_phase(
                "model/apple",
                "Downloading vendor/Qwen3-ASR/model.safetensors"
            ),
            "下载 Qwen3 模型文件"
        );
        assert_eq!(
            super::activity::model_transfer_phase("model/apple", "Loading vendor/Qwen3-ASR"),
            "加载识别模型"
        );
        assert_eq!(
            super::activity::model_transfer_phase("model/apple", "Compiling model"),
            "编译识别模型"
        );
    }

    #[test]
    fn online_route_round_trip_preserves_explicit_local_engine_and_alias() {
        for (engine, initial_model) in [
            (Some(AsrProvider::Cpu), "QWEN3-ASR-1.7B"),
            (Some(AsrProvider::Gpu), "qwen3-asr-1.7b-q8_0.gguf"),
            (Some(AsrProvider::Coreml), "whisper-large-v3-turbo"),
            (Some(AsrProvider::Npu), "Fixture/Custom-Whisper"),
            (None, "qwen3-0.6b"),
        ] {
            let mut provider = engine;
            let mut local = engine;
            let mut model = initial_model.to_owned();
            // Re-clicking the online route must not replace the remembered engine with Api.
            for _ in 0..2 {
                assert!(
                    apply_provider_choice(
                        &mut provider,
                        &mut local,
                        &mut model,
                        ProviderChoice::Online,
                        AsrProvider::Coreml,
                    )
                    .is_none()
                );
                assert_eq!(provider, Some(AsrProvider::Api));
                assert_eq!(local, engine);
            }
            assert!(
                apply_provider_choice(
                    &mut provider,
                    &mut local,
                    &mut model,
                    ProviderChoice::Local,
                    AsrProvider::Coreml,
                )
                .is_none()
            );
            assert_eq!(provider, engine);
            assert_eq!(model, initial_model);
            let saved = engine_preferences(&GenerationPreferences::default(), provider, &model);
            assert_eq!(saved.options.provider, engine);
            assert_eq!(saved.options.asr_model.as_deref(), Some(initial_model));
        }
    }

    #[test]
    fn switching_to_an_incompatible_engine_selects_a_supported_model_with_notice() {
        let mut provider = Some(AsrProvider::Npu);
        let mut local = provider;
        let mut model = "whisper-tiny".to_owned();
        let notice = apply_provider_choice(
            &mut provider,
            &mut local,
            &mut model,
            ProviderChoice::Engine(Some(AsrProvider::Coreml)),
            AsrProvider::Coreml,
        )
        .unwrap();
        assert_eq!(provider, Some(AsrProvider::Coreml));
        assert_eq!(local, provider);
        assert_eq!(model, "qwen3-1.7b");
        assert!(notice.contains("whisper-tiny"));
        assert!(notice.contains("Apple 原生"));
        assert!(setup_model_supported(AsrProvider::Coreml, &model));
        assert_eq!(
            local_model_choices(AsrProvider::Coreml, &model)
                .iter()
                .filter(|(id, _, _)| id == &model)
                .count(),
            1
        );

        model = "qwen3-0.6b".into();
        assert!(
            apply_provider_choice(
                &mut provider,
                &mut local,
                &mut model,
                ProviderChoice::Engine(Some(AsrProvider::Cpu)),
                AsrProvider::Coreml,
            )
            .is_some()
        );
        assert_eq!(model, "qwen3-1.7b");
        assert!(setup_model_supported(AsrProvider::Cpu, &model));
    }

    #[test]
    fn current_alias_repository_and_invalid_value_remain_visible_in_model_choices() {
        for (provider, current, supported) in [
            (AsrProvider::Coreml, "whisper-large-v3-turbo", true),
            (AsrProvider::Npu, "Fixture/Custom-Whisper", true),
            (AsrProvider::Npu, "whisper-large", true),
            (AsrProvider::Coreml, "whisper-tiny", false),
        ] {
            let choices = local_model_choices(provider, current);
            let selected: Vec<_> = choices.iter().filter(|(id, _, _)| id == current).collect();
            assert_eq!(selected.len(), 1);
            assert_eq!(selected[0].1, current);
            assert_eq!(setup_model_supported(provider, current), supported);
            if !supported {
                assert!(selected[0].2.contains("不适用"));
            }
        }
    }

    #[test]
    fn cache_names_normalize_compatible_aliases_without_rewriting_saved_values() {
        for (provider, current, cache) in [
            (AsrProvider::Coreml, "whisper-large-v3-turbo", "whisper"),
            (AsrProvider::Coreml, "qwen3-asr-0.6b", "qwen3-0.6b"),
            (AsrProvider::Cpu, "qwen3-asr-1.7b-q8_0.gguf", "qwen3-1.7b"),
            (
                AsrProvider::Npu,
                "Fixture/Custom-Whisper",
                "Fixture/Custom-Whisper",
            ),
        ] {
            assert_eq!(setup_cache_model(provider, current), cache);
            let saved =
                engine_preferences(&GenerationPreferences::default(), Some(provider), current);
            assert_eq!(saved.options.asr_model.as_deref(), Some(current));
        }
        // Apple's permissive normalizer alone would turn this NPU model into Whisper.
        // The shared backend compatibility check must reject that silent substitution.
        assert!(!setup_model_supported(AsrProvider::Coreml, "whisper-tiny"));
        assert_eq!(
            setup_cache_model(AsrProvider::Coreml, "whisper-tiny"),
            "whisper-tiny"
        );
        for provider in [
            AsrProvider::Cpu,
            AsrProvider::Gpu,
            AsrProvider::Coreml,
            AsrProvider::Npu,
        ] {
            for (id, _, _) in local_model_choices(provider, "qwen3-1.7b") {
                assert!(setup_model_supported(provider, &id), "{provider:?}: {id}");
            }
        }
    }

    #[test]
    fn existing_ai_configuration_checks_only_new_capabilities_and_never_disabled_ones() {
        let mut existing = GenerationPreferences::default();
        existing.ai_proofread = true;
        existing.ai_summary = false;
        existing.vision = true;

        assert!(newly_enabled_ai_tests(&existing, true, false).is_empty());
        assert!(newly_enabled_ai_tests(&existing, false, false).is_empty());
        assert_eq!(
            newly_enabled_ai_tests(&existing, true, true),
            vec![TestKind::Summary]
        );
        assert_eq!(
            newly_enabled_ai_tests(&existing, false, true),
            vec![TestKind::Summary]
        );

        existing.ai_proofread = false;
        existing.ai_summary = true;
        assert_eq!(
            newly_enabled_ai_tests(&existing, true, true),
            vec![TestKind::Proofread, TestKind::Vision]
        );
        assert!(newly_enabled_ai_tests(&existing, false, false).is_empty());

        existing.vision = false;
        assert_eq!(
            newly_enabled_ai_tests(&existing, true, true),
            vec![TestKind::Proofread]
        );
    }

    #[test]
    fn engine_choice_changes_only_engine_fields_and_preserves_other_preferences() {
        let mut current = GenerationPreferences::default();
        current.ai_proofread = true;
        current.ai_summary = true;
        current.ai_concurrency = 5;
        current.prompt = Some("Keep technical terms".into());
        current.options.keep_video = Some(true);
        current.options.model_dir = Some("/existing/cache".into());
        let next = engine_preferences(&current, Some(AsrProvider::Gpu), "qwen3-1.7b");
        let mut expected = current.clone();
        expected.options.provider = Some(AsrProvider::Gpu);
        expected.last_local_provider = Some(AsrProvider::Gpu);
        expected.options.asr_model = Some("qwen3-1.7b".into());
        assert_eq!(next, expected);
        let cloud = engine_preferences(&next, Some(AsrProvider::Api), "ignored");
        assert_eq!(cloud.last_local_provider, Some(AsrProvider::Gpu));
        assert_eq!(cloud.options.asr_model, next.options.asr_model);
        assert_eq!(cloud.options.model_dir, next.options.model_dir);
    }

    #[test]
    fn checks_cover_selected_capabilities_and_never_treat_other_contracts_as_passes() {
        let config = ServiceConfiguration {
            name: "Test".into(),
            protocol: ServiceProtocol::AiChat,
            endpoint: "https://example.test/v1/chat/completions".into(),
            model: "model-a".into(),
            authentication: Authentication::None,
            credential: None,
            credential_source: None,
        };
        let passed = |kind: TestKind| {
            (
                kind,
                ServiceTestEvidence {
                    fingerprint: config.fingerprint(kind.contract()),
                    contract: kind.contract().into(),
                    tested_at: 0,
                    outcome: TestOutcome::Passed,
                    message: "样例通过".into(),
                    details: Vec::new(),
                },
            )
        };
        let required = requested_tests(ServicePurpose::Ai, true, true, false);
        assert!(!evidence_covers(
            &config,
            &required,
            &[passed(TestKind::Proofread)]
        ));
        let evidence = vec![passed(TestKind::Proofread), passed(TestKind::Summary)];
        assert!(evidence_covers(&config, &required, &evidence));
        let mut changed = config.clone();
        changed.model = "model-b".into();
        assert!(!evidence_covers(&changed, &required, &evidence));
        let mut cancelled = evidence.clone();
        cancelled[0].1.outcome = TestOutcome::OutcomeUnknown;
        assert!(!evidence_covers(&config, &required, &cancelled));
        assert_eq!(
            requested_tests(ServicePurpose::Speech, true, true, true),
            vec![TestKind::Speech]
        );
        assert_eq!(
            requested_tests(ServicePurpose::Ai, false, true, true),
            vec![TestKind::Summary]
        );
        assert_eq!(
            requested_tests(ServicePurpose::Ai, true, false, true),
            vec![TestKind::Proofread, TestKind::Vision]
        );
    }

    #[test]
    fn saved_api_key_fields_show_dots_instead_of_an_empty_box() {
        assert_eq!(saved_api_key_placeholder(true), "••••••••");
        assert_eq!(saved_api_key_placeholder(false), "");
    }
}
