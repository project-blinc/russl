//! The `russl!` macro. A thin shell over `russl_compiler`: it compiles its input at build time
//! and expands to an artifact per target as consts, or to an error at each offending token.

use proc_macro::TokenStream;
use quote::{quote, quote_spanned};
use russl_compiler::{Diagnostic, Severity};

/// Compile a russl module at build time. Expands to one const per target: `WGSL`, `MSL`, `HLSL`,
/// `SPIRV` and `GLSL`.
#[proc_macro]
pub fn russl(input: TokenStream) -> TokenStream {
    match russl_compiler::compile(input.into()) {
        Ok(a) => {
            let warnings = a.warnings.iter().map(warning);
            let (wgsl, msl, hlsl, spirv) = (&a.wgsl, &a.msl, &a.hlsl, &a.spirv);
            let glsl = a.glsl.iter().map(|g| {
                let (entry, source) = (&g.entry_point, &g.source);
                quote! { (#entry, #source) }
            });
            // A const takes no space in a binary that never reads it, so each platform keeps
            // only the artifact it uses.
            quote! {
                #(#warnings)*
                pub const WGSL: &str = #wgsl;
                pub const MSL: &str = #msl;
                pub const HLSL: &str = #hlsl;
                pub const SPIRV: &[u32] = &[#(#spirv),*];
                /// GLSL ES 3.00 per entry point: (entry point name, source).
                pub const GLSL: &[(&str, &str)] = &[#(#glsl),*];
            }
            .into()
        }
        Err(diagnostics) => {
            let out = diagnostics.iter().map(|d| match d.severity {
                Severity::Error => {
                    let message = &d.message;
                    quote_spanned! { d.span => ::core::compile_error!(#message); }
                }
                Severity::Warning => warning(d),
            });
            quote! { #(#out)* }.into()
        }
    }
}

/// A warning at the source token. A stable proc macro cannot emit one directly; using a
/// deprecated item at that span makes rustc report the note there.
fn warning(d: &Diagnostic) -> proc_macro2::TokenStream {
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
