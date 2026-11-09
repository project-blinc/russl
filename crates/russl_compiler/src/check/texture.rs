//! Texture operations. A texture's dimension and kind decide how many coordinates it takes,
//! whether it takes a layer, and what a read returns.

use super::Checker;
use crate::builtins::Tex;
use crate::hir::*;
use crate::types::{Scalar, TextureDim, TextureKind, Type, TypeId};

impl Checker<'_> {
    pub(super) fn texture(&mut self, e: ExprId, op: Tex, args: &[ExprId]) -> TypeId {
        let error = self.types.error;
        let name = op.method();
        let Some(&image) = args.first() else {
            self.error_at(e, format!("`{name}` is called on a texture"));
            return error;
        };
        let image_ty = self.infer(image);
        let (dim, kind) = match self.types.get(image_ty) {
            Type::Texture { dim, kind } => (dim, kind),
            Type::Error => return error,
            _ => {
                let have = self.ty_name(image_ty);
                self.error_at(
                    image,
                    format!("`{name}` is a texture method, not one of `{have}`"),
                );
                return error;
            }
        };
        let layered = dim == TextureDim::D2Array;
        let coords = match dim {
            TextureDim::D1 => 1,
            TextureDim::D2 | TextureDim::D2Array => 2,
            TextureDim::D3 | TextureDim::Cube => 3,
        };
        let depth = kind == TextureKind::Depth;
        let rest = &args[1..];
        let want = match op {
            Tex::Sample => 2,
            Tex::SampleLevel => 3,
            Tex::Load => 2,
            Tex::Dimensions => 0,
        } + usize::from(layered && op != Tex::Dimensions);
        if rest.len() != want {
            self.error_at(
                e,
                format!(
                    "`{name}` on this texture takes {want} arguments, {} given",
                    rest.len()
                ),
            );
            return error;
        }
        let texel = match kind {
            TextureKind::Depth => self.types.f32,
            TextureKind::Sampled(s) | TextureKind::Multisampled(s) => {
                self.types.add(Type::Vector(4, s))
            }
        };
        match op {
            Tex::Sample | Tex::SampleLevel => {
                if matches!(
                    kind,
                    TextureKind::Multisampled(_) | TextureKind::Sampled(Scalar::I32 | Scalar::U32)
                ) {
                    self.error_at(
                        image,
                        "only a float or depth texture is sampled; read this one with `load`",
                    );
                    return error;
                }
                let sampler = self.types.add(Type::Sampler { comparison: false });
                self.check(rest[0], Some(sampler));
                let coord = self.float_coords(coords);
                self.check(rest[1], Some(coord));
                let mut next = 2;
                if layered {
                    self.int_arg(rest[next], 1, "a layer");
                    next += 1;
                }
                if op == Tex::SampleLevel {
                    if depth {
                        self.int_arg(rest[next], 1, "a depth texture's level");
                    } else {
                        let f32 = self.types.f32;
                        self.check(rest[next], Some(f32));
                    }
                } else {
                    self.fragment_only(e, "`sample`");
                }
                texel
            }
            Tex::Load => {
                if dim == TextureDim::Cube {
                    self.error_at(image, "a cube texture is sampled, not loaded");
                    return error;
                }
                self.int_arg(rest[0], coords, "texel coordinates");
                if layered {
                    self.int_arg(rest[1], 1, "a layer");
                }
                let what = if matches!(kind, TextureKind::Multisampled(_)) {
                    "a sample index"
                } else {
                    "a level"
                };
                self.int_arg(rest[want - 1], 1, what);
                texel
            }
            Tex::Dimensions => match dim {
                TextureDim::D1 => self.types.u32,
                TextureDim::D3 => self.types.add(Type::Vector(3, Scalar::U32)),
                _ => self.types.add(Type::Vector(2, Scalar::U32)),
            },
        }
    }

    fn float_coords(&mut self, n: u8) -> TypeId {
        if n == 1 {
            self.types.f32
        } else {
            self.types.add(Type::Vector(n, Scalar::F32))
        }
    }

    /// An `i32` or `u32` argument with `n` components.
    fn int_arg(&mut self, e: ExprId, n: u8, what: &str) {
        let t = self.check_concrete(e);
        let ok = match self.types.get(t) {
            Type::Scalar(s) => n == 1 && matches!(s, Scalar::I32 | Scalar::U32),
            Type::Vector(m, s) => m == n && matches!(s, Scalar::I32 | Scalar::U32),
            Type::Error => true,
            _ => false,
        };
        if !ok {
            let have = self.ty_name(t);
            let want = if n == 1 {
                "`i32` or `u32`".to_string()
            } else {
                format!("`IVec{n}` or `UVec{n}`")
            };
            self.error_at(e, format!("{what} is {want}, not `{have}`"));
        }
    }

    /// Record an operation only a fragment shader can run, checked once the call graph is known.
    pub(super) fn fragment_only(&mut self, e: ExprId, what: &'static str) {
        if let super::Target::Fn(f) = self.target {
            let span = self.body().expr(e).span;
            self.fragment_ops.push((f, span, what));
        }
    }
}
