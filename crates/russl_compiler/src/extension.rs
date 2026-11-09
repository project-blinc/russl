//! Extensions: what a framework adds to russl for its own shaders without changing russl.
//!
//! - A prelude: a library module whose public items every module of a program sees, as a
//!   `use prelude::*` would bring them in. A module's own items shadow them.
//! - Extern functions: a shader declares `extern { fn aux(i: u32) -> Vec4; }` and the
//!   extension supplies the definition, so one shader links against per-backend or per-tier
//!   implementations.
//! - Builtins the extension lowers to naga itself, for what russl source cannot say.
//! - Rules over the checked program: its bindings, entry points and pipeline constants.
//!
//! A framework's macro passes its extension to `russl_expand`; `russl_build` takes one too.

use crate::Diagnostic;
use proc_macro2::{Span, TokenStream};

pub trait Extension {
    /// The extension's name, for messages: `blinc`.
    fn name(&self) -> &str;

    /// russl source of the prelude every module sees, if any.
    fn prelude(&self) -> Option<TokenStream> {
        None
    }

    /// The definition of the extern function `name`: a russl `fn` item with the signature the
    /// shader declares, or `None` when the extension has none by that name.
    fn extern_fn(&self, name: &str) -> Option<TokenStream> {
        let _ = name;
        None
    }

    /// Builtins the extension lowers itself, called as free functions.
    fn builtins(&self) -> Vec<Builtin> {
        Vec::new()
    }

    /// Rules over the checked program; push an error or warning for each breach.
    fn check(&self, program: &ProgramView, report: &mut Vec<Diagnostic>) {
        let _ = (program, report);
    }
}

/// No extension: what `russl!` compiles with.
pub struct NoExtension;

impl Extension for NoExtension {
    fn name(&self) -> &str {
        "russl"
    }
}

/// A builtin an extension lowers to naga.
pub struct Builtin {
    pub name: String,
    /// Parameter types, as russl spells them: `"Vec2"`, `"u32"`.
    pub params: Vec<String>,
    /// The result type, as russl spells it.
    pub ret: String,
    /// Build the result from the arguments' expressions.
    #[allow(clippy::type_complexity)]
    pub lower: Box<
        dyn Fn(&mut Emit<'_>, &[naga::Handle<naga::Expression>]) -> naga::Handle<naga::Expression>,
    >,
}

/// Where a builtin's lowering appends its expressions, in the function being lowered.
pub struct Emit<'a> {
    pub(crate) append: &'a mut dyn FnMut(naga::Expression) -> naga::Handle<naga::Expression>,
}

impl Emit<'_> {
    pub fn append(&mut self, e: naga::Expression) -> naga::Handle<naga::Expression> {
        (self.append)(e)
    }
}

/// What a checked program offers an extension's rules.
#[derive(Debug, Clone)]
pub struct ProgramView {
    /// The resources the program binds: the root's and those its entry points reach.
    pub bindings: Vec<BindingView>,
    pub entry_points: Vec<EntryView>,
    /// The root module's constants.
    pub constants: Vec<ConstView>,
}

