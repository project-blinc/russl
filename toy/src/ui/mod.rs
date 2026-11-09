//! The toy's window. An activity rail opens the inspector sidebar; beside it the sketch's
//! modules are tabs in the editor, next to the live preview, over the panel of problems, each
//! target's output and the console. The status bar counts problems and toggles the panels.
//!
//! Everything that changes is a signal, and the window binds to it in place: text through
//! `Text::bound`, what opens and closes through `visible`, counts and empty states through
//! computeds over the problems. Lists are `for_each` regions, so a change builds only the rows
//! it adds. The look is the stylesheet's: Rust lays out, CSS colours.

mod editor;
mod inspector;
mod panel;
mod status;
mod store;

pub(crate) use store::{ModuleTab, Outputs, store};

use blinc_app::prelude::*;
use blinc_app::windowed::WindowedContext;
use blinc_cn::prelude::*;
use blinc_core::FlowShader;
use blinc_core::reactive::computed;
use std::sync::Arc;

/// The face code is set in, bundled under its OFL licence.
pub const CODE_FONT: &[u8] = include_bytes!("../../assets/JetBrainsMono-Regular.ttf");

/// The toy's look: every colour from the theme's variables, code in JetBrains Mono.
const TOY_CSS: &str = r#"
:root { --blinc-code-font-family: "JetBrains Mono", monospace; }
.toy-root { background: var(--background); }
.toy-area { padding: 6px; }
.cn-resizable-line { background: transparent; }
.toy-panel {
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 6px;
    overflow: hidden;
}
.cn-sidebar.toy-rail { border-right: none; }
.cn-tabs-list { border-radius: 0; }
.toy-inspector {
    border-left: 1px solid var(--border);
}
.toy-header {
    height: 40px;
    padding: 0 12px;
    gap: 8px;
    border-bottom: 1px solid var(--border);
}
.toy-title { font-size: 13px; color: var(--text-secondary); }
.toy-section { padding: 12px 12px 4px 12px; }
.toy-section-title { font-size: 11px; font-weight: 600; color: var(--text-tertiary); }
.toy-row { height: 36px; padding: 0 12px; gap: 8px; }
.toy-row-name { font-size: 13px; font-family: monospace; color: var(--text-primary); }
.toy-row-value { font-size: 12px; color: var(--text-secondary); }
.toy-empty { padding: 8px 16px; font-size: 13px; color: var(--text-tertiary); }
.toy-problem { height: 28px; padding: 0 16px; gap: 8px; cursor: pointer; }
.toy-problem:hover { background: color-mix(in srgb, var(--text-primary) 6%, transparent); }
.toy-problem-message { font-size: 13px; color: var(--text-primary); }
.toy-error { color: var(--error); }
.toy-warning { color: var(--warning); }
.toy-problem-place { font-size: 12px; color: var(--text-tertiary); }
.toy-console { padding: 8px 12px; gap: 4px; }
.toy-console-line { font-size: 12px; font-family: monospace; color: var(--text-secondary); }
.toy-status {
    height: 28px;
    padding: 0 12px;
    gap: 12px;
}
.toy-status-text { font-size: 12px; color: var(--text-tertiary); }
.toy-status-problems { gap: 8px; cursor: pointer; }
.toy-count { gap: 4px; }
.toy-status-count { font-size: 12px; color: var(--text-secondary); }
.toy-icon-button {
    padding: 4px;
    border-radius: 4px;
    cursor: pointer;
    color: var(--text-secondary);
}
.toy-icon-button:hover { background: color-mix(in srgb, var(--text-primary) 8%, transparent); }
"#;

/// The theme the toy runs in: Blinc's restrained theme, with the cn components' styles and the
/// toy's rules.
pub fn theme_bundle() -> blinc_theme::ThemeBundle {
    blinc_theme::RestrainedTheme::bundle()
        .with_css(blinc_cn::cn_styles::CN_STYLES)
        .with_css(TOY_CSS)
}

/// A panel header's height.
pub(crate) const HEADER_H: f32 = 40.0;
/// The rail's width, folded to its icons (`--cn-sidebar-collapsed-width`); the side container
/// folds to it and its border.
const RAIL_W: f32 = 41.0;
/// The inspector's narrowest: the side panel is dragged no narrower, and a narrower window
/// clips it rather than squeezing its rows.
const INSPECTOR_MIN_W: f32 = 240.0;
/// The inspector sidebar's opening width.
const SIDEBAR_W: f32 = 300.0;

/// A resizable split. Its handle is a thin line, shown while hovered or dragged.
pub(crate) fn split() -> blinc_cn::ResizableGroupBuilder {
    cn::resizable_group()
        .handle_thickness(2.0)
        .handle_hit_padding(2.0)
}

