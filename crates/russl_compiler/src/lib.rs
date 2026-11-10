//! The russl compiler.
//!
//! Source in Rust syntax goes through a front end of its own: identifiers are interned, types
//! live in a hash-consed arena, names resolve to ids, and an analyzer types every expression
//! and checks the module's rules. The checked module is lowered straight to a [`naga::Module`],
//! validated, and written for every target: WGSL, MSL, HLSL, SPIR-V and GLSL ES. The macro calls
//! [`compile_program`] with the modules rustc delivered; the build driver and the CLI read a
//! [`Program`] from files.

mod builtins;
mod check;
mod diag;
mod extension;
mod files;
mod front;
mod hir;
mod intern;
mod lower;
mod native;
mod preprocess;
mod reflect;
#[cfg(feature = "ariadne")]
mod render;
mod spirv;
mod targets;
mod types;

pub use diag::{Diagnostic, Severity};
pub use extension::{
    BindingKind, BindingView, Builtin, ConstView, Emit, EntryView, Extension, NoExtension,
    ProgramView,
};
pub use files::{Program, SourceFile};
pub use native::{
    GlslBindings, GlslTexture, HlslBinding, HlslSlot, MslEntry, MslResource, MslSlot,
    NativeBindings,
};
pub use preprocess::all_branches;
pub use reflect::{
    BindingLayout, FieldLayout, HostScalar, HostType, Reflection, StructImport, StructLayout,
    VertexInput, VertexInputs,
};
#[cfg(feature = "ariadne")]
pub use render::{render, render_program};

use proc_macro2::{Span, TokenStream};

/// What a compiled shader module produces: one artifact per target, so no platform compiles
/// shaders at run time, and the warnings found on the way.
#[derive(Debug, Clone)]
#[non_exhaustive]
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
    /// What the host needs to know: struct layouts, bindings, the pipeline constants as built,
    /// vertex inputs.
    pub reflection: Reflection,
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
/// expects it) and the path's tokens, with the spans they were written with.
pub fn use_paths(source: TokenStream) -> Result<Vec<(String, TokenStream)>, Diagnostic> {
    // An import in any branch of a conditional is an import: the branch is chosen when the whole
    // program is compiled.
    let source = preprocess::all_branches(source)?;
    let file: syn::File =
        syn::parse2(source).map_err(|e| Diagnostic::error(e.span(), e.to_string()))?;
    Ok(front::use_paths(&file)
        .into_iter()
        .map(|(key, path)| (key, quote::quote! { #(#path)::* }))
        .collect())
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
    compile_program_ext(modules, constants, &NoExtension)
}

/// Compile a program with a framework's [`Extension`]: its prelude seen by every module, its
/// extern functions linked, its builtins, and its rules over the checked program.
pub fn compile_program_ext(
    modules: &[Module],
    constants: &[Constant],
    ext: &dyn Extension,
) -> Result<Artifacts, Vec<Diagnostic>> {
    let mut sink = diag::Sink::default();
    let mut files = Vec::with_capacity(modules.len() + 1);
    let mut imports: Vec<Vec<(String, usize)>> = Vec::with_capacity(modules.len() + 1);
    // `#if` reads the program's pipeline constants, the same in every module.
    let sources: Vec<&TokenStream> = modules.iter().map(|m| &m.source).collect();
    let defines = preprocess::defines(&sources, constants).map_err(|d| vec![d])?;
    for m in modules {
        let source = preprocess::apply(m.source.clone(), &defines).map_err(|d| vec![d])?;
        let mut file = syn::parse2::<syn::File>(source)
            .map_err(|e| vec![Diagnostic::error(e.span(), e.to_string())])?;
        extension::link(&mut file, ext, &mut sink);
        files.push(file);
        imports.push(m.imports.clone());
    }
    if let Some(prelude) = ext.prelude() {
        let mut file = syn::parse2::<syn::File>(prelude).map_err(|e| {
            vec![Diagnostic::error(
                e.span(),
                format!("the prelude of the extension `{}`: {e}", ext.name()),
            )]
        })?;
        extension::link(&mut file, ext, &mut sink);
        let at = files.len();
        let glob: syn::Item =
            syn::parse_str(&format!("use {}::*;", extension::PRELUDE)).expect("a use item");
        for (f, i) in files.iter_mut().zip(&mut imports) {
            f.items.insert(0, glob.clone());
            i.push((extension::PRELUDE.to_string(), at));
        }
        files.push(file);
        imports.push(Vec::new());
    }
    if sink.has_errors() {
        return Err(sink.items);
    }
    let sources: Vec<front::Source<'_>> = files
        .iter()
        .zip(&imports)
        .map(|(file, imports)| front::Source { file, imports })
        .collect();
    let mut names = intern::Interner::default();
    let mut types = types::Types::default();
    let mut spans = diag::Spans::default();
    let builtins = ext.builtins();

    let (mut module, mut generics) = front::program(
        &sources, &mut names, &mut types, &mut spans, &mut sink, &builtins,
    );
    if sink.has_errors() {
        return Err(sink.items);
    }
    let analysis = check::module(
        &mut names,
        &mut types,
        &mut module,
        &mut spans,
        &mut sink,
        &mut generics,
    );
    if sink.has_errors() {
        return Err(sink.items);
    }
    let view = extension::view(&module, &analysis, &types, &names, &spans);
    ext.check(&view, &mut sink.items);
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
            reflection: reflect::program(&module, &analysis.emitted, &types, &names),
        });
    }
    let naga_module = lower::module(
        &module, &analysis, &types, &names, &spans, &mut sink, &builtins,
    );
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
    let built = std::mem::take(&mut artifacts.reflection.constants);
    let native = std::mem::take(&mut artifacts.reflection.native);
    artifacts.reflection = reflect::program(&module, &analysis.emitted, &types, &names);
    artifacts.reflection.constants = built;
    artifacts.reflection.native = native;
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
