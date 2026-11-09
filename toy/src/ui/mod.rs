//! The toy's window. The sketch's modules are tabs in the editor, beside the live preview and
//! the inspector of its pipeline constants; problems, each target's output and the console sit
//! in the panel below; the status bar counts problems and opens and closes the panels. Every
//! edit recompiles; a sketch with errors keeps the last good picture and marks them inline.

mod editor;
mod inspector;
mod panel;
mod status;

use crate::sketch::{self, Problem, ROOT, STARTER, Source};
use blinc_app::prelude::*;
use blinc_app::windowed::WindowedContext;
use blinc_cn::prelude::*;
use blinc_core::reactive::signal;
use blinc_core::{FlowShader, Signal};
use blinc_theme::{ColorToken, ThemeState};
use russl_compiler::{BindingKind, ConstValue, Constant};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

/// The face code is set in, bundled under its OFL licence.
pub const CODE_FONT: &[u8] = include_bytes!("../../assets/JetBrainsMono-Regular.ttf");

/// The toy's own rules: code in JetBrains Mono.
const TOY_CSS: &str = r#":root { --blinc-code-font-family: "JetBrains Mono", monospace; }"#;

/// What the last good compile produced, per target.
#[derive(Clone, Default)]
pub(crate) struct Outputs {
    pub wgsl: String,
    pub msl: String,
    pub hlsl: String,
    pub glsl: String,
}

/// One module of the sketch: its tab's name and its editor, which holds its text.
pub(crate) struct ModuleTab {
    pub name: String,
    pub editor: SharedCodeEditorState,
}

/// The toy's state, and the signals the window reads.
pub(crate) struct Toy {
    pub modules: Mutex<Vec<ModuleTab>>,
    /// Bumped when a module is added, so the tabs are built again.
    pub modules_rev: Signal<u64>,
    /// The tab shown: a module's name.
    pub active: Signal<String>,
    chosen: Mutex<BTreeMap<String, ConstValue>>,
    generation: Mutex<u64>,
    pub shader: Signal<Option<Arc<FlowShader>>>,
    pub problems: Signal<Vec<Problem>>,
    pub constants: Signal<Vec<(String, ConstValue)>>,
    pub bindings: Signal<Vec<(String, String)>>,
    pub outputs: Signal<Outputs>,
    /// The panel's tab: problems or a target.
    pub out_tab: Signal<String>,
    pub console: Signal<Vec<String>>,
    /// The last compile: milliseconds, or a negative number when it failed.
    pub last_ms: Signal<f32>,
    pub panel_open: Signal<bool>,
    pub inspector_open: Signal<bool>,
    /// The panel's height, as its splitter leaves it.
    pub bottom_h: Signal<f32>,
    pub window_h: Signal<f32>,
    start: std::time::Instant,
}

pub(crate) fn toy() -> &'static Toy {
    static TOY: OnceLock<Toy> = OnceLock::new();
    TOY.get_or_init(|| {
        let t = Toy {
            modules: Mutex::new(vec![ModuleTab {
                name: ROOT.to_string(),
                editor: code_editor_state(STARTER),
            }]),
            modules_rev: signal(0),
            active: signal(ROOT.to_string()),
            chosen: Mutex::new(BTreeMap::new()),
            generation: Mutex::new(0),
            shader: signal(None),
            problems: signal(Vec::new()),
            constants: signal(Vec::new()),
            bindings: signal(Vec::new()),
            outputs: signal(Outputs::default()),
            out_tab: signal("problems".to_string()),
            console: signal(Vec::new()),
            last_ms: signal(0.0),
            panel_open: signal(true),
            inspector_open: signal(true),
            bottom_h: signal(0.0),
            window_h: signal(0.0),
            start: std::time::Instant::now(),
        };
        recompile(&t);
        t
    })
}

/// Replace the sketch with `sources`, `sources[0]` its root, and compile it.
pub fn load_sketch(sources: &[Source]) {
    let t = toy();
    *t.modules.lock().unwrap() = sources
        .iter()
        .map(|s| ModuleTab {
            name: s.name.clone(),
            editor: code_editor_state(s.text.clone()),
        })
        .collect();
    if let Some(first) = sources.first() {
        t.active.set(first.name.clone());
    }
    t.modules_rev.update(|r| r + 1);
    recompile(t);
}

/// The most console lines kept.
const CONSOLE_LINES: usize = 400;

/// Add a line to the console, stamped with the time since the toy started.
pub(crate) fn log(t: &Toy, line: String) {
    let at = t.start.elapsed().as_secs_f32();
    t.console.update(|mut lines| {
        lines.push(format!("[{at:8.2}s] {line}"));
        let excess = lines.len().saturating_sub(CONSOLE_LINES);
        lines.drain(..excess);
        lines
    });
}

