//! Semantic design tokens and animated appearance for all desktop pages.
use crate::palettes::{PaletteId, ThemePreferences};
use gpui::{App, Pixels, Rems, Rgba, Window, px, rems, rgb};
use gpui_component::{Theme, ThemeMode};
use std::{cell::RefCell, time::Instant};
#[path = "choice_group.rs"]
mod choice_group;
pub use choice_group::SingleChoiceGroup;

macro_rules! tokens {
    ($($name:ident),+ $(,)?) => {
        #[allow(non_camel_case_types)]
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        #[repr(usize)]
        pub enum ColorToken { $($name),+ }
        $(pub const $name: ColorToken = ColorToken::$name;)+
        const ALL_TOKENS: &[ColorToken] = &[$(ColorToken::$name),+];
    }
}
tokens!(
    CANVAS,
    SURFACE,
    INSET,
    INK,
    GRAY,
    FAINT,
    HAIRLINE,
    CARD_LINE,
    CONTROL,
    ACCENT,
    ACCENT_STRONG,
    ACCENT_SOFT,
    PRIMARY,
    PRIMARY_HOVER,
    PRIMARY_ACTIVE,
    ON_PRIMARY,
    SUCCESS,
    SUCCESS_BG,
    WARNING,
    WARNING_BG,
    DANGER,
    DANGER_BG,
    BADGE_PROGRESS,
    BADGE_PROGRESS_BG,
    PROGRESS_FILL,
    PROGRESS_TRACK,
    FIND_HIGHLIGHT,
    FIND_CURRENT,
    FIND_CURRENT_LINE,
    SEGMENT_TRACK,
    SWITCH_TRACK_OFF,
    HOVER_WARM,
    SELECTION,
    PRIMARY_CONTAINER,
    ON_PRIMARY_CONTAINER,
    SECONDARY_CONTAINER,
    ON_SECONDARY_CONTAINER,
    SURFACE_HIGH
);
const COLOR_COUNT: usize = ALL_TOKENS.len();

pub const MUTED: ColorToken = GRAY;
pub const LINE: ColorToken = HAIRLINE;
pub const BLUE: ColorToken = ACCENT_STRONG;
pub const TINT: ColorToken = ACCENT_SOFT;

pub fn blend(from: Rgba, to: Rgba, t: f32) -> Rgba {
    let t = t.clamp(0., 1.);
    Rgba {
        r: from.r + (to.r - from.r) * t,
        g: from.g + (to.g - from.g) * t,
        b: from.b + (to.b - from.b) * t,
        a: from.a + (to.a - from.a) * t,
    }
}

fn palette_colors(id: PaletteId) -> [Rgba; COLOR_COUNT] {
    let p = id.colors();
    let tint = |foreground, amount| blend(rgb(p.surface), rgb(foreground), amount);
    std::array::from_fn(|index| match ALL_TOKENS[index] {
        CANVAS => rgb(p.canvas),
        SURFACE => rgb(p.surface),
        INSET => rgb(p.inset),
        INK => rgb(p.text),
        GRAY => rgb(p.muted),
        FAINT => rgb(p.subtle),
        HAIRLINE => rgb(p.border),
        // M3 cards are separated by tone rather than a full-strength outline.
        CARD_LINE => blend(rgb(p.surface), rgb(p.border), 0.55),
        CONTROL => rgb(p.control),
        ACCENT | ACCENT_STRONG | PRIMARY | PROGRESS_FILL => rgb(p.accent),
        ON_PRIMARY => rgb(p.on_accent),
        PRIMARY_HOVER => blend(rgb(p.accent), rgb(p.text), 0.10),
        PRIMARY_ACTIVE => blend(rgb(p.accent), rgb(p.text), 0.20),
        // M3 container roles derived from each palette's own accent and text, so
        // every preset keeps its identity (assets/themes/SOURCES.md).
        ACCENT_SOFT | SECONDARY_CONTAINER => tint(p.accent, if id.is_dark() { 0.26 } else { 0.24 }),
        ON_SECONDARY_CONTAINER => blend(rgb(p.text), rgb(p.accent), if id.is_dark() { 0.30 } else { 0.40 }),
        PRIMARY_CONTAINER => tint(p.accent, if id.is_dark() { 0.38 } else { 0.24 }),
        ON_PRIMARY_CONTAINER => blend(rgb(p.text), rgb(p.accent), 0.20),
        SURFACE_HIGH => blend(rgb(p.canvas), rgb(p.text), if id.is_dark() { 0.07 } else { 0.05 }),
        SUCCESS => rgb(p.success),
        SUCCESS_BG => tint(p.success, 0.12),
        WARNING => rgb(p.warning),
        WARNING_BG => tint(p.warning, 0.12),
        DANGER => rgb(p.danger),
        DANGER_BG => tint(p.danger, 0.12),
        BADGE_PROGRESS => rgb(p.accent),
        BADGE_PROGRESS_BG => tint(p.accent, 0.10),
        // M3 linear progress: the track is the indicator's tonal container.
        PROGRESS_TRACK => tint(p.accent, if id.is_dark() { 0.26 } else { 0.24 }),
        SEGMENT_TRACK | SWITCH_TRACK_OFF => rgb(p.border),
        HOVER_WARM => tint(p.text, 0.05),
        SELECTION => rgb(p.selection),
        FIND_HIGHLIGHT => tint(p.warning, 0.16),
        FIND_CURRENT => tint(p.warning, 0.32),
        FIND_CURRENT_LINE => rgb(p.warning),
    })
}

