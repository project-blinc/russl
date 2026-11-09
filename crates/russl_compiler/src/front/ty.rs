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
                if let Some(&ty) = self.generic_types.get(&name) {
                    return ty;
                }
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
                        match self.scope().get(&sym) {
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

    /// An array length: a positive integer built at compile time from literals, constants,
    /// arithmetic and casts.
    fn array_len(&mut self, e: &syn::Expr) -> Option<u32> {
        match self.const_int(e, self.current, 0) {
            Ok(n) if n > 0 && n <= u32::MAX as i128 => Some(n as u32),
            Ok(n) => {
                self.sink.error(
                    e.span(),
                    format!("an array length is positive, and this is {n}"),
                );
                None
            }
            Err((span, message)) => {
                self.sink.error(span, message);
                None
            }
        }
    }

    /// The value of an integer expression known when the shader is built, read in the module
    /// being built.
    pub fn const_value(&mut self, e: &syn::Expr) -> Result<i128, (proc_macro2::Span, String)> {
        self.const_int(e, self.current, 0)
    }

    /// Whether `ident` names a constant here: a const parameter or a module constant.
    pub fn is_constant(&self, ident: &syn::Ident) -> bool {
        let name = ident.to_string();
        self.generic_consts.contains_key(&name)
            || self.scopes[self.current]
                .iter()
                .any(|(s, i)| matches!(i, Item::Const(_)) && self.names.get(*s) == name)
    }

    /// The value of an integer expression known when the shader is built, read in `module`.
    fn const_int(
        &mut self,
        e: &syn::Expr,
        module: usize,
        depth: u32,
    ) -> Result<i128, (proc_macro2::Span, String)> {
        let fail = |e: &dyn Spanned, m: &str| Err((e.span(), m.to_string()));
        match e {
            syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Int(i),
                ..
            }) => i
                .base10_parse::<i128>()
                .map_err(|err| (i.span(), err.to_string())),
            syn::Expr::Paren(p) => self.const_int(&p.expr, module, depth),
            syn::Expr::Group(g) => self.const_int(&g.expr, module, depth),
            syn::Expr::Cast(c) => self.const_int(&c.expr, module, depth),
            syn::Expr::Unary(syn::ExprUnary {
                op: syn::UnOp::Neg(_),
                expr,
                ..
            }) => Ok(-self.const_int(expr, module, depth)?),
            syn::Expr::Binary(b) => {
                let (l, r) = (
                    self.const_int(&b.left, module, depth)?,
                    self.const_int(&b.right, module, depth)?,
                );
                use syn::BinOp as B;
                let v = match b.op {
                    B::Add(_) => l.checked_add(r),
                    B::Sub(_) => l.checked_sub(r),
                    B::Mul(_) => l.checked_mul(r),
                    B::Div(_) => l.checked_div(r),
                    B::Rem(_) => l.checked_rem(r),
                    B::Shl(_) => u32::try_from(r).ok().and_then(|r| l.checked_shl(r)),
                    B::Shr(_) => u32::try_from(r).ok().and_then(|r| l.checked_shr(r)),
                    B::BitAnd(_) => Some(l & r),
                    B::BitOr(_) => Some(l | r),
                    B::BitXor(_) => Some(l ^ r),
                    _ => return fail(b, "an array length takes integer arithmetic only"),
                };
                v.ok_or_else(|| (b.span(), "this overflows or divides by zero".to_string()))
            }
            syn::Expr::Path(p) if p.path.get_ident().is_some() => {
                let ident = p.path.get_ident().expect("checked");
                if let Some(&(v, _)) = self.generic_consts.get(&ident.to_string()) {
                    return Ok(v);
                }
                let sym = self.names.intern(&ident.to_string());
                match self.scopes[module].get(&sym).copied() {
                    Some(Item::Const(id)) => {
                        if depth > 64 {
                            return fail(
                                ident,
                                "these constants are defined in terms of each other",
                            );
                        }
                        match self.const_src[id.index()].clone() {
                            Some((init, m)) => self.const_int(&init, m, depth + 1),
                            None => fail(
                                ident,
                                &format!(
                                    "`{ident}` is a pipeline constant, set per pipeline; an array length is fixed when the shader is built"
                                ),
                            ),
                        }
                    }
                    _ => fail(ident, &format!("cannot find constant `{ident}`")),
                }
            }
            other => fail(
                other,
                "an array length is an integer built from literals and constants",
            ),
        }
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
