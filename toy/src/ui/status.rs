//! The status bar: the sketch's error and warning counts (a click opens the problems), the last
//! compile, and the toggles of the panel and the inspector.

use super::{STATUS_H, Toy, icon_button};
use blinc_app::prelude::*;
use blinc_cn::prelude::*;
use blinc_theme::{ColorToken, ThemeState};

pub(super) fn bar(t: &'static Toy) -> impl ElementBuilder {
    stateful_with_key::<NoState>("toy-status")
        .deps([
            t.problems.id(),
            t.last_ms.id(),
            t.panel_open.id(),
            t.inspector_open.id(),
        ])
        .on_state(move |_| {
            let theme = ThemeState::get();
            let problems = t.problems.get();
            let errors = problems.iter().filter(|problem| problem.error).count();
            let warnings = problems.len() - errors;
            let ms = t.last_ms.get();
            let build = if ms < 0.0 {
                "build failed: showing the last good shader".to_string()
            } else {
                format!("compiled in {ms:.1} ms")
            };
            let count = |icon: &'static str, n: usize, tone: ColorToken| {
                div()
                    .flex_row()
                    .items_center()
                    .gap(1.0)
                    .child(cn::icon(icon).size_px(14.0).color(if n > 0 {
                        tone
                    } else {
                        ColorToken::TextTertiary
                    }))
                    .child(
                        text(n.to_string())
                            .size(12.0)
                            .color(theme.color(ColorToken::TextSecondary)),
                    )
            };
            div()
                .w_full()
                .h(STATUS_H)
                .px(3.0)
                .flex_row()
                .items_center()
                .gap(3.0)
                .border_top(1.0, theme.color(ColorToken::Border))
                .child(
                    div()
                        .id("toy-status-problems")
                        .flex_row()
                        .items_center()
                        .gap(2.0)
                        .cursor_pointer()
                        .on_click(move |_| {
                            t.panel_open.set(true);
                            t.out_tab.set("problems".to_string());
                        })
                        .child(count(icons::CIRCLE_X, errors, ColorToken::Error))
                        .child(count(icons::TRIANGLE_ALERT, warnings, ColorToken::Warning)),
                )
                .child(
                    text(build)
                        .size(12.0)
                        .color(theme.color(ColorToken::TextTertiary)),
                )
                .child(div().flex_1())
                .child(icon_button(
                    "toy-toggle-panel",
                    icons::PANEL_BOTTOM,
                    "Toggle the panel",
                    t.panel_open.get(),
                    move || t.panel_open.update(|o| !o),
                ))
                .child(icon_button(
                    "toy-toggle-inspector",
                    icons::PANEL_RIGHT,
                    "Toggle the inspector",
                    t.inspector_open.get(),
                    move || t.inspector_open.update(|o| !o),
                ))
                .child(
                    text("russl")
                        .size(12.0)
                        .color(theme.color(ColorToken::TextTertiary)),
                )
        })
}
