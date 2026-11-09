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
    let paths = match russl_compiler::use_paths(root.clone()) {
        Ok(p) => p,
        Err(d) => return errors(&[d]).into(),
    };
    let pending = paths
        .into_iter()
        .map(|(key, path)| Pending {
            path: quote! { #path::__russl_source },
            root_key: Some(key),
            dep: None,
        })
        .collect();
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
    let mut modules = vec![russl_compiler::Module {
        source: st.root.clone(),
        imports: st.root_imports_by_index(),
    }];
    for m in &st.loaded {
        modules.push(russl_compiler::Module {
            source: m.source.clone(),
            imports: st.imports_by_index(m),
        });
    }
    match russl_compiler::compile_program(&modules) {
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
