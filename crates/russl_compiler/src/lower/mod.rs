//! The analyzed IR to a naga module: types with WGSL layout, constants and pipeline constants
//! folded by naga's evaluator, resource globals, functions and entry points. Every naga span is
//! an index into the source-span table.

mod func;

use crate::check::Analysis;
use crate::diag::{Sink, Spans, naga_span};
use crate::hir::{self, ConstId, FnId, Io};
use crate::intern::Interner;
use crate::types::{Scalar, TextureDim, TextureKind, Type, TypeId, Types};
use naga::Handle;
use std::collections::HashMap;
use std::num::NonZeroU32;

#[derive(Copy, Clone)]
enum ConstRef {
    Const(Handle<naga::Constant>),
    Override(Handle<naga::Override>),
}

pub struct Lower<'a> {
    pub module: naga::Module,
    layouter: naga::proc::Layouter,
    tracker: naga::proc::ExpressionKindTracker,
    types: HashMap<TypeId, Handle<naga::Type>>,
    consts: HashMap<ConstId, ConstRef>,
    globals: HashMap<hir::GlobalId, Handle<naga::GlobalVariable>>,
    fns: HashMap<FnId, Handle<naga::Function>>,
    hir: &'a hir::Module,
    ty: &'a Types,
    names: &'a Interner,
    spans: &'a Spans,
    sink: &'a mut Sink,
}

pub fn module(
    hir: &hir::Module,
    analysis: &Analysis,
    ty: &Types,
    names: &Interner,
    spans: &Spans,
    sink: &mut Sink,
) -> naga::Module {
    let mut l = Lower {
        module: naga::Module::default(),
        layouter: naga::proc::Layouter::default(),
        tracker: naga::proc::ExpressionKindTracker::new(),
        types: HashMap::new(),
        consts: HashMap::new(),
        globals: HashMap::new(),
        fns: HashMap::new(),
        hir,
        ty,
        names,
        spans,
        sink,
    };
    // The root module's own constants and globals, in source order; an imported one is lowered
    // when something reached refers to it.
    for (i, k) in hir.consts.iter().enumerate() {
        if k.module == 0 {
            l.constant(ConstId(i as u32));
        }
    }
    for (i, g) in hir.globals.iter().enumerate() {
        if g.module == 0 {
            l.global(hir::GlobalId(i as u32));
        }
    }
    for &f in analysis
        .order
        .iter()
        .filter(|f| analysis.reachable.contains(f))
    {
        let function = func::function(&mut l, f);
        let span = naga_span(hir.functions[f.index()].span);
        let handle = l.module.functions.append(function, span);
        l.fns.insert(f, handle);
    }
    for (i, f) in hir.functions.iter().enumerate() {
        let Some(stage) = f.stage else { continue };
        if f.module != 0 {
            continue;
        }
        let function = func::function(&mut l, FnId(i as u32));
        let (stage, workgroup_size) = match stage {
            hir::Stage::Vertex => (naga::ShaderStage::Vertex, [0; 3]),
            hir::Stage::Fragment => (naga::ShaderStage::Fragment, [0; 3]),
            hir::Stage::Compute(size) => (naga::ShaderStage::Compute, size),
        };
        l.module.entry_points.push(naga::EntryPoint {
            name: names.get(f.name).to_string(),
            stage,
            early_depth_test: None,
            workgroup_size,
            workgroup_size_overrides: None,
            function,
        });
    }
    l.module
}