/// Compile the sketch with the chosen constants, publish what came of it, and mark each
/// module's problems in its editor.
pub(crate) fn recompile(t: &Toy) {
    let began = std::time::Instant::now();
    let (sources, editors): (Vec<Source>, Vec<SharedCodeEditorState>) = t
        .modules
        .lock()
        .unwrap()
        .iter()
        .map(|m| {
            let text = m.editor.lock().unwrap().value();
            (
                Source {
                    name: m.name.clone(),
                    text,
                },
                m.editor.clone(),
            )
        })
        .unzip();
    let chosen: Vec<Constant> = t
        .chosen
        .lock()
        .unwrap()
        .iter()
        .map(|(name, value)| Constant {
            name: name.clone(),
            value: *value,
            span: proc_macro2::Span::call_site(),
        })
        .collect();
    let generation = {
        let mut g = t.generation.lock().unwrap();
        *g += 1;
        *g
    };
    let problems = match sketch::compile(&sources, &chosen, generation) {
        Ok((compiled, warnings)) => {
            publish(t, &compiled.artifacts);
            t.shader.set(Some(compiled.shader));
            let ms = began.elapsed().as_secs_f32() * 1000.0;
            t.last_ms.set(ms);
            log(
                t,
                format!(
                    "compiled #{generation} in {ms:.1} ms, {} warnings",
                    warnings.len()
                ),
            );
            warnings
        }
        // The last good shader stays on screen.
        Err(problems) => {
            t.last_ms.set(-1.0);
            let errors = problems.iter().filter(|problem| problem.error).count();
            let first = problems.first().map(Problem::text).unwrap_or_default();
            log(
                t,
                format!("#{generation} failed with {errors} errors: {first}"),
            );
            problems
        }
    };
    for (i, editor) in editors.iter().enumerate() {
        set_code_diagnostics(editor, inline(&problems, i));
    }
    t.problems.set(problems);
}

/// Module `module`'s problems, as its editor marks them.
fn inline(problems: &[Problem], module: usize) -> Vec<CodeDiagnostic> {
    problems
        .iter()
        .filter(|problem| problem.module == module)
        .filter_map(|problem| {
            let line = problem.line?;
            let start = problem.column - 1;
            let end = if problem.end.0 == line {
                problem.end.1 - 1
            } else {
                usize::MAX
            };
            let severity = if problem.error {
                DiagnosticSeverity::Error
            } else {
                DiagnosticSeverity::Warning
            };
            Some(CodeDiagnostic::new(
                line - 1,
                start,
                end.max(start + 1),
                severity,
                problem.message.clone(),
            ))
        })
        .collect()
}

/// What the inspector and panel show of a good compile.
fn publish(t: &Toy, a: &russl_compiler::Artifacts) {
    let glsl = match &a.glsl_missing {
        Some(why) => format!("// No GLSL: {why}"),
        None => a
            .glsl
            .iter()
            .map(|g| format!("// {} ({:?})\n{}", g.entry_point, g.stage, g.source))
            .collect::<Vec<_>>()
            .join("\n"),
    };
    t.outputs.set(Outputs {
        wgsl: a.wgsl.clone(),
        msl: a.msl.clone(),
        hlsl: a.hlsl.clone(),
        glsl,
    });
    t.constants.set(a.reflection.constants.clone());
    t.bindings.set(
        a.reflection
            .bindings
            .iter()
            .map(|b| {
                let kind = match b.kind {
                    BindingKind::Uniform => "uniform",
                    BindingKind::Storage { .. } => "storage",
                    BindingKind::Handle => "texture",
                };
                let size = b.size.map(|s| format!(" · {s} B")).unwrap_or_default();
                (
                    b.name.clone(),
                    format!("{}:{} · {kind}{size}", b.group, b.binding),
                )
            })
            .collect(),
    );
}

/// A pipeline constant's new value, then a recompile.
pub(crate) fn choose(name: &str, value: ConstValue) {
    let t = toy();
    t.chosen.lock().unwrap().insert(name.to_string(), value);
    let shown = match value {
        ConstValue::Bool(b) => b.to_string(),
        ConstValue::Int(i) => i.to_string(),
        ConstValue::Float(f) => format!("{f:.2}"),
    };
    log(t, format!("{name} = {shown}"));
    recompile(t);
}

/// The status bar's height.
pub(crate) const STATUS_H: f32 = 28.0;
/// A panel header's height.
pub(crate) const HEADER_H: f32 = 40.0;
/// Space around the window's edge.
const EDGE: f32 = 2.0;

/// A bordered region, as every panel sits in.
pub(crate) fn bordered() -> Div {
    let theme = ThemeState::get();
    div()
        .w_full()
        .h_full()
        .bg(theme.color(ColorToken::Surface))
        .border(1.0, theme.color(ColorToken::Border))
        .rounded(6.0)
        .overflow_clip()
}

