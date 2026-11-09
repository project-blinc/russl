//! The panel below: the problems and each target's output in tabs, beside the console. The
//! problems and the console are `for_each` regions over their signals, and each output is a
//! code block bound to its text.

use super::{Outputs, bordered, header, icon_button, split, store};
use crate::highlight::ShaderHighlighter;
use crate::sketch::Problem;
use blinc_app::prelude::*;
use blinc_cn::prelude::*;
use blinc_core::reactive::computed;
use blinc_layout::widgets::text_area::TextPosition;

pub(super) fn panel(width: f32) -> impl ElementBuilder {
    let tabs = bordered().child(
        cn::tabs(store().out_tab)
            .tab("problems", "Problems", problems)
            .tab(
                "wgsl",
                "WGSL",
                output(|o| o.wgsl.clone(), ShaderHighlighter::wgsl),
            )
            .tab(
                "msl",
                "MSL",
                output(|o| o.msl.clone(), ShaderHighlighter::msl),
            )
            .tab(
                "hlsl",
                "HLSL",
                output(|o| o.hlsl.clone(), ShaderHighlighter::hlsl),
            )
            .tab(
                "glsl",
                "GLSL ES",
                output(|o| o.glsl.clone(), ShaderHighlighter::glsl),
            ),
    );
    split()
        .horizontal()
        .key("toy-split-bottom")
        .panel(
            cn::resizable_panel()
                .id("outputs")
                .flex_grow()
                .min_size(320.0)
                .child(tabs),
        )
        .panel(
            cn::resizable_panel()
                .id("console")
                .default_size((width * 0.32).floor())
                .min_size(220.0)
                .child(console()),
        )
}

/// One target's output, highlighted: the block follows the text in place, rebuilding the lines
/// a compile changed.
fn output(
    pick: fn(&Outputs) -> String,
    lang: fn() -> ShaderHighlighter,
) -> impl Fn() -> Div + Send + Sync + 'static {
    move || {
        let out = store().outputs;
        let text = computed(move |g| g.get(out).map(|o| pick(&o)).unwrap_or_default());
        div().w_full().h_full().child(
            scroll()
                .w_full()
                .h_full()
                .direction(ScrollDirection::Vertical)
                .child(
                    Code::bound(text)
                        .font_size(12.0)
                        .line_numbers(true)
                        .syntax(SyntaxConfig::new(lang()))
                        .w_full(),
                ),
        )
    }
}

/// The problems, one row each: its severity, its message, and where it is. A click shows the
/// module and puts the caret there.
fn problems() -> Div {
    let problems = store().problems;
    div()
        .w_full()
        .h_full()
        .flex_col()
        .overflow_clip()
        .child(
            div()
                .class("toy-empty")
                .visible(computed(move |g| {
                    g.get(problems).unwrap_or_default().is_empty()
                }))
                .child(text("No problems in the sketch.")),
        )
        .child(div().w_full().flex_col().for_each(
            problems,
            |problem: &Problem| problem.clone(),
            move |problem: Problem| problem_row(problem),
        ))
}

fn problem_row(problem: Problem) -> Div {
    let (icon, tone) = if problem.error {
        (icons::CIRCLE_X, "toy-error")
    } else {
        (icons::TRIANGLE_ALERT, "toy-warning")
    };
    let module = store()
        .modules
        .lock()
        .unwrap()
        .get(problem.module)
        .map(|m| m.name.clone())
        .unwrap_or_default();
    let place = match problem.line {
        Some(line) => format!("{module}.russl [{line}, {}]", problem.column),
        None => format!("{module}.russl"),
    };
    let at = problem.line.map(|l| (l - 1, problem.column - 1));
    let target = problem.module;
    div()
        .class("toy-problem")
        .w_full()
        .flex_row()
        .items_center()
        .on_click(move |_| reveal(target, at))
        .child(div().class(tone).child(cn::icon(icon).size_px(15.0)))
        .child(
            div()
                .flex_1()
                .child(text(problem.message).class("toy-problem-message").no_wrap()),
        )
        .child(text(place).class("toy-problem-place"))
}

/// Show module `index`'s tab with the caret at `at` (line, column, from 0).
fn reveal(index: usize, at: Option<(usize, usize)>) {
    let found = store()
        .modules
        .lock()
        .unwrap()
        .get(index)
        .map(|m| (m.name.clone(), m.editor.clone()));
    let Some((name, editor)) = found else { return };
    store().show(&name);
    if let Some((line, column)) = at
        && let Ok(mut d) = editor.lock()
    {
        d.cursor = TextPosition { line, column };
        d.selection_start = None;
        d.ensure_cursor_visible();
    }
}

/// The console: what the toy has done, the newest line at the bottom.
fn console() -> impl ElementBuilder {
    bordered()
        .flex_col()
        .child(header("Console").child(div().flex_1()).child(icon_button(
            "toy-panel-close",
            icons::X,
            "Close the panel",
            move || store().panel_open.set(false),
        )))
        .child(
            div()
                .class("toy-console")
                .w_full()
                .flex_1()
                .flex_col()
                .justify_end()
                .overflow_clip()
                .for_each(
                    store().console,
                    |line: &String| line.clone(),
                    |line: String| text(line).class("toy-console-line"),
                ),
        )
}
