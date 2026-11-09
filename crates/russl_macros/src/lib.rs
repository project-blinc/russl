//! The `russl!` macro: `russl_expand` with no extension. A framework's own shader macro is the
//! same two functions with its extension and its continuation's path.

use proc_macro::TokenStream;
use russl_compiler::NoExtension;
use russl_expand::Expander;

fn expander() -> Expander<'static> {
    Expander {
        continuation: quote::quote! { ::russl::__russl_continue },
        extension: &NoExtension,
    }
}

/// Compile a russl module at build time. Expands to one const per target (`WGSL`, `MSL`, `HLSL`,
/// `SPIRV` and `GLSL`), or to nothing but its import interface for a library module.
#[proc_macro]
pub fn russl(input: TokenStream) -> TokenStream {
    expander().expand(input.into()).into()
}

/// The continuation of an import: the state so far, with the delivered module at its end.
#[doc(hidden)]
#[proc_macro]
pub fn __russl_continue(input: TokenStream) -> TokenStream {
    expander().resume(input.into()).into()
}
