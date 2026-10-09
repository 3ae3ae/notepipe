//! Four independently saved setting groups and a shared service editor.
use super::*;
#[path = "model_diagnostics.rs"]
pub(crate) mod model_diagnostics;
#[path = "generation_settings.rs"]
mod generation_settings;
#[path = "service_editor.rs"]
mod service_editor;
use crate::credentials::Secret;
use crate::preferences::{
    ApplicationPreferences, Authentication, BindingScope, GenerationPreferences, PreferenceGroup,
    ServiceDraft, ServiceProtocol, ServicePurpose, ServiceVersion,
};
use crate::service_test::{self, TestKind};
use crate::theme::*;
use anyhow::{Context as _, anyhow};
use gpui_component::{
    button::*,
    checkbox::Checkbox,
    input::{InputContentType, Textarea, TextareaState},
    menu::{DropdownMenu, PopupMenuItem},
    switch::Switch,
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

actions!(
    course2md_settings,
    [
        NextSettingsTab,
        PreviousSettingsTab,
        FirstSettingsTab,
        LastSettingsTab
    ]
);

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum EditField {
    Name,
    Address,
    Model,
    Key,
    LocalModel,
    Languages,
}

struct ServiceEditor {
    draft: ServiceDraft,
    models: crate::model_discovery::State,
    target: Option<String>,
    repair_task: Option<(String, Vec<String>)>,
    scroll: ScrollHandle,
    also_default: bool,
    return_focus: Option<FocusHandle>,
    errors: Vec<preferences::FieldError>,
    status: Option<String>,
    test_kind: TestKind,
    test_running: Option<Arc<AtomicBool>>,
    evidence: Option<preferences::ServiceTestEvidence>,
    received_tests: BTreeMap<String, preferences::ServiceTestEvidence>,
    pending_binding: Option<ServiceVersion>,
    saved_configuration: Option<ServiceVersion>,
    show_key: bool,
    test_details_open: bool,
    save_failed: bool,
}

pub struct OrdinaryPreferenceIssue {
    pub group: PreferenceGroup,
    pub message: String,
    pub can_retry: bool,
}

pub(crate) struct State {
    model_diagnostics: model_diagnostics::State,
    tab_focus: [FocusHandle; 5],
    inputs: BTreeMap<EditField, Entity<InputState>>,
    prompt: Entity<TextareaState>,
    editor: Option<ServiceEditor>,
    initialized: bool,
    show_prompt: bool,
    language_details_open: bool,
    asr_details_open: bool,
    model_details_open: bool,
    diagnostics_details_open: bool,
    feedback: BTreeMap<PreferenceGroup, (String, bool, Instant)>,
    feedback_details: BTreeMap<PreferenceGroup, String>,
    expanded_feedback: std::collections::BTreeSet<PreferenceGroup>,
    generation_block_messages: std::collections::BTreeSet<String>,
    pending_generation: Option<GenerationPreferences>,
    pending_application: Option<ApplicationPreferences>,
    _subscriptions: Vec<Subscription>,
}

impl State {
    pub fn new(window: &mut Window, cx: &mut Context<Desktop>) -> Self {
        cx.bind_keys([
            KeyBinding::new("right", NextSettingsTab, Some("SettingsTabs")),
            KeyBinding::new("down", NextSettingsTab, Some("SettingsTabs")),
            KeyBinding::new("left", PreviousSettingsTab, Some("SettingsTabs")),
            KeyBinding::new("up", PreviousSettingsTab, Some("SettingsTabs")),
            KeyBinding::new("home", FirstSettingsTab, Some("SettingsTabs")),
            KeyBinding::new("end", LastSettingsTab, Some("SettingsTabs")),
        ]);
        let inputs: BTreeMap<_, _> = [
            EditField::Name,
            EditField::Address,
            EditField::Model,
            EditField::Key,
            EditField::LocalModel,
            EditField::Languages,
        ]
        .into_iter()
        .map(|field| {
            let placeholder = match field {
                EditField::Address => "https://api.example.com/v1",
                EditField::Model => "",
                EditField::Languages => "逗号分隔，例如 zh-Hans, en",
                _ => "",
            };
            (
                field,
                cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder(placeholder)
                        .masked(field == EditField::Key)
                }),
            )
        })
        .collect();
        let mut subscriptions = Vec::new();
        for (field, input) in &inputs {
            let field = *field;
            subscriptions.push(
                cx.subscribe_in(input, window, move |this, _, event, _, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.setting_input_changed(field, cx);
                    }
                }),
            );
        }
        let prompt = cx.new(|cx| TextareaState::new(window, cx).rows(6));
        subscriptions.push(cx.subscribe(&prompt, |this, _, event, cx| {
            if matches!(event, InputEvent::Change) {
                this.save_prompt_draft(cx);
            }
        }));
        Self {
            model_diagnostics: Default::default(),
            tab_focus: std::array::from_fn(|_| cx.focus_handle()),
            inputs,
            prompt,
            editor: None,
            initialized: false,
            show_prompt: false,
            language_details_open: false,
            asr_details_open: false,
            model_details_open: false,
            diagnostics_details_open: false,
            feedback: BTreeMap::new(),
            feedback_details: BTreeMap::new(),
            expanded_feedback: Default::default(),
            generation_block_messages: Default::default(),
            pending_generation: None,
            pending_application: None,
            _subscriptions: subscriptions,
        }
    }
}

struct ServiceDialog {
    desktop: Entity<Desktop>,
    _observation: Subscription,
}
impl Render for ServiceDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.desktop.update(cx, |desktop, cx| {
            desktop.service_editor_content(false, window, cx)
        })
    }
}

pub(crate) fn text(id: impl Into<ElementId>, value: impl Into<SharedString>) -> Stateful<Div> {
    theme::accessible_text(id, value)
}
pub(super) fn field_label(
    id: impl Into<ElementId>,
    value: impl Into<SharedString>,
) -> Stateful<Div> {
    text(id, value)
        .text_size(TEXT_BODY)
        .font_weight(FontWeight::SEMIBOLD)
}

fn setting_label(id: impl Into<ElementId>, icon: Icon, value: impl Into<SharedString>) -> Div {
    semantic_label(
        id,
        value.into(),
        icon.size(rems(20. / 14.)).text_color(color(GRAY)),
    )
}

/// A complete preference owns its label, value, help and trailing actions.
/// Controls align to this item's trailing edge and reflow as a unit.
pub(super) fn settings_row(
    id: impl Into<ElementId>,
    icon: Icon,
    label: &'static str,
    hint: &'static str,
    control: impl IntoElement,
) -> Div {
    settings_form_row(id, icon, label, hint, control, false)
}

/// Editing fields use a top label: errors and related actions stay with the
/// input without forcing long values into the preference label/control grid.
pub(super) fn settings_field_row(
    id: impl Into<ElementId>,
    icon: Icon,
    label: &'static str,
    field: impl IntoElement,
) -> Div {
    settings_form_row(id, icon, label, "", field, true)
}

fn settings_form_row(
    id: impl Into<ElementId>,
    icon: Icon,
    label: &'static str,
    hint: &'static str,
    control: impl IntoElement,
    editing_field: bool,
) -> Div {
    let id = id.into();
    if editing_field {
        return v_flex()
            .w_full()
            .min_w_0()
            .gap_2()
            .child(setting_label(id.clone(), icon, label))
            .child(div().w_full().min_w_0().child(control));
    }
    let row = h_flex()
        .w_full()
        .min_w_0()
        .min_h(CONTROL_HEIGHT)
        .items_start()
        .flex_wrap()
        .gap_4()
        .child(
            v_flex()
                .flex_1()
                .flex_basis(rems(180. / 14.))
                .min_w_0()
                .max_w_full()
                .min_h(CONTROL_HEIGHT)
                .justify_center()
                .child(setting_label(id.clone(), icon, label)),
        )
        .child(
            h_flex()
                .flex_1()
                .flex_basis(rems(320. / 14.))
                .max_w_full()
                .min_w_0()
                .min_h(CONTROL_HEIGHT)
                .items_center()
                .justify_end()
                .child(control),
        );
    setting_surface().child(row).when(!hint.is_empty(), |view| {
        view.child(theme::supporting_info(
            SharedString::from(format!("{id:?}-hint")),
            hint,
        ))
    })
}

/// Status rows inside an existing card: label on the shared axis, value trailing.
/// Do not wrap these in `settings_row`; that would give each item its own surface.
pub(super) fn settings_status_row(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    value: impl IntoElement,
) -> Div {
    h_flex()
        .w_full()
        .min_w_0()
        .min_h(CONTROL_HEIGHT)
        .items_center()
        .gap_4()
        .child(field_label(id, label).flex_1().min_w_0().max_w_full())
        .child(
            h_flex()
                .flex_shrink_0()
                .items_center()
                .justify_end()
                .child(value),
        )
}

