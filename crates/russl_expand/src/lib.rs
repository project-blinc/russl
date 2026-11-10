//! The expansion behind `russl!`, for it and for a framework's own shader macro. It compiles a
//! module at build time and expands to an artifact per target as consts, or to an error at each
//! offending token. A framework's macro passes its [`Extension`] and the path of its own
//! continuation macro; modules expanded by either import each other.
//!
//! A proc macro sees only its own tokens, so imports travel by callback. Every expansion
//! defines `russl_module` in its Rust module: a macro that hands the module's source to a
//! continuation. `use crate::sdf::common::f;` expands to `crate::sdf::common::russl_module!`,
//! which calls the continuation (`::russl::__russl_continue!` for `russl!`) with the source
//! appended; the continuation queues
//! that module's own dependencies and, once every module of the program is in hand, compiles.
//! Each expansion also re-exports every module it loaded, directly or not, as
//! `__russl_dep_<k>`, so an importer reaches a dependency's dependencies without knowing where
//! they live.

mod host;
mod state;

use proc_macro2::TokenStream as Tokens;
use quote::{format_ident, quote, quote_spanned};
use russl_compiler::{Diagnostic, Extension, Severity};
use state::{Pending, State};

/// `module::russl_module`, the macro that delivers a module's source. Its name carries the span
/// of the module it is looked up in, so a Rust module with no russl module in it is reported at
/// the `use` that names it: ``could not find `russl_module` in `plain` ``.
fn source_path(module: Tokens, span: Option<proc_macro2::Span>) -> Tokens {
    let name = syn::Ident::new(
        "russl_module",
        span.unwrap_or_else(proc_macro2::Span::call_site),
    );
    quote! { #module::#name }
}

/// The expansion, as one macro configures it.
pub struct Expander<'a> {
    /// The path of the macro that resumes an import: `::russl::__russl_continue`.
    pub continuation: Tokens,
    pub extension: &'a dyn Extension,
}

impl Expander<'_> {
    /// Expand a shader module: fetch what it imports, then compile.
    pub fn expand(&self, root: Tokens) -> Tokens {
        let pending = match specialization(&root) {
            Err(d) => return errors(&d),
            Ok(Some((target, _))) => vec![Pending {
                path: source_path(
                    quote! { #target },
                    target.segments.last().map(|s| s.ident.span()),
                ),
                root_key: Some(SPECIALIZE.into()),
                dep: None,
            }],
            Ok(None) => match russl_compiler::use_paths(root.clone()) {
                Ok(paths) => paths
                    .into_iter()
                    .map(|(key, path)| Pending {
                        path: {
                            let span = path.clone().into_iter().last().map(|t| t.span());
                            source_path(path, span)
                        },
                        root_key: Some(key),
                        dep: None,
                    })
                    .collect(),
                Err(d) => return errors(&[d]),
            },
        };
        self.step(State {
            root,
            loaded: Vec::new(),
            root_imports: Vec::new(),
            pending,
        })
    }

    /// Resume after an import: the state so far, with the delivered module at its end.
    pub fn resume(&self, input: Tokens) -> Tokens {
        match syn::parse2::<state::Continue>(input) {
            Ok(c) => {
                let mut st = c.state;
                st.deliver(c.module);
                self.step(st)
            }
            Err(e) => e.to_compile_error(),
        }
    }

    /// Fetch the next pending module, or compile when none is left.
    fn step(&self, st: State) -> Tokens {
        if let Some(next) = st.pending.first() {
            let path = next.path.clone();
            let state = st.to_tokens();
            let cont = &self.continuation;
            return quote! { #path! { #cont { #state } } };
        }
        self.compile(&st)
    }

    fn compile(&self, st: &State) -> Tokens {
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
        match russl_compiler::compile_program_ext(&modules, &constants, self.extension) {
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
                        /// GLSL ES 3.00 per entry point: (entry point name, source). Empty when the
                        /// shader needs what WebGL2 lacks, such as storage buffers.
                        pub const GLSL: &[(&str, &str)] = &[#(#glsl),*];
                    }
                };
                // A specialisation's structs are its target's, whose twins live with it.
                let host = if specialized.is_some() {
                    host::items(
                        &russl_compiler::Reflection {
                            structs: Vec::new(),
                            struct_imports: Vec::new(),
                            ..a.reflection.clone()
                        },
                        false,
                    )
                } else {
                    let bytemuck = file_attrs(&st.root)
                        .is_some_and(|attrs| attrs.iter().any(|a| a.path().is_ident("bytemuck")));
                    host::items(&a.reflection, bytemuck)
                };
                quote! { #(#warnings)* #consts #host #interface }
            }
            Err(d) => errors(&d),
        }
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
    let export =
        file_attrs(&st.root).is_some_and(|attrs| attrs.iter().any(|a| a.path().is_ident("export")));
    let reexport = if export {
        quote! { pub }
    } else {
        quote! { pub(crate) }
    };
    let aliases = st.loaded.iter().enumerate().map(|(k, m)| {
        let alias = format_ident!("__russl_dep_{}", k);
        let path = &m.path;
        quote! { #[doc(hidden)] #reexport use #path as #alias; }
    });
    // A module other crates import is exported at the crate root, where its name must be unique
    // for the whole crate. Any other is scoped to its Rust module, so the same file can sit under
    // several modules.
    let visibility = if export {
        quote! { #[macro_export] }
    } else {
        quote! {}
    };
    quote! {
        #[doc(hidden)]
        #visibility
        macro_rules! #source_macro {
            ($cb:path { $($state:tt)* }) => {
                $cb! { $($state)* module { id #id deps [#(#deps)*] imports [#(#imports)*] source { #source } } }
            };
        }
        #[doc(hidden)]
        #reexport use #source_macro as russl_module;
        #(#aliases)*
    }
}

/// The inner attributes of a module's source (`#![export]` and the like), read with every
/// conditional branch kept: the attributes are the module's own, not a branch's.
fn file_attrs(root: &Tokens) -> Option<Vec<syn::Attribute>> {
    let flat = russl_compiler::all_branches(root.clone()).ok()?;
    syn::parse2::<syn::File>(flat).ok().map(|f| f.attrs)
}

/// The key under which a specialisation's target sits among the root's imports.
const SPECIALIZE: &str = "#specialize";

/// A module of the form `#![specialize(path)]` followed by `const NAME: T = value;` items: the
/// module it specialises, and the values it gives.
fn specialization(
    root: &Tokens,
) -> Result<Option<(syn::Path, Vec<russl_compiler::Constant>)>, Vec<Diagnostic>> {
    let file: syn::File = russl_compiler::all_branches(root.clone())
        .map_err(|d| vec![d])
        .and_then(|flat| {
            syn::parse2(flat).map_err(|e| vec![Diagnostic::error(e.span(), e.to_string())])
        })?;
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
