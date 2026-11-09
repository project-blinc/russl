//! The type arena. Types are hash-consed, so two equal types share one [`TypeId`] and type
//! equality is an integer comparison.

use crate::intern::{Interner, Symbol};
use std::collections::HashMap;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct TypeId(u32);

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct StructId(pub u32);

/// Scalar kinds. The abstract kinds are the types of unsuffixed literals until context
/// decides, as in WGSL.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Scalar {
    Bool,
    I32,
    U32,
    F32,
    F16,
    AbstractInt,
    AbstractFloat,
}

impl Scalar {
    pub fn is_float(self) -> bool {
        matches!(self, Scalar::F32 | Scalar::F16 | Scalar::AbstractFloat)
    }

    pub fn is_int(self) -> bool {
        matches!(self, Scalar::I32 | Scalar::U32 | Scalar::AbstractInt)
    }

    pub fn is_numeric(self) -> bool {
        self.is_float() || self.is_int()
    }

    pub fn is_abstract(self) -> bool {
        matches!(self, Scalar::AbstractInt | Scalar::AbstractFloat)
    }

    /// Whether an abstract scalar converts to `to` without a cast.
    pub fn converts_to(self, to: Scalar) -> bool {
        self == to
            || match self {
                Scalar::AbstractInt => to.is_numeric(),
                Scalar::AbstractFloat => to.is_float(),
                _ => false,
            }
    }

