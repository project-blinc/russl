//! `toy_shader!`: a framework's own shader macro. Its extension gives every shader a frame
//! uniform and an extern `wave`, and keeps bindings in group 0.

use proc_macro::TokenStream;
use russl_compiler::{Diagnostic, Extension, ProgramView};
use russl_expand::Expander;

struct Toy;

impl Extension for Toy {
    fn name(&self) -> &str {
        "toy"
    }

    fn prelude(&self) -> Option<proc_macro2::TokenStream> {
        Some(quote::quote! {
            pub struct Frame { pub time: f32, pub size: Vec2 }
            #[group(0)] #[binding(0)] pub static FRAME: Uniform<Frame>;
        })
    }

    fn extern_fn(&self, name: &str) -> Option<proc_macro2::TokenStream> {
        (name == "wave").then(|| quote::quote! { fn wave(t: f32) -> f32 { t.sin() * 0.5 + 0.5 } })
    }

    fn check(&self, program: &ProgramView, report: &mut Vec<Diagnostic>) {
        for b in program.bindings.iter().filter(|b| b.group != 0) {
            report.push(Diagnostic::error(
                b.span,
                format!(
                    "toy binds group 0 only; `{}` is in group {}",
                    b.name, b.group
                ),
            ));
        }
    }
}

fn expander() -> Expander<'static> {
    Expander {
        continuation: quote::quote! { ::russl_fixture_toy_macro::__toy_continue },
        extension: &Toy,
    }
}

#[proc_macro]
pub fn toy_shader(input: TokenStream) -> TokenStream {
    expander().expand(input.into()).into()
}

#[doc(hidden)]
#[proc_macro]
pub fn __toy_continue(input: TokenStream) -> TokenStream {
    expander().resume(input.into()).into()
}