struct PaintState {
    target: Option<PaletteId>,
    from: [Rgba; COLOR_COUNT],
    current: [Rgba; COLOR_COUNT],
    started: Instant,
}
thread_local! {
    // GPUI renders application chrome and all overlays on its UI thread.
    // Thread-local paint values keep helpers context-free without unsafe globals.
    static PAINT: RefCell<PaintState> = RefCell::new(PaintState {
        target: None, from: palette_colors(PaletteId::Paper),
        current: palette_colors(PaletteId::Paper), started: Instant::now(),
    });
}

pub fn color(token: ColorToken) -> Rgba {
    PAINT.with(|paint| paint.borrow().current[token as usize])
}

pub fn system_dark(cx: &App) -> bool {
    ThemeMode::from(cx.window_appearance()).is_dark()
}

/// Resolve system changes on each notified frame, and retarget palette changes
/// from their current paint values. Persisted settings are the sole authority.
pub fn apply_preference(preference: &ThemePreferences, window: &mut Window, cx: &mut App) {
    let id = preference.resolve(system_dark(cx));
    let target = palette_colors(id);
    let (changed, running) = PAINT.with(|paint| {
        let mut paint = paint.borrow_mut();
        let now = Instant::now();
        if paint.target.is_none() {
            paint.target = Some(id);
            paint.current = target;
            paint.from = target;
            return (true, false);
        }
        if paint.target != Some(id) {
            paint.from = paint.current;
            paint.target = Some(id);
            paint.started = now;
        }
        let t = if cx.reduce_motion() {
            1.
        } else {
            (now.duration_since(paint.started).as_secs_f32()
                / (crate::motion::PALETTE_MS as f32 / 1000.))
                .min(1.)
        };
        let eased = crate::motion::ease_out(t);
        let next = std::array::from_fn(|i| blend(paint.from[i], target[i], eased));
        let changed = next != paint.current;
        paint.current = next;
        (changed, t < 1.)
    });
    if changed {
        sync_component_theme(id.is_dark(), cx);
    }
    if running {
        window.request_animation_frame();
    }
}

fn sync_component_theme(dark: bool, cx: &mut App) {
    let theme = Theme::global_mut(cx);
    theme.mode = if dark {
        ThemeMode::Dark
    } else {
        ThemeMode::Light
    };
    theme.radius = px(8.);
    theme.radius_lg = RADIUS_CARD;
    let colors = &mut theme.colors;
    macro_rules! set {
        ($token:expr => $($field:ident),+ $(,)?) => { $(colors.$field = color($token).into();)+ };
    }
    set!(CANVAS => background, title_bar, tab_bar, status_bar, tiles);
    set!(SURFACE => button, popover, list, table, accordion, group_box, tab_active);
    set!(INSET => sidebar, table_head, table_foot, description_list_label, list_head, list_even, table_even, tab, secondary, button_secondary);
    set!(INK => foreground, button_foreground, popover_foreground, secondary_foreground, button_secondary_foreground, sidebar_foreground, group_box_foreground, table_head_foreground, table_foot_foreground, tab_active_foreground);
    set!(GRAY => muted_foreground, description_list_label_foreground, tab_foreground);
    set!(HAIRLINE => border, title_bar_border, sidebar_border, status_bar_border, table_row_border, window_border);
    set!(CONTROL => input, scrollbar_thumb, switch);
    set!(FAINT => scrollbar_thumb_hover);
    set!(ACCENT => accent_foreground, primary, button_primary, caret, ring, link, progress_bar, slider_bar, drag_border, sidebar_primary);
    set!(ON_PRIMARY => primary_foreground, button_primary_foreground, sidebar_primary_foreground);
    set!(PRIMARY_HOVER => primary_hover, button_primary_hover, link_hover);
    set!(PRIMARY_ACTIVE => primary_active, button_primary_active, link_active);
    set!(ACCENT_SOFT => accent, drop_target, list_active, sidebar_accent, table_active);
    set!(ACCENT_STRONG => list_active_border, table_active_border, sidebar_accent_foreground);
    set!(HOVER_WARM => button_hover, secondary_hover, button_secondary_hover, list_hover, table_hover);
    set!(SEGMENT_TRACK => muted, skeleton, tab_bar_segmented, button_active, secondary_active, button_secondary_active);
    set!(SELECTION => selection);
    set!(SURFACE => switch_thumb, slider_thumb);
    set!(DANGER => danger, button_danger_foreground, danger_hover, danger_active);
    set!(ON_PRIMARY => danger_foreground, success_foreground, warning_foreground, info_foreground);
    set!(DANGER_BG => button_danger, button_danger_hover, button_danger_active);
    set!(SUCCESS => success, button_success_foreground, success_hover, success_active);
    set!(SUCCESS_BG => button_success, button_success_hover, button_success_active);
    set!(WARNING => warning, button_warning_foreground, warning_hover, warning_active);
    set!(WARNING_BG => button_warning, button_warning_hover, button_warning_active);
    set!(ACCENT => info, info_hover, info_active, button_info_foreground);
    set!(ACCENT_SOFT => button_info, button_info_hover, button_info_active);
    colors.overlay = gpui::hsla(0., 0., 0., if dark { 0.58 } else { 0.28 });
    colors.scrollbar = gpui::hsla(0., 0., 0., 0.);
    theme.tokens = theme.colors.into();
    Theme::sync_base(cx);
}

