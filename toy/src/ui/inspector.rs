//! The inspector, beside the rail in the side container: the sketch's pipeline constants, each with its control, and what the
//! shader binds. Both are `for_each` regions: a constant keeps its row, and its control, across
//! compiles.

use super::{header, icon_button, store};
use blinc_app::prelude::*;
use blinc_cn::prelude::*;
use blinc_core::reactive::signal;
use russl_compiler::ConstValue;

/// A section's title (`.toy-section-title`).
fn section(title: &str) -> Div {
    div()
        .class("toy-section")
        .w_full()
        .child(text(title.to_uppercase()).class("toy-section-title"))
}

/// One row: a name on the left, its control or value on the right (`.toy-row`).
fn row(name: &str, control: impl ElementBuilder + 'static) -> Div {
    div()
        .class("toy-row")
        .w_full()
        .flex_row()
        .items_center()
        .child(
            div()
                .flex_1()
                .min_w(0.0)
                .child(text(name).class("toy-row-name")),
        )
        .child(div().flex_shrink_0().child(control))
}

/// A constant's row and the control that sets it.
fn constant(name: String, value: ConstValue) -> Div {
    let n = name.clone();
    match value {
        ConstValue::Bool(on) => row(
            &name,
            cn::switch(signal(on))
                .size(SwitchSize::Small)
                .on_change(move |v| store().choose(&n, ConstValue::Bool(v))),
        ),
        ConstValue::Float(v) => row(
            &name,
            cn::number_gauge(signal(v))
                .range(0.0..=(v * 4.0).max(1.0))
                .step(0.05)
                .format(|x| format!("{x:.2}"))
                .size(GaugeSize::Small)
                .on_change(move |x| store().choose(&n, ConstValue::Float(x))),
        ),
        ConstValue::Int(v) => row(
            &name,
            cn::number_gauge(signal(v as f64))
                .range(0.0..=(v as f64 * 4.0).max(8.0))
                .step(1.0)
                .format(|x| format!("{x:.0}"))
                .size(GaugeSize::Small)
                .on_change(move |x| store().choose(&n, ConstValue::Int(x.round() as i64))),
        ),
    }
}

pub(super) fn inspector() -> impl ElementBuilder {
    div()
        .w_full()
        .h_full()
        .flex_col()
        .child(header("Inspector").child(div().flex_1()).child(icon_button(
            "toy-inspector-close",
            icons::X,
            "Close the inspector",
            move || store().inspector_open.set(false),
        )))
        .child(section("Pipeline constants"))
        .child(div().w_full().flex_col().for_each(
            store().constants,
            |(name, _): &(String, ConstValue)| name.clone(),
            |(name, value): (String, ConstValue)| constant(name, value),
        ))
        .child(cn::separator())
        .child(section("Bindings"))
        .child(div().w_full().flex_col().for_each(
            store().bindings,
            |b: &(String, String)| b.clone(),
            |(name, what): (String, String)| row(&name, text(what).class("toy-row-value")),
        ))
}
