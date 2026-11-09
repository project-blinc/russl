//! The russl compiler.
//!
//! Source in Rust syntax goes through a front end of its own: identifiers are interned, types
//! live in a hash-consed arena, names resolve to ids, and an analyzer types every expression
//! and checks the module's rules. The checked module is lowered straight to a [`naga::Module`],
//! validated, and written for every target: WGSL, MSL, HLSL, SPIR-V and GLSL ES. The macro, the
//! build driver and the CLI all call [`compile`].

mod builtins;
mod check;
mod diag;
mod front;
mod hir;
mod intern;
mod lower;
#[cfg(feature = "ariadne")]
mod render;
mod targets;
mod types;

pub use diag::{Diagnostic, Severity};
#[cfg(feature = "ariadne")]
pub use render::render;

use proc_macro2::{Span, TokenStream};

/// What a compiled shader module produces: one artifact per target, so no platform compiles
/// shaders at run time, and the warnings found on the way.
#[derive(Debug, Clone)]
pub struct Artifacts {
    /// WGSL, for WebGPU (browsers accept nothing else) and wgpu.
    pub wgsl: String,
    /// Metal Shading Language, for macOS and iOS.
    pub msl: String,
    /// HLSL, for Direct3D 12.
    pub hlsl: String,
    /// SPIR-V words, for Vulkan.
    pub spirv: Vec<u32>,
    /// GLSL ES 3.00 for WebGL2, one source per vertex or fragment entry point. Empty when the
    /// module needs what WebGL2 lacks, such as storage buffers; `glsl_missing` names it.
    pub glsl: Vec<GlslEntry>,
    pub glsl_missing: Option<String>,
    pub warnings: Vec<Diagnostic>,
    /// The module has no entry points: a library for other modules to import. It was checked,
    /// and no target was written.
    pub library: bool,
}

/// One module of a program: its source, and for each module path its `use` items name, the
/// index of that module among the program's modules.
pub struct Module {
    pub source: TokenStream,
    pub imports: Vec<(String, usize)>,
}

/// One entry point written as GLSL ES.
#[derive(Debug, Clone)]
pub struct GlslEntry {
    pub entry_point: String,
    pub stage: Stage,
    pub source: String,
}

/// A shader stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Vertex,
    Fragment,
    Compute,
}