/* ---------- 几何（px，不随字号缩放） ---------- */
pub const RADIUS_PILL: Pixels = px(999.);
pub const RADIUS_CARD: Pixels = px(16.);
/// Settings reference surfaces share a 24px corner radius.
pub const SETTINGS_CARD_RADIUS: Pixels = px(24.);
pub const RADIUS_HERO: Pixels = px(28.);
pub const RADIUS_SMALL: Pixels = px(8.);
/// Ordinary controls share this height, including padding, at every text scale.
pub const CONTROL_HEIGHT: Rems = rems(40. / 14.);
/// Leading icon slot of an icon + label cluster, and the gap before its text.
/// Supporting lines of the same item start at `ICON_SLOT + ICON_GAP`.
pub const SETTING_ITEM_INSET: Rems = rems(20. / 14.);
pub const ICON_SLOT: Rems = rems(20. / 14.);
pub const ICON_GAP: Rems = rems(8. / 14.);
pub const LABEL_INSET: Rems = rems(28. / 14.);

/// Task dialogs sit below the shared title bar with the same page gutter.
pub fn task_dialog_top(window: &Window) -> Pixels {
    window.rem_size() * (40. / 14.) + px(40.)
}

/* ---------- 栏宽（rems，随字号缩放的结构尺寸） ---------- */
pub const COLUMN: Rems = rems(65.714);
pub const TOC_PANEL: Rems = rems(16.);

/* ---------- 字级（rems；14px 为 1rem 基准） ---------- */
pub const TEXT_AUX: Rems = rems(0.857);
pub const TEXT_BODY: Rems = rems(1.);
pub const TEXT_TITLE: Rems = rems(1.286);
pub const TEXT_READER: Rems = rems(1.143);
/// Main page headline.
pub const TEXT_DISPLAY: Rems = rems(2.);

/// Page title shared by library, tasks, settings and a non-hero workbench.
/// As on the M3 canvas, the title is text alone; the app bar already marks
/// the destination with its icon.
pub fn page_heading(
    id: impl Into<gpui::ElementId>,
    title: impl Into<gpui::SharedString>,
) -> gpui::Div {
    use gpui::{prelude::*, *};
    div().flex().items_center().min_w_0().child(
        accessible_text(id, title.into())
            .role(Role::Heading)
            .text_size(TEXT_DISPLAY)
            .line_height(rems(36. / 14.))
            .font_weight(FontWeight::SEMIBOLD),
    )
}

/* ---------- 中性阴影 ---------- */
fn shadow_color(alpha: f32) -> gpui::Hsla {
    gpui::hsla(0., 0., 0., alpha)
}
/// Hero input card: near 0/1/2 @5% + far 0/16/40 @8%.
pub fn shadow_hero() -> Vec<gpui::BoxShadow> {
    vec![
        gpui::BoxShadow::new(px(0.), px(1.), shadow_color(0.05)).blur_radius(px(2.)),
        gpui::BoxShadow::new(px(0.), px(16.), shadow_color(0.08)).blur_radius(px(40.)),
    ]
}
/// M3 level-1 card elevation: 0/1/2 @6%, kept faint so tone carries the layer.
pub fn shadow_card() -> Vec<gpui::BoxShadow> {
    vec![gpui::BoxShadow::new(px(0.), px(1.), shadow_color(0.06)).blur_radius(px(2.))]
}
/// Overlay panels and dialogs: 0/12/32 @16%.
pub fn shadow_popover() -> Vec<gpui::BoxShadow> {
    vec![gpui::BoxShadow::new(px(0.), px(12.), shadow_color(0.16)).blur_radius(px(32.))]
}