/// One setting's content box inside a settings card; the card owns the surface.
pub(super) fn setting_surface() -> Div {
    v_flex()
        .w_full()
        .min_w_0()
        .flex_shrink_0()
        .px(rems(20. / 14.))
        .py(rems(14. / 14.))
        .gap_2()
}

pub(super) fn settings_detail_group(id: impl Into<ElementId>, icon: Icon, title: &'static str) -> Div {
    v_flex().w_full().min_w_0().gap_3().child(
        h_flex()
            .min_w_0()
            .items_center()
            .gap_2()
            .child(
                icon
                .size(rems(20. / 14.))
                .flex_shrink_0()
                .text_color(color(MUTED)),
            )
            .child(field_label(id, title).role(Role::Heading).min_w_0()),
    )
}

/// Shared skeleton of the Codex/Ollama account sections. The title row owns
/// one trailing slot (busy spinner, status badge, then actions) that stays on
/// the title line when it fits and wraps below as a unit when narrow. The
/// optional detail, policy, extra note and failure/notice lines follow. The
/// connection state machines stay in each module; this is render-only.
pub(crate) fn account_card(
    id: &'static str,
    icon: Icon,
    title: &'static str,
    kind: BadgeKind,
    label: &'static str,
    detail: Option<String>,
    busy: bool,
    policy: Option<&'static str>,
    extra: Option<(&'static str, &'static str)>,
    notice: Option<(&'static str, String, bool)>,
    cx: &App,
    actions: Vec<AnyElement>,
) -> Div {
    let title_row = account_title_row(
        SharedString::from(format!("{id}-heading")),
        icon,
        title,
        h_flex()
            .gap_2()
            .items_center()
            .flex_wrap()
            .when(busy, |row| {
                row.child(crate::motion::spinner(
                    SharedString::from(format!("{id}-busy")),
                    cx,
                ))
            })
            .child(badge(kind).child(label))
            .children(actions),
    );
    let line = move |suffix: &str, message: String, error: bool| {
        theme::accessible_text(SharedString::from(format!("{id}-{suffix}")), message)
            .w_full()
            .min_w_0()
            .whitespace_normal()
            .text_size(TEXT_AUX)
            .text_color(color(if error { DANGER } else { MUTED }))
    };
    v_flex()
        .w_full()
        .min_w_0()
        .gap_2()
        .child(title_row)
        .when_some(detail, |view, detail| view.child(line("detail", detail, false)))
        .when_some(policy, |view, policy| {
            view.child(theme::supporting_info(
                SharedString::from(format!("{id}-policy")),
                policy,
            ))
        })
        .when_some(extra, |view, (suffix, text)| {
            view.child(theme::supporting_info(
                SharedString::from(format!("{id}-{suffix}")),
                text,
            ))
        })
        .when_some(notice, |view, (suffix, message, is_error)| {
            view.child(line(suffix, message, is_error))
        })
}

/// A resource title (account, connection, location) with one trailing slot
/// for its status and actions. The slot stays on the title line when it fits
/// and wraps below the title as a unit only when the row is too narrow.
pub(crate) fn account_title_row(
    id: impl Into<ElementId>,
    icon: Icon,
    title: impl Into<SharedString>,
    trailing: impl IntoElement,
) -> Div {
    h_flex()
        .w_full()
        .min_w_0()
        .min_h(CONTROL_HEIGHT)
        .items_center()
        .flex_wrap()
        .gap_x_4()
        .gap_y_2()
        .child(
            div()
                .flex_1()
                .min_w(rems(10.))
                .child(setting_label(id, icon, title)),
        )
        .child(
            div()
                .max_w_full()
                .min_w_0()
                .flex()
                .items_center()
                .child(trailing),
        )
}

/// Diagnostic values begin on a common reading axis. They can wrap and grow
/// vertically without moving short values to the far edge of the window.
pub(super) fn settings_detail_row(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    value: impl IntoElement,
) -> Div {
    h_flex()
        .w_full()
        .min_w_0()
        .min_h(rems(28. / 14.))
        .items_start()
        .flex_wrap()
        .gap_x_4()
        .gap_y_1()
        .child(
            field_label(id, label)
                .w(rems(160. / 14.))
                .max_w_full()
                .flex_shrink_0()
                .whitespace_normal(),
        )
        .child(
            v_flex()
                .flex_1()
                .flex_basis(rems(240. / 14.))
                .min_w_0()
                .items_start()
                .child(value),
        )
}

pub(super) fn settings_value(
    id: impl Into<ElementId>,
    value: impl Into<SharedString>,
) -> Stateful<Div> {
    text(id, value)
        .w_full()
        .min_w_0()
        .whitespace_normal()
        .text_size(TEXT_BODY)
        .text_color(color(INK))
}
fn service_protocol_label(protocol: ServiceProtocol) -> &'static str {
    match protocol {
        ServiceProtocol::SpeechTranscriptions => "语音转录",
        ServiceProtocol::SpeechChat => "音频对话",
        // AI 服务类型的唯一文案来源：ServiceProtocol::ai_kind_label
        ai => ai.ai_kind_label(),
    }
}

fn settings_tab_icon(index: usize) -> Icon {
    match index {
        4 => icons::palette(),
        // 生成笔记 = 笔记/文档对象（tune 留给偏好/高级覆盖项，见 crosspage#8 图标指称）
        0 => icons::subtitles(),
        1 => icons::cloud(),
        2 => icons::storage(),
        _ => icons::info(),
    }
}
const SETTINGS_TABS: [(usize, &str); 5] = [
    (4, "外观"),
    (0, "生成笔记"),
    (1, "服务与账号"),
    (2, "存储"),
    (3, "应用"),
];

fn settings_tab_label(index: usize) -> &'static str {
    ["生成笔记", "服务与账号", "存储", "应用", "外观"][index.min(4)]
}

fn group(id: &'static str, title: &'static str) -> SettingsGroup {
    SettingsGroup::new(id, title)
}

/// One settings section as in the M3 canvas: a primary-colored title above a
/// single card whose items are separated by inset dividers.
#[derive(IntoElement)]
pub(crate) struct SettingsGroup {
    id: &'static str,
    title: &'static str,
    action: Option<AnyElement>,
    items: Vec<Vec<AnyElement>>,
}

impl SettingsGroup {
    pub(crate) fn new(id: &'static str, title: &'static str) -> Self {
        Self { id, title, action: None, items: Vec::new() }
    }

    /// Content that belongs to the previous item (a disclosure, a summary
    /// line): it shares that item's boundary instead of adding a divider.
    pub(crate) fn detail(mut self, element: impl IntoElement) -> Self {
        match self.items.last_mut() {
            Some(item) => item.push(element.into_any_element()),
            None => self.items.push(vec![element.into_any_element()]),
        }
        self
    }

    /// A quiet action that applies to the whole group, trailing its title.
    pub(crate) fn action(mut self, action: impl IntoElement) -> Self {
        self.action = Some(action.into_any_element());
        self
    }
}

impl ParentElement for SettingsGroup {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.items.extend(elements.into_iter().map(|element| vec![element]));
    }
}

impl RenderOnce for SettingsGroup {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let mut card = settings_card();
        for (index, item) in self.items.into_iter().enumerate() {
            if index > 0 {
                card = card.child(
                    div()
                        .h(px(1.))
                        .mx(rems(20. / 14.))
                        .flex_shrink_0()
                        .bg(color(HAIRLINE)),
                );
            }
            card = card.child(v_flex().w_full().min_w_0().children(item));
        }
        v_flex()
            .w_full()
            .min_w_0()
            .flex_shrink_0()
            .gap(rems(10. / 14.))
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .min_h(rems(24. / 14.))
                    .items_center()
                    .child(
                        theme::section_title(self.id, self.title).flex_1(),
                    )
                    .children(self.action),
            )
            .child(card)
    }
}

