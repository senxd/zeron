//! Selection toolbar, mini composer, numbered bubbles, and the composer chip
//! for transcript annotations.

use gpui::{
    AnyElement, Bounds, Context, Entity, MouseButton, Pixels, Point, SharedString, Window, div,
    prelude::*, px,
};

use crate::annotations::{self, TranscriptAnnotation};
use crate::composer::ComposerInput;
use crate::theme::Theme;

pub(crate) fn toolbar<T: 'static>(
    origin: Point<Pixels>,
    theme: &Theme,
    cx: &Context<T>,
    add: impl Fn(&mut T, &mut Window, &mut Context<T>) + 'static,
) -> AnyElement {
    let button = div()
        .id("annotation-add-to-chat")
        .h(px(28.0))
        .px(px(10.0))
        .flex()
        .items_center()
        .rounded(px(8.0))
        .text_size(px(12.0))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(theme.text)
        .cursor_pointer()
        .hover(|s| s.bg(crate::theme::ink(0.08)))
        .child("Add to chat")
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(cx.listener(move |this, _, window, cx| {
            cx.stop_propagation();
            add(this, window, cx);
        }));
    floating(
        origin,
        gpui::Anchor::BottomLeft,
        div()
            .occlude()
            .mb(px(8.0))
            .child(crate::popover::popover_card(theme).p(px(2.0)).child(button))
            .into_any_element(),
    )
}

pub(crate) fn mini_composer<T: 'static>(
    origin: Point<Pixels>,
    input: Entity<ComposerInput>,
    theme: &Theme,
    cx: &Context<T>,
    dismiss: impl Fn(&mut T, &mut Context<T>) + 'static,
    delete: impl Fn(&mut T, &mut Context<T>) + 'static,
) -> AnyElement {
    let dismiss_escape = dismiss;
    let delete_click = delete;
    floating(
        origin,
        gpui::Anchor::BottomLeft,
        div()
            .occlude()
            .mb(px(8.0))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    cx.stop_propagation();
                    dismiss_escape(this, cx);
                }
            }))
            .child(
                div()
                    .w(px(280.0))
                    .h(px(36.0))
                    .flex()
                    .items_center()
                    .px(px(14.0))
                    .rounded(px(18.0))
                    .bg(theme.surface_overlay)
                    .border_1()
                    .border_color(theme.border)
                    .shadow_lg()
                    .overflow_hidden()
                    .text_size(px(13.0))
                    .text_color(theme.text)
                    .child(input.into_any_element())
                    .child(
                        div()
                            .id("annotation-delete")
                            .role(gpui::Role::Button)
                            .aria_label("Delete annotation")
                            .mr(px(8.0))
                            .size(px(18.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                delete_click(this, cx);
                            }))
                            .child(
                                crate::icons::icon(crate::icons::CLOSE)
                                    .size(px(11.0))
                                    .text_color(theme.text_muted),
                            ),
                    ),
            )
            .into_any_element(),
    )
}

pub(crate) fn number_bubble<T: 'static>(
    origin: Point<Pixels>,
    index: usize,
    id: SharedString,
    theme: &Theme,
    cx: &Context<T>,
    open: impl Fn(&mut T, &mut Window, &mut Context<T>) + 'static,
) -> AnyElement {
    let label = index.to_string();
    floating(
        origin,
        gpui::Anchor::BottomLeft,
        div()
            .id(id)
            .mb(px(2.0))
            .ml(px(-6.0))
            .relative()
            .size(px(22.0))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                open(this, window, cx);
            }))
            .child(
                crate::icons::icon(crate::icons::CHAT_ROUND_FILL)
                    .absolute()
                    .inset_0()
                    .size(px(22.0))
                    .text_color(theme.accent),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme.on_accent)
                    .child(label),
            )
            .into_any_element(),
    )
}

pub(crate) fn composer_chip<T: 'static>(
    annotations: &[TranscriptAnnotation],
    theme: &Theme,
    cx: &Context<T>,
    clear: impl Fn(&mut T, &mut Context<T>) + 'static,
) -> gpui::Div {
    let count = annotations.len();
    let badge = crate::badges::MessageBadge {
        icon: crate::icons::CHAT_ROUND_LINE,
        label: annotations::chip_label(count).into(),
        details: annotations::badge_details(annotations),
    };
    div().flex().flex_row().items_center().child(
        crate::badges::render("composer-annotations", &badge, theme).child(
            div()
                .id("composer-annotations-clear")
                .ml(px(2.0))
                .size(px(16.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .cursor_pointer()
                .hover(|s| s.bg(crate::theme::ink(0.10)))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    clear(this, cx);
                }))
                .child(
                    crate::icons::icon(crate::icons::CLOSE)
                        .size(px(10.0))
                        .text_color(theme.text_muted),
                ),
        ),
    )
}

fn floating(origin: Point<Pixels>, anchor: gpui::Anchor, child: AnyElement) -> AnyElement {
    gpui::deferred(
        gpui::anchored()
            .position(origin)
            .anchor(anchor)
            .snap_to_window_with_margin(px(8.0))
            .child(child),
    )
    .into_any_element()
}

pub(crate) fn bounds_origin(bounds: Bounds<Pixels>) -> Point<Pixels> {
    bounds.origin
}

pub(crate) fn bounds_end(bounds: Bounds<Pixels>) -> Point<Pixels> {
    gpui::point(bounds.right(), bounds.origin.y)
}