/// Plain Div text is not exposed by GPUI's native accessibility bridge. Keep labels on
/// leaf elements so an ancestor does not replace the accessibility of its controls.
pub fn accessible_text(
    id: impl Into<gpui::ElementId>,
    value: impl Into<gpui::SharedString>,
) -> gpui::Stateful<gpui::Div> {
    use gpui::*;
    let value = value.into();
    div()
        .id(id)
        .role(Role::Label)
        .aria_label(value.clone())
        .child(value)
}

/// Failure feedback uses one accessible alert form everywhere. Callers
/// chain their own size and wrapping rules.
pub fn error_text(
    id: impl Into<gpui::ElementId>,
    message: impl Into<gpui::SharedString>,
) -> gpui::Stateful<gpui::Div> {
    use gpui::*;
    accessible_text(id, message)
        .role(Role::Alert)
        .text_color(color(DANGER))
}

/// Primary interface labels have one icon column and one text column. The
/// slot also accepts a live spinner, keeping state changes on the same axis.
pub fn semantic_label(
    id: impl Into<gpui::ElementId>,
    label: impl Into<gpui::SharedString>,
    icon: impl gpui::IntoElement,
) -> gpui::Div {
    use gpui::{prelude::*, *};
    div()
        .flex()
        .items_center()
        .min_w_0()
        .gap(ICON_GAP)
        .child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .size(ICON_SLOT)
                .flex_shrink_0()
                .child(icon),
        )
        .child(
            accessible_text(id, label)
                .min_w_0()
                .whitespace_normal()
                .text_size(TEXT_BODY)
                .font_weight(FontWeight::SEMIBOLD),
        )
}

/// Short supporting text inside an existing setting or resource boundary.
/// The enclosing item supplies the surface; a second frame would compete with it.
pub fn supporting_info(
    id: impl Into<gpui::ElementId>,
    text: impl Into<gpui::SharedString>,
) -> gpui::Div {
    use gpui::{prelude::*, *};
    div()
        .flex()
        .w_full()
        .min_w_0()
        .items_start()
        .gap(ICON_GAP)
        // The info icon sits in the same slot as a label's icon, so this text
        // starts on the label text ruler of the item it explains.
        .child(
            div()
                .flex()
                .justify_center()
                .w(ICON_SLOT)
                .h(rems(18. / 14.))
                .items_center()
                .flex_shrink_0()
                .child(
                    crate::icons::info()
                        .size(rems(16. / 14.))
                        .text_color(color(GRAY)),
                ),
        )
        .child(
            accessible_text(id, text)
                .flex_1()
                .min_w_0()
                .whitespace_normal()
                .text_size(TEXT_AUX)
                .line_height(rems(18. / 14.))
                .text_color(color(GRAY)),
        )
}

/// A supporting fact of an icon-labelled item (an address, a path, a status
/// sentence). It starts on the item's label text ruler, not under its icon.
pub fn item_detail(
    id: impl Into<gpui::ElementId>,
    text: impl Into<gpui::SharedString>,
    tone: ColorToken,
) -> gpui::Stateful<gpui::Div> {
    use gpui::prelude::*;
    accessible_text(id, text)
        .w_full()
        .min_w_0()
        .pl(LABEL_INSET)
        .whitespace_normal()
        .text_size(TEXT_AUX)
        .line_height(rems(18. / 14.))
        .text_color(color(tone))
}

/// Standalone guidance has its own boundary. Inside a setting, use supporting_info.
pub fn info_callout(
    id: impl Into<gpui::ElementId>,
    text: impl Into<gpui::SharedString>,
) -> gpui::Div {
    use gpui::{prelude::*, *};
    div()
        .flex()
        .w_full()
        .min_w_0()
        .items_start()
        .gap(rems(10. / 14.))
        .px(rems(16. / 14.))
        .py(rems(12. / 14.))
        .rounded(RADIUS_CARD)
        .bg(color(SURFACE_HIGH))
        .child(
            crate::icons::info()
                .size(rems(18. / 14.))
                .mt(rems(1. / 14.))
                .flex_shrink_0()
                .text_color(color(GRAY)),
        )
        .child(
            accessible_text(id, text)
                .flex_1()
                .min_w_0()
                .whitespace_normal()
                .text_size(TEXT_AUX)
                .line_height(rems(18. / 14.))
                .text_color(color(GRAY)),
        )
}

/// Related facts share a bounded label column and a stable value start.
pub fn detail_row(
    id: impl Into<gpui::ElementId>,
    label: impl Into<gpui::SharedString>,
    icon: impl gpui::IntoElement,
    value: impl gpui::IntoElement,
) -> gpui::Div {
    use gpui::{prelude::*, *};
    div()
        .flex()
        .w_full()
        .min_w_0()
        .min_h(rems(32. / 14.))
        .items_center()
        .flex_wrap()
        .gap(rems(12. / 14.))
        .child(
            semantic_label(id, label, icon)
                .w(rems(160. / 14.))
                .max_w_full()
                .flex_shrink_0(),
        )
        .child(
            div()
                .flex_1()
                .flex_basis(rems(200. / 14.))
                .min_w_0()
                .max_w_full()
                .font_weight(FontWeight::NORMAL)
                .child(value),
        )
}

