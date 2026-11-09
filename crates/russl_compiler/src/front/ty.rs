//! Type syntax to arena types.

use super::{Front, Item};
use crate::types::{Scalar, TextureDim, TextureKind, Type, TypeId};
use syn::spanned::Spanned;

impl Front<'_> {
    pub fn ty(&mut self, t: &syn::Type) -> TypeId {
        match t {
            syn::Type::Path(p) if p.qself.is_none() && p.path.segments.len() == 1 => {
                let seg = &p.path.segments[0];
                let name = seg.ident.to_string();
                if let Some(ty) = builtin_type(&name) {
                    return self.types.add(ty);
                }
                let texture = |dim| move |kind| Type::Texture { dim, kind };
                let sampled =
                    |this: &mut Self, make: &dyn Fn(TextureKind) -> Type, multi: bool| match this
                        .type_arg(seg)
                        .and_then(|t| this.types.scalar_of(t))
                    {
                        Some(s) => {
                            let kind = if multi {
                                TextureKind::Multisampled(s)
                            } else {
                                TextureKind::Sampled(s)
                            };
                            this.types.add(make(kind))
                        }
                        None => {
                            this.sink.error(
                                seg.span(),
                                format!("`{name}` takes a scalar type, as in `{name}<f32>`"),
                            );
                            this.types.error
                        }
                    };
                match name.as_str() {
                    "Texture1D" => sampled(self, &texture(TextureDim::D1), false),
                    "Texture2D" => sampled(self, &texture(TextureDim::D2), false),
                    "Texture2DArray" => sampled(self, &texture(TextureDim::D2Array), false),
                    "Texture3D" => sampled(self, &texture(TextureDim::D3), false),
                    "TextureCube" => sampled(self, &texture(TextureDim::Cube), false),
                    "TextureMultisampled2D" => sampled(self, &texture(TextureDim::D2), true),
                    "TextureDepth2D" => self.types.add(texture(TextureDim::D2)(TextureKind::Depth)),
                    "TextureDepthCube" => self
                        .types
                        .add(texture(TextureDim::Cube)(TextureKind::Depth)),
                    "Atomic" => match self.type_arg(seg).and_then(|t| self.types.scalar_of(t)) {
                        Some(s @ (Scalar::I32 | Scalar::U32)) => self.types.add(Type::Atomic(s)),
                        _ => {
                            self.sink.error(seg.span(), "`Atomic` holds `i32` or `u32`");
                            self.types.error
                        }
                    },
                    _ => {
                        let sym = self.names.intern(&name);
                        match self.items.get(&sym) {
                            Some(Item::Struct(id)) if seg.arguments.is_none() => {
                                self.types.add(Type::Struct(*id))
                            }
                            _ => {
                                self.sink
                                    .error(seg.ident.span(), format!("unknown type `{name}`"));
                                self.types.error
                            }
                        }
                    }
                }
            }
            syn::Type::Array(a) => {
                let elem = self.ty(&a.elem);
                match self.array_len(&a.len) {
                    Some(n) => self.types.add(Type::Array { elem, len: Some(n) }),
                    None => self.types.error,
                }
            }
            syn::Type::Slice(s) => {
                let elem = self.ty(&s.elem);
                self.types.add(Type::Array { elem, len: None })
            }
            syn::Type::Paren(p) => self.ty(&p.elem),
            syn::Type::Tuple(t) if t.elems.is_empty() => self.types.unit,
            other => {
                self.sink
                    .error(other.span(), "this type has no shader equivalent");
                self.types.error
            }
        }
    }

    fn type_arg(&mut self, seg: &syn::PathSegment) -> Option<TypeId> {
        match &seg.arguments {
            syn::PathArguments::AngleBracketed(a) if a.args.len() == 1 => match &a.args[0] {
                syn::GenericArgument::Type(t) => Some(self.ty(t)),
                _ => None,
            },
            _ => None,
        }
    }

    /// An array length: an integer literal. Lengths from constants come with const evaluation.
    fn array_len(&mut self, e: &syn::Expr) -> Option<u32> {
        if let syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Int(i),
            ..
        }) = e
            && let Ok(n) = i.base10_parse::<u32>()
            && n > 0
        {
            return Some(n);
        }
        self.sink
            .error(e.span(), "an array length is a positive integer literal");
        None
    }
}

/// Scalars, vectors and matrices by their Rust names: `f32`, `Vec3`, `UVec2`, `Mat4`.
pub fn builtin_type(name: &str) -> Option<Type> {
    let scalar = match name {
        "f32" => Some(Scalar::F32),
        "f16" => Some(Scalar::F16),
        "i32" => Some(Scalar::I32),
        "u32" => Some(Scalar::U32),
        "bool" => Some(Scalar::Bool),
        _ => None,
    };
    if let Some(s) = scalar {
        return Some(Type::Scalar(s));
    }
    match name {
        "Sampler" => return Some(Type::Sampler { comparison: false }),
        "SamplerComparison" => return Some(Type::Sampler { comparison: true }),
        _ => {}
    }
    let (s, rest) = [
        ("Vec", Scalar::F32),
        ("HVec", Scalar::F16),
        ("IVec", Scalar::I32),
        ("UVec", Scalar::U32),
        ("BVec", Scalar::Bool),
    ]
    .iter()
    .find_map(|(p, s)| name.strip_prefix(p).map(|r| (*s, r)))
    .unwrap_or((Scalar::F32, ""));
    if let Ok(n @ 2..=4) = rest.parse::<u8>() {
        return Some(Type::Vector(n, s));
    }
    if let Some(r) = name.strip_prefix("Mat")
        && let Ok(n @ 2..=4) = r.parse::<u8>()
    {
        return Some(Type::Matrix {
            cols: n,
            rows: n,
            scalar: Scalar::F32,
        });
    }
    None
}
