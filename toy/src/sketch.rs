//! A sketch compiled for the preview. The toy's extension gives every sketch its frame uniform
//! and the `Frag` it shades; the toy appends the vertex stage that covers the preview, after the
//! sketch's own lines so their numbers stay the sketch's.

use blinc_core::{FlowShader, FlowTarget};
use proc_macro2::TokenStream;
use russl_compiler::{
    Artifacts, Constant, Diagnostic, Extension, Module, ProgramView, Severity, Stage,
};
use std::borrow::Cow;
use std::sync::Arc;

/// What every sketch sees. `Toy` is the uniform block Blinc's flow pipeline fills, field for
/// field.
pub const PRELUDE: &str = r#"
/// The frame: `bounds.zw` is the preview's size in pixels, `pointer` the mouse over it from 0 to
/// 1, `time` seconds since the toy started.
pub struct Toy {
    pub viewport: Vec2,
    pub time: f32,
    pub frame: f32,
    pub bounds: Vec4,
    pub pointer: Vec2,
    pub corner_radius: f32,
    pub _padding: f32,
}
#[group(0)] #[binding(0)] pub static TOY: Uniform<Toy>;

/// One fragment of the preview: `uv` runs from 0 at the top left to 1 at the bottom right.
pub struct Frag {
    #[builtin(position)] pub position: Vec4,
    #[location(0)] pub uv: Vec2,
}
"#;

/// The preview's two triangles, appended after the sketch.
const VERTEX: &str = r#"
const TOY_CORNERS: [u32; 6] = [0, 1, 2, 2, 1, 3];

#[vertex]
fn vs_main(#[builtin(vertex_index)] vi: u32) -> Frag {
    let corner = TOY_CORNERS[vi];
    let x = (corner & 1) as f32;
    let y = (corner >> 1) as f32;
    Frag { position: Vec4::new(x * 2.0 - 1.0, y * 2.0 - 1.0, 0.0, 1.0), uv: Vec2::new(x, 1.0 - y) }
}
"#;

/// The sketch a new toy opens with.
pub const STARTER: &str = r#"// Edit me: the preview redraws as you type.
#[pipeline] const RINGS: bool = true;
#[pipeline] const SPEED: f32 = 1.0;

fn palette(t: f32) -> Vec3 {
    Vec3::new(0.5, 0.5, 0.5) + Vec3::splat(0.5) * (Vec3::new(t, t + 0.33, t + 0.67) * 6.2832).cos()
}

#[fragment]
#[location(0)]
fn fs_main(input: Frag) -> Vec4 {
    let size = TOY.bounds.zw;
    let p = (input.uv - 0.5) * size / size.y;
    let m = (TOY.pointer - 0.5) * size / size.y;
    let d = (p - m).length();
    let t = TOY.time * SPEED;
    let wave = if RINGS { (d * 24.0 - t * 4.0).sin() * 0.5 + 0.5 } else { 1.0 - d };
    Vec4::new(palette(d + t * 0.1) * wave, 1.0)
}
"#;

struct ToyExtension;

impl Extension for ToyExtension {
    fn name(&self) -> &str {
        "toy"
    }

    fn prelude(&self) -> Option<TokenStream> {
        Some(PRELUDE.parse().expect("the prelude parses"))
    }

    fn check(&self, program: &ProgramView, report: &mut Vec<Diagnostic>) {
        for b in program.bindings.iter().filter(|b| b.name != "TOY") {
            report.push(Diagnostic::error(
                b.span,
                format!(
                    "the preview binds only `TOY`; `{}` has nothing behind it",
                    b.name
                ),
            ));
        }
        let fragment = program
            .entry_points
            .iter()
            .any(|e| e.name == "fs_main" && e.stage == Stage::Fragment);
        if !fragment {
            report.push(Diagnostic::error(
                proc_macro2::Span::call_site(),
                "a sketch shades the preview with `#[fragment] #[location(0)] fn fs_main(input: Frag) -> Vec4`",
            ));
        }
    }
}

/// A compiled sketch: the shader the preview draws, and every target's output.
pub struct Compiled {
    pub shader: Arc<FlowShader>,
    pub artifacts: Artifacts,
}