pub fn init(cx: &mut App) {
    choice_group::init(cx);
    Theme::change(ThemeMode::Light, None, cx);
    Theme::global_mut(cx).font_size = px(14.);
    sync_component_theme(false, cx);
}

/// 展开/收起内容的共享动效入口（经 theme 导出供页面统一使用）
pub fn disclosure(
    id: impl Into<gpui::ElementId>,
    open: bool,
    content: gpui::Div,
    window: &mut Window,
    cx: &mut App,
) -> gpui::AnyElement {
    crate::motion::disclosure(id, open, content, window, cx)
}

/// One baseline for ordinary actions; content buttons explicitly opt into auto height.
pub fn control(id: impl Into<gpui::ElementId>) -> gpui_component::button::Button {
    use gpui::Styled;
    gpui_component::button::Button::new(id)
        .h(CONTROL_HEIGHT)
        .min_h(CONTROL_HEIGHT)
        .min_w(CONTROL_HEIGHT)
        .rounded(RADIUS_PILL)
        .py_0()
        .flex_shrink_0()
        .text_size(rems(1.0))
        .font_weight(gpui::FontWeight::SEMIBOLD)
}

/// A framed single-line field. Multiline editors and embedded find text use
/// their own content layout, without inventing another ordinary field size.
pub fn text_input(
    state: &gpui::Entity<gpui_component::input::InputState>,
) -> gpui_component::input::Input {
    use gpui::Styled;
    gpui_component::input::Input::new(state)
        .h(CONTROL_HEIGHT)
        .min_h(CONTROL_HEIGHT)
        .rounded(RADIUS_PILL)
        .px(rems(16. / 14.))
        .text_size(TEXT_BODY)
}

/* ---------- 共享控件 ---------- */

/// M3 list section title: primary-colored, semibold, above the list it names.
/// Shared by settings groups, task groups and the workbench's recent notes.
pub fn section_title(
    id: impl Into<gpui::ElementId>,
    title: impl Into<gpui::SharedString>,
) -> gpui::Stateful<gpui::Div> {
    use gpui::prelude::*;
    accessible_text(id, title)
        .role(gpui::Role::Heading)
        .text_size(rems(15. / 14.))
        .line_height(rems(22. / 14.))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(color(ACCENT_STRONG))
}

/// M3 search bar: a filled, borderless pill with a leading search icon. The
/// field's own frame is off, so the bar draws the focus outline itself.
pub fn search_field(
    state: &gpui::Entity<gpui_component::input::InputState>,
    label: impl Into<gpui::SharedString>,
    height: Rems,
    window: &Window,
    cx: &App,
) -> gpui::Div {
    use gpui::{Focusable, prelude::*};
    let focused = state.read(cx).focus_handle(cx).is_focused(window);
    gpui::div()
        .flex()
        .items_center()
        .w_full()
        .min_w_0()
        .h(height)
        .px(rems(16. / 14.))
        .gap(rems(10. / 14.))
        .rounded(RADIUS_PILL)
        .bg(color(SURFACE_HIGH))
        .border_2()
        .border_color(if focused {
            color(ACCENT)
        } else {
            gpui::Rgba { a: 0., ..color(SURFACE_HIGH) }
        })
        .child(
            crate::icons::search()
                .size(rems(18. / 14.))
                .flex_shrink_0()
                .text_color(color(GRAY)),
        )
        .child(
            gpui::div().flex_1().min_w_0().child(
                gpui_component::input::Input::new(state)
                    .appearance(false)
                    .aria_label(label.into())
                    .w_full()
                    .px_0()
                    .text_size(TEXT_BODY)
                    .cleanable(true),
            ),
        )
}

/// Standard card container: surface fill, card border and corner radius.
/// Callers chain their own gap and contents on top of the shared frame.
pub fn card_surface() -> gpui::Div {
    use gpui::Styled;
    gpui_component::v_flex()
        .w_full()
        .p_4()
        .bg(color(SURFACE))
        .border_1()
        .border_color(color(CARD_LINE))
        .rounded(RADIUS_CARD)
        .shadow(shadow_card())
}

