//! Shader attributes on items, fields and parameters.

use crate::diag::Sink;
use crate::hir::{Builtin, Io, Space, Stage};
use syn::parse::{Parse, ParseStream};
use syn::spanned::Spanned;

/// `#[vertex]`, `#[fragment]` or `#[compute(workgroup_size(x, y, z))]`.
pub fn stage(attrs: &[syn::Attribute], sink: &mut Sink) -> Option<Stage> {
    for a in attrs {
        if a.path().is_ident("vertex") {
            return Some(Stage::Vertex);
        }
        if a.path().is_ident("fragment") {
            return Some(Stage::Fragment);
        }
        if a.path().is_ident("compute") {
            return match a.parse_args::<WorkgroupSize>() {
                Ok(ws) => Some(Stage::Compute(ws.0)),
                Err(_) => {
                    sink.error(a.span(), "expected `#[compute(workgroup_size(x, y, z))]`");
                    Some(Stage::Compute([1, 1, 1]))
                }
            };
        }
    }
    None
}

/// `#[location(n)]` (with `#[flat]`) or `#[builtin(name)]`.
pub fn io(attrs: &[syn::Attribute], sink: &mut Sink) -> Option<Io> {
    let flat = attrs.iter().any(|a| a.path().is_ident("flat"));
    let invariant = attrs.iter().any(|a| a.path().is_ident("invariant"));
    for a in attrs {
        if a.path().is_ident("location") {
            return match a.parse_args::<syn::LitInt>().and_then(|l| l.base10_parse()) {
                Ok(location) => Some(Io::Location { location, flat }),
                Err(e) => {
                    sink.error(e.span(), "expected `#[location(n)]`");
                    None
                }
            };
        }
        if a.path().is_ident("builtin") {
            let Ok(name) = a.parse_args::<syn::Ident>() else {
                sink.error(a.span(), "expected `#[builtin(name)]`");
                return None;
            };
            let b = match name.to_string().as_str() {
                "position" => Builtin::Position { invariant },
                "vertex_index" => Builtin::VertexIndex,
                "instance_index" => Builtin::InstanceIndex,
                "front_facing" => Builtin::FrontFacing,
                "frag_depth" => Builtin::FragDepth,
                "sample_index" => Builtin::SampleIndex,
                "sample_mask" => Builtin::SampleMask,
                "local_invocation_id" => Builtin::LocalInvocationId,
                "local_invocation_index" => Builtin::LocalInvocationIndex,
                "global_invocation_id" => Builtin::GlobalInvocationId,
                "workgroup_id" => Builtin::WorkgroupId,
                "num_workgroups" => Builtin::NumWorkgroups,
                other => {
                    sink.error(name.span(), format!("unknown builtin `{other}`"));
                    return None;
                }
            };
            return Some(Io::Builtin(b));
        }
    }
    None
}

/// `#[pipeline]` or `#[pipeline(id)]`: `Some(id)` for a pipeline constant.
pub fn pipeline(attrs: &[syn::Attribute], sink: &mut Sink) -> Option<Option<u16>> {
    let a = attrs.iter().find(|a| a.path().is_ident("pipeline"))?;
    match &a.meta {
        syn::Meta::Path(_) => Some(None),
        _ => match a.parse_args::<syn::LitInt>().and_then(|l| l.base10_parse()) {
            Ok(id) => Some(Some(id)),
            Err(e) => {
                sink.error(e.span(), "expected `#[pipeline]` or `#[pipeline(id)]`");
                Some(None)
            }
        },
    }
}

/// The integer in `#[name(n)]`.
pub fn int(attrs: &[syn::Attribute], name: &str, sink: &mut Sink) -> Option<u32> {
    let a = attrs.iter().find(|a| a.path().is_ident(name))?;
    match a.parse_args::<syn::LitInt>().and_then(|l| l.base10_parse()) {
        Ok(n) => Some(n),
        Err(e) => {
            sink.error(e.span(), format!("expected `#[{name}(n)]`"));
            None
        }
    }
}

/// The address space a resource's wrapper names, and the type inside it.
pub fn address_space(ty: &syn::Type) -> (Option<Space>, &syn::Type) {
    if let syn::Type::Path(p) = ty
        && let Some(seg) = p.path.segments.last()
        && let syn::PathArguments::AngleBracketed(args) = &seg.arguments
        && let Some(syn::GenericArgument::Type(inner)) = args.args.first()
    {
        let space = match seg.ident.to_string().as_str() {
            "Uniform" => Space::Uniform,
            "Storage" => Space::Storage { writable: false },
            "StorageMut" => Space::Storage { writable: true },
            "Workgroup" => Space::Workgroup,
            _ => return (None, ty),
        };
        return (Some(space), inner);
    }
    (None, ty)
}

/// `workgroup_size(x, y, z)`, one to three dimensions.
struct WorkgroupSize([u32; 3]);

impl Parse for WorkgroupSize {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let key: syn::Ident = input.parse()?;
        if key != "workgroup_size" {
            return Err(syn::Error::new(key.span(), "expected `workgroup_size(..)`"));
        }
        let content;
        syn::parenthesized!(content in input);
        let dims = content.parse_terminated(syn::LitInt::parse, syn::Token![,])?;
        if dims.is_empty() || dims.len() > 3 {
            return Err(syn::Error::new(
                key.span(),
                "one to three workgroup dimensions",
            ));
        }
        let mut size = [1u32; 3];
        for (slot, d) in size.iter_mut().zip(&dims) {
            *slot = d.base10_parse()?;
        }
        Ok(WorkgroupSize(size))
    }
}

/// `#[attrs] static NAME: T;` or `#[attrs] const NAME: T;`, which Rust has no item for.
pub struct Decl {
    pub attrs: Vec<syn::Attribute>,
    pub vis: syn::Visibility,
    pub is_const: bool,
    pub ident: syn::Ident,
    pub ty: syn::Type,
}

impl Parse for Decl {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let attrs = input.call(syn::Attribute::parse_outer)?;
        let vis: syn::Visibility = input.parse()?;
        let is_const = if input.peek(syn::Token![const]) {
            let _: syn::Token![const] = input.parse()?;
            true
        } else {
            let _: syn::Token![static] = input.parse()?;
            false
        };
        let ident = input.parse()?;
        let _: syn::Token![:] = input.parse()?;
        let ty = input.parse()?;
        let _: syn::Token![;] = input.parse()?;
        Ok(Decl {
            attrs,
            vis,
            is_const,
            ident,
            ty,
        })
    }
}