    /// The concrete scalar an abstract one becomes with no other context.
    pub fn concrete(self) -> Scalar {
        match self {
            Scalar::AbstractInt => Scalar::I32,
            Scalar::AbstractFloat => Scalar::F32,
            s => s,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Scalar::Bool => "bool",
            Scalar::I32 => "i32",
            Scalar::U32 => "u32",
            Scalar::F32 => "f32",
            Scalar::F16 => "f16",
            Scalar::AbstractInt => "{integer}",
            Scalar::AbstractFloat => "{float}",
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum TextureDim {
    D1,
    D2,
    D2Array,
    D3,
    Cube,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum TextureKind {
    Sampled(Scalar),
    Multisampled(Scalar),
    Depth,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Type {
    Unit,
    Scalar(Scalar),
    Vector(u8, Scalar),
    Matrix {
        cols: u8,
        rows: u8,
        scalar: Scalar,
    },
    /// `len` is `None` for a runtime-sized array.
    Array {
        elem: TypeId,
        len: Option<u32>,
    },
    Struct(StructId),
    Atomic(Scalar),
    Sampler {
        comparison: bool,
    },
    Texture {
        dim: TextureDim,
        kind: TextureKind,
    },
    /// The type of an expression that already produced an error, so it reports once.
    Error,
}

/// A struct's members, in declaration order.
#[derive(Debug)]
pub struct StructDef {
    pub name: Symbol,
    pub fields: Vec<(Symbol, TypeId)>,
}

/// Every type a module uses.
#[derive(Debug)]
pub struct Types {
    types: Vec<Type>,
    map: HashMap<Type, TypeId>,
    pub structs: Vec<StructDef>,
    pub unit: TypeId,
    pub error: TypeId,
    pub bool: TypeId,
    pub i32: TypeId,
    pub u32: TypeId,
    pub f32: TypeId,
}

impl Default for Types {
    fn default() -> Self {
        let mut t = Types {
            types: Vec::new(),
            map: HashMap::new(),
            structs: Vec::new(),
            unit: TypeId(0),
            error: TypeId(0),
            bool: TypeId(0),
            i32: TypeId(0),
            u32: TypeId(0),
            f32: TypeId(0),
        };
        t.unit = t.add(Type::Unit);
        t.error = t.add(Type::Error);
        t.bool = t.add(Type::Scalar(Scalar::Bool));
        t.i32 = t.add(Type::Scalar(Scalar::I32));
        t.u32 = t.add(Type::Scalar(Scalar::U32));
        t.f32 = t.add(Type::Scalar(Scalar::F32));
        t
    }
}

impl Types {
    pub fn add(&mut self, ty: Type) -> TypeId {
        if let Some(&id) = self.map.get(&ty) {
            return id;
        }
        let id = TypeId(self.types.len() as u32);
        self.types.push(ty);
        self.map.insert(ty, id);
        id
    }

    pub fn get(&self, id: TypeId) -> Type {
        self.types[id.0 as usize]
    }

    pub fn scalar(&mut self, s: Scalar) -> TypeId {
        self.add(Type::Scalar(s))
    }

    /// The scalar of a scalar, vector or matrix.
    pub fn scalar_of(&self, id: TypeId) -> Option<Scalar> {
        match self.get(id) {
            Type::Scalar(s) | Type::Vector(_, s) | Type::Matrix { scalar: s, .. } => Some(s),
            _ => None,
        }
    }

    /// The same shape with its scalar replaced: how an abstract literal takes a concrete type.
    pub fn with_scalar(&mut self, id: TypeId, s: Scalar) -> TypeId {
        match self.get(id) {
            Type::Scalar(_) => self.add(Type::Scalar(s)),
            Type::Vector(n, _) => self.add(Type::Vector(n, s)),
            Type::Matrix { cols, rows, .. } => self.add(Type::Matrix {
                cols,
                rows,
                scalar: s,
            }),
            Type::Array { elem, len } => {
                let elem = self.with_scalar(elem, s);
                self.add(Type::Array { elem, len })
            }
            _ => id,
        }
    }

    pub fn is_abstract(&self, id: TypeId) -> bool {
        match self.get(id) {
            Type::Array { elem, .. } => self.is_abstract(elem),
            _ => self.scalar_of(id).is_some_and(Scalar::is_abstract),
        }
    }

    /// The type an abstract type becomes with no other context.
    pub fn concrete(&mut self, id: TypeId) -> TypeId {
        match self.scalar_of(id) {
            Some(s) if s.is_abstract() => self.with_scalar(id, s.concrete()),
            _ => match self.get(id) {
                Type::Array { elem, len } if self.is_abstract(elem) => {
                    let elem = self.concrete(elem);
                    self.add(Type::Array { elem, len })
                }
                _ => id,
            },
        }
    }

    /// Whether a value of `from` can stand where `to` is expected, converting abstract literals.
    pub fn converts_to(&self, from: TypeId, to: TypeId) -> bool {
        if from == to || self.get(from) == Type::Error || self.get(to) == Type::Error {
            return true;
        }
        match (self.get(from), self.get(to)) {
            (Type::Scalar(a), Type::Scalar(b)) => a.converts_to(b),
            (Type::Vector(n, a), Type::Vector(m, b)) => n == m && a.converts_to(b),
            (
                Type::Matrix {
                    cols: c,
                    rows: r,
                    scalar: a,
                },
                Type::Matrix {
                    cols: c2,
                    rows: r2,
                    scalar: b,
                },
            ) => c == c2 && r == r2 && a.converts_to(b),
            (Type::Array { elem: a, len: n }, Type::Array { elem: b, len: m }) => {
                n == m && self.converts_to(a, b)
            }
            _ => false,
        }
    }

    /// The type as the source spells it, for diagnostics.
    pub fn name(&self, id: TypeId, names: &Interner) -> String {
        match self.get(id) {
            Type::Unit => "()".into(),
            Type::Error => "{unknown}".into(),
            Type::Scalar(s) => s.name().into(),
            Type::Vector(n, s) => format!("{}Vec{n}", vector_prefix(s)),
            Type::Matrix { cols, rows, .. } if cols == rows => format!("Mat{cols}"),
            Type::Matrix { cols, rows, .. } => format!("Mat{cols}x{rows}"),
            Type::Array { elem, len: Some(n) } => format!("[{}; {n}]", self.name(elem, names)),
            Type::Array { elem, len: None } => format!("[{}]", self.name(elem, names)),
            Type::Struct(s) => names.get(self.structs[s.0 as usize].name).into(),
            Type::Atomic(s) => format!("Atomic<{}>", s.name()),
            Type::Sampler { comparison: false } => "Sampler".into(),
            Type::Sampler { comparison: true } => "SamplerComparison".into(),
            Type::Texture { .. } => "texture".into(),
        }
    }

    /// Whether a type may live in host-shared memory (uniform and storage buffers).
    /// WGSL's size and alignment of a host-shareable type, in bytes. A runtime-sized array
    /// counts one element.
    pub fn size_align(&self, id: TypeId) -> (u32, u32) {
        let scalar = |s: Scalar| if s == Scalar::F16 { 2 } else { 4 };
        let vector = |n: u8, s: Scalar| {
            let w = scalar(s);
            (n as u32 * w, if n == 2 { 2 * w } else { 4 * w })
        };
        match self.get(id) {
            Type::Scalar(s) | Type::Atomic(s) => (scalar(s), scalar(s)),
            Type::Vector(n, s) => vector(n, s),
            Type::Matrix { cols, rows, scalar } => {
                let (size, align) = vector(rows, scalar);
                (cols as u32 * round_up(align, size), align)
            }
            Type::Array { elem, len } => {
                let (size, align) = self.size_align(elem);
                (len.unwrap_or(1) * round_up(align, size), align)
            }
            Type::Struct(s) => {
                let (mut end, mut align) = (0, 1);
                for &(_, t) in &self.structs[s.0 as usize].fields {
                    let (size, a) = self.size_align(t);
                    end = round_up(a, end) + size;
                    align = align.max(a);
                }
                (round_up(align, end), align)
            }
            _ => (0, 1),
        }
    }

    pub fn host_shareable(&self, id: TypeId) -> bool {
        match self.get(id) {
            Type::Scalar(s) | Type::Vector(_, s) | Type::Matrix { scalar: s, .. } => {
                s != Scalar::Bool && !s.is_abstract()
            }
            Type::Atomic(_) => true,
            Type::Array { elem, .. } => self.host_shareable(elem),
            Type::Struct(s) => self.structs[s.0 as usize]
                .fields
                .iter()
                .all(|&(_, t)| self.host_shareable(t)),
            _ => false,
        }
    }
}

fn vector_prefix(s: Scalar) -> &'static str {
    match s {
        Scalar::Bool => "B",
        Scalar::I32 => "I",
        Scalar::U32 => "U",
        Scalar::F16 => "H",
        _ => "",
    }
}

/// `n` rounded up to a multiple of `align`.
pub fn round_up(align: u32, n: u32) -> u32 {
    n.div_ceil(align) * align
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_types_share_one_id() {
        let mut t = Types::default();
        let a = t.add(Type::Vector(3, Scalar::F32));
        assert_eq!(t.add(Type::Vector(3, Scalar::F32)), a);
        assert_ne!(t.add(Type::Vector(3, Scalar::U32)), a);
    }

    #[test]
    fn an_abstract_float_vector_becomes_f32() {
        let mut t = Types::default();
        let abs = t.add(Type::Vector(2, Scalar::AbstractFloat));
        let f = t.add(Type::Vector(2, Scalar::F32));
        assert!(t.converts_to(abs, f));
        assert_eq!(t.concrete(abs), f);
        let u = t.add(Type::Vector(2, Scalar::U32));
        assert!(!t.converts_to(abs, u));
    }
}
