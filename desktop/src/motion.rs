//! Small, shared motion vocabulary: entrances, retargetable values and live work.
use crate::activity::{TransferEta, TransferMetrics};
use crate::theme::{
    ACCENT, GRAY, INK, SECONDARY_CONTAINER, TEXT_AUX, TEXT_BODY, accessible_text,
    color,
};
use gpui::{prelude::*, *};
use gpui_component::{Sizable, h_flex, v_flex};
use std::time::Duration;

pub const ENTER_MS: u64 = 160;
pub const VALUE_MS: u64 = 200;
/// 调色板切换过渡时长（theme::apply_preference 用，纳入同一运动词汇表）
pub const PALETTE_MS: u64 = 280;

pub fn ease_out(t: f32) -> f32 {
    1. - (1. - t.clamp(0., 1.)).powi(3)
}

/// IDs belong to a logical state, not a frame or a progress value.
pub fn enter<E: IntoElement + Styled + 'static>(id: impl Into<ElementId>, view: E) -> AnyElement {
    // Keep the same ancestor ID chain when the preference changes. GPUI's
    // AnimationElement already paints the final state without scheduling frames
    // under reduce-motion; removing it here remounts every keyed child.
    view.with_animation(
        id,
        Animation::new(Duration::from_millis(ENTER_MS))
            .with_easing(ease_out)
            .with_max_fps(60.),
        |view, t| view.opacity(t),
    )
    .into_any_element()
}

/// A change of task state settles into place, so the new title and available
/// action read as a consequence of the user's previous action.
pub fn state_enter<E: IntoElement + Styled + 'static>(
    id: impl Into<ElementId>,
    view: E,
) -> AnyElement {
    let id = id.into();
    #[cfg(feature = "performance")]
    let trace_id = id.clone();
    view.with_animation(
        id,
        // 与 Show/dismiss overlay 同一 starting range（150-220ms，states-and-motion.md）
        Animation::new(Duration::from_millis(200))
            .with_easing(ease_out)
            .with_max_fps(60.),
        move |view, t| {
            #[cfg(feature = "performance")]
            crate::performance::record_motion(trace_id.clone(), 1., t);
            view.relative().top(px(8. * (1. - t))).opacity(t)
        },
    )
    .into_any_element()
}

pub fn spinner(id: impl Into<ElementId>, cx: &App) -> AnyElement {
    let icon = crate::icons::loader_circle()
        .small()
        .text_color(color(ACCENT));
    if cx.reduce_motion() {
        return icon.into_any_element();
    }
    icon.with_animation(
        id,
        Animation::new(Duration::from_millis(1000))
            .repeat()
            .with_max_fps(60.),
        |icon, t| icon.rotate(radians(std::f32::consts::TAU * t)),
    )
    .into_any_element()
}

/// Unmeasured work uses motion, never a fabricated completion percentage.
pub fn indeterminate_progress(id: impl Into<ElementId>, cx: &App) -> AnyElement {
    let segment = div().absolute().h_full().w(relative(0.3)).rounded_full().bg(color(ACCENT));
    let segment = if cx.reduce_motion() {
        segment.left(relative(0.35)).into_any_element()
    } else {
        segment.with_animation(id, Animation::new(Duration::from_millis(1400)).repeat().with_max_fps(60.),
            |segment, t| segment.left(relative(t * 1.3 - 0.3))).into_any_element()
    };
    div().relative().w_full().h(px(3.)).rounded_full().overflow_hidden()
        .bg(color(SECONDARY_CONTAINER)).child(segment).into_any_element()
}

pub fn value(id: impl Into<ElementId>, target: f32, window: &mut Window, cx: &mut App) -> f32 {
    let id = id.into();
    #[cfg(feature = "performance")]
    let trace_id = id.clone();
    let value = gpui_base::transition(
        id,
        target,
        gpui_base::Transition::new(Duration::from_millis(VALUE_MS)).ease(ease_out),
        window,
        cx,
    );
    #[cfg(feature = "performance")]
    crate::performance::record_motion(trace_id, target, value);
    value
}

/// Selection starts and ends at rest; no overshoot or abrupt launch.
pub fn selection_value(
    id: impl Into<ElementId>,
    target: f32,
    window: &mut Window,
    cx: &mut App,
) -> f32 {
    let id = id.into();
    #[cfg(feature = "performance")]
    let trace_id = id.clone();
    let amount = gpui_base::transition(
        id,
        target,
        gpui_base::Transition::new(Duration::from_millis(180)).ease(|t| t * t * (3. - 2. * t)),
        window,
        cx,
    );
    #[cfg(feature = "performance")]
    crate::performance::record_motion(trace_id, target, amount);
    amount
}