impl Lower<'_> {
    fn name(&self, sym: crate::intern::Symbol) -> Option<String> {
        Some(self.names.get(sym).to_string())
    }

    /// The naga type of an arena type, with WGSL's layout for structs and arrays.
    pub fn ty(&mut self, id: TypeId) -> Handle<naga::Type> {
        if let Some(&h) = self.types.get(&id) {
            return h;
        }
        let inner = match self.ty.get(id) {
            Type::Scalar(s) => naga::TypeInner::Scalar(scalar(s)),
            Type::Vector(n, s) => naga::TypeInner::Vector {
                size: vector_size(n),
                scalar: scalar(s),
            },
            Type::Matrix {
                cols,
                rows,
                scalar: s,
            } => naga::TypeInner::Matrix {
                columns: vector_size(cols),
                rows: vector_size(rows),
                scalar: scalar(s),
            },
            Type::Array { elem, len } => {
                let base = self.ty(elem);
                self.layout();
                let stride = self.layouter[base].to_stride();
                let size = match len {
                    Some(n) => {
                        naga::ArraySize::Constant(NonZeroU32::new(n).expect("checked positive"))
                    }
                    None => naga::ArraySize::Dynamic,
                };
                naga::TypeInner::Array { base, size, stride }
            }
            Type::Struct(s) => return self.structure(id, s),
            Type::Atomic(s) => naga::TypeInner::Atomic(scalar(s)),
            Type::Sampler { comparison } => naga::TypeInner::Sampler { comparison },
            Type::Texture { dim, kind } => {
                let (dim, arrayed) = match dim {
                    TextureDim::D1 => (naga::ImageDimension::D1, false),
                    TextureDim::D2 => (naga::ImageDimension::D2, false),
                    TextureDim::D2Array => (naga::ImageDimension::D2, true),
                    TextureDim::D3 => (naga::ImageDimension::D3, false),
                    TextureDim::Cube => (naga::ImageDimension::Cube, false),
                };
                let class = match kind {
                    TextureKind::Sampled(s) => naga::ImageClass::Sampled {
                        kind: scalar(s).kind,
                        multi: false,
                    },
                    TextureKind::Multisampled(s) => naga::ImageClass::Sampled {
                        kind: scalar(s).kind,
                        multi: true,
                    },
                    TextureKind::Depth => naga::ImageClass::Depth { multi: false },
                };
                naga::TypeInner::Image {
                    dim,
                    arrayed,
                    class,
                }
            }
            Type::Unit | Type::Error => unreachable!("never lowered"),
        };
        let h = self
            .module
            .types
            .insert(naga::Type { name: None, inner }, naga::Span::UNDEFINED);
        self.types.insert(id, h);
        h
    }

    fn layout(&mut self) {
        if let Err(e) = self.layouter.update(self.module.to_ctx()) {
            self.sink
                .error(proc_macro2::Span::call_site(), format!("layout: {e}"));
        }
    }

    /// A struct, its members at WGSL's offsets.
    fn structure(&mut self, id: TypeId, s: crate::types::StructId) -> Handle<naga::Type> {
        let def = &self
            .hir
            .structs
            .iter()
            .find(|st| st.ty == id)
            .expect("a declared struct");
        let fields: Vec<(crate::intern::Symbol, TypeId, Option<Io>)> =
            def.fields.iter().map(|f| (f.name, f.ty, f.io)).collect();
        let (name, span) = (def.name, def.span);
        let _ = s;
        let mut members = Vec::with_capacity(fields.len());
        let mut offset = 0u32;
        let mut align = naga::proc::Alignment::ONE;
        for (fname, fty, io) in fields {
            let h = self.ty(fty);
            self.layout();
            let lay = self.layouter[h];
            offset = lay.alignment.round_up(offset);
            align = align.max(lay.alignment);
            let binding = io.map(|io| binding(io, &self.module.types[h].inner));
            members.push(naga::StructMember {
                name: self.name(fname),
                ty: h,
                binding,
                offset,
            });
            offset += lay.size;
        }
        let inner = naga::TypeInner::Struct {
            members,
            span: align.round_up(offset),
        };
        let h = self.module.types.insert(
            naga::Type {
                name: self.name(name),
                inner,
            },
            naga_span(span),
        );
        self.types.insert(id, h);
        h
    }

    /// A constant or pipeline constant, its value folded by naga's evaluator.
    fn constant(&mut self, id: ConstId) -> ConstRef {
        if let Some(&c) = self.consts.get(&id) {
            return c;
        }
        let k = &self.hir.consts[id.index()];
        let ty = self.ty(k.ty);
        let span = naga_span(k.span);
        let name = self.name(k.name);
        let pipeline = k.pipeline;
        let init = k.value.as_ref().and_then(|b| b.value.map(|v| (b, v)));
        let init = init.and_then(|(body, v)| self.const_expr(body, v, pipeline.is_some()));
        let r = match pipeline {
            Some(id) => ConstRef::Override(
                self.module
                    .overrides
                    .append(naga::Override { name, id, ty, init }, span),
            ),
            None => match init {
                Some(init) => ConstRef::Const(
                    self.module
                        .constants
                        .append(naga::Constant { name, ty, init }, span),
                ),
                None => unreachable!("a constant without a value is reported before lowering"),
            },
        };
        self.consts.insert(id, r);
        r
    }

    /// A constant-initializer expression in the module's global expressions.
    fn const_expr(
        &mut self,
        body: &hir::Body,
        e: hir::ExprId,
        in_override: bool,
    ) -> Option<Handle<naga::Expression>> {
        use hir::ExprKind as K;
        let expr = body.expr(e);
        let span = naga_span(expr.span);
        let parts = |l: &mut Self, ids: &[hir::ExprId]| {
            ids.iter()
                .map(|&i| l.const_expr(body, i, in_override))
                .collect::<Option<Vec<_>>>()
        };
        let naga_expr = match &expr.kind {
            K::Lit(lit) => naga::Expression::Literal(literal(lit, self.ty.get(expr.ty))),
            K::Zero => naga::Expression::ZeroValue(self.ty(expr.ty)),
            K::Const(c) => match self.constant(*c) {
                ConstRef::Const(h) => naga::Expression::Constant(h),
                ConstRef::Override(h) => naga::Expression::Override(h),
            },
            K::Unary(op, a) => {
                let a_ty = body.expr(*a).ty;
                let op = unary(*op, self.ty.scalar_of(a_ty));
                naga::Expression::Unary {
                    op,
                    expr: self.const_expr(body, *a, in_override)?,
                }
            }
            K::Binary(op, a, b) => {
                let (l, r) = (
                    self.const_expr(body, *a, in_override)?,
                    self.const_expr(body, *b, in_override)?,
                );
                naga::Expression::Binary {
                    op: binary(*op),
                    left: l,
                    right: r,
                }
            }
            K::Construct(ids) => naga::Expression::Compose {
                ty: self.ty(expr.ty),
                components: parts(self, ids)?,
            },
            K::Splat(a) => {
                let Type::Vector(n, _) = self.ty.get(expr.ty) else {
                    return None;
                };
                naga::Expression::Splat {
                    size: vector_size(n),
                    value: self.const_expr(body, *a, in_override)?,
                }
            }
            K::Cast(a) | K::Bitcast(a) => {
                let s = scalar(self.ty.scalar_of(expr.ty)?);
                let convert = matches!(expr.kind, K::Cast(_)).then_some(s.width);
                naga::Expression::As {
                    expr: self.const_expr(body, *a, in_override)?,
                    kind: s.kind,
                    convert,
                }
            }
            K::Builtin(crate::builtins::Builtin::Math(fun), ids) => {
                let args = parts(self, ids)?;
                naga::Expression::Math {
                    fun: *fun,
                    arg: args[0],
                    arg1: args.get(1).copied(),
                    arg2: args.get(2).copied(),
                    arg3: args.get(3).copied(),
                }
            }
            _ => return None,
        };
        let mut eval = naga::proc::ConstantEvaluator::for_wgsl_module(
            &mut self.module,
            &mut self.tracker,
            &mut self.layouter,
            in_override,
        );
        match eval.try_eval_and_append(naga_expr, span) {
            Ok(h) => Some(h),
            Err(err) => {
                self.sink.error(
                    self.spans.get(expr.span),
                    format!("cannot evaluate this constant: {err}"),
                );
                None
            }
        }
    }

    /// A global, lowered the first time it is needed.
    pub fn global(&mut self, id: hir::GlobalId) -> Handle<naga::GlobalVariable> {
        if let Some(&h) = self.globals.get(&id) {
            return h;
        }
        let g = &self.hir.globals[id.index()];
        let ty = self.ty(g.ty);
        let space = match g.space {
            hir::Space::Uniform => naga::AddressSpace::Uniform,
            hir::Space::Storage { writable } => naga::AddressSpace::Storage {
                access: if writable {
                    naga::StorageAccess::LOAD | naga::StorageAccess::STORE
                } else {
                    naga::StorageAccess::LOAD
                },
            },
            hir::Space::Handle => naga::AddressSpace::Handle,
            hir::Space::Private => naga::AddressSpace::Private,
            hir::Space::Workgroup => naga::AddressSpace::WorkGroup,
        };
        let init = g
            .init
            .as_ref()
            .and_then(|b| b.value.and_then(|v| self.const_expr(b, v, false)));
        let binding = g
            .binding
            .map(|(group, binding)| naga::ResourceBinding { group, binding });
        let var = naga::GlobalVariable {
            name: self.name(g.name),
            space,
            binding,
            ty,
            init,
        };
        let h = self.module.global_variables.append(var, naga_span(g.span));
        self.globals.insert(id, h);
        h
    }
}