/// A selectable preview surface. Its opaque contents cannot conceal pointer
/// feedback: hover changes the outline/shadow, and press dims the whole card.
pub fn selection_card(
    id: impl Into<gpui::ElementId>,
    selected: bool,
    amount: f32,
) -> gpui_base::Button {
    use gpui::{prelude::*, *};
    gpui_base::Button::new(id)
        .selected(selected)
        .aria_toggled(if selected {
            gpui::accesskit::Toggled::True
        } else {
            gpui::accesskit::Toggled::False
        })
        .flex()
        .flex_col()
        .h_auto()
        .min_h(px(0.))
        .min_w_0()
        .rounded(RADIUS_CARD)
        // M3 selection is tonal: the chosen card fills with the secondary
        // container; the 2px outline is reserved for keyboard focus.
        .border_2()
        .border_color(blend(color(CARD_LINE), color(SECONDARY_CONTAINER), amount))
        .bg(blend(color(SURFACE), color(SECONDARY_CONTAINER), amount))
        .cursor_pointer()
        .hover(|style| style.shadow_sm())
        .active(|style| style.opacity(0.9).shadow_none())
        .focus(|style| style.border_color(color(ACCENT)))
}

/// A choice that needs a short explanation stays visible as one interactive
/// object. The title, description and indicator share the same content bounds.
pub fn described_choice(
    id: impl Into<gpui::ElementId>,
    title: impl Into<gpui::SharedString>,
    description: impl Into<gpui::SharedString>,
    icon: gpui_component::Icon,
    selected: bool,
    window: &mut Window,
    cx: &mut App,
) -> gpui_base::Button {
    use gpui::{prelude::*, *};
    let id = id.into();
    let amount = crate::motion::selection_value(
        SharedString::from(format!("choice-card-{id:?}")),
        if selected { 1. } else { 0. },
        window,
        cx,
    );
    let title = title.into();
    let description = description.into();
    selection_card(id, selected, amount)
        .accessibility_label(title.clone())
        .w_full()
        .h_full()
        .justify_start()
        .items_stretch()
        .p(rems(12. / 14.))
        .gap(rems(8. / 14.))
        .text_color(color(INK))
        .child(
            div()
                .flex()
                .w_full()
                .min_w_0()
                .items_center()
                .gap(rems(8. / 14.))
                .child(
                    icon.size(rems(20. / 14.))
                        .flex_shrink_0()
                        .text_color(color(if selected { ACCENT } else { GRAY })),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(TEXT_BODY)
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .size(rems(18. / 14.))
                        .rounded_full()
                        .border_1()
                        .border_color(color(if selected { ACCENT } else { CONTROL }))
                        .bg(if selected {
                            color(ACCENT)
                        } else {
                            color(SURFACE)
                        })
                        .flex()
                        .items_center()
                        .justify_center()
                        .when(selected, |v| {
                            v.child(
                                crate::icons::check()
                                    .size(rems(1.))
                                    .text_color(color(ON_PRIMARY)),
                            )
                        }),
                ),
        )
        .when(!description.is_empty(), |card| {
            card.child(
                div()
                    .w_full()
                    .min_w_0()
                    .pl(rems(28. / 14.))
                    .whitespace_normal()
                    .text_size(TEXT_AUX)
                    .line_height(rems(18. / 14.))
                    .text_color(color(GRAY))
                    .child(description),
            )
        })
}