pub fn progress(
    id: impl Into<ElementId>,
    progress: f32,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let target = if progress.is_finite() {
        progress.clamp(0., 1.)
    } else {
        0.
    };
    let amount = value(id, target, window, cx);
    div()
        .w_full()
        .h(px(6.))
        .rounded_full()
        .overflow_hidden()
        .bg(color(SECONDARY_CONTAINER))
        .child(
            div()
                .h_full()
                .w(relative(amount))
                .rounded_full()
                .bg(color(ACCENT)),
        )
        .into_any_element()
}

/// Model downloads keep quantity, labeled speed and labeled remaining time
/// on one stable block so the numbers do not hide behind a single sentence.
pub fn transfer_status(
    id: impl Into<ElementId>,
    title: impl Into<SharedString>,
    metrics: &TransferMetrics,
    fraction: Option<f32>,
    window: &mut Window,
    cx: &mut App,
) -> Div {
    let id = id.into();
    let title = title.into();
    let mut view = v_flex().w_full().min_w_0().gap_2().child(
        accessible_text(SharedString::from(format!("{id:?}-title")), title)
            .text_size(TEXT_BODY)
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(color(INK)),
    );
    if !metrics.quantity.is_empty() {
        view = view.child(
            accessible_text(
                SharedString::from(format!("{id:?}-quantity")),
                metrics.quantity.clone(),
            )
            .text_size(TEXT_BODY)
            .text_color(color(INK)),
        );
    }
    if metrics.speed.is_some() || matches!(&metrics.eta, Some(TransferEta::Remaining(_))) {
        let mut meters = h_flex().w_full().min_w_0().gap_6().flex_wrap();
        if let Some(speed) = &metrics.speed {
            meters = meters.child(transfer_meter(
                SharedString::from(format!("{id:?}-speed")),
                "速度",
                speed.clone(),
            ));
        }
        if let Some(TransferEta::Remaining(value)) = &metrics.eta {
            meters = meters.child(transfer_meter(
                SharedString::from(format!("{id:?}-eta")),
                "预计剩余",
                value.clone(),
            ));
        }
        view = view.child(meters);
    }
    if let Some(TransferEta::Note(value)) = &metrics.eta {
        view = view.child(
            accessible_text(
                SharedString::from(format!("{id:?}-eta-note")),
                value.clone(),
            )
            .text_size(TEXT_AUX)
            .text_color(color(GRAY)),
        );
    }
    if let Some(note) = &metrics.note {
        view = view.child(
            accessible_text(SharedString::from(format!("{id:?}-note")), note.clone())
                .text_size(TEXT_AUX)
                .text_color(color(GRAY)),
        );
    }
    view.when_some(fraction, |view, fraction| {
        view.child(progress(
            SharedString::from(format!("{id:?}-bar")),
            fraction,
            window,
            cx,
        ))
    })
}

fn transfer_meter(
    id: impl Into<SharedString>,
    label: &'static str,
    value: impl Into<SharedString>,
) -> Div {
    let id = id.into();
    v_flex()
        .min_w(rems(7.))
        .gap_1()
        .child(
            accessible_text(SharedString::from(format!("{id}-label")), label)
                .text_size(TEXT_AUX)
                .text_color(color(GRAY)),
        )
        .child(
            accessible_text(SharedString::from(format!("{id}-value")), value)
                .text_size(TEXT_BODY)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(color(INK)),
        )
}