pub fn scalar(s: Scalar) -> naga::Scalar {
    match s {
        Scalar::Bool => naga::Scalar::BOOL,
        Scalar::I32 | Scalar::AbstractInt => naga::Scalar::I32,
        Scalar::U32 => naga::Scalar::U32,
        Scalar::F32 | Scalar::AbstractFloat => naga::Scalar::F32,
        Scalar::F16 => naga::Scalar::F16,
    }
}

pub fn vector_size(n: u8) -> naga::VectorSize {
    match n {
        2 => naga::VectorSize::Bi,
        3 => naga::VectorSize::Tri,
        _ => naga::VectorSize::Quad,
    }
}

pub fn literal(lit: &hir::Lit, ty: Type) -> naga::Literal {
    let s = match ty {
        Type::Scalar(s) => s.concrete(),
        _ => Scalar::F32,
    };
    match (lit, s) {
        (hir::Lit::Bool(b), _) => naga::Literal::Bool(*b),
        (hir::Lit::Int(v), Scalar::U32) => naga::Literal::U32(*v as u32),
        (hir::Lit::Int(v), Scalar::F32) => naga::Literal::F32(*v as f32),
        (hir::Lit::Int(v), Scalar::F16) => naga::Literal::F16(half::f16::from_f64(*v as f64)),
        (hir::Lit::Int(v), _) => naga::Literal::I32(*v as i32),
        (hir::Lit::Float(v), Scalar::F16) => naga::Literal::F16(half::f16::from_f64(*v)),
        (hir::Lit::Float(v), _) => naga::Literal::F32(*v as f32),
    }
}

