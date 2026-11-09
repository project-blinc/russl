//! The panel below: the problems and each target's output in tabs, beside the console.

use super::{Outputs, Toy, bordered, header, icon_button, toy};
use crate::highlight::ShaderHighlighter;
use blinc_app::prelude::*;
use blinc_cn::prelude::*;
use blinc_layout::widgets::text_area::TextPosition;
use blinc_theme::{ColorToken, ThemeState};

pub(super) fn panel(t: &'static Toy, width: f32) -> impl ElementBuilder {
    let out = t.outputs;
    let view = move |pick: fn(&Outputs) -> String, lang: fn() -> ShaderHighlighter| {
        move || {
            div().w_full().h_full().child(
                scroll()
                    .w_full()
                    .h_full()
                    .direction(ScrollDirection::Vertical)
                    .child(
                        code(pick(&out.get()))
                            .w_full()
                            .font_size(12.0)
                            .line_numbers(true)
                            .syntax(SyntaxConfig::new(lang())),
                    ),
            )
        }
    };
    let tabs = bordered().child(
        stateful_with_key::<NoState>("toy-outputs")
            .deps([out.id(), t.problems.id()])
            .on_state(move |_| {
                let count = toy().problems.get().len();
                let problems_tab = if count == 0 {
                    "Problems".to_string()
                } else {
                    format!("Problems ({count})")
                };
                div().w_full().h_full().child(
                    cn::tabs(toy().out_tab)
                        .tab("problems", problems_tab, || problems(toy()))
                        .tab(
                            "wgsl",
                            "WGSL",
                            view(|o| o.wgsl.clone(), ShaderHighlighter::wgsl),
                        )
                        .tab(
                            "msl",
                            "MSL",
                            view(|o| o.msl.clone(), ShaderHighlighter::msl),
                        )
                        .tab(
                            "hlsl",
                            "HLSL",
                            view(|o| o.hlsl.clone(), ShaderHighlighter::hlsl),
                        )
                        .tab(
                            "glsl",
                            "GLSL ES",
                            view(|o| o.glsl.clone(), ShaderHighlighter::glsl),
                        ),
                )
            }),
    );
    cn::resizable_group()
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
                .child(console(t)),
        )
}

/// The problems, one row each: its severity, its message, and where it is. A click shows the
/// module and puts the caret there.
fn problems(t: &'static Toy) -> Div {
    let theme = ThemeState::get();
    let list = t.problems.get();
    let mut rows = div().w_full().h_full().py(1.0).flex_col().overflow_clip();
    if list.is_empty() {
        return rows.child(
            div().px(4.0).py(2.0).child(
                text("No problems in the sketch.")
                    .size(13.0)
                    .color(theme.color(ColorToken::TextTertiary)),
            ),
        );
    }
    let names: Vec<String> = t
        .modules
        .lock()
        .unwrap()
        .iter()
        .map(|m| m.name.clone())
        .collect();
    for problem in list.iter().take(60) {
        let (icon, tone) = if problem.error {
            (icons::CIRCLE_X, ColorToken::Error)
        } else {
            (icons::TRIANGLE_ALERT, ColorToken::Warning)
        };
        let module = names.get(problem.module).cloned().unwrap_or_default();
        let place = match problem.line {
            Some(line) => format!("{module}.russl [{line}, {}]", problem.column),
            None => format!("{module}.russl"),
        };
        let at = problem.line.map(|l| (l - 1, problem.column - 1));
        let target = problem.module;
        rows = rows.child(
            div()
                .w_full()
                .h(28.0)
                .px(4.0)
                .flex_row()
                .items_center()
                .gap(2.0)
                .cursor_pointer()
                .on_click(move |_| reveal(target, at))
                .child(cn::icon(icon).size_px(15.0).color(tone))
                .child(
                    div().flex_1().child(
                        text(problem.message.clone())
                            .size(13.0)
                            .no_wrap()
                            .color(theme.color(ColorToken::TextPrimary)),
                    ),
                )
                .child(
                    text(place)
                        .size(12.0)
                        .color(theme.color(ColorToken::TextTertiary)),
                ),
        );
    }
    rows
}

/// Show module `index`'s tab with the caret at `at` (line, column, from 0).
fn reveal(index: usize, at: Option<(usize, usize)>) {
    let t = toy();
    let found = t
        .modules
        .lock()
        .unwrap()
        .get(index)
        .map(|m| (m.name.clone(), m.editor.clone()));
    let Some((name, editor)) = found else { return };
    t.active.set(name);
    if let Some((line, column)) = at
        && let Ok(mut d) = editor.lock()
    {
        d.cursor = TextPosition { line, column };
        d.selection_start = None;
        d.ensure_cursor_visible();
    }
}

/// The console: what the toy has done, the newest line at the bottom.
fn console(t: &'static Toy) -> impl ElementBuilder {
    let lines = t.console;
    bordered()
        .flex_col()
        .child(header("Console").child(div().flex_1()).child(icon_button(
            "toy-panel-close",
            icons::X,
            "Close the panel",
            false,
            move || t.panel_open.set(false),
        )))
        .child(
            div().w_full().flex_1().child(
                stateful_with_key::<NoState>("toy-console")
                    .deps([lines.id()])
                    .on_state(move |_| {
                        let theme = ThemeState::get();
                        let mut body = div()
                            .w_full()
                            .h_full()
                            .px(3.0)
                            .py(2.0)
                            .flex_col()
                            .justify_end()
                            .gap(1.0)
                            .overflow_clip();
                        let all = lines.get();
                        for line in all.iter().skip(all.len().saturating_sub(200)) {
                            body = body.child(
                                text(line.clone())
                                    .size(12.0)
                                    .font_family(FontFamily::monospace())
                                    .color(theme.color(ColorToken::TextSecondary)),
                            );
                        }
                        body
                    }),
            ),
        )
}
