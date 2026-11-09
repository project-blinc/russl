//! Rust for the host side of a module: a twin of each struct with the bytes a buffer holds,
//! and constants for its bindings, pipeline constants and vertex inputs.
//!
//! A twin is `#[repr(C)]` with explicit padding, so it has no padding bytes of its own: its
//! bytes are exactly a buffer's. Vectors are arrays (`Vec3` is `[f32; 3]`), a `Vec3` an array
//! holds is `[f32; 4]` as its stride is, matrix columns are padded the same way, and `f16` is
//! its `u16` bits. Sizes and offsets are asserted when the crate builds.

use proc_macro2::TokenStream as Tokens;
use quote::{format_ident, quote};
use russl_compiler::{BindingKind, ConstValue, HostScalar, HostType, Reflection, StructLayout};

/// The items for a module's reflection. `bytemuck` adds `Pod` and `Zeroable` impls through
/// `::bytemuck`, for crates that depend on it.
pub fn items(r: &Reflection, bytemuck: bool) -> Tokens {
    let imports = r.struct_imports.iter().map(|i| {
        let module: Tokens = i.module.parse().unwrap_or_default();
        let (name, alias) = (format_ident!("{}", i.name), format_ident!("{}", i.alias));
        let vis = if i.public {
            quote! { pub }
        } else {
            quote! {}
        };
        quote! {
            #[allow(unused_imports)]
            #vis use #module::#name as #alias;
        }
    });
    let alias_of = |name: &str, module: usize| -> String {
        if module == 0 {
            return name.to_string();
        }
        r.struct_imports
            .iter()
            .find(|i| i.name == name)
            .map_or_else(|| name.to_string(), |i| i.alias.clone())
    };
    let twins = r.structs.iter().map(|s| twin(s, &alias_of, bytemuck));
    let bindings = r.bindings.iter().map(|b| {
        let group = format_ident!("{}_GROUP", b.name.to_uppercase());
        let binding = format_ident!("{}_BINDING", b.name.to_uppercase());
        let (g, n) = (b.group, b.binding);
        let size = b.size.map(|size| {
            let ident = format_ident!("{}_SIZE", b.name.to_uppercase());
            let size = size as u64;
            quote! {
                /// The size of the bound type: the buffer's minimum binding size.
                #[allow(dead_code)]
                pub const #ident: u64 = #size;
            }
        });
        let what = match b.kind {
            BindingKind::Uniform => "uniform buffer",
            BindingKind::Storage { writable: false } => "read-only storage buffer",
            BindingKind::Storage { writable: true } => "read-write storage buffer",
            BindingKind::Handle => "texture or sampler",
        };
        let doc = format!("`{}`, a {what}.", b.name);
        quote! {
            #[doc = #doc]
            #[allow(dead_code)]
            pub const #group: u32 = #g;
            #[doc = #doc]
            #[allow(dead_code)]
            pub const #binding: u32 = #n;
            #size
        }
    });
    let constants = r.constants.iter().map(|(name, v)| {
        let v = match v {
            ConstValue::Bool(b) => f64::from(u8::from(*b)),
            ConstValue::Int(i) => *i as f64,
            ConstValue::Float(f) => *f,
        };
        quote! { (#name, #v) }
    });
    let inputs = r.vertex_inputs.iter().map(|v| {
        let ident = format_ident!("{}_INPUTS", v.entry_point.to_uppercase());
        let rows = v.inputs.iter().map(|i| {
            let (name, location, format) = (&i.name, i.location, i.format);
            quote! { (#name, #location, #format) }
        });
        let doc = format!(
            "What `{}` reads from vertex buffers: (name, location, `wgpu::VertexFormat` variant).",
            v.entry_point
        );
        quote! {
            #[doc = #doc]
            #[allow(dead_code)]
            pub const #ident: &[(&str, u32, &str)] = &[#(#rows),*];
        }
    });
    quote! {
        #(#imports)*
        #(#twins)*
        #(#bindings)*
        /// The pipeline constants as this module was built with them, as wgpu lists them.
        #[allow(dead_code)]
        pub const CONSTANTS: &[(&str, f64)] = &[#(#constants),*];
        #(#inputs)*
    }
}

/// The Rust type that holds `ty`'s bytes, and its size.
fn rust_type(ty: &HostType, alias_of: &dyn Fn(&str, usize) -> String) -> Tokens {
    let scalar = |s: HostScalar| match s {
        HostScalar::F32 => quote! { f32 },
        HostScalar::F16 => quote! { u16 },
        HostScalar::I32 => quote! { i32 },
        HostScalar::U32 => quote! { u32 },
    };
    match ty {
        HostType::Scalar(s) | HostType::Atomic(s) => scalar(*s),
        HostType::Vector(n, s) => {
            let (s, n) = (scalar(*s), *n as usize);
            quote! { [#s; #n] }
        }
        HostType::Matrix {
            cols,
            rows,
            scalar: s,
        } => {
            // A column is padded to its alignment: three rows take four.
            let rows = if *rows == 3 { 4 } else { *rows as usize };
            let (s, cols) = (scalar(*s), *cols as usize);
            quote! { [[#s; #rows]; #cols] }
        }
        HostType::Array { elem, len, .. } => {
            let elem = match &**elem {
                // Held at a 16-byte stride, so the fourth component is the padding.
                HostType::Vector(3, s) => rust_type(&HostType::Vector(4, *s), alias_of),
                other => rust_type(other, alias_of),
            };
            let len = len.unwrap_or(0) as usize;
            quote! { [#elem; #len] }
        }
        HostType::Struct { name, module } => {
            let ident = format_ident!("{}", alias_of(name, *module));
            quote! { #ident }
        }
        // `bool` takes four bytes in WGSL; no buffer holds one.
        HostType::Unsupported(_) => quote! { u32 },
    }
}

fn twin(s: &StructLayout, alias_of: &dyn Fn(&str, usize) -> String, bytemuck: bool) -> Tokens {
    let name = format_ident!("{}", s.name);
    let mut fields = Vec::new();
    let mut params = Vec::new();
    let mut inits = Vec::new();
    let mut asserts = Vec::new();
    let mut offsets = Vec::new();
    let mut cursor = 0u32;
    let mut pad = 0usize;
    let mut tail_doc = None;
    for f in &s.fields {
        if let HostType::Array {
            len: None, stride, ..
        } = &f.ty
        {
            // A runtime-sized array follows the twin in the buffer, one element per stride.
            tail_doc = Some(format!(
                "Followed in its buffer by `{}`, a runtime-sized array with a {stride}-byte stride.",
                f.name
            ));
            break;
        }
        if f.offset > cursor {
            let p = format_ident!("_pad{}", pad);
            let n = (f.offset - cursor) as usize;
            fields.push(quote! { #p: [u8; #n] });
            inits.push(quote! { #p: [0; #n] });
            pad += 1;
        }
        let ident = format_ident!("{}", f.name);
        let ty = rust_type(&f.ty, alias_of);
        let doc = format!("At byte {}.", f.offset);
        fields.push(quote! { #[doc = #doc] pub #ident: #ty });
        params.push(quote! { #ident: #ty });
        inits.push(quote! { #ident });
        let off = f.offset as usize;
        asserts.push(quote! {
            assert!(::core::mem::offset_of!(#name, #ident) == #off);
        });
        let off_ident = format_ident!("{}_OFFSET", f.name.to_uppercase());
        offsets.push(quote! { pub const #off_ident: usize = #off; });
        cursor = f.offset + f.size;
    }
    let size = if tail_doc.is_some() { cursor } else { s.size };
    if size > cursor {
        let p = format_ident!("_pad{}", pad);
        let n = (size - cursor) as usize;
        fields.push(quote! { #p: [u8; #n] });
        inits.push(quote! { #p: [0; #n] });
    }
    let size = size as usize;
    let doc = format!(
        "The host's `{}`: the bytes a buffer holds, {size} of them.{}",
        s.name,
        tail_doc.map(|t| format!(" {t}")).unwrap_or_default()
    );
    let pod = bytemuck.then(|| {
        quote! {
            // SAFETY: plain numbers only, and every padding byte is a field, so no byte is
            // uninitialised and any bit pattern is a value.
            unsafe impl ::bytemuck::Zeroable for #name {}
            unsafe impl ::bytemuck::Pod for #name {}
        }
    });
    quote! {
        #[doc = #doc]
        #[repr(C)]
        #[derive(Clone, Copy, Debug, PartialEq)]
        #[allow(dead_code)]
        pub struct #name {
            #(#fields),*
        }

        #[allow(dead_code)]
        impl #name {
            /// The size a buffer holds it in.
            pub const SIZE: usize = #size;
            #(#offsets)*

            #[allow(clippy::too_many_arguments)]
            pub const fn new(#(#params),*) -> Self {
                Self { #(#inits),* }
            }

            /// Every byte zero.
            pub const fn zeroed() -> Self {
                // SAFETY: every field is plain numbers, for which zero is a value.
                unsafe { ::core::mem::zeroed() }
            }

            /// The bytes to upload.
            pub fn as_bytes(&self) -> &[u8] {
                // SAFETY: `repr(C)` with every padding byte a field, so all `SIZE` bytes are
                // initialised.
                unsafe { ::core::slice::from_raw_parts((self as *const Self).cast::<u8>(), Self::SIZE) }
            }

            /// The bytes of several, back to back as an array of them is laid out.
            pub fn slice_as_bytes(items: &[Self]) -> &[u8] {
                // SAFETY: as for `as_bytes`; a slice has no gaps between elements.
                unsafe {
                    ::core::slice::from_raw_parts(items.as_ptr().cast::<u8>(), ::core::mem::size_of_val(items))
                }
            }
        }

        const _: () = {
            assert!(::core::mem::size_of::<#name>() == #size);
            #(#asserts)*
        };
        #pod
    }
}
