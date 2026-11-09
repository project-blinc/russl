//! The `russl!` macro. A thin shell over `russl_compiler`: it compiles its input at build time
//! and expands to an artifact per target as consts, or to a compile error at the offending token.

use proc_macro::TokenStream;
use quote::quote_spanned;

/// Compile a russl module at build time. Expands to one const per target: `WGSL`, `MSL`, `HLSL`,
/// `SPIRV` and `GLSL`.
#[proc_macro]
pub fn russl(input: TokenStream) -> TokenStream {
    match russl_compiler::compile(input.into()) {
        Ok(a) => {
            let (wgsl, msl, hlsl, spirv) = (a.wgsl, a.msl, a.hlsl, a.spirv);
            let glsl = a.glsl.iter().map(|g| {
                let (entry, source) = (&g.entry_point, &g.source);
                quote::quote! { (#entry, #source) }
            });
            // A const takes no space in a binary that never reads it, so each platform keeps
            // only the artifact it uses.
            quote::quote! {
                pub const WGSL: &str = #wgsl;
                pub const MSL: &str = #msl;
                pub const HLSL: &str = #hlsl;
                pub const SPIRV: &[u32] = &[#(#spirv),*];
                /// GLSL ES 3.00 per entry point: (entry point name, source).
                pub const GLSL: &[(&str, &str)] = &[#(#glsl),*];
            }
            .into()
        }
        Err(err) => {
            let message = err.message;
            quote_spanned! { err.span => ::core::compile_error!(#message); }.into()
        }
    }
}
