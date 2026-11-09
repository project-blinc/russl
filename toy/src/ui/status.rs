//! The status bar: the sketch's error and warning counts (a click opens the problems), the last
//! compile, and the toggles of the panel and the inspector. The counts are computeds over the
//! problems, bound to their text.

use super::{icon_button, store};
use blinc_app::prelude::*;
use blinc_cn::prelude::*;
use blinc_core::reactive::computed;
use blinc_layout::binding::IntoReactive;

pub(super) fn bar() -> impl ElementBuilder {
    let problems = store().problems;
    // How many problems are errors (`true`) or warnings, as the count shows it.
    let count = move |icon: &'static str, errors: bool, tone: &str| {
        let n = computed(move |g| {
            let all = g.get(problems).unwrap_or_default();
            all.iter()
                .filter(|problem| problem.error == errors)
                .count()
                .to_string()
        });
        div()
            .class("toy-count")
            .flex_row()
            .items_center()
            .child(div().class(tone).child(cn::icon(icon).size_px(14.0)))
            .child(Text::bound(n.into_reactive()).class("toy-status-count"))
    };
    div()
        .class("toy-status")
        .w_full()
        .flex_row()
        .items_center()
        .child(
            div()
                .id("toy-status-problems")
                .class("toy-status-problems")
                .flex_row()
                .items_center()
                .on_click(move |_| {
                    store().panel_open.set(true);
                    store().out_tab.set("problems".to_string());
                })
                .child(count(icons::CIRCLE_X, true, "toy-error"))
                .child(count(icons::TRIANGLE_ALERT, false, "toy-warning")),
        )
        .child(Text::bound(store().build.into_reactive()).class("toy-status-text"))
        .child(div().flex_1())
        .child(icon_button(
            "toy-toggle-panel",
            icons::PANEL_BOTTOM,
            "Toggle the panel",
            move || store().panel_open.update(|o| !o),
        ))
        .child(icon_button(
            "toy-toggle-inspector",
            icons::PANEL_LEFT,
            "Toggle the inspector",
            move || store().inspector_open.update(|o| !o),
        ))
        .child(text("russl").class("toy-status-text"))
}
