//! What the host needs to know of a compiled module: the layout of its structs as WGSL lays
//! them out in buffers, its bindings, the pipeline constants it was built with, and what its
//! vertex entry points read. The macro turns it into Rust twins and constants.

use crate::extension::BindingKind;
use crate::hir::{self, Io};
use crate::intern::Interner;
use crate::types::{Scalar, Type, TypeId, Types, round_up};

#[derive(Debug, Clone, Default)]
pub struct Reflection {
    /// The module's own structs, in declaration order.
    pub structs: Vec<StructLayout>,
    /// The resources the program binds.
    pub bindings: Vec<BindingLayout>,
    /// The module's pipeline constants, at the values this artifact was built with.
    pub constants: Vec<(String, crate::ConstValue)>,
    /// What each vertex entry point reads from vertex buffers.
    pub vertex_inputs: Vec<VertexInputs>,
    /// The structs the module imports, for host code that names them.
    pub struct_imports: Vec<StructImport>,
    /// How each native target binds the module's resources.
    pub native: crate::native::NativeBindings,
}

/// `use path::Name as Alias;` naming a struct.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructImport {
    /// The module part of the `use` path: `super::common`.
    pub module: String,
    pub name: String,
    pub alias: String,
    pub public: bool,
}

#[derive(Debug, Clone)]
pub struct StructLayout {
    pub name: String,
    /// The size a buffer holds it in: its last member's end rounded up to its alignment.
    pub size: u32,
    pub align: u32,
    pub fields: Vec<FieldLayout>,
    /// Whether a buffer can hold it: no `bool` anywhere inside.
    pub host_shareable: bool,
}

#[derive(Debug, Clone)]
pub struct FieldLayout {
    pub name: String,
    pub offset: u32,
    pub size: u32,
    pub ty: HostType,
}