/// Compact forms use a leading label above the field. Padding belongs to the
/// enclosing scroll viewport so focus effects never meet its clipping edge.
pub fn stacked_field(
    id: impl Into<gpui::ElementId>,
    label: impl Into<gpui::SharedString>,
    icon: gpui_component::Icon,
    field: impl gpui::IntoElement,
) -> gpui::Div {
    use gpui::{prelude::*, *};
    div()
        .flex()
        .flex_col()
        .w_full()
        .min_w_0()
        .gap(rems(8. / 14.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(rems(8. / 14.))
                .child(
                    icon.size(rems(18. / 14.))
                        .flex_shrink_0()
                        .text_color(color(GRAY)),
                )
                .child(
                    accessible_text(id, label)
                        .text_size(TEXT_BODY)
                        .font_weight(FontWeight::SEMIBOLD),
                ),
        )
        .child(field)
}

/// The single forward action of a page or region, in the active theme's accent.
pub fn primary_pill(id: impl Into<gpui::ElementId>) -> gpui_component::button::Button {
    use gpui::Styled;
    use gpui_component::button::ButtonVariants;
    control(id)
        .primary()
        .rounded(RADIUS_PILL)
        .px(rems(16. / 14.))
        .gap(rems(8. / 14.))
        .font_weight(gpui::FontWeight::SEMIBOLD)
}

/// Top-level destination in the app bar. The current destination sits on the
/// tonal secondary container; others are quiet with an 8% M3 hover layer.
pub fn nav_action(
    id: impl Into<gpui::ElementId>,
    selected: bool,
    cx: &App,
) -> gpui_component::button::Button {
    use gpui::Styled;
    use gpui_component::button::{ButtonCustomVariant, ButtonVariants};
    let (bg, fg) = if selected {
        (color(SECONDARY_CONTAINER), color(ON_SECONDARY_CONTAINER))
    } else {
        (gpui::Rgba { a: 0., ..color(CANVAS) }, color(GRAY))
    };
    let layer = |amount| blend(bg, gpui::Rgba { a: 1., ..fg }, amount);
    control(id)
        .custom(
            ButtonCustomVariant::new(cx)
                .color(bg.into())
                .foreground(fg.into())
                .hover(layer(0.08).into())
                .active(layer(0.10).into())
                .shadow(false),
        )
        // gpui-component dilutes a custom fill to 20%; paint the token itself.
        .bg(bg)
        .px(rems(14. / 14.))
        .gap(rems(8. / 14.))
}

/// M3 filled tonal button: a secondary action that still carries weight,
/// e.g. opening a finished note from a list row.
pub fn tonal_pill(id: impl Into<gpui::ElementId>, cx: &App) -> gpui_component::button::Button {
    use gpui::Styled;
    use gpui_component::button::{ButtonCustomVariant, ButtonVariants};
    let bg = color(SECONDARY_CONTAINER);
    let fg = color(ON_SECONDARY_CONTAINER);
    control(id)
        .custom(
            ButtonCustomVariant::new(cx)
                .color(bg.into())
                .foreground(fg.into())
                .hover(blend(bg, fg, 0.08).into())
                .active(blend(bg, fg, 0.10).into())
                .shadow(false),
        )
        .bg(bg)
        .px(rems(16. / 14.))
        .gap(rems(8. / 14.))
}

/// M3 disabled filled button: on-surface at 12% with 38% content, so an
/// unavailable action never reads as a selected tonal surface.
pub fn disabled_when(
    button: gpui_component::button::Button,
    disabled: bool,
    cx: &App,
) -> gpui_component::button::Button {
    use gpui::Styled;
    use gpui_component::button::{ButtonCustomVariant, ButtonVariants};
    if !disabled {
        return button;
    }
    let ink = color(INK);
    let fill = gpui::Rgba { a: 0.12, ..ink };
    button.custom(
        ButtonCustomVariant::new(cx)
            .color(fill.into())
            .foreground(gpui::Rgba { a: 0.38, ..ink }.into())
            .hover(fill.into())
            .active(fill.into())
            .shadow(false),
    )
    .bg(fill)
}

/// M3 filter chip: 32px, 8px corners. Selected chips sit on the secondary
/// container with a leading check; others are outlined.
pub fn filter_chip(
    id: impl Into<gpui::ElementId>,
    selected: bool,
    cx: &App,
) -> gpui_component::button::Button {
    use gpui::Styled;
    use gpui_component::button::{ButtonCustomVariant, ButtonVariants};
    let size = rems(32. / 14.);
    let (bg, fg) = if selected {
        (color(SECONDARY_CONTAINER), color(ON_SECONDARY_CONTAINER))
    } else {
        (gpui::Rgba { a: 0., ..color(CANVAS) }, color(INK))
    };
    let layer = |amount| blend(bg, gpui::Rgba { a: 1., ..fg }, amount);
    let chip = gpui_component::button::Button::new(id)
        .custom(
            ButtonCustomVariant::new(cx)
                .color(bg.into())
                .foreground(fg.into())
                .hover(layer(0.08).into())
                .active(layer(0.10).into())
                .shadow(false),
        )
        .bg(bg)
        .h(size)
        .min_h(size)
        .px(rems(12. / 14.))
        .gap(rems(6. / 14.))
        .rounded(px(8.))
        .flex_shrink_0()
        .text_size(TEXT_BODY)
        .font_weight(gpui::FontWeight::MEDIUM)
        .border_1()
        .border_color(if selected { color(SECONDARY_CONTAINER) } else { color(CONTROL) });
    if selected {
        chip.icon(crate::icons::check())
    } else {
        chip
    }
}

/// Meaningful secondary action with a control-strength border.
pub fn outline_pill(id: impl Into<gpui::ElementId>) -> gpui_component::button::Button {
    use gpui::Styled;
    control(id)
        .outline()
        .rounded(RADIUS_PILL)
        .px(rems(16. / 14.))
        .gap(rems(8. / 14.))
}

/// gpui-component's medium button inline padding (`px_2p5`). A quiet action
/// placed on a content edge moves out by this much so its ink, not its hover
/// shape, meets the edge.
pub const QUIET_INSET: gpui::Rems = gpui::rems(0.625);

/// Tertiary inline action: quiet gray text.
///
/// Do not add a hover text color here: gpui-component's Button applies its own
/// variant hover during render, and a user-set hover style trips the base
/// button's `hover style already set` debug assertion (debug builds panic).
pub fn quiet(id: impl Into<gpui::ElementId>) -> gpui_component::button::Button {
    use gpui::Styled;
    use gpui_component::button::ButtonVariants;
    control(id)
        .ghost()
        .rounded(RADIUS_PILL)
        .text_color(color(GRAY))
}

/// Disclosure under an icon-labelled item: the chevron takes the item's icon
/// slot and the label starts on the item's label ruler, so the button's own
/// inline padding hangs outside the content edge.
pub fn item_disclosure(
    id: impl Into<gpui::ElementId>,
    label: impl Into<gpui::SharedString>,
    open: bool,
) -> gpui_component::button::Button {
    use gpui::{prelude::*, *};
    let chevron = if open {
        crate::icons::chevron_up()
    } else {
        crate::icons::chevron_down()
    };
    quiet(id).self_start().ml(-QUIET_INSET).child(
        div()
            .flex()
            .items_center()
            .gap(ICON_GAP)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(ICON_SLOT)
                    .flex_shrink_0()
                    .child(chevron.size(rems(16. / 14.))),
            )
            .child(label.into()),
    )
}