/// The settings card shared by groups and standalone settings: tonal surface,
/// faint outline, M3 level-1 elevation.
pub(crate) fn settings_card() -> Div {
    v_flex()
        .w_full()
        .min_w_0()
        .flex_shrink_0()
        .bg(color(SURFACE))
        .rounded(RADIUS_CARD)
        .border_1()
        .border_color(color(CARD_LINE))
        .shadow(shadow_card())
        .overflow_hidden()
}
pub(super) fn preference(
    icon: Option<Icon>,
    label: &'static str,
    hint: &'static str,
    control: Switch,
) -> Div {
    setting_surface().child(
        h_flex()
            .w_full()
            .min_w_0()
            .items_start()
            .gap_4()
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(
                        v_flex()
                            .min_h(CONTROL_HEIGHT)
                            .justify_center()
                            .child(match icon {
                                Some(icon) => setting_label(
                                    SharedString::from(format!("preference-label-{label}")),
                                    icon,
                                    label,
                                ),
                                None => v_flex().child(field_label(
                                    SharedString::from(format!("preference-label-{label}")),
                                    label,
                                )),
                            }),
                    )
                    .when(!hint.is_empty(), |view| {
                        view.child(theme::supporting_info(
                            SharedString::from(format!("preference-hint-{label}")),
                            hint,
                        ))
                    }),
            )
            .child(
                h_flex()
                    .min_h(CONTROL_HEIGHT)
                    .flex_shrink_0()
                    .items_center()
                    .child(crate::focus_scroll::FocusRing::new(
                        SharedString::from(format!("preference-focus-{label}")),
                        coral_switch(control).accessibility_label(label).py_1(),
                    )),
            ),
    )
}

