//! The editor's store: the sketch, its compile, and every signal the window binds.

use crate::sketch::{self, Problem, ROOT, STARTER, Source};
use blinc_app::prelude::*;
use blinc_core::owner::Owner;
use blinc_core::reactive::signal;
use blinc_core::{FlowShader, Signal};
use russl_compiler::{BindingKind, ConstValue, Constant};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

/// What the last good compile produced, per target.
#[derive(Clone, Default, PartialEq)]
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

/// Everything the editor holds: the sketch's modules and every signal the window binds. The
/// window reads it through [`store`], in builders and in handlers alike.
pub(crate) struct EditorStore {
    pub modules: Mutex<Vec<ModuleTab>>,
    /// The modules' names, in tab order: the tabs follow it.
    pub names: Signal<Vec<String>>,
    /// The tab shown: a module's name.
    pub active: Signal<String>,
    chosen: Mutex<BTreeMap<String, ConstValue>>,
    generation: Mutex<u64>,
    /// The shader drawn: a clear one until the first good compile.
    pub shader: Signal<Arc<FlowShader>>,
    pub problems: Signal<Vec<Problem>>,
    pub constants: Signal<Vec<(String, ConstValue)>>,
    pub bindings: Signal<Vec<(String, String)>>,
    pub outputs: Signal<Outputs>,
    /// The panel's tab: problems or a target.
    pub out_tab: Signal<String>,
    pub console: Signal<Vec<String>>,
    pub build: Signal<String>,
    pub panel_open: Signal<bool>,
    pub inspector_open: Signal<bool>,
    /// The activity rail stays folded to its icons.
    pub rail_collapsed: Signal<bool>,
    start: std::time::Instant,
}

/// The editor's store.
pub(crate) fn store() -> &'static EditorStore {
    static STORE: OnceLock<EditorStore> = OnceLock::new();
    // The store lives as long as the app, so its signals belong to no build: the first call is
    // made inside one, and a rebuild disposes what the build before it made.
    STORE.get_or_init(|| Owner::detached(EditorStore::new))
}

impl EditorStore {
    fn new() -> Self {
        let s = Self {
            modules: Mutex::new(vec![ModuleTab {
                name: ROOT.to_string(),
                editor: code_editor_state(STARTER),
            }]),
            names: signal(vec![ROOT.to_string()]),
            active: signal(ROOT.to_string()),
            chosen: Mutex::new(BTreeMap::new()),
            generation: Mutex::new(0),
            shader: signal(Arc::new(
                blinc_layout::flow_shader(
                    "@flow clear { target: fragment; output color = vec4(0.0, 0.0, 0.0, 0.0); }",
                )
                .expect("the clear flow parses"),
            )),
            problems: signal(Vec::new()),
            constants: signal(Vec::new()),
            bindings: signal(Vec::new()),
            outputs: signal(Outputs::default()),
            out_tab: signal("problems".to_string()),
            console: signal(Vec::new()),
            build: signal(String::new()),
            panel_open: signal(true),
            inspector_open: signal(true),
            rail_collapsed: signal(true),
            start: std::time::Instant::now(),
        };
        s.recompile();
        s
    }

    /// Replace the sketch with `sources`, `sources[0]` its root, and compile it.
    pub(crate) fn load_sketch(&self, sources: &[Source]) {
        *self.modules.lock().unwrap() = sources
            .iter()
            .map(|s| ModuleTab {
                name: s.name.clone(),
                editor: code_editor_state(s.text.clone()),
            })
            .collect();
        if let Some(first) = sources.first() {
            self.active.set(first.name.clone());
        }
        self.names
            .set(sources.iter().map(|s| s.name.clone()).collect());
        self.recompile();
    }

    /// The most console lines kept.
    const CONSOLE_LINES: usize = 400;

    /// Add a line to the console, stamped with the time since the toy started.
    pub(crate) fn log(&self, line: String) {
        let at = self.start.elapsed().as_secs_f32();
        self.console.update(|mut lines| {
            lines.push(format!("[{at:8.2}s] {line}"));
            let excess = lines.len().saturating_sub(Self::CONSOLE_LINES);
            lines.drain(..excess);
            lines
        });
    }

    /// Compile the sketch with the chosen constants, publish what came of it, and mark each
    /// module's problems in its editor.
    pub(crate) fn recompile(&self) {
        let began = std::time::Instant::now();
        let (sources, editors): (Vec<Source>, Vec<SharedCodeEditorState>) = self
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
        let chosen: Vec<Constant> = self
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
            let mut g = self.generation.lock().unwrap();
            *g += 1;
            *g
        };
        let problems = match sketch::compile(&sources, &chosen) {
            Ok((compiled, warnings)) => {
                self.publish(&compiled.artifacts);
                set_if_changed(self.shader, compiled.shader, |a, b| a.name == b.name);
                let ms = began.elapsed().as_secs_f32() * 1000.0;
                self.build.set(format!("compiled in {ms:.1} ms"));
                self.log(format!(
                    "compiled #{generation} in {ms:.1} ms, {} warnings",
                    warnings.len()
                ));
                warnings
            }
            // The last good shader stays on screen.
            Err(problems) => {
                self.build
                    .set("build failed: showing the last good shader".to_string());
                let errors = problems.iter().filter(|problem| problem.error).count();
                let first = problems.first().map(Problem::text).unwrap_or_default();
                self.log(format!(
                    "#{generation} failed with {errors} errors: {first}"
                ));
                problems
            }
        };
        for (i, editor) in editors.iter().enumerate() {
            set_code_diagnostics(editor, Self::inline(&problems, i));
        }
        set_if_changed(self.problems, problems, |a, b| a == b);
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
    fn publish(&self, a: &russl_compiler::Artifacts) {
        let glsl = match &a.glsl_missing {
            Some(why) => format!("// No GLSL: {why}"),
            None => a
                .glsl
                .iter()
                .map(|g| format!("// {} ({:?})\n{}", g.entry_point, g.stage, g.source))
                .collect::<Vec<_>>()
                .join("\n"),
        };
        let outputs = Outputs {
            wgsl: a.wgsl.clone(),
            msl: a.msl.clone(),
            hlsl: a.hlsl.clone(),
            glsl,
        };
        set_if_changed(self.outputs, outputs, |a, b| a == b);
        set_if_changed(self.constants, a.reflection.constants.clone(), |a, b| {
            a == b
        });
        let bindings = a
            .reflection
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
            .collect();
        set_if_changed(self.bindings, bindings, |a, b| a == b);
    }

    /// A pipeline constant's new value, then a recompile.
    pub(crate) fn choose(&self, name: &str, value: ConstValue) {
        self.chosen.lock().unwrap().insert(name.to_string(), value);
        let shown = match value {
            ConstValue::Bool(b) => b.to_string(),
            ConstValue::Int(i) => i.to_string(),
            ConstValue::Float(f) => format!("{f:.2}"),
        };
        self.log(format!("{name} = {shown}"));
        self.recompile();
    }
}

/// Set `signal` to `value` unless `same` says it holds that already, so what binds it does not
/// redo its work for an edit that changed nothing it shows.
fn set_if_changed<T: Clone + Send + 'static>(
    signal: Signal<T>,
    value: T,
    same: impl Fn(&T, &T) -> bool,
) {
    if !signal.try_get().is_some_and(|now| same(&now, &value)) {
        signal.set(value);
    }
}