/// A panel's header bar: its title, and anything after it.
pub(crate) fn header(title: &str) -> Div {
    let theme = ThemeState::get();
    div()
        .w_full()
        .h(HEADER_H)
        .px(3.0)
        .flex_row()
        .items_center()
        .gap(2.0)
        .border_bottom(1.0, theme.color(ColorToken::Border))
        .child(
            text(title)
                .size(13.0)
                .color(theme.color(ColorToken::TextSecondary)),
        )
}

/// An icon that acts: a toggle or a command, with a tooltip saying what.
pub(crate) fn icon_button(
    id: &str,
    icon: &'static str,
    tip: &str,
    on: bool,
    action: impl Fn() + Send + Sync + 'static,
) -> impl ElementBuilder {
    let (id, tip) = (id.to_string(), tip.to_string());
    let action = Arc::new(action);
    cn::tooltip(move || {
        let action = Arc::clone(&action);
        div()
            .id(id.clone())
            .p(1.0)
            .rounded(4.0)
            .cursor_pointer()
            .on_click(move |_| action())
            .child(cn::icon(icon).size_px(16.0).color(if on {
                ColorToken::TextPrimary
            } else {
                ColorToken::TextTertiary
            }))
    })
    .text(tip)
}

/// The editor's height: the window less the status bar, the panel when open, the tab header and
/// the edges.
pub(crate) fn editor_height(t: &Toy) -> f32 {
    let panel = if t.panel_open.get() {
        t.bottom_h.get() + 4.0
    } else {
        0.0
    };
    (t.window_h.get() - STATUS_H - panel - HEADER_H - EDGE * 4.0).max(120.0)
}

pub fn build_ui(ctx: &mut WindowedContext) -> impl ElementBuilder + use<> {
    if ctx.rebuild_count == 0 {
        ctx.add_css(TOY_CSS);
    }
    let (width, height) = (ctx.width, ctx.height);
    let t = toy();
    if t.bottom_h.get() <= 0.0 {
        t.bottom_h.set((height * 0.34).floor());
    }
    if t.window_h.get() != height {
        t.window_h.set(height);
    }
    stateful_with_key::<NoState>("toy-root")
        .deps([t.panel_open.id(), t.inspector_open.id()])
        .on_state(move |_| {
            let theme = ThemeState::get();
            let main: Div = if t.panel_open.get() {
                div().w_full().h_full().child(
                    cn::resizable_group()
                        .vertical()
                        .key("toy-split-vertical")
                        .on_resize(|sizes| {
                            if let Some(&bottom) = sizes.get(1) {
                                toy().bottom_h.set(bottom);
                            }
                        })
                        .panel(
                            cn::resizable_panel()
                                .id("top")
                                .flex_grow()
                                .min_size(240.0)
                                .child(top_area(t, width)),
                        )
                        .panel(
                            cn::resizable_panel()
                                .id("bottom")
                                .default_size(t.bottom_h.get())
                                .min_size(HEADER_H * 3.0)
                                .child(panel::panel(t, width)),
                        ),
                )
            } else {
                div().w_full().h_full().child(top_area(t, width))
            };
            div()
                .w(width)
                .h(height)
                .bg(theme.color(ColorToken::Background))
                .flex_col()
                .child(div().w_full().flex_1().p(EDGE).child(main))
                .child(status::bar(t))
        })
}

/// The editor beside the preview, and the inspector when open, split by splitters.
fn top_area(t: &'static Toy, width: f32) -> impl ElementBuilder {
    let mut group = cn::resizable_group()
        .horizontal()
        .key(if t.inspector_open.get() {
            "toy-split-top"
        } else {
            "toy-split-top-closed"
        })
        .panel(
            cn::resizable_panel()
                .id("editor")
                .default_size((width * 0.48).floor())
                .min_size(320.0)
                .child(editor::editor(t)),
        )
        .panel(
            cn::resizable_panel()
                .id("preview")
                .flex_grow()
                .min_size(240.0)
                .child(bordered().child(preview(t.shader))),
        );
    if t.inspector_open.get() {
        group = group.panel(
            cn::resizable_panel()
                .id("inspector")
                .default_size(290.0)
                .min_size(220.0)
                .child(inspector::inspector(t)),
        );
    }
    group
}

fn preview(shader: Signal<Option<Arc<FlowShader>>>) -> impl ElementBuilder {
    stateful_with_key::<NoState>("toy-preview")
        .deps([shader.id()])
        .on_state(move |_| {
            let frame = div().w_full().h_full().overflow_clip();
            match shader.get() {
                Some(s) => frame.child(canvas(|_, _| {}).w_full().h_full().flow(s)),
                None => frame,
            }
        })
}