/// An icon action inside a field shares that field's boundary. Its compact
/// target leaves room for the input's text and focus ring at every text scale.
pub fn input_action(id: impl Into<gpui::ElementId>) -> gpui_component::button::Button {
    use gpui::Styled;
    let size = rems(28. / 14.);
    quiet(id).h(size).min_h(size).w(size).min_w(size).px_0()
}

/// Status badge kinds; text always pairs with its tinted background.
pub enum BadgeKind {
    Success,
    Warning,
    Danger,
    Progress,
    Neutral,
}

/// Icon, padding and label belong to one container; callers append the label.
pub fn badge(kind: BadgeKind) -> gpui::Div {
    use gpui::{Styled, div, prelude::*};
    use gpui_component::Sizable;
    let (text, bg, icon) = match kind {
        BadgeKind::Success => (SUCCESS, SUCCESS_BG, Some(crate::icons::check_circle())),
        BadgeKind::Warning => (WARNING, WARNING_BG, Some(crate::icons::warning())),
        BadgeKind::Danger => (DANGER, DANGER_BG, Some(crate::icons::error())),
        BadgeKind::Progress => (BADGE_PROGRESS, BADGE_PROGRESS_BG, Some(crate::icons::schedule())),
        // Neutral 为纯文字片：ⓘ 专属说明文字（review2-settings#3）
        BadgeKind::Neutral => (GRAY, INSET, None),
    };
    let mut view = div()
        .flex()
        .items_center()
        .flex_shrink_0()
        .gap(px(6.))
        .min_h(px(24.))
        .px(px(8.))
        .py(px(3.))
        .rounded_full()
        .bg(color(bg))
        .text_color(color(text))
        .text_size(TEXT_AUX)
        .font_weight(gpui::FontWeight::MEDIUM)
        .whitespace_nowrap();
    if let Some(icon) = icon {
        view = view.child(icon.small());
    }
    view
}

/// A floating label for an outlined control. The control owns its border;
/// supporting status stays in normal flow below it and can grow naturally.
pub fn outlined_field_label(
    id: impl Into<gpui::ElementId>,
    label: impl Into<gpui::SharedString>,
    content: impl gpui::IntoElement,
) -> gpui::Div {
    outlined_field_label_sized(id, label, content, TEXT_AUX)
}

pub fn outlined_field_label_sized(
    id: impl Into<gpui::ElementId>,
    label: impl Into<gpui::SharedString>,
    content: impl gpui::IntoElement,
    label_size: gpui::Rems,
) -> gpui::Div {
    use gpui::{prelude::*, *};
    div().relative().w_full().min_w_0().mt(rems(8. / 14.))
        .child(content)
        .child(accessible_text(id, label)
            .absolute().top(rems(-8. / 14.)).left(rems(12. / 14.))
            .px(rems(4. / 14.))
            .bg(color(SURFACE)).text_color(color(MUTED))
            .text_size(label_size).line_height(rems(16. / 14.))
            .font_weight(FontWeight::NORMAL))
}

/// Preference switches share the accent and scale their geometry with UI text.
pub fn coral_switch(switch: gpui_component::switch::Switch) -> gpui_component::switch::Switch {
    switch
        .color(color(ACCENT_STRONG))
        .track_height(rems(20. / 14.))
}

/// Larger switches for a full-width processing panel's preference rows.
pub fn processing_switch(switch: gpui_component::switch::Switch) -> gpui_component::switch::Switch {
    coral_switch(switch).reference_geometry().track_height(rems(32. / 14.))
}

/// GPUI Component's Root resets rem size from Theme on every render. Update that
/// authority as well as the current window so parent and modal renders agree.
pub fn apply_scale(scale: f32, window: &mut Window, cx: &mut App) {
    let scale = if [1.0, 1.25, 1.5, 2.0].contains(&scale) {
        scale
    } else {
        1.0
    };
    let font_size = px(14.0 * scale);
    if Theme::global(cx).font_size != font_size {
        let theme = Theme::global_mut(cx);
        theme.font_size = font_size;
        theme.mono_font_size = px(13.0 * scale);
        Theme::sync_base(cx);
    }
    window.set_rem_size(font_size);
}