/// A panel, as every region of the toy sits in (`.toy-panel`).
pub(crate) fn bordered() -> Div {
    div().class("toy-panel").w_full().h_full()
}

/// A panel's header bar: its title, and anything after it (`.toy-header`).
pub(crate) fn header(title: &str) -> Div {
    div()
        .class("toy-header")
        .w_full()
        .flex_row()
        .items_center()
        .child(text(title).class("toy-title"))
}

/// An icon that acts, with a tooltip saying what.
pub(crate) fn icon_button(
    id: &str,
    icon: &'static str,
    tip: &str,
    action: impl Fn() + Send + Sync + 'static,
) -> impl ElementBuilder {
    let id = id.to_string();
    let action = Arc::new(action);
    cn::tooltip(move || {
        let action = Arc::clone(&action);
        div()
            .id(id.clone())
            .class("toy-icon-button")
            .on_click(move |_| action())
            .child(cn::icon(icon).size_px(16.0))
    })
    .text(tip.to_string())
}

/// Replace the sketch with `sources`, `sources[0]` its root, and compile it.
pub fn load_sketch(sources: &[crate::sketch::Source]) {
    store().load_sketch(sources);
}

pub fn build_ui(ctx: &mut WindowedContext) -> impl ElementBuilder + use<> {
    let (width, height) = (ctx.width, ctx.height);
    div()
        .class("toy-root")
        .w(width)
        .h(height)
        .flex_col()
        .child(div().w_full().flex_1().child(workspace(width, height)))
        .child(status::bar())
}

/// The side container, beside the main area: the rail and, while `inspector_open`, the
/// inspector. Closed, it folds to the rail.
fn workspace(width: f32, height: f32) -> Div {
    let open = store().inspector_open;
    let folded = computed(move |g| !g.get(open).unwrap_or(true));
    div().class("toy-area").w_full().h_full().child(
        split()
            .horizontal()
            .key("toy-split-sidebar")
            .panel(
                cn::resizable_panel()
                    .id("side")
                    .default_size(RAIL_W + SIDEBAR_W)
                    .min_size(RAIL_W + INSPECTOR_MIN_W + 2.0)
                    .max_size(RAIL_W + 520.0)
                    .collapsed(folded, RAIL_W + 2.0)
                    .child(side()),
            )
            .panel(
                cn::resizable_panel()
                    .id("main")
                    .flex_grow()
                    .min_size(480.0)
                    .child(main(width - RAIL_W - SIDEBAR_W, height)),
            ),
    )
}

/// The rail and the inspector in one panel (`.toy-side`), the inspector after a divider.
fn side() -> Div {
    let rail = cn::sidebar(store().rail_collapsed)
        .class("toy-rail")
        .show_toggle(false)
        .toggle_item(
            "Inspector",
            to_svg(icons::SLIDERS_HORIZONTAL, 16.0),
            store().inspector_open,
            move || store().inspector_open.update(|o| !o),
        );
    div()
        .class("toy-panel")
        .w_full()
        .h_full()
        .flex_row()
        .child(rail)
        .child(
            div()
                .class("toy-inspector")
                .flex_1()
                .h_full()
                .min_w(INSPECTOR_MIN_W)
                .visible(store().inspector_open)
                .child(inspector::inspector()),
        )
}

/// The editor and preview, over the panel, shown while `panel_open`.
fn main(width: f32, height: f32) -> impl ElementBuilder {
    split()
        .vertical()
        .key("toy-split-vertical")
        .panel(
            cn::resizable_panel()
                .id("top")
                .flex_grow()
                .min_size(240.0)
                .child(top_area(width)),
        )
        .panel(
            cn::resizable_panel()
                .id("bottom")
                .default_size((height * 0.26).floor())
                .min_size(HEADER_H * 3.0)
                .visible(store().panel_open)
                .child(panel::panel(width)),
        )
}

/// The editor beside the preview, which takes the larger share.
fn top_area(width: f32) -> impl ElementBuilder {
    split()
        .horizontal()
        .key("toy-split-top")
        .panel(
            cn::resizable_panel()
                .id("editor")
                .default_size((width * 0.42).floor())
                .min_size(320.0)
                .child(editor::editor()),
        )
        .panel(
            cn::resizable_panel()
                .id("preview")
                .flex_grow()
                .min_size(320.0)
                .child(bordered().child(preview(store().shader))),
        )
}

/// The preview: the shader of the last good compile, which the canvas follows in place.
fn preview(shader: Signal<Arc<FlowShader>>) -> impl ElementBuilder {
    canvas(|_, _| {}).w_full().h_full().flow(shader)
}