#[derive(Debug, Clone)]
pub struct BindingView {
    pub name: String,
    pub group: u32,
    pub binding: u32,
    pub kind: BindingKind,
    /// The bound type as russl spells it, without its address space: `Globals`, `[Primitive]`.
    pub ty: String,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingKind {
    Uniform,
    Storage {
        writable: bool,
    },
    /// A texture or a sampler.
    Handle,
}

#[derive(Debug, Clone)]
pub struct EntryView {
    pub name: String,
    pub stage: crate::Stage,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct ConstView {
    pub name: String,
    pub ty: String,
    pub pipeline: bool,
    pub span: Span,
}

/// The module name the prelude takes among a program's modules.
pub(crate) const PRELUDE: &str = "__russl_prelude";

/// Replace each `extern { fn .. ; }` block with the extension's definitions. A definition must
/// have the declared signature, parameter names aside; it keeps the declaration's visibility.
pub(crate) fn link(file: &mut syn::File, ext: &dyn Extension, sink: &mut crate::diag::Sink) {
    use quote::ToTokens;
    use syn::spanned::Spanned;
    let mut items = Vec::with_capacity(file.items.len());
    for item in std::mem::take(&mut file.items) {
        let syn::Item::ForeignMod(block) = item else {
            items.push(item);
            continue;
        };
        if let Some(abi) = block.abi.name.as_ref().filter(|n| n.value() != "russl") {
            sink.error(
                abi.span(),
                "an extern block is `extern { .. }` or `extern \"russl\" { .. }`",
            );
            continue;
        }
        for foreign in block.items {
            let syn::ForeignItem::Fn(decl) = foreign else {
                sink.error(foreign.span(), "an extern block declares functions only");
                continue;
            };
            let name = decl.sig.ident.to_string();
            let Some(tokens) = ext.extern_fn(&name) else {
                let message = if ext.name() == NoExtension.name() {
                    format!("`{name}` is extern, and no extension is in use to define it")
                } else {
                    format!(
                        "`{name}` is extern, and the extension `{}` does not define it",
                        ext.name()
                    )
                };
                sink.error(decl.sig.ident.span(), message);
                continue;
            };
            let mut def = match syn::parse2::<syn::ItemFn>(tokens) {
                Ok(d) => d,
                Err(e) => {
                    sink.error(
                        decl.sig.ident.span(),
                        format!("the extension `{}` defines `{name}` as something other than a function: {e}", ext.name()),
                    );
                    continue;
                }
            };
            // Signatures match by their types; parameter names are the definition's own.
            let shape = |sig: &syn::Signature| {
                let inputs: Vec<String> = sig
                    .inputs
                    .iter()
                    .map(|i| match i {
                        syn::FnArg::Typed(p) => p.ty.to_token_stream().to_string(),
                        syn::FnArg::Receiver(r) => r.to_token_stream().to_string(),
                    })
                    .collect();
                (
                    sig.ident.to_string(),
                    inputs,
                    sig.output.to_token_stream().to_string(),
                )
            };
            if shape(&def.sig) != shape(&decl.sig) {
                sink.error(
                    decl.sig.span(),
                    format!(
                        "the extension `{}` defines `{name}` as `{}`, not as declared here",
                        ext.name(),
                        def.sig.to_token_stream()
                    ),
                );
                continue;
            }
            def.vis = decl.vis;
            def.attrs.extend(decl.attrs);
            items.push(syn::Item::Fn(def));
        }
    }
    file.items = items;
}

/// What the extension's rules see of a checked program.
pub(crate) fn view(
    module: &crate::hir::Module,
    analysis: &crate::check::Analysis,
    types: &crate::types::Types,
    names: &crate::intern::Interner,
    spans: &crate::diag::Spans,
) -> ProgramView {
    use crate::hir::{Space, Stage as S};
    let bindings = analysis
        .emitted
        .iter()
        .map(|g| &module.globals[g.index()])
        .filter_map(|g| {
            let (group, binding) = g.binding?;
            let kind = match g.space {
                Space::Uniform => BindingKind::Uniform,
                Space::Storage { writable } => BindingKind::Storage { writable },
                Space::Handle => BindingKind::Handle,
                Space::Private | Space::Workgroup => return None,
            };
            Some(BindingView {
                name: names.get(g.name).to_string(),
                group,
                binding,
                kind,
                ty: types.name(g.ty, names),
                span: spans.get(g.span),
            })
        })
        .collect();
    let entry_points = module
        .functions
        .iter()
        .filter(|f| f.module == 0)
        .filter_map(|f| {
            let stage = match f.stage? {
                S::Vertex => crate::Stage::Vertex,
                S::Fragment => crate::Stage::Fragment,
                S::Compute(_) => crate::Stage::Compute,
            };
            Some(EntryView {
                name: names.get(f.name).to_string(),
                stage,
                span: spans.get(f.span),
            })
        })
        .collect();
    let constants = module
        .consts
        .iter()
        .filter(|k| k.module == 0)
        .map(|k| ConstView {
            name: names.get(k.name).to_string(),
            ty: types.name(k.ty, names),
            pipeline: k.pipeline.is_some(),
            span: spans.get(k.span),
        })
        .collect();
    ProgramView {
        bindings,
        entry_points,
        constants,
    }
}