pub fn disclosure(id: impl Into<ElementId>, open: bool, content: Div) -> AnyElement {
    if open {
        // Keep intrinsic measurement, padding and child layout in the normal
        // tree. Measuring in prepaint and clipping to a previous frame's height
        // cuts off controls when the content or available width changes.
        enter(id, content)
    } else {
        div().hidden().into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{disclosure, ease_out, enter, value};
    use gpui::{
        App, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _, Pixels,
        Render, RenderOnce, Styled as _, TestAppContext, VisualTestContext, Window, div, px, size,
    };
    use std::{cell::Cell, rc::Rc, time::Duration};

    #[derive(IntoElement)]
    struct StatefulChild {
        target: f32,
        mounts: Rc<Cell<usize>>,
        sample: Rc<Cell<f32>>,
    }

    impl RenderOnce for StatefulChild {
        fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
            window.use_keyed_state("motion-child-state", cx, |_, _| {
                self.mounts.set(self.mounts.get() + 1);
            });
            self.sample
                .set(value("motion-child-value", self.target, window, cx));
            div().size(px(40.))
        }
    }

    struct PreferenceHarness {
        target: f32,
        mounts: Rc<Cell<usize>>,
        sample: Rc<Cell<f32>>,
    }

    impl Render for PreferenceHarness {
        fn render(&mut self, _: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            enter(
                "motion-preference-parent",
                div().child(StatefulChild {
                    target: self.target,
                    mounts: self.mounts.clone(),
                    sample: self.sample.clone(),
                }),
            )
        }
    }

    #[gpui::test]
    fn reduce_motion_preserves_child_state_and_later_value_transitions(cx: &mut TestAppContext) {
        let mounts = Rc::new(Cell::new(0));
        let sample = Rc::new(Cell::new(-1.));
        let (view, cx) = cx.add_window_view({
            let mounts = mounts.clone();
            let sample = sample.clone();
            move |_, _| PreferenceHarness {
                target: 0.,
                mounts,
                sample,
            }
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert_eq!(mounts.get(), 1);
        assert_eq!(sample.get(), 0.);

        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.target = 1.;
                cx.set_reduce_motion(true);
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
        assert_eq!(mounts.get(), 1, "preference must not remount the child");
        assert_eq!(
            sample.get(),
            1.,
            "reduced motion paints the target immediately"
        );

        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.target = 0.;
                cx.set_reduce_motion(false);
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
        assert_eq!(mounts.get(), 1);
        assert_eq!(
            sample.get(),
            1.,
            "restored motion retains the previous target"
        );
        cx.executor().advance_clock(Duration::from_millis(50));
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
        assert!(
            sample.get() > 0. && sample.get() < 1.,
            "the next change must paint an intermediate value"
        );
        assert_eq!(mounts.get(), 1);
    }

    struct RevealHarness {
        open: bool,
        width: Pixels,
        content_height: Pixels,
    }

    impl Render for RevealHarness {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let content = div()
                .debug_selector(|| "motion-reveal-surface".into())
                .flex()
                .flex_col()
                .p(px(16.))
                .child(
                    div()
                        .debug_selector(|| "motion-reveal-content".into())
                        .h(self.content_height),
                );
            let reveal = crate::motion::disclosure("geometry-reveal", self.open, content);
            div()
                .debug_selector(|| "motion-layout-root".into())
                .flex()
                .flex_col()
                .w(self.width)
                .gap(px(8.))
                .child(div().h(px(20.)))
                .child(reveal)
                .child(
                    div()
                        .debug_selector(|| "motion-layout-footer".into())
                        .h(px(20.)),
                )
        }
    }

    fn draw_open_reveal(
        view: &Entity<RevealHarness>,
        cx: &mut VisualTestContext,
        width: Pixels,
        content_height: Pixels,
    ) {
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.open = true;
                view.width = width;
                view.content_height = content_height;
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
    }

    fn assert_reveal_geometry(cx: &mut VisualTestContext, width: Pixels, height: Pixels) {
        let root = cx.debug_bounds("motion-layout-root").unwrap();
        let surface = cx.debug_bounds("motion-reveal-surface").unwrap();
        let content = cx.debug_bounds("motion-reveal-content").unwrap();
        let footer = cx.debug_bounds("motion-layout-footer").unwrap();
        assert_eq!(surface.left(), root.left());
        assert_eq!(surface.top(), root.top() + px(28.));
        assert_eq!(surface.size, size(width, height + px(32.)));
        assert_eq!(content.top() - surface.top(), px(16.));
        assert_eq!(surface.bottom() - content.bottom(), px(16.));
        assert_eq!(footer.top() - surface.bottom(), px(8.));
        assert!(footer.bottom() <= root.bottom());
    }

    #[gpui::test]
    fn disclosure_first_frame_preserves_natural_size_and_padding(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, _| RevealHarness {
            open: false,
            width: px(240.),
            content_height: px(40.),
        });
        draw_open_reveal(&view, cx, px(240.), px(40.));
        assert_reveal_geometry(cx, px(240.), px(40.));
    }

    #[gpui::test]
    fn disclosure_content_resize_updates_following_rows_in_the_same_frame(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, _| RevealHarness {
            open: false,
            width: px(240.),
            content_height: px(40.),
        });
        draw_open_reveal(&view, cx, px(240.), px(40.));
        draw_open_reveal(&view, cx, px(180.), px(96.));
        assert_reveal_geometry(cx, px(180.), px(96.));
    }

    #[gpui::test]
    fn disclosure_closes_without_leaving_height_or_children(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, _| RevealHarness {
            open: false,
            width: px(240.),
            content_height: px(40.),
        });
        draw_open_reveal(&view, cx, px(240.), px(40.));
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.open = false;
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
        let root = cx.debug_bounds("motion-layout-root").unwrap();
        let footer = cx.debug_bounds("motion-layout-footer").unwrap();
        assert_eq!(footer.top(), root.top() + px(28.));
        assert!(cx.debug_bounds("motion-reveal-surface").is_none());
        assert!(cx.debug_bounds("motion-reveal-content").is_none());
    }

    #[test]
    fn easing_finishes_exactly_and_preserves_forward_motion() {
        assert_eq!(ease_out(0.), 0.);
        assert_eq!(ease_out(1.), 1.);
        let samples = (0..=100)
            .map(|i| ease_out(i as f32 / 100.))
            .collect::<Vec<_>>();
        assert!(samples.windows(2).all(|p| p[0] <= p[1]));
    }
}
