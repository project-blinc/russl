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
    /// GLSL ES 3.00 for WebGL2, one source per vertex or fragment entry point.
    pub glsl: Vec<GlslEntry>,
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
    if !module
        .functions
        .iter()
        .any(|f| f.module == 0 && f.stage.is_some())
    {
        return Ok(Artifacts {
            wgsl: String::new(),
            msl: String::new(),
            hlsl: String::new(),
            spirv: Vec::new(),
            glsl: Vec::new(),
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
    let mut artifacts = targets::emit(&naga_module, &info).map_err(|m| {
        let mut all = sink.items.clone();
        all.push(Diagnostic::error(Span::call_site(), m));
        all
    })?;
    artifacts.warnings = sink.items;
    Ok(artifacts)
}