impl Desktop {
    fn generation_edit_base(&self) -> GenerationPreferences {
        self.settings_ui
            .pending_generation
            .as_ref()
            .or_else(|| self.preferences.generation_intent())
            .unwrap_or_else(|| self.preferences.generation())
            .clone()
    }
    pub(crate) fn application_edit_base(&self) -> ApplicationPreferences {
        self.settings_ui
            .pending_application
            .as_ref()
            .or_else(|| self.preferences.application_intent())
            .unwrap_or_else(|| self.preferences.application())
            .clone()
    }
    fn setting_value(&self, field: EditField, cx: &App) -> String {
        self.settings_ui.inputs[&field].read(cx).value().to_string()
    }
    /// Scroll handle owning the active settings editor (or the settings page
    /// itself) when reveal-on-focus applies.
    fn active_editor_scroll(&self) -> (bool, Option<ScrollHandle>) {
        let editor = self.settings_ui.editor.as_ref().filter(|editor| {
            editor.target.is_some()
                || editor.repair_task.is_some()
                || (self.page == Page::Settings && self.settings_tab == 1)
        });
        (
            self.page == Page::Settings || editor.is_some(),
            editor.map(|editor| editor.scroll.clone()),
        )
    }
    fn reveal_setting(&self, id: impl Into<ElementId>, child: impl IntoElement) -> Div {
        let child = child.into_any_element();
        let (reveals, editor_scroll) = self.active_editor_scroll();
        if reveals {
            div()
                .w_full()
                .min_w_0()
                .flex_shrink_0()
                .child(crate::focus_scroll::RevealFocus::new(
                    id,
                    child,
                    editor_scroll
                        .unwrap_or_else(|| self.scrolls[Page::Settings as usize].clone()),
                ))
        } else {
            div().w_full().min_w_0().flex_shrink_0().child(child)
        }
    }
    fn setting_choices(
        &self,
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
    ) -> SingleChoiceGroup {
        let (reveals, editor_scroll) = self.active_editor_scroll();
        SingleChoiceGroup::new(id, label).when(reveals, |group| {
            group.reveal_in(
                editor_scroll.unwrap_or_else(|| self.scrolls[Page::Settings as usize].clone()),
            )
        })
    }
    fn setting_preference(
        &self,
        icon: Icon,
        label: &'static str,
        hint: &'static str,
        control: Switch,
    ) -> Div {
        self.reveal_setting(
            SharedString::from(format!("preference-reveal-{label}")),
            preference(Some(icon), label, hint, control),
        )
    }
    fn setting_field(&self, field: EditField, label: &'static str, _cx: &App) -> Div {
        self.setting_field_with_action(field, label, None)
    }
    /// A field whose input carries a trailing quiet action on the same line
    /// (the key's 显示/隐藏), as the onboarding key field does.
    fn setting_field_with_action(
        &self,
        field: EditField,
        label: &'static str,
        action: Option<AnyElement>,
    ) -> Div {
        let error = self
            .settings_ui
            .editor
            .as_ref()
            .and_then(|editor| {
                editor.errors.iter().find(|error| {
                    error.field
                        == match field {
                            EditField::Address => "address",
                            EditField::Model => "model",
                            EditField::Key => "api_key",
                            _ => "",
                        }
                })
            })
            .map(|error| error.message.clone());
        self.reveal_setting(
            ("setting-field-reveal", field as usize),
            settings_field_row(
                ("setting-field-label", field as usize),
                match field {
                    EditField::Name => icons::edit(),
                    EditField::Address => icons::cloud(),
                    EditField::Key => icons::shield(),
                    EditField::Model | EditField::LocalModel => icons::storage(),
                    EditField::Languages => icons::subtitles(),
                },
                label,
                v_flex()
                    .w_full()
                    .gap_2()
                    .child({
                        let input = text_input(&self.settings_ui.inputs[&field])
                            .w_full()
                            .when(field == EditField::Key, |input| {
                                input.content_type(InputContentType::Password)
                            })
                            .aria_label(
                                error
                                    .as_ref()
                                    .map(|error| format!("{label}，{error}"))
                                    .unwrap_or_else(|| label.to_owned()),
                            )
                            .readonly(
                                self.settings_ui
                                    .editor
                                    .as_ref()
                                    .is_some_and(|editor| editor.pending_binding.is_some()),
                            )
                            .when(error.is_some(), |input| input.border_color(color(DANGER)));
                        match action {
                            Some(action) => h_flex()
                                .w_full()
                                .min_w_0()
                                .gap_2()
                                .items_center()
                                .child(div().flex_1().min_w_0().child(input))
                                .child(action)
                                .into_any_element(),
                            None => input.into_any_element(),
                        }
                    })
                    .when_some(error, |view, error| {
                        view.child(
                            text(("setting-error", field as usize), error)
                                .text_sm()
                                .text_color(color(DANGER)),
                        )
                    }),
            ),
        )
    }
    fn hydrate_settings_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings_ui.initialized {
            return;
        }
        self.settings_ui.initialized = true;
        let value = self
            .preferences
            .generation_intent()
            .unwrap_or_else(|| self.preferences.generation())
            .clone();
        if self.preferences.generation_intent().is_some() {
            self.settings_ui.pending_generation = Some(value.clone());
            self.set_settings_feedback(
                PreferenceGroup::Generation,
                "已恢复未保存的修改".into(),
                true,
            );
        }
        if let Some(intent) = self.preferences.application_intent().cloned() {
            self.settings_ui.pending_application = Some(intent);
            self.set_settings_feedback(
                PreferenceGroup::Application,
                "已恢复未保存的修改".into(),
                true,
            );
        }
        let languages = value
            .subtitle_languages_draft
            .unwrap_or_else(|| value.preferred_subtitle_languages.join(", "));
        let model = value
            .local_model_draft
            .unwrap_or_else(|| value.options.asr_model.unwrap_or_default());
        self.settings_ui.inputs[&EditField::Languages]
            .update(cx, |input, cx| input.set_value(languages, window, cx));
        self.settings_ui.inputs[&EditField::LocalModel]
            .update(cx, |input, cx| input.set_value(model, window, cx));
        self.settings_ui.prompt.update(cx, |input, cx| {
            input.set_value(
                value.prompt_draft.or(value.prompt).unwrap_or_default(),
                window,
                cx,
            )
        });
    }
    pub(crate) fn settings_have_problem(&self) -> bool {
        [
            PreferenceGroup::Generation,
            PreferenceGroup::Application,
            PreferenceGroup::Services,
        ]
        .into_iter()
        .any(|group| self.preferences.is_blocked(group))
            || self.preferences.generation_intent().is_some()
            || self.preferences.application_intent().is_some()
            || self.settings_ui.pending_generation.is_some()
            || self.settings_ui.pending_application.is_some()
            || self
                .settings_ui
                .feedback
                .values()
                .any(|(_, error, _)| *error)
            || self
                .settings_ui
                .editor
                .as_ref()
                .is_some_and(|editor| editor.save_failed)
    }

    pub(crate) fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.page != Page::Settings {
            self.settings_origin = Some(self.page);
            self.settings_return_focus = window.focused(cx);
        }
        self.navigate(Page::Settings, cx);
        // 事件入口的准备动作（不是渲染副作用）：钳制遗留 tab、同步 tab stops、
        // 首次水合输入框、按 key 去重的模型诊断检查
        self.prepare_settings_view(window, cx);
        self.settings_ui.tab_focus[self.settings_tab.min(4)].focus(window, cx);
    }

    pub(crate) fn prepare_settings_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.normalize_settings_tab();
        self.sync_settings_tab_stops();
        self.hydrate_settings_inputs(window, cx);
        self.ensure_settings_model_diagnostic(cx);
        self.ensure_account_cards(cx);
    }

    fn normalize_settings_tab(&mut self) {
        // 5 是旧分页的遗留值，无写入路径；读到时迁移到「服务与账号」
        if self.settings_tab == 5 {
            self.settings_tab = 1;
        }
        if self.settings_tab > 4 {
            self.settings_tab = 4;
        }
    }
    fn sync_settings_tab_stops(&mut self) {
        for (index, focus) in self.settings_ui.tab_focus.iter_mut().enumerate() {
            *focus = focus.clone().tab_stop(index == self.settings_tab);
        }
    }
    fn select_settings_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.settings_tab = index;
        self.normalize_settings_tab();
        self.sync_settings_tab_stops();
        self.scrolls[Page::Settings as usize].set_offset(point(px(0.), px(0.)));
        self.settings_ui.tab_focus[index].focus(window, cx);
        self.ensure_account_cards(cx);
        cx.notify();
    }

    /// 服务与账号页：补齐两个账号卡片的状态（忙碌/已查守卫保证不重复发起）。
    fn ensure_account_cards(&mut self, cx: &mut Context<Self>) {
        if self.settings_tab != 1 {
            return;
        }
        if self.codex.status.is_none() && !self.codex.busy() {
            self.codex_refresh_status(crate::codex_ui::CodexSurface::Settings, cx);
        }
        if !self.ollama.checked() {
            self.ollama_refresh(cx);
        }
    }

    fn settings_navigation(
        &self,
        sidebar: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let scale = f32::from(window.rem_size()) / 14.;
        let short = !sidebar && crate::views::settings_content_width(window) < 640. * scale;
        let group = SingleChoiceGroup::new("settings-tabs", "设置分类")
            .tabs()
            .full_width()
            .options(SETTINGS_TABS.into_iter().map(|(index, label)| {
                (
                    index.to_string(),
                    if short {
                        match index {
                            0 => "生成",
                            1 => "服务",
                            _ => label,
                        }
                    } else {
                        label
                    }
                    .to_owned(),
                )
            }))
            .selected(self.settings_tab.to_string())
            .focus_handles(
                SETTINGS_TABS
                    .into_iter()
                    .map(|(index, _)| self.settings_ui.tab_focus[index].clone()),
            )
            .icon("4", settings_tab_icon(4))
            .icon("0", settings_tab_icon(0))
            .icon("1", settings_tab_icon(1))
            .icon("2", settings_tab_icon(2))
            .icon("3", settings_tab_icon(3))
            .on_change(cx.listener(|this, value: &SharedString, window, cx| {
                if let Ok(index) = value.parse() {
                    this.select_settings_tab(index, window, cx);
                }
            }));
        if sidebar {
            div()
                .w(px(crate::views::settings_sidebar_width(window)))
                .flex_shrink_0()
                .child(group.vertical())
                .into_any_element()
        } else {
            div()
                .w_full()
                .flex_shrink_0()
                .child(group)
                .into_any_element()
        }
    }

    pub fn settings_page(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        // 渲染不得有副作用：hydrate/诊断检查/tab 钳制都在 open_settings、
        // select_settings_tab、restore_settings_group 等事件入口完成
        let sidebar = crate::views::settings_uses_sidebar(window);
        let content_width = crate::views::settings_content_width(window);
        let layout_width = content_width
            + if sidebar {
                crate::views::settings_sidebar_width(window) + crate::views::SETTINGS_COLUMN_GAP
            } else {
                0.
            };
        let header = theme::page_heading(
            "settings-page-title",
            "设置",
        )
        .flex_shrink_0();
        let header = h_flex()
            .w_full()
            .min_w_0()
            .items_center()
            .justify_between()
            .gap_4()
            .child(header)
            .when(
                self.settings_origin == Some(Page::Result) && self.preview.is_some(),
                |row| {
                    row.child(
                        quiet("return-to-reading")
                            .icon(icons::arrow_left())
                            .label("返回笔记")
                            .on_click(cx.listener(|this, _, window, cx| {
                                let focus = this.settings_return_focus.take();
                                this.settings_origin = None;
                                this.navigate(Page::Result, cx);
                                if let Some(focus) = focus {
                                    window.on_next_frame(move |window, cx| focus.focus(window, cx));
                                }
                            })),
                    )
                },
            );
        let navigation = self.settings_navigation(sidebar, window, cx);
        let mut panel = v_flex()
            .relative()
            .id("settings-panel")
            .role(Role::TabPanel)
            .aria_label(settings_tab_label(self.settings_tab))
            .flex_1()
            .min_w_0()
            .min_h_0()
            .w(px(content_width))
            .gap(px(24.));
        for group in [
            PreferenceGroup::Generation,
            PreferenceGroup::Services,
            PreferenceGroup::Application,
        ] {
            let current_group = match self.settings_tab {
                0 => Some(PreferenceGroup::Generation),
                1 => Some(PreferenceGroup::Services),
                3 | 4 => Some(PreferenceGroup::Application),
                _ => None,
            };
            if current_group == Some(group) {
                if self.preferences.is_blocked(group)
                    || self
                        .settings_group_notice(group)
                        .is_some_and(|(_, error)| error)
                {
                    panel = panel.child(
                        div()
                            .id(("fixed-settings-feedback", group as usize))
                            .w_full()
                            .min_w_0()
                            .flex_shrink_0()
                            .p(px(12.))
                            .rounded(RADIUS_SMALL)
                            .bg(color(DANGER_BG))
                            .child(self.group_feedback(group, cx)),
                    );
                }
                continue;
            }
            let target = match group {
                PreferenceGroup::Generation => 0,
                PreferenceGroup::Services => 1,
                PreferenceGroup::Application => 4,
            };
            if target != self.settings_tab
                && let Some((message, true)) = self.settings_group_notice(group)
            {
                panel = panel.child(
                    h_flex()
                        .w_full()
                        .min_w_0()
                        .flex_shrink_0()
                        .gap_2()
                        .flex_wrap()
                        .p_3()
                        .rounded(RADIUS_SMALL)
                        .bg(color(DANGER_BG))
                        .child(
                            text(
                                ("settings-failed-group", target),
                                if message.starts_with(group.label()) {
                                    message
                                } else {
                                    format!("{}：{message}", group.label())
                                },
                            )
                            .role(Role::Alert)
                            .flex_1()
                            .min_w_0()
                            .text_color(color(DANGER))
                            .text_sm(),
                        )
                        .child(
                            control(("open-failed-settings-group", target))
                                .icon(icons::settings())
                                .label("查看设置")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.select_settings_tab(target, window, cx);
                                })),
                        ),
                );
            }
        }
        let content = match self.settings_tab {
            0 => self.generation_settings_page(window, cx),
            1 => self.services_settings_page(window, cx),
            2 => self.storage_settings_page(cx),
            3 => self.application_settings_page(window, cx),
            _ => self.appearance_page(window, cx),
        };
        let editing_service = self.settings_tab == 1
            && self
                .settings_ui
                .editor
                .as_ref()
                .is_some_and(|editor| editor.target.is_none() && editor.repair_task.is_none());
        let panel = panel
            .child(
                div()
                    .id("settings-content-scroll")
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .w_full()
                    .when(!editing_service, |view| view.overflow_y_scroll())
                    .track_scroll(&self.scrolls[Page::Settings as usize])
                    .p(px(4.))
                    .pr(px(12.))
                    .child(
                        div()
                            .w_full()
                            .min_w_0()
                            .when(editing_service, |view| view.h_full().min_h_0())
                            .when(!editing_service, |view| view.pb(px(24.)))
                            .child(content),
                    ),
            )
            .when(!editing_service, |panel| {
                panel.child(
                    crate::backend::vertical_scrollbar(&self.scrolls[Page::Settings as usize]),
                )
            });
        let body = div()
            .flex()
            .w_full()
            .min_w_0()
            .min_h_0()
            .flex_1()
            .when(sidebar, |view| {
                view.flex_row()
                    .items_stretch()
                    .gap(px(crate::views::SETTINGS_COLUMN_GAP))
            })
            .when(!sidebar, |view| view.flex_col().gap(px(24.)))
            .child(navigation)
            .child(panel);
        v_flex()
            .w_full()
            .h_full()
            .min_w_0()
            .min_h_0()
            .max_w(px(layout_width))
            .mx_auto()
            .pt(px(24.))
            .gap(px(24.))
            .child(header)
            .child(body)
            .into_any_element()
    }

    fn services_settings_page(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let mut view = v_flex().w_full().min_w_0().flex_shrink_0().gap_6();
        // Keep the active form near the start of the page, including in a long service list.
        let inline_editor = self
            .settings_ui
            .editor
            .as_ref()
            .filter(|editor| editor.target.is_none() && editor.repair_task.is_none());
        if inline_editor.is_some() {
            return view
                .h_full()
                .min_h_0()
                .child(self.service_editor_card(window, cx))
                .into_any_element();
        }
        view = view.child(
            group("account-settings-heading", "来源账号").child(
                setting_surface().gap_3().child(self.account_settings_page(cx)),
            ),
        );
        // Codex 订阅登录与 Ollama 本地发现：AI 服务的连接入口与状态（与服务列表平级可见）。
        // Ollama 不是账号，组名按「连接」命名而不是「AI 账号」。
        view = view.child(
            group("ai-accounts-heading", "AI 服务连接")
                .child(setting_surface().child(self.codex_account_section(
                    crate::codex_ui::CodexSurface::Settings,
                    false,
                    cx,
                )))
                .child(setting_surface().child(self.ollama_account_section(cx))),
        );
        let latest = self.preferences.latest_versions();
        for (purpose, heading_id, heading, add_label) in [
            (
                ServicePurpose::Speech,
                "speech-services-heading",
                "语音服务",
                "添加语音服务",
            ),
            (
                ServicePurpose::Ai,
                "ai-services-heading",
                "AI 服务",
                "添加 AI 服务",
            ),
        ] {
            let mut section = group(heading_id, heading);
            let mut empty = true;
            for version in latest.values().filter(|version| {
                version.config.protocol.purpose() == purpose
                    && !self
                        .preferences
                        .service_retired_in_snapshot(&version.service_id)
            }) {
                empty = false;
                section = section.child(self.service_card(version, cx));
            }
            // The add action is the list's last item. An empty list says so in
            // the same row for both kinds; otherwise the speech consequence sits
            // beside the action because adding is where the choice is made.
            let lead = if empty {
                Some(
                    text(("services-empty", purpose as usize), "尚未添加")
                        .text_color(color(MUTED))
                        .into_any_element(),
                )
            } else if purpose == ServicePurpose::Speech {
                Some(
                    theme::supporting_info("speech-services-purpose", "音频会发送到所选服务")
                        .into_any_element(),
                )
            } else {
                None
            };
            section = section.child(
                setting_surface().child(
                    h_flex()
                        .w_full()
                        .min_w_0()
                        .min_h(CONTROL_HEIGHT)
                        .gap_4()
                        .items_center()
                        .flex_wrap()
                        .child(div().flex_1().min_w(rems(12.)).children(lead))
                        .child(
                            outline_pill(("add-service", purpose as usize))
                                .icon(icons::add())
                                .label(add_label)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.open_settings_service_editor(purpose, None, window, cx);
                                })),
                        ),
                ),
            );
            view = view.child(section);
        }
        view.into_any_element()
    }

    /// A saved service with capability-specific test results and quiet actions.
    fn service_card(&self, version: &ServiceVersion, cx: &mut Context<Self>) -> Div {
        let id = version.id.clone();
        let service_id = version.service_id.clone();
        let purpose = version.config.protocol.purpose();
        let defaults = self.preferences.default_refs();
        let default_version = match purpose {
            ServicePurpose::Speech => defaults.asr.as_deref(),
            ServicePurpose::Ai => defaults.llm.as_deref(),
        }
        .and_then(|id| self.preferences.version(id))
        .filter(|default| default.service_id == version.service_id);
        let outdated_default = default_version.filter(|default| default.id != version.id);
        let default_description = default_version
            .map(|default| {
                if default.id == version.id {
                    " · 默认服务".to_owned()
                } else {
                    String::new()
                }
            })
            .unwrap_or_default();
        let kinds: &[TestKind] = if purpose == ServicePurpose::Speech {
            &[TestKind::Speech]
        } else {
            &[TestKind::Proofread, TestKind::Summary, TestKind::Vision]
        };
        let tests: Vec<_> = kinds
            .iter()
            .filter_map(|kind| {
                self.preferences
                    .test_evidence(&version.config, kind.contract())
                    .map(|evidence| (*kind, evidence))
            })
            .collect();
        let latest_test = tests
            .iter()
            .max_by_key(|(_, evidence)| evidence.tested_at)
            .map(|(kind, evidence)| {
                format!(
                    "最近测试：{} · {}",
                    kind.label(),
                    crate::reader_navigation::timestamp_local(
                        evidence.tested_at.saturating_mul(1000)
                    )
                )
            });
        let mut status_badges = Vec::new();
        if tests.is_empty() {
            status_badges.push(badge(BadgeKind::Neutral).child(text(
                SharedString::from(format!("saved-service-badge-{id}")),
                "尚未测试",
            )));
        }
        for (kind, evidence) in &tests {
            let (status_kind, outcome) = match evidence.outcome {
                preferences::TestOutcome::Passed => (BadgeKind::Success, "通过"),
                preferences::TestOutcome::OutcomeUnknown => (BadgeKind::Warning, "结果未确认"),
                preferences::TestOutcome::NotSent => (BadgeKind::Neutral, "未完成"),
                _ => (BadgeKind::Warning, "未通过"),
            };
            status_badges.push(badge(status_kind).child(text(
                SharedString::from(format!("saved-service-test-{id}-{}", kind.contract())),
                format!("{}{outcome}", kind.label()),
            )));
        }
        let failures: Vec<_> = tests
            .iter()
            .filter(|(_, evidence)| evidence.outcome != preferences::TestOutcome::Passed)
            .map(|(kind, evidence)| format!("{}：{}", kind.label(), evidence.message))
            .collect();
        let host = version.config.host();
        let description = if host.is_empty() || version.config.name.trim() == host {
            format!("{}{}", version.config.model, default_description)
        } else {
            format!("{} · {}{}", version.config.model, host, default_description)
        };
        let edit_id = id.clone();
        let test_id = id.clone();
        let default_id = id.clone();
        setting_surface()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .flex_wrap()
                    .child(
                        (if purpose == ServicePurpose::Speech {
                            icons::microphone()
                        } else {
                            icons::science()
                        })
                        .size(px(20.))
                        .text_color(color(ACCENT_STRONG))
                        .flex_shrink_0(),
                    )
                    .child(
                        text(
                            SharedString::from(format!("saved-service-title-{id}")),
                            version.config.name.clone(),
                        )
                        .font_weight(FontWeight::SEMIBOLD)
                        .flex_1()
                        .min_w_0()
                        .whitespace_normal(),
                    )
                    .child(
                        quiet(SharedString::from(format!("test-saved-service-{id}")))
                            .icon(icons::science())
                            .label("测试…")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_settings_service_editor(
                                    purpose,
                                    Some(test_id.clone()),
                                    window,
                                    cx,
                                );
                            })),
                    )
                    .child(
                        quiet(SharedString::from(format!("edit-saved-service-{id}")))
                            .icon(icons::edit())
                            .label("编辑")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_settings_service_editor(
                                    purpose,
                                    Some(edit_id.clone()),
                                    window,
                                    cx,
                                );
                            })),
                    )
                    .child(
                        quiet(SharedString::from(format!(
                            "delete-saved-service-{service_id}"
                        )))
                        .icon(icons::delete())
                        .label("删除…")
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.confirm_delete_service(service_id.clone(), window, cx)
                            },
                        )),
                    ),
            )
            .child(
                text(
                    SharedString::from(format!("saved-service-description-{id}")),
                    description,
                )
                .text_size(TEXT_AUX)
                .text_color(color(GRAY)),
            )
            .when_some(outdated_default, |card, default| {
                card.child(
                    h_flex()
                        .w_full()
                        .min_w_0()
                        .gap_2()
                        .items_center()
                        .flex_wrap()
                        .child(
                            text(
                                SharedString::from(format!("saved-service-old-default-{id}")),
                                format!(
                                    "以后生成的笔记仍使用旧配置 v{} · {}",
                                    default.number, default.config.model
                                ),
                            )
                            .flex_1()
                            .min_w_0()
                            .whitespace_normal()
                            .text_size(TEXT_AUX)
                            .text_color(color(MUTED)),
                        )
                        .child(
                            quiet(SharedString::from(format!("update-default-service-{id}")))
                                .icon(icons::check_circle())
                                .label("用于以后生成…")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.confirm_default_service(default_id.clone(), window, cx);
                                })),
                        ),
                )
            })
            .child(h_flex().gap_2().flex_wrap().children(status_badges))
            .when_some(latest_test, |card, latest_test| {
                card.child(
                    text(
                        SharedString::from(format!("saved-service-last-test-{id}")),
                        latest_test,
                    )
                    .text_size(TEXT_AUX)
                    .text_color(color(MUTED)),
                )
            })
            .when(!failures.is_empty(), |card| {
                card.child(
                    text(
                        SharedString::from(format!("saved-service-test-status-{id}")),
                        failures.join("\n"),
                    )
                    .text_size(TEXT_AUX)
                    .text_color(color(GRAY)),
                )
            })
    }

    fn setting_input_changed(&mut self, field: EditField, cx: &mut Context<Self>) {
        if !self.settings_ui.initialized
            && matches!(field, EditField::LocalModel | EditField::Languages)
        {
            return;
        }
        if matches!(field, EditField::LocalModel | EditField::Languages) {
            let input = self.setting_value(field, cx);
            let mut next = self.generation_edit_base();
            let previous = if field == EditField::LocalModel {
                next.local_model_draft
                    .clone()
                    .unwrap_or_else(|| next.options.asr_model.clone().unwrap_or_default())
            } else {
                next.subtitle_languages_draft
                    .clone()
                    .unwrap_or_else(|| next.preferred_subtitle_languages.join(", "))
            };
            if previous == input {
                return;
            }
            if field == EditField::LocalModel {
                next.local_model_draft = Some(input);
            } else {
                next.subtitle_languages_draft = Some(input);
            }
            self.save_generation_text_draft(next, cx);
            return;
        }
        let input = self.setting_value(field, cx);
        if let Some(editor) = &mut self.settings_ui.editor {
            if matches!(field, EditField::Address | EditField::Key) {
                editor.models.invalidate();
            }
            match field {
                EditField::Name => editor.draft.name = input,
                EditField::Address => editor.draft.address = input,
                EditField::Model => editor.draft.model = input,
                _ => {}
            }
            editor.errors.clear();
            editor.saved_configuration = None;
            if field != EditField::Name {
                editor.evidence = None;
            }
            editor.status = None;
        }
        cx.notify();
    }
    fn save_prompt_draft(&mut self, cx: &mut Context<Self>) {
        if !self.settings_ui.initialized {
            return;
        }
        let text = self.settings_ui.prompt.read(cx).value().to_string();
        let mut next = self.generation_edit_base();
        if next
            .prompt_draft
            .as_ref()
            .or(next.prompt.as_ref())
            .map(String::as_str)
            .unwrap_or("")
            == text
            // The prefilled standard rule is not an unsaved edit.
            || (next.prompt.is_none()
                && next.prompt_draft.is_none()
                && text.trim() == course2md::llm::DEFAULT_PROMPT.trim())
        {
            return;
        }
        next.prompt_draft = Some(text);
        self.save_generation_text_draft(next, cx);
    }
    fn save_generation_text_draft(&mut self, next: GenerationPreferences, cx: &mut Context<Self>) {
        let previous = self
            .ordinary_preferences_submit_issue()
            .map(|issue| issue.message);
        let had_pending = self.settings_ui.pending_generation.is_some()
            || self.preferences.generation_intent().is_some();
        match self.preferences.save_generation(next.clone()) {
            Ok(()) => {
                self.settings_ui.pending_generation = None;
                self.set_settings_feedback(
                    PreferenceGroup::Generation,
                    "尚未应用".into(),
                    false,
                );
                if had_pending {
                    self.refresh_preference_defaults(cx);
                }
                self.clear_resolved_generation_errors(previous);
            }
            Err(error) => {
                self.settings_ui.pending_generation = Some(next);
                self.set_settings_save_failure(PreferenceGroup::Generation, &error);
            }
        }
        cx.notify();
    }
    fn save_open_service_draft(
        &mut self,
        include_key: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(editor) = &self.settings_ui.editor else {
            return true;
        };
        if editor.pending_binding.is_some() || editor.saved_configuration.is_some() {
            return true;
        }
        let mut draft = editor.draft.clone();
        draft.name = self.setting_value(EditField::Name, cx);
        draft.address = self.setting_value(EditField::Address, cx);
        draft.model = self.setting_value(EditField::Model, cx);
        let key = include_key
            .then(|| self.setting_value(EditField::Key, cx))
            .filter(|key| !key.trim().is_empty());
        if key.is_none()
            && draft.revision == 0
            && draft.name.is_empty()
            && draft.address.is_empty()
            && draft.model.is_empty()
            && draft.credential.is_none()
        {
            return true;
        }
        if key.is_none() && self.preferences.draft(&draft.id) == Some(&draft) {
            return true;
        }
        match self
            .preferences
            .save_service_draft(draft, key.map(Secret::new))
        {
            Ok(draft) => {
                if let Some(editor) = &mut self.settings_ui.editor {
                    editor.draft = draft;
                    editor.save_failed = false;
                    editor.status = None;
                }
                if include_key {
                    self.settings_ui.inputs[&EditField::Key]
                        .update(cx, |input, cx| input.set_value("", window, cx));
                }
                cx.notify();
                true
            }
            Err(error) => {
                if let Some(editor) = &mut self.settings_ui.editor {
                    editor.status = Some(format!("这些修改尚未保存：{error:#}"));
                    editor.save_failed = true;
                }
                cx.notify();
                false
            }
        }
    }
    fn apply_language_preferences(&mut self, cx: &mut Context<Self>) {
        let raw = self.setting_value(EditField::Languages, cx);
        let languages: Vec<String> = raw
            .split([',', '，', '\n'])
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
            .collect();
        if languages
            .iter()
            .any(|v| v.len() > 35 || !v.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
        {
            self.set_settings_feedback(
                PreferenceGroup::Generation,
                "语言代码只能包含字母、数字和连字符，请用逗号分隔各语言".into(),
                true,
            );
            cx.notify();
            return;
        }
        let mut next = self.generation_edit_base();
        next.preferred_subtitle_languages = languages;
        next.subtitle_languages_draft = None;
        self.commit_generation(next, cx);
    }
    fn set_settings_feedback(&mut self, group: PreferenceGroup, message: String, error: bool) {
        if group == PreferenceGroup::Generation && error {
            self.settings_ui
                .generation_block_messages
                .insert(message.clone());
        }
        self.settings_ui.feedback_details.remove(&group);
        if !error {
            self.settings_ui.expanded_feedback.remove(&group);
        }
        self.settings_ui
            .feedback
            .insert(group, (message, error, Instant::now()));
    }
    fn set_settings_save_failure(&mut self, group: PreferenceGroup, error: &anyhow::Error) {
        self.set_settings_feedback(group, preferences::save_failure_message(group, error), true);
        self.settings_ui.feedback_details.insert(
            group,
            format!("设置目录：{}\n{error:#}", self.preferences.root().display()),
        );
    }
    pub(crate) fn settings_group_notice(&self, group: PreferenceGroup) -> Option<(String, bool)> {
        if let Some((message, error, when)) = self.settings_ui.feedback.get(&group)
            && (*error || when.elapsed() < Duration::from_secs(6))
        {
            return Some((message.clone(), *error));
        }
        self.preferences
            .issues()
            .iter()
            .rev()
            .find(|issue| issue.group == group)
            .map(|issue| {
                (
                    issue.message.clone(),
                    issue.kind != preferences::SettingsIssueKind::Recovered,
                )
            })
    }
    pub(crate) fn group_feedback(&self, group: PreferenceGroup, cx: &mut Context<Self>) -> Div {
        self.group_feedback_with_retry_emphasis(group, false, cx)
    }
    pub(crate) fn group_feedback_with_retry_emphasis(
        &self,
        group: PreferenceGroup,
        primary_retry: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut view = v_flex().w_full().min_w_0().gap_2();
        if let Some((message, error)) = self.settings_group_notice(group) {
            view = view.child(crate::motion::enter(
                SharedString::from(format!("preference-feedback-{}-{message}", group as usize)),
                h_flex()
                    .w_full()
                    .min_w_0()
                    .gap_2()
                    .items_center()
                    .child(
                        if error {
                            icons::warning()
                        } else {
                            icons::check_circle()
                        }
                        .size_4()
                        .flex_shrink_0()
                        .text_color(color(if error {
                            DANGER
                        } else {
                            SUCCESS
                        })),
                    )
                    .child(
                        text(("settings-group-feedback", group as usize), message.clone())
                            .flex_1()
                            .min_w_0()
                            .text_size(TEXT_BODY)
                            .text_color(color(if error { DANGER } else { MUTED })),
                    )));
            let has_pending = match group {
                PreferenceGroup::Generation => self.settings_ui.pending_generation.is_some(),
                PreferenceGroup::Application => self.settings_ui.pending_application.is_some(),
                PreferenceGroup::Services => false,
            };
            if error && has_pending {
                let id = ("retry-settings-save", group as usize);
                let retry = if primary_retry {
                    primary_pill(id)
                } else {
                    control(id)
                };
                view = view.child(
                    retry
                        .icon(icons::refresh())
                        .label("重试保存")
                        .self_start()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.retry_ordinary_preferences(group, cx);
                        })),
                );
            }
            let detail = self
                .settings_ui
                .feedback_details
                .get(&group)
                .cloned()
                .or_else(|| {
                    self.preferences
                        .issues()
                        .iter()
                        .rev()
                        .find(|issue| issue.group == group)
                        .and_then(|issue| issue.detail.clone())
                });
            if let Some(detail) = detail {
                let expanded = self.settings_ui.expanded_feedback.contains(&group);
                view = view.child(
                    control(("settings-feedback-details", group as usize))
                        .icon(icons::info())
                        .ghost()
                        .self_start()
                        .label(if expanded {
                            "收起技术详情"
                        } else {
                            "查看技术详情"
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !this.settings_ui.expanded_feedback.remove(&group) {
                                this.settings_ui.expanded_feedback.insert(group);
                            }
                            cx.notify();
                        })),
                );
                if expanded {
                    view = view.child(
                        div()
                            .id(("settings-feedback-technical-scroll", group as usize))
                            .w_full()
                            .min_w_0()
                            .max_h(px(140.))
                            .overflow_y_scroll()
                            .p_2()
                            .child(
                                text(("settings-feedback-technical", group as usize), detail)
                                    .w_full()
                                    .min_w_0()
                                    .whitespace_normal()
                                    .text_sm()
                                    .text_color(color(MUTED)),
                            ),
                    );
                }
            }
        }
        if self.preferences.is_blocked(group) {
            view = view.child(
                control(("reset-settings-group", group as usize))
                    .icon(icons::refresh())
                    .label("保留原文件并重置此组")
                    .self_start()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.restore_settings_group(group, window, cx);
                    })),
            );
        }
        view
    }
    pub(super) fn register_storage_location(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("添加存储位置".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = prompt.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(Ok(Some(paths))) => {
                        if let Some(path) = paths.into_iter().next() {
                            let name = path
                                .file_name()
                                .and_then(|v| v.to_str())
                                .unwrap_or("课程库")
                                .to_owned();
                            let duplicate = this.workspace.as_ref().is_some_and(|w| {
                                w.state
                                    .libraries
                                    .iter()
                                    .any(|l| l.root.canonicalize().ok() == path.canonicalize().ok())
                            });
                            if !duplicate
                                && let Ok(identity) = std::fs::read_to_string(path.join(".course2md-library-id"))
                                && let Some(existing) = this.workspace.as_ref().and_then(|workspace| workspace.state.library(identity.trim()))
                            {
                                this.workspace_error = Some(format!(
                                    "「{}」已登记，可在该课程库中“重新定位…”",
                                    existing.name
                                ));
                                cx.notify();
                                return;
                            }
                            match this
                                .workspace
                                .as_mut()
                                .ok_or_else(|| anyhow!("课程库记录暂时不可用"))
                                .and_then(|w| w.register_library(path, name, false))
                            {
                                Ok(_) => {
                                    this.message = Some(
                                        if duplicate {
                                            "这个存储位置已经在课程库中"
                                        } else {
                                            "已添加存储位置"
                                        }
                                        .into(),
                                    );
                                    this.refresh_library(cx);
                                }
                                Err(error) => {
                                    this.workspace_error =
                                        Some(format!("存储位置尚未添加：{error:#}"))
                                }
                            }
                        }
                    }
                    Ok(Ok(None)) => {}
                    Ok(Err(error)) => {
                        this.workspace_error = Some(format!("无法选择存储位置：{error:#}"))
                    }
                    Err(error) => {
                        this.workspace_error = Some(format!("无法打开文件选择器：{error}"))
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
    pub(crate) fn appearance_controls(&self, cx: &mut Context<Self>) -> Div {
        div().w_full().child(group("appearance-motion", "界面偏好")
            .child(settings_row(
                "app-font-scale-label",
                icons::zoom_in(),
                "界面文字大小",
                "",
                self.setting_choices("app-font-scale", "应用文字大小")
                    .options(crate::preferences::FONT_SCALES.into_iter().map(|scale| {
                        (
                            (scale * 100.).round().to_string(),
                            format!("{}%", (scale * 100.) as u32),
                        )
                    }))
                    .full_width()
                    .selected(
                        (self.preferences.application().font_scale * 100.)
                            .round()
                            .to_string(),
                    )
                    .on_change(
                        cx.listener(move |this, selected: &SharedString, window, cx| {
                            let Ok(percent) = selected.parse::<f32>() else {
                                return;
                            };
                            let scale = percent / 100.;
                            let mut next = this.application_edit_base();
                            next.font_scale = scale;
                            if this.commit_application(next, cx) {
                                theme::apply_scale(scale, window, cx);
                            }
                        }),
                    ),
            ))
            .child(
                self.setting_preference(
                    icons::pause(),
                    "减少动态效果",
                    "",
                    Switch::new("app-reduce-motion")
                        .checked(self.preferences.application().desktop.reduce_motion)
                        .on_click(cx.listener(|this, enabled, _, cx| {
                            let mut next = this.application_edit_base();
                            next.desktop.reduce_motion = *enabled;
                            this.commit_application(next, cx);
                        })),
                ),
            ))
    }
    fn application_settings_page(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .w_full()
            .min_w_0()
            .gap_6()
            .child(self.environment_page(window, cx))
            .child(
                group("about-heading", "关于")
                    .child(setting_surface().child(self.about_page(cx)))
                    .child(settings_row(
                "restart-onboarding-label",
                icons::book_open(),
                "用户引导",
                "",
                outline_pill("restart-onboarding")
                    .label("重新开始用户引导")
                    .icon(icons::restart())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.start_onboarding(window, cx);
                    })),
            )),
            )
            .into_any_element()
    }
    pub(crate) fn commit_application(
        &mut self,
        next: ApplicationPreferences,
        cx: &mut Context<Self>,
    ) -> bool {
        match self.preferences.save_application(next.clone()) {
            Ok(()) => {
                self.settings_ui.pending_application = None;
                self.desktop_settings = self.preferences.application().desktop.clone();
                cx.set_reduce_motion(self.desktop_settings.reduce_motion);
                self.set_settings_feedback(PreferenceGroup::Application, "已保存".into(), false);
                cx.notify();
                true
            }
            Err(error) => {
                self.settings_ui.pending_application = Some(next);
                self.set_settings_save_failure(PreferenceGroup::Application, &error);
                cx.notify();
                false
            }
        }
    }
    fn environment_page(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let mut card = setting_surface();
        if let Some(e) = &self.environment {
            let provider = self
                .preferences
                .generation()
                .options
                .provider
                .unwrap_or_else(|| self.recommended_local_provider());
            let speech_service = provider == course2md::config::AsrProvider::Api;
            let local_ready = e.engine
                && match provider {
                    course2md::config::AsrProvider::Coreml => e.apple,
                    course2md::config::AsrProvider::Gpu => e.llama && e.gpu.is_some(),
                    course2md::config::AsrProvider::Cpu => e.llama,
                    course2md::config::AsrProvider::Npu => e.npu,
                    course2md::config::AsrProvider::Api => false,
                };
            for (index, (label, ready, optional)) in [
                ("视频处理与导出", e.engine && e.ffmpeg && e.ffprobe, false),
                ("读取在线视频", e.ytdlp, false),
                ("本机识别环境", local_ready, speech_service),
            ]
            .into_iter()
            .enumerate()
            {
                card = card.child(settings_status_row(
                    ("diagnostic-capability", index),
                    label,
                    badge(if optional {
                        BadgeKind::Neutral
                    } else if ready {
                        BadgeKind::Success
                    } else {
                        BadgeKind::Warning
                    })
                    .child(if optional {
                        "当前未使用"
                    } else if ready {
                        "可用"
                    } else {
                        "待修复"
                    }),
                ));
            }
            if !e.engine {
                card = card
                    .child(info_callout(
                        "repair-bundled-engine",
                        "转换程序无法运行，重新安装应用后笔记仍会保留",
                    ))
                    .child(
                        control("download-repair-app")
                            .icon(icons::download())
                            .label("下载安装包")
                            .self_start()
                            .on_click(|_, _, cx| {
                                cx.open_url("https://github.com/mizorewww/course2md/releases")
                            }),
                    );
            }
        } else {
            card = card.child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(crate::motion::spinner("diagnostic-checking-spinner", cx))
                    .child(settings_value("diagnostic-checking", "正在检查本机能力…")),
            );
        }
        let open = self.settings_ui.diagnostics_details_open;
        card = card.child(
            h_flex()
                .w_full()
                .min_w_0()
                .gap_2()
                .flex_wrap()
                .child(
                    control("refresh-environment")
                        .icon(icons::refresh())
                        .label("重新检查环境")
                        .loading(self.environment.is_none())
                        .disabled(self.environment.is_none())
                        .on_click(cx.listener(|this, _, _, cx| this.refresh_environment(cx))),
                )
                .child(
                    quiet("toggle-diagnostics-details")
                        .icon(icons::info())
                        .label(if open {
                            "收起诊断详情"
                        } else {
                            "查看诊断详情"
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.settings_ui.diagnostics_details_open =
                                !this.settings_ui.diagnostics_details_open;
                            cx.notify();
                        })),
                ),
        );
        let mut details = v_flex().w_full().min_w_0().gap_6().pt_3();
        if let Some(e) = &self.environment {
            let mut programs = settings_detail_group("diagnostic-programs-heading", icons::computer(), "所需程序");
            let needs_llama = matches!(
                self.preferences
                    .generation()
                    .options
                    .provider
                    .unwrap_or_else(|| self.recommended_local_provider()),
                course2md::config::AsrProvider::Cpu | course2md::config::AsrProvider::Gpu
            );
            for (index, (name, found)) in [
                ("转换程序", e.engine),
                ("ffmpeg", e.ffmpeg),
                ("ffprobe", e.ffprobe),
                ("yt-dlp", e.ytdlp),
                ("llama-server", e.llama),
            ]
            .into_iter()
            .filter(|(name, _)| *name != "llama-server" || needs_llama)
            .enumerate()
            {
                programs = programs.child(settings_detail_row(
                    ("diagnostic-tool-label", index),
                    name,
                    settings_value(
                        ("diagnostic-tool", index),
                        if found { "已安装" } else { "未安装" },
                    ),
                ));
            }
            if !e.ffmpeg || !e.ffprobe || !e.ytdlp {
                let (help, command) = if cfg!(target_os = "macos") {
                    (
                        "在终端运行",
                        "brew install ffmpeg yt-dlp",
                    )
                } else if cfg!(target_os = "windows") {
                    (
                        "在 PowerShell 运行",
                        "winget install Gyan.FFmpeg; winget install yt-dlp.yt-dlp",
                    )
                } else {
                    (
                        "用系统包管理器安装 ffmpeg，再运行",
                        "pipx install yt-dlp",
                    )
                };
                programs = programs.child(settings_detail_row(
                    "media-tools-repair-label",
                    "修复方式",
                    v_flex()
                        .w_full()
                        .min_w_0()
                        .gap_2()
                        .child(settings_value("install-media-tools-help", help))
                        .child(settings_value("install-media-tools-command", command))
                        .child(
                            quiet("copy-media-install-command")
                                .icon(icons::content_copy())
                                .label("复制安装命令")
                                .self_start()
                                .on_click(move |_, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(command.into()))
                                }),
                        ),
                ));
            }
            details = details.child(programs);
        }
        details = details.child(self.model_hardware_details(cx));
        // Expanded details belong to the status item, not a divided item of
        // their own; the model panel follows as its own section.
        v_flex()
            .w_full()
            .min_w_0()
            .gap_6()
            .child(group("diagnostics-heading", "运行检查").child(
                v_flex().w_full().min_w_0().child(card).child(crate::motion::disclosure(
                    "diagnostics-detail-content",
                    open,
                    details.px(rems(20. / 14.)).pb(rems(16. / 14.)),
                    window,
                    cx,
                )),
            ))
            .child(self.model_diagnostics_panel(window, cx))
    }
    // Wrappers only while the other interface modules are being integrated.
    /// Service editor drafts are deliberately excluded: an existing published service
    /// continues to be usable. An unapplied failed ordinary commit must never be silently
    /// replaced by the older effective defaults when submitting a task.
    pub fn ordinary_preferences_ready_for_submit(&self) -> Result<()> {
        if let Some(issue) = self.ordinary_preferences_submit_issue() {
            return Err(anyhow!(issue.message));
        }
        Ok(())
    }
    pub fn ordinary_preferences_submit_issue(&self) -> Option<OrdinaryPreferenceIssue> {
        let group = PreferenceGroup::Generation;
        if self.preferences.is_blocked(PreferenceGroup::Generation) {
            return Some(OrdinaryPreferenceIssue {
                group,
                message: "生成选项暂时无法读取，原文件与当前输入已保留".into(),
                can_retry: false,
            });
        }
        if self.settings_ui.pending_generation.is_some()
            || self.preferences.generation_intent().is_some()
        {
            return Some(OrdinaryPreferenceIssue {
                group,
                message: self
                    .settings_group_notice(group)
                    .filter(|(_, error)| *error)
                    .map(|(message, _)| message)
                    .unwrap_or_else(|| "生成选项的修改尚未保存，当前输入已保留".into()),
                can_retry: true,
            });
        }
        None
    }
    pub fn retry_ordinary_preferences(
        &mut self,
        group: PreferenceGroup,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.preferences.is_blocked(group) {
            return false;
        }
        match group {
            PreferenceGroup::Generation => {
                if let Some(next) = self
                    .settings_ui
                    .pending_generation
                    .as_ref()
                    .or_else(|| self.preferences.generation_intent())
                    .cloned()
                {
                    self.commit_generation(next, cx)
                } else {
                    true
                }
            }
            PreferenceGroup::Application => {
                if let Some(next) = self
                    .settings_ui
                    .pending_application
                    .as_ref()
                    .or_else(|| self.preferences.application_intent())
                    .cloned()
                {
                    self.commit_application(next, cx)
                } else {
                    true
                }
            }
            PreferenceGroup::Services => false,
        }
    }
    /// Only the explicitly named group is reset; the form and its source stay in place.
    pub fn restore_ordinary_preferences(
        &mut self,
        group: PreferenceGroup,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if group == PreferenceGroup::Services {
            return false;
        }
        self.restore_settings_group(group, window, cx)
    }
    fn restore_settings_group(
        &mut self,
        group: PreferenceGroup,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let previous = self
            .ordinary_preferences_submit_issue()
            .map(|issue| issue.message);
        match self.preferences.reset_group(group) {
            Ok(()) => {
                match group {
                    PreferenceGroup::Generation => self.settings_ui.pending_generation = None,
                    PreferenceGroup::Application => self.settings_ui.pending_application = None,
                    PreferenceGroup::Services => {}
                }
                self.settings_ui.initialized = false;
                // 事件路径立刻重新水合，不等下一次进入设置（渲染不做这件事）
                self.hydrate_settings_inputs(window, cx);
                self.set_settings_feedback(group, "原文件已保留，此组设置已重置".into(), false);
                if group == PreferenceGroup::Application {
                    self.desktop_settings = self.preferences.application().desktop.clone();
                    cx.set_reduce_motion(self.desktop_settings.reduce_motion);
                } else {
                    self.refresh_preference_defaults(cx);
                }
                if group == PreferenceGroup::Generation {
                    self.clear_resolved_generation_errors(previous);
                }
                if !self.preference_defaults_pending {
                    self.advance_conversion_when_ready(cx);
                }
                cx.notify();
                true
            }
            Err(error) => {
                self.set_settings_save_failure(group, &error);
                cx.notify();
                false
            }
        }
    }
    fn clear_resolved_generation_errors(&mut self, previous: Option<String>) {
        if self.ordinary_preferences_submit_issue().is_some() {
            return;
        }
        let mut messages = std::mem::take(&mut self.settings_ui.generation_block_messages);
        if let Some(previous) = previous {
            messages.insert(previous);
        }
        for error in [&mut self.source_validation, &mut self.message] {
            if error.as_ref().is_some_and(|error| messages.contains(error)) {
                *error = None;
            }
        }
    }
    pub fn relocate_settings_paths(
        &mut self,
        old: &std::path::Path,
        new: &std::path::Path,
        cx: &mut Context<Self>,
    ) -> Result<()> {
        let previous = self
            .ordinary_preferences_submit_issue()
            .map(|issue| issue.message);
        let result = self.preferences.relocate_generation_paths(old, new);
        self.settings_ui.pending_generation = self.preferences.generation_intent().cloned();
        if let Err(error) = &result {
            self.set_settings_feedback(
                PreferenceGroup::Generation,
                format!("模型位置尚未更新，修改已保留，可重试保存：{error:#}"),
                true,
            );
        } else {
            self.refresh_preference_defaults(cx);
            self.clear_resolved_generation_errors(previous);
        }
        cx.notify();
        result
    }
    /// Ordinary preferences save immediately. Service form edits become active
    /// only through Save and do not create a separate quit-time save workflow.
    pub fn flush_settings_for_exit(&mut self, cx: &mut Context<Self>) -> bool {
        if let Some(editor) = &self.settings_ui.editor {
            if let Some(cancel) = &editor.test_running {
                cancel.store(true, Ordering::Release);
            }
        }
        if let Some(next) = self.settings_ui.pending_generation.clone() {
            self.commit_generation(next, cx);
        }
        if let Some(next) = self.settings_ui.pending_application.clone() {
            self.commit_application(next, cx);
        }
        if !self.preferences.unsaved_intents_are_preserved() {
            self.message = Some("修改暂时无法保存，请重试".into());
            cx.notify();
            return false;
        }
        true
    }
}