pub fn unary(op: hir::UnOp, s: Option<Scalar>) -> naga::UnaryOperator {
    match (op, s) {
        (hir::UnOp::Neg, _) => naga::UnaryOperator::Negate,
        (hir::UnOp::Not, Some(Scalar::Bool)) => naga::UnaryOperator::LogicalNot,
        (hir::UnOp::Not, _) => naga::UnaryOperator::BitwiseNot,
    }
}

pub fn binary(op: hir::BinOp) -> naga::BinaryOperator {
    use hir::BinOp as B;
    use naga::BinaryOperator as N;
    match op {
        B::Add => N::Add,
        B::Sub => N::Subtract,
        B::Mul => N::Multiply,
        B::Div => N::Divide,
        B::Rem => N::Modulo,
        B::And => N::LogicalAnd,
        B::Or => N::LogicalOr,
        B::BitAnd => N::And,
        B::BitOr => N::InclusiveOr,
        B::BitXor => N::ExclusiveOr,
        B::Shl => N::ShiftLeft,
        B::Shr => N::ShiftRight,
        B::Eq => N::Equal,
        B::Ne => N::NotEqual,
        B::Lt => N::Less,
        B::Le => N::LessEqual,
        B::Gt => N::Greater,
        B::Ge => N::GreaterEqual,
    }
}

/// An IO binding, with WGSL's default interpolation: perspective for floats, flat otherwise.
pub fn binding(io: Io, ty: &naga::TypeInner) -> naga::Binding {
    match io {
        Io::Builtin(b) => naga::Binding::BuiltIn(match b {
            hir::Builtin::Position { invariant } => naga::BuiltIn::Position { invariant },
            hir::Builtin::VertexIndex => naga::BuiltIn::VertexIndex,
            hir::Builtin::InstanceIndex => naga::BuiltIn::InstanceIndex,
            hir::Builtin::FrontFacing => naga::BuiltIn::FrontFacing,
            hir::Builtin::FragDepth => naga::BuiltIn::FragDepth,
            hir::Builtin::SampleIndex => naga::BuiltIn::SampleIndex,
            hir::Builtin::SampleMask => naga::BuiltIn::SampleMask,
            hir::Builtin::LocalInvocationId => naga::BuiltIn::LocalInvocationId,
            hir::Builtin::LocalInvocationIndex => naga::BuiltIn::LocalInvocationIndex,
            hir::Builtin::GlobalInvocationId => naga::BuiltIn::GlobalInvocationId,
            hir::Builtin::WorkgroupId => naga::BuiltIn::WorkGroupId,
            hir::Builtin::NumWorkgroups => naga::BuiltIn::NumWorkGroups,
        }),
        Io::Location { location, flat } => {
            let float = ty.scalar_kind() == Some(naga::ScalarKind::Float);
            let (interpolation, sampling) = if float && !flat {
                (
                    Some(naga::Interpolation::Perspective),
                    Some(naga::Sampling::Center),
                )
            } else {
                (Some(naga::Interpolation::Flat), None)
            };
            naga::Binding::Location {
                location,
                interpolation,
                sampling,
                blend_src: None,
            }
        }
    }
}