/// One problem in the sketch.
#[derive(Clone, Debug, PartialEq)]
pub struct Problem {
    /// The module it is in, by its index among the sketch's modules.
    pub module: usize,
    /// From 1; `None` for one about the sketch as a whole.
    pub line: Option<usize>,
    /// From 1.
    pub column: usize,
    /// Where the span ends: the line (from 1) and column (from 1, one past its last character).
    pub end: (usize, usize),
    pub message: String,
    pub error: bool,
}

impl Problem {
    pub fn text(&self) -> String {
        let kind = if self.error { "error" } else { "warning" };
        match self.line {
            Some(line) => format!("{line}:{}: {kind}: {}", self.column, self.message),
            None => format!("{kind}: {}", self.message),
        }
    }
}

/// One module of a sketch: its name, which other modules `use` it by (`use crate::name::f;`),
/// and its text.
#[derive(Clone, Debug)]
pub struct Source {
    pub name: String,
    pub text: String,
}

/// The first module is the sketch's root, which holds `fs_main`.
pub const ROOT: &str = "main";

/// Compile `sources`, `sources[0]` the root, with values for some of the root's pipeline
/// constants. `generation` names the shader, as Blinc caches pipelines by name.
pub fn compile(
    sources: &[Source],
    constants: &[Constant],
    generation: u64,
) -> Result<(Compiled, Vec<Problem>), Vec<Problem>> {
    let lines: Vec<usize> = sources
        .iter()
        .map(|s| s.text.lines().count().max(1))
        .collect();
    let mut modules = Vec::with_capacity(sources.len());
    // What `Span::file` reports for each module's tokens: how a diagnostic finds its module.
    let mut files = Vec::with_capacity(sources.len());
    for (i, src) in sources.iter().enumerate() {
        let text = if i == 0 {
            format!("{}\n{VERTEX}", src.text)
        } else {
            src.text.clone()
        };
        let tokens: TokenStream = text.parse().map_err(|e: proc_macro2::LexError| {
            let (a, b) = (e.span().start(), e.span().end());
            vec![Problem {
                module: i,
                line: Some(a.line),
                column: a.column + 1,
                end: (b.line, b.column + 1),
                message: e.to_string(),
                error: true,
            }]
        })?;
        files.push(tokens.clone().into_iter().next().map(|t| t.span().file()));
        modules.push(Module {
            source: tokens,
            imports: Vec::new(),
        });
    }
    // `use crate::name::..`, `super::name` or `self::name`: every module sits at the root.
    for m in &mut modules {
        let paths = russl_compiler::use_paths(m.source.clone()).unwrap_or_default();
        for (key, _) in paths {
            let name = key.rsplit("::").next().unwrap_or(&key);
            if let Some(i) = sources.iter().position(|s| s.name == name) {
                m.imports.push((key, i));
            }
        }
    }
    let problem = |d: &Diagnostic| {
        let key = d.span.file();
        let module = files
            .iter()
            .position(|f| f.as_deref() == Some(key.as_str()))
            .unwrap_or(0);
        let (a, b) = (d.span.start(), d.span.end());
        let inside = a.line >= 1 && a.line <= lines[module];
        Problem {
            module,
            line: inside.then_some(a.line),
            column: a.column + 1,
            end: (b.line, b.column + 1),
            message: d.message.clone(),
            error: d.severity == Severity::Error,
        }
    };
    match russl_compiler::compile_program_ext(&modules, constants, &ToyExtension) {
        Ok(artifacts) => {
            let warnings = artifacts
                .warnings
                .iter()
                .map(problem)
                .filter(|p| p.line.is_some())
                .collect();
            let shader = Arc::new(FlowShader {
                name: Cow::Owned(format!("russl_toy_{generation}")),
                target: FlowTarget::Fragment,
                wgsl: Cow::Owned(artifacts.wgsl.clone()),
                needs_scene: false,
                animated: true,
                dynamic_fields: 0,
                buffers: Cow::Borrowed(&[]),
            });
            Ok((Compiled { shader, artifacts }, warnings))
        }
        Err(d) => {
            // Errors first: they are what stops the preview.
            let mut problems: Vec<Problem> = d.iter().map(problem).collect();
            problems.sort_by_key(|p| !p.error);
            Err(problems)
        }
    }
}
