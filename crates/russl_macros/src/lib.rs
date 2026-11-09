//! The `russl!` macro. It compiles its input at build time and expands to an artifact per
//! target as consts, or to an error at each offending token.
//!
//! A proc macro sees only its own tokens, so imports travel by callback. Every expansion
//! defines `__russl_source` in its Rust module: a macro that hands the module's source to a
//! continuation. `use crate::sdf::common::f;` expands to `crate::sdf::common::__russl_source!`,
//! which calls `::russl::__russl_continue!` with the source appended; the continuation queues
//! that module's own dependencies and, once every module of the program is in hand, compiles.
//! Each expansion also re-exports every module it loaded, directly or not, as
//! `__russl_dep_<k>`, so an importer reaches a dependency's dependencies without knowing where
//! they live.

mod state;

use proc_macro::TokenStream;
use proc_macro2::TokenStream as Tokens;
use quote::{format_ident, quote, quote_spanned};
use russl_compiler::{Diagnostic, Severity};
use state::{Pending, State};

/// Compile a russl module at build time. Expands to one const per target (`WGSL`, `MSL`, `HLSL`,
/// `SPIRV` and `GLSL`), or to nothing but its import interface for a library module.
#[proc_macro]
pub fn russl(input: TokenStream) -> TokenStream {
    let root: Tokens = input.into();
    let pending = match specialization(&root) {
        Err(d) => return errors(&d).into(),
        Ok(Some((target, _))) => vec![Pending {
            path: quote! { #target::__russl_source },
            root_key: Some(SPECIALIZE.into()),
            dep: None,
        }],
        Ok(None) => match russl_compiler::use_paths(root.clone()) {
            Ok(paths) => paths
                .into_iter()
                .map(|(key, path)| Pending {
                    path: quote! { #path::__russl_source },
                    root_key: Some(key),
                    dep: None,
                })
                .collect(),
            Err(d) => return errors(&[d]).into(),
        },
    };
    step(State {
        root,
        loaded: Vec::new(),
        root_imports: Vec::new(),
        pending,
    })
    .into()
}

/// The continuation of an import: the state so far, with the delivered module at its end.
#[doc(hidden)]
#[proc_macro]
pub fn __russl_continue(input: TokenStream) -> TokenStream {
    match syn::parse::<state::Continue>(input) {
        Ok(c) => {
            let mut st = c.state;
            st.deliver(c.module);
            step(st).into()
        }
        Err(e) => e.to_compile_error().into(),
    }
}

/// Fetch the next pending module, or compile when none is left.
fn step(st: State) -> Tokens {
    if let Some(next) = st.pending.first() {
        let path = next.path.clone();
        let state = st.to_tokens();
        return quote! { #path! { ::russl::__russl_continue { #state } } };
    }
    compile(&st)
}

fn compile(st: &State) -> Tokens {
    let specialized = match specialization(&st.root) {
        Ok(s) => s,
        Err(d) => return errors(&d),
    };
    // The program's modules by id; "" stands for the root itself.
    let mut order: Vec<&str> = Vec::new();
    let mut constants = Vec::new();
    match &specialized {
        // A specialisation compiles the module it names, as the program's root.
        Some((_, values)) => {
            let Some(target) = st
                .root_imports
                .iter()
                .find(|(k, _)| k == SPECIALIZE)
                .map(|(_, id)| id.as_str())
            else {
                return quote! { ::core::compile_error!("russl: the specialised module was not delivered"); };
            };
            order.push(target);
            order.extend(
                st.loaded
                    .iter()
                    .map(|m| m.id.as_str())
                    .filter(|id| *id != target),
            );
            constants = values.clone();
        }
        None => {
            order.push("");
            order.extend(st.loaded.iter().map(|m| m.id.as_str()));
        }
    }
    let modules: Vec<russl_compiler::Module> = order
        .iter()
        .map(|id| match st.loaded.iter().find(|m| m.id == *id) {
            Some(m) => russl_compiler::Module {
                source: m.source.clone(),
                imports: st.imports_in(&m.imports, &order),
            },
            None => russl_compiler::Module {
                source: st.root.clone(),
                imports: st.imports_in(&st.root_imports, &order),
            },
        })
        .collect();
    match russl_compiler::compile_program_with(&modules, &constants) {
        Ok(a) => {
            let warnings = a.warnings.iter().map(warning);
            let interface = interface(st);
            let consts = if a.library {
                quote! {}
            } else {
                let (wgsl, msl, hlsl, spirv) = (&a.wgsl, &a.msl, &a.hlsl, &a.spirv);
                let glsl = a.glsl.iter().map(|g| {
                    let (entry, source) = (&g.entry_point, &g.source);
                    quote! { (#entry, #source) }
                });
                // A const takes no space in a binary that never reads it, so each platform keeps
                // only the artifact it uses.
                quote! {
                    pub const WGSL: &str = #wgsl;
                    pub const MSL: &str = #msl;
                    pub const HLSL: &str = #hlsl;
                    pub const SPIRV: &[u32] = &[#(#spirv),*];
                    /// GLSL ES 3.00 per entry point: (entry point name, source).
                    pub const GLSL: &[(&str, &str)] = &[#(#glsl),*];
                }
            };
            quote! { #(#warnings)* #consts #interface }
        }
        Err(d) => errors(&d),
    }
}

/// What other modules import this one through: its source macro and a re-export of every
/// module it loaded.
fn interface(st: &State) -> Tokens {
    let id = st.root_id();
    let source_macro = format_ident!("__russl_source_{}", id);
    let source = &st.root;
    let deps: Vec<&str> = st.loaded.iter().map(|m| m.id.as_str()).collect();
    let imports = st.root_imports.iter().map(|(k, i)| quote! { (#k, #i) });
    let aliases = st.loaded.iter().enumerate().map(|(k, m)| {
        let alias = format_ident!("__russl_dep_{}", k);
        let path = &m.path;
        quote! { #[doc(hidden)] pub use #path as #alias; }
    });
    quote! {
        #[doc(hidden)]
        #[macro_export]
        macro_rules! #source_macro {
            ($cb:path { $($state:tt)* }) => {
                $cb! { $($state)* module { id #id deps [#(#deps)*] imports [#(#imports)*] source { #source } } }
            };
        }
        #[doc(hidden)]
        pub use #source_macro as __russl_source;
        #(#aliases)*
    }
}

/// The key under which a specialisation's target sits among the root's imports.
const SPECIALIZE: &str = "#specialize";

/// A module of the form `#![specialize(path)]` followed by `const NAME: T = value;` items: the
/// module it specialises, and the values it gives.
fn specialization(
    root: &Tokens,
) -> Result<Option<(syn::Path, Vec<russl_compiler::Constant>)>, Vec<Diagnostic>> {
    let file: syn::File =
        syn::parse2(root.clone()).map_err(|e| vec![Diagnostic::error(e.span(), e.to_string())])?;
    let Some(attr) = file.attrs.iter().find(|a| a.path().is_ident("specialize")) else {
        return Ok(None);
    };
    let target: syn::Path = attr.parse_args().map_err(|e| {
        vec![Diagnostic::error(
            e.span(),
            "expected `#![specialize(path::to::module)]`",
        )]
    })?;
    let mut constants = Vec::new();
    let mut bad = Vec::new();
    for item in &file.items {
        let syn::Item::Const(c) = item else {
            use syn::spanned::Spanned;
            bad.push(Diagnostic::error(
                item.span(),
                "a specialisation holds only `const NAME: T = value;` items",
            ));
            continue;
        };
        match literal(&c.expr) {
            Some(value) => constants.push(russl_compiler::Constant {
                name: c.ident.to_string(),
                value,
                span: c.ident.span(),
            }),
            None => {
                use syn::spanned::Spanned;
                bad.push(Diagnostic::error(
                    c.expr.span(),
                    "a specialised value is a literal",
                ));
            }
        }
    }
    if bad.is_empty() {
        Ok(Some((target, constants)))
    } else {
        Err(bad)
    }
}

/// A bool, integer or float literal, possibly negated.
fn literal(e: &syn::Expr) -> Option<russl_compiler::ConstValue> {
    use russl_compiler::ConstValue as V;
    match e {
        syn::Expr::Lit(l) => match &l.lit {
            syn::Lit::Bool(b) => Some(V::Bool(b.value)),
            syn::Lit::Int(i) => i.base10_parse().ok().map(V::Int),
            syn::Lit::Float(f) => f.base10_parse().ok().map(V::Float),
            _ => None,
        },
        syn::Expr::Unary(u) if matches!(u.op, syn::UnOp::Neg(_)) => match literal(&u.expr)? {
            V::Int(i) => Some(V::Int(-i)),
            V::Float(f) => Some(V::Float(-f)),
            V::Bool(_) => None,
        },
        syn::Expr::Paren(p) => literal(&p.expr),
        _ => None,
    }
}

fn errors(diagnostics: &[Diagnostic]) -> Tokens {
    let out = diagnostics.iter().map(|d| match d.severity {
        Severity::Error => {
            let message = &d.message;
            quote_spanned! { d.span => ::core::compile_error!(#message); }
        }
        Severity::Warning => warning(d),
    });
    quote! { #(#out)* }
}

/// A warning at the source token. A stable proc macro cannot emit one directly; using a
/// deprecated item at that span makes rustc report the note there.
fn warning(d: &Diagnostic) -> Tokens {
    let note = format!("russl: {}", d.message);
    quote_spanned! { d.span =>
        const _: () = {
            #[deprecated(note = #note)]
            #[allow(non_upper_case_globals)]
            const russl_warning: () = ();
            russl_warning
        };
    }
}
