//! The inspector: the sketch's pipeline constants, each with its control, and what the shader
//! binds. It folds away with its own close button or the status bar's.

use super::{Toy, bordered, choose, header, icon_button};
use blinc_app::prelude::*;
use blinc_cn::prelude::*;
use blinc_core::reactive::signal;
use blinc_theme::{ColorToken, ThemeState};
use russl_compiler::ConstValue;

/// A section's title.
fn section(title: &str) -> Div {
    let theme = ThemeState::get();
    div().w_full().px(3.0).pt(3.0).pb(1.0).child(
        text(title.to_uppercase())
            .size(11.0)
            .weight(FontWeight::SemiBold)
            .color(theme.color(ColorToken::TextTertiary)),
    )
}

/// One row: a name on the left, its control or value on the right.
fn row(name: &str, control: impl ElementBuilder + 'static) -> Div {
    let theme = ThemeState::get();
    div()
        .w_full()
        .min_h(36.0)
        .px(3.0)
        .flex_row()
        .items_center()
        .gap(2.0)
        .child(
            div().w(96.0).child(
                text(name)
                    .size(13.0)
                    .font_family(FontFamily::monospace())
                    .color(theme.color(ColorToken::TextPrimary)),
            ),
        )
        .child(div().flex_1().flex_row().justify_end().child(control))
}

pub(super) fn inspector(t: &'static Toy) -> impl ElementBuilder {
    let (constants, bindings) = (t.constants, t.bindings);
    bordered()
        .flex_col()
        .child(header("Inspector").child(div().flex_1()).child(icon_button(
            "toy-inspector-close",
            icons::X,
            "Close the inspector",
            false,
            move || t.inspector_open.set(false),
        )))
        .child(
            stateful_with_key::<NoState>("toy-inspector")
                .deps([constants.id(), bindings.id()])
                .on_state(move |_| {
                    let theme = ThemeState::get();
                    let mut body = div()
                        .w_full()
                        .flex_col()
                        .child(section("Pipeline constants"));
                    let values = constants.get();
                    if values.is_empty() {
                        body = body.child(
                            div().px(3.0).child(
                                text("None yet: declare one with #[pipeline] const.")
                                    .size(12.0)
                                    .color(theme.color(ColorToken::TextTertiary)),
                            ),
                        );
                    }
                    for (name, value) in values {
                        let n = name.clone();
                        body = body.child(match value {
                            ConstValue::Bool(on) => row(
                                &name,
                                cn::switch(signal(on))
                                    .on_change(move |v| choose(&n, ConstValue::Bool(v))),
                            ),
                            ConstValue::Float(v) => row(
                                &name,
                                cn::number_gauge(signal(v))
                                    .range(0.0..=(v * 4.0).max(1.0))
                                    .step(0.05)
                                    .format(|x| format!("{x:.2}"))
                                    .w(140.0)
                                    .on_change(move |x| choose(&n, ConstValue::Float(x))),
                            ),
                            ConstValue::Int(v) => row(
                                &name,
                                cn::number_gauge(signal(v as f64))
                                    .range(0.0..=(v as f64 * 4.0).max(8.0))
                                    .step(1.0)
                                    .format(|x| format!("{x:.0}"))
                                    .w(140.0)
                                    .on_change(move |x| {
                                        choose(&n, ConstValue::Int(x.round() as i64))
                                    }),
                            ),
                        });
                    }
                    body = body.child(cn::separator()).child(section("Bindings"));
                    for (name, what) in bindings.get() {
                        body = body.child(row(
                            &name,
                            text(what)
                                .size(12.0)
                                .color(theme.color(ColorToken::TextSecondary)),
                        ));
                    }
                    body
                }),
        )
}