/// A value given for one of the root module's pipeline constants.
#[derive(Clone, Debug)]
pub struct Constant {
    pub name: String,
    pub value: ConstValue,
    /// Where the value was written, for errors.
    pub span: Span,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum ConstValue {
    Bool(bool),
    Int(i64),
    Float(f64),
}

/// Compile one module of russl source that imports nothing.
pub fn compile(source: TokenStream) -> Result<Artifacts, Vec<Diagnostic>> {
    compile_program(&[Module {
        source,
        imports: Vec::new(),
    }])
}

/// The module paths a module's `use` items name, each once: a key (as [`Module::imports`]
/// expects it) and the path's tokens.
pub fn use_paths(source: TokenStream) -> Result<Vec<(String, TokenStream)>, Diagnostic> {
    let file: syn::File =
        syn::parse2(source).map_err(|e| Diagnostic::error(e.span(), e.to_string()))?;
    Ok(front::use_paths(&file))
}

/// Compile a program: `modules[0]` is the module being compiled, the others what it imports.
/// On failure, every error and warning found.
pub fn compile_program(modules: &[Module]) -> Result<Artifacts, Vec<Diagnostic>> {
    compile_program_with(modules, &[])
}

/// Compile a program with values for some of its root's pipeline constants: the artifacts are
/// the root as those values specialise it. The rest keep their defaults.
pub fn compile_program_with(
    modules: &[Module],
    constants: &[Constant],
) -> Result<Artifacts, Vec<Diagnostic>> {
    let mut files = Vec::with_capacity(modules.len());
    for m in modules {
        files.push(
            syn::parse2::<syn::File>(m.source.clone())
                .map_err(|e| vec![Diagnostic::error(e.span(), e.to_string())])?,
        );
    }
    let sources: Vec<front::Source<'_>> = files
        .iter()
        .zip(modules)
        .map(|(file, m)| front::Source {
            file,
            imports: &m.imports,
        })
        .collect();
    let mut names = intern::Interner::default();
    let mut types = types::Types::default();
    let mut spans = diag::Spans::default();
    let mut sink = diag::Sink::default();

    let mut module = front::program(&sources, &mut names, &mut types, &mut spans, &mut sink);
    if sink.has_errors() {
        return Err(sink.items);
    }
    let analysis = check::module(&names, &mut types, &mut module, &spans, &mut sink);
    if sink.has_errors() {
        return Err(sink.items);
    }
    let values = specialize(&module, &names, &types, constants, &mut sink);
    if sink.has_errors() {
        return Err(sink.items);
    }
    if !module
        .functions
        .iter()
        .any(|f| f.module == 0 && f.stage.is_some())
    {
        if let Some(c) = constants.first() {
            sink.error(c.span, "only a module with entry points can be specialised");
            return Err(sink.items);
        }
        return Ok(Artifacts {
            wgsl: String::new(),
            msl: String::new(),
            hlsl: String::new(),
            spirv: Vec::new(),
            glsl: Vec::new(),
            glsl_missing: None,
            warnings: sink.items,
            library: true,
        });
    }
    let naga_module = lower::module(&module, &analysis, &types, &names, &spans, &mut sink);
    if sink.has_errors() {
        return Err(sink.items);
    }

    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&naga_module)
    .map_err(|e| {
        let span = e
            .spans()
            .find_map(|(s, _)| spans.for_naga(*s))
            .unwrap_or_else(Span::call_site);
        let mut message = e.as_inner().to_string();
        let mut source = std::error::Error::source(e.as_inner());
        while let Some(s) = source {
            message.push_str(": ");
            message.push_str(&s.to_string());
            source = s.source();
        }
        let mut all = sink.items.clone();
        all.push(Diagnostic::error(
            span,
            format!("invalid shader: {message}"),
        ));
        all
    })?;
    let mut artifacts = targets::emit(&naga_module, &info, &values).map_err(|m| {
        let mut all = sink.items.clone();
        all.push(Diagnostic::error(Span::call_site(), m));
        all
    })?;
    artifacts.warnings = sink.items;
    Ok(artifacts)
}

/// Check each given value against the root's pipeline constants, and key it the way naga looks
/// it up: by the constant's id if it has one, else by name.
fn specialize(
    module: &hir::Module,
    names: &intern::Interner,
    types: &types::Types,
    constants: &[Constant],
    sink: &mut diag::Sink,
) -> naga::back::PipelineConstants {
    use types::{Scalar, Type};
    let mut out = naga::back::PipelineConstants::default();
    for c in constants {
        let found = module
            .consts
            .iter()
            .find(|k| k.module == 0 && names.get(k.name) == c.name);
        let Some(k) = found else {
            sink.error(
                c.span,
                format!("the specialised module has no constant `{}`", c.name),
            );
            continue;
        };
        let Some(id) = k.pipeline else {
            sink.error(
                c.span,
                format!(
                    "`{}` is not a `#[pipeline]` constant, so it cannot be specialised",
                    c.name
                ),
            );
            continue;
        };
        let value = match (types.get(k.ty), c.value) {
            (Type::Scalar(Scalar::Bool), ConstValue::Bool(b)) => Some(if b { 1.0 } else { 0.0 }),
            (Type::Scalar(Scalar::I32), ConstValue::Int(v)) if i32::try_from(v).is_ok() => {
                Some(v as f64)
            }
            (Type::Scalar(Scalar::U32), ConstValue::Int(v)) if u32::try_from(v).is_ok() => {
                Some(v as f64)
            }
            (Type::Scalar(Scalar::F32 | Scalar::F16), ConstValue::Int(v)) => Some(v as f64),
            (Type::Scalar(Scalar::F32 | Scalar::F16), ConstValue::Float(v)) => Some(v),
            _ => None,
        };
        match value {
            Some(v) => {
                let key = id.map(|n| n.to_string()).unwrap_or_else(|| c.name.clone());
                if out.insert(key, v).is_some() {
                    sink.error(c.span, format!("`{}` is given twice", c.name));
                }
            }
            None => {
                let want = types.name(k.ty, names);
                sink.error(
                    c.span,
                    format!("`{}` is `{want}`; this value does not fit", c.name),
                );
            }
        }
    }
    out
}