/// A buffer member's type, as the host lays out the same bytes.
#[derive(Debug, Clone, PartialEq)]
pub enum HostType {
    Scalar(HostScalar),
    Vector(u8, HostScalar),
    /// Column-major; each column is a vector of `rows`, padded to its alignment.
    Matrix {
        cols: u8,
        rows: u8,
        scalar: HostScalar,
    },
    /// `len` is `None` for a runtime-sized array. `stride` is the distance between elements.
    Array {
        elem: Box<HostType>,
        len: Option<u32>,
        stride: u32,
    },
    /// A struct of the program's module `module` (0 is the one compiled).
    Struct {
        name: String,
        module: usize,
    },
    Atomic(HostScalar),
    /// What a buffer cannot hold.
    Unsupported(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostScalar {
    F32,
    F16,
    I32,
    U32,
}

#[derive(Debug, Clone)]
pub struct BindingLayout {
    pub name: String,
    pub group: u32,
    pub binding: u32,
    pub kind: BindingKind,
    /// For a buffer, the size of its type (one element of a runtime-sized array): wgpu's
    /// minimum binding size.
    pub size: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct VertexInputs {
    pub entry_point: String,
    pub inputs: Vec<VertexInput>,
}

#[derive(Debug, Clone)]
pub struct VertexInput {
    pub name: String,
    pub location: u32,
    /// The `wgpu::VertexFormat` variant that holds it: `Float32x2`.
    pub format: &'static str,
}

pub(crate) fn program(
    module: &hir::Module,
    emitted: &[hir::GlobalId],
    types: &Types,
    names: &Interner,
) -> Reflection {
    let structs = module
        .structs
        .iter()
        .filter(|s| s.module == 0)
        .map(|s| {
            let (size, align) = types.size_align(s.ty);
            let mut end = 0;
            let fields = s
                .fields
                .iter()
                .map(|f| {
                    let (fsize, falign) = types.size_align(f.ty);
                    let offset = round_up(falign, end);
                    end = offset + fsize;
                    FieldLayout {
                        name: names.get(f.name).to_string(),
                        offset,
                        size: fsize,
                        ty: host_type(f.ty, module, types, names),
                    }
                })
                .collect();
            StructLayout {
                name: crate::types::ident_name(names.get(s.name)),
                size,
                align,
                fields,
                host_shareable: types.host_shareable(s.ty),
            }
        })
        .collect();
    let bindings = emitted
        .iter()
        .map(|g| &module.globals[g.index()])
        .filter_map(|g| {
            let (group, binding) = g.binding?;
            let kind = match g.space {
                hir::Space::Uniform => BindingKind::Uniform,
                hir::Space::Storage { writable } => BindingKind::Storage { writable },
                hir::Space::Handle => BindingKind::Handle,
                _ => return None,
            };
            let size = (kind != BindingKind::Handle).then(|| types.size_align(g.ty).0);
            Some(BindingLayout {
                name: names.get(g.name).to_string(),
                group,
                binding,
                kind,
                size,
            })
        })
        .collect();
    let vertex_inputs = module
        .functions
        .iter()
        .filter(|f| f.module == 0 && f.stage == Some(hir::Stage::Vertex))
        .map(|f| {
            let mut inputs = Vec::new();
            for p in &f.params {
                let local = &f.body.locals[p.local.index()];
                let name = names.get(local.name).to_string();
                match (p.io, types.get(local.ty)) {
                    (Some(Io::Location { location, .. }), _) => {
                        if let Some(format) = vertex_format(types.get(local.ty)) {
                            inputs.push(VertexInput {
                                name,
                                location,
                                format,
                            });
                        }
                    }
                    (None, Type::Struct(_)) => {
                        let st = module.structs.iter().find(|x| x.ty == local.ty);
                        for field in st.map(|st| &st.fields[..]).unwrap_or_default() {
                            if let Some(Io::Location { location, .. }) = field.io
                                && let Some(format) = vertex_format(types.get(field.ty))
                            {
                                inputs.push(VertexInput {
                                    name: names.get(field.name).to_string(),
                                    location,
                                    format,
                                });
                            }
                        }
                    }
                    _ => {}
                }
            }
            inputs.sort_by_key(|i| i.location);
            VertexInputs {
                entry_point: names.get(f.name).to_string(),
                inputs,
            }
        })
        .collect();
    let struct_imports = module
        .struct_imports
        .iter()
        .map(|(module, name, alias, public)| StructImport {
            module: module.clone(),
            name: name.clone(),
            alias: alias.clone(),
            public: *public,
        })
        .collect();
    Reflection {
        structs,
        bindings,
        constants: Vec::new(),
        vertex_inputs,
        struct_imports,
        native: Default::default(),
    }
}

fn host_type(ty: TypeId, module: &hir::Module, types: &Types, names: &Interner) -> HostType {
    let scalar = |s: Scalar| match s {
        Scalar::F32 | Scalar::AbstractFloat => Some(HostScalar::F32),
        Scalar::F16 => Some(HostScalar::F16),
        Scalar::I32 | Scalar::AbstractInt => Some(HostScalar::I32),
        Scalar::U32 => Some(HostScalar::U32),
        Scalar::Bool => None,
    };
    let unsupported = || HostType::Unsupported(types.name(ty, names));
    match types.get(ty) {
        Type::Scalar(s) => scalar(s).map_or_else(unsupported, HostType::Scalar),
        Type::Vector(n, s) => scalar(s).map_or_else(unsupported, |s| HostType::Vector(n, s)),
        Type::Matrix {
            cols,
            rows,
            scalar: s,
        } => scalar(s).map_or_else(unsupported, |scalar| HostType::Matrix {
            cols,
            rows,
            scalar,
        }),
        Type::Array { elem, len } => {
            let (size, align) = types.size_align(elem);
            HostType::Array {
                elem: Box::new(host_type(elem, module, types, names)),
                len,
                stride: round_up(align, size),
            }
        }
        Type::Struct(id) => {
            let st = module.structs.iter().find(|s| s.ty == ty);
            HostType::Struct {
                name: crate::types::ident_name(names.get(types.structs[id.0 as usize].name)),
                module: st.map_or(0, |s| s.module as usize),
            }
        }
        Type::Atomic(s) => scalar(s).map_or_else(unsupported, HostType::Atomic),
        _ => unsupported(),
    }
}

/// The `wgpu::VertexFormat` a vertex input of type `t` reads.
fn vertex_format(t: Type) -> Option<&'static str> {
    Some(match t {
        Type::Scalar(Scalar::F32) => "Float32",
        Type::Vector(2, Scalar::F32) => "Float32x2",
        Type::Vector(3, Scalar::F32) => "Float32x3",
        Type::Vector(4, Scalar::F32) => "Float32x4",
        Type::Scalar(Scalar::U32) => "Uint32",
        Type::Vector(2, Scalar::U32) => "Uint32x2",
        Type::Vector(3, Scalar::U32) => "Uint32x3",
        Type::Vector(4, Scalar::U32) => "Uint32x4",
        Type::Scalar(Scalar::I32) => "Sint32",
        Type::Vector(2, Scalar::I32) => "Sint32x2",
        Type::Vector(3, Scalar::I32) => "Sint32x3",
        Type::Vector(4, Scalar::I32) => "Sint32x4",
        Type::Vector(2, Scalar::F16) => "Float16x2",
        Type::Vector(4, Scalar::F16) => "Float16x4",
        _ => return None,
    })
}
