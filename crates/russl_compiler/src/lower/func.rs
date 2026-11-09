//! Functions to naga: expressions with naga's `Emit` bookkeeping, places as pointers, and Rust's
//! control flow mapped onto naga's.

use super::{ConstRef, Lower, binary, literal, scalar, unary, vector_size};
use crate::builtins::{Builtin, Tex};
use crate::diag::naga_span;
use crate::hir::{self, ExprId, ExprKind, FnId, LocalId, StmtKind};
use crate::types::{Scalar, TextureDim, TextureKind, Type};
use naga::{Expression as E, Handle, Statement as S};
use std::collections::HashMap;

/// How a local is held: an immutable value, or a variable behind a pointer.
#[derive(Copy, Clone)]
enum Slot {
    Value(Handle<E>),
    Var(Handle<naga::LocalVariable>),
}

struct FnLower<'l, 'a> {
    l: &'l mut Lower<'a>,
    body: &'a hir::Body,
    func: naga::Function,
    emitter: naga::proc::Emitter,
    blocks: Vec<naga::Block>,
    slots: HashMap<LocalId, Slot>,
}

pub fn function<'a>(l: &mut Lower<'a>, id: FnId) -> naga::Function {
    let hir: &'a hir::Module = l.hir;
    let f: &'a hir::Function = &hir.functions[id.index()];
    let body = &f.body;
    let mut func = naga::Function {
        name: Some(l.names.get(f.name).to_string()),
        ..Default::default()
    };
    let mut slots = HashMap::new();
    let mut entry_args = Vec::new();
    for (i, p) in f.params.iter().enumerate() {
        let local = &body.locals[p.local.index()];
        let ty = l.ty(local.ty);
        let binding = p.io.map(|io| super::binding(io, &l.module.types[ty].inner));
        func.arguments.push(naga::FunctionArgument {
            name: Some(l.names.get(local.name).to_string()),
            ty,
            binding,
        });
        entry_args.push((p.local, i as u32, ty, local.mutable, local.name));
    }
    if let Some((ty, io)) = f.ret
        && ty != l.ty.unit
    {
        let h = l.ty(ty);
        let binding = io.map(|io| super::binding(io, &l.module.types[h].inner));
        func.result = Some(naga::FunctionResult { ty: h, binding });
    }
    let mut fl = FnLower {
        l,
        body,
        func,
        emitter: Default::default(),
        blocks: vec![naga::Block::new()],
        slots: HashMap::new(),
    };
    fl.emitter.start(&fl.func.expressions);
    for (local, index, ty, mutable, name) in entry_args {
        let arg = fl.append(E::FunctionArgument(index), naga::Span::UNDEFINED);
        if mutable {
            // Parameters are immutable in WGSL: a `mut` one is copied into a variable.
            let var = fl.func.local_variables.append(
                naga::LocalVariable {
                    name: Some(fl.l.names.get(name).to_string()),
                    ty,
                    init: None,
                },
                naga::Span::UNDEFINED,
            );
            let ptr = fl.append(E::LocalVariable(var), naga::Span::UNDEFINED);
            fl.push(
                S::Store {
                    pointer: ptr,
                    value: arg,
                },
                naga::Span::UNDEFINED,
            );
            slots.insert(local, Slot::Var(var));
        } else {
            slots.insert(local, Slot::Value(arg));
        }
    }
    fl.slots = slots;
    fl.block(&body.block);
    fl.flush();
    let _ = fl.emitter.finish(&fl.func.expressions);
    let mut func = fl.func;
    func.body = fl.blocks.pop().expect("the body block");
    func
}

impl FnLower<'_, '_> {
    fn block_mut(&mut self) -> &mut naga::Block {
        self.blocks.last_mut().expect("a block")
    }

    /// Close the running `Emit` range into the current block and start a new one.
    fn flush(&mut self) {
        if let Some((stmt, span)) = self.emitter.finish(&self.func.expressions) {
            self.block_mut().push(stmt, span);
        }
        self.emitter.start(&self.func.expressions);
    }

    fn push(&mut self, stmt: S, span: naga::Span) {
        self.flush();
        self.block_mut().push(stmt, span);
    }

    /// Append an expression. Those naga treats as already evaluated sit outside `Emit` ranges.
    fn append(&mut self, expr: E, span: naga::Span) -> Handle<E> {
        let pre = matches!(
            expr,
            E::Literal(_)
                | E::Constant(_)
                | E::Override(_)
                | E::ZeroValue(_)
                | E::FunctionArgument(_)
                | E::GlobalVariable(_)
                | E::LocalVariable(_)
                | E::CallResult(_)
        );
        if pre {
            if let Some((stmt, s)) = self.emitter.finish(&self.func.expressions) {
                self.block_mut().push(stmt, s);
            }
            let h = self.func.expressions.append(expr, span);
            self.emitter.start(&self.func.expressions);
            h
        } else {
            self.func.expressions.append(expr, span)
        }
    }

    fn nested(&mut self, stmts: &hir::Block) -> naga::Block {
        self.flush();
        self.blocks.push(naga::Block::new());
        self.block(stmts);
        self.flush();
        self.blocks.pop().expect("the nested block")
    }

    fn block(&mut self, stmts: &hir::Block) {
        for s in stmts {
            self.stmt(s);
        }
    }

    fn stmt(&mut self, s: &hir::Stmt) {
        let span = naga_span(s.span);
        match &s.kind {
            StmtKind::Let { local, init } => {
                let l = &self.body.locals[local.index()];
                if l.mutable {
                    let ty = self.l.ty(l.ty);
                    let name = Some(self.l.names.get(l.name).to_string());
                    let var = self.func.local_variables.append(
                        naga::LocalVariable {
                            name,
                            ty,
                            init: None,
                        },
                        span,
                    );
                    if let Some(init) = init {
                        let value = self.value(*init);
                        let ptr = self.append(E::LocalVariable(var), span);
                        self.push(
                            S::Store {
                                pointer: ptr,
                                value,
                            },
                            span,
                        );
                    }
                    self.slots.insert(*local, Slot::Var(var));
                } else if let Some(init) = init {
                    let value = self.value(*init);
                    self.func
                        .named_expressions
                        .entry(value)
                        .or_insert_with(|| self.l.names.get(l.name).to_string());
                    self.slots.insert(*local, Slot::Value(value));
                }
            }
            StmtKind::Assign { target, op, value } => {
                let ptr = self.place(*target).expect("the analyzer checked the place");
                let mut v = self.value(*value);
                if let Some(op) = op {
                    let current = self.append(E::Load { pointer: ptr }, span);
                    let tt = self.body.expr(*target).ty;
                    let vt = self.body.expr(*value).ty;
                    v = self.binary(*op, current, tt, v, vt, span);
                }
                self.push(
                    S::Store {
                        pointer: ptr,
                        value: v,
                    },
                    span,
                );
            }
            StmtKind::Expr(e) => {
                if let ExprKind::Call(f, args) = &self.body.expr(*e).kind {
                    self.call(*f, args, span);
                } else {
                    self.value(*e);
                }
            }
            StmtKind::If {
                cond,
                then,
                otherwise,
            } => {
                let condition = self.value(*cond);
                let accept = self.nested(then);
                let reject = otherwise
                    .as_ref()
                    .map(|o| self.nested(o))
                    .unwrap_or_default();
                self.push(
                    S::If {
                        condition,
                        accept,
                        reject,
                    },
                    span,
                );
            }
            StmtKind::While { cond, body } => {
                self.flush();
                self.blocks.push(naga::Block::new());
                let c = self.value(*cond);
                self.break_unless(c, span);
                self.block(body);
                self.flush();
                let body = self.blocks.pop().expect("loop body");
                self.push(
                    S::Loop {
                        body,
                        continuing: naga::Block::new(),
                        break_if: None,
                    },
                    span,
                );
            }
            StmtKind::Loop { body } => {
                let body = self.nested(body);
                self.push(
                    S::Loop {
                        body,
                        continuing: naga::Block::new(),
                        break_if: None,
                    },
                    span,
                );
            }
            StmtKind::For {
                var,
                start,
                end,
                inclusive,
                body,
            } => self.for_loop(*var, *start, *end, *inclusive, body, span),
            StmtKind::Switch { selector, cases } => self.switch(*selector, cases, span),
            StmtKind::Break => self.push(S::Break, span),
            StmtKind::Continue => self.push(S::Continue, span),
            StmtKind::Return(v) => {
                let value = v.map(|v| self.value(v));
                self.push(S::Return { value }, span);
            }
            StmtKind::Discard => self.push(S::Kill, span),
            StmtKind::Block(b) => {
                let block = self.nested(b);
                self.push(S::Block(block), span);
            }
        }
    }

    /// `if !cond { break }`, the head of a `while` or `for` loop.
    fn break_unless(&mut self, cond: Handle<E>, span: naga::Span) {
        let stop = self.append(
            E::Unary {
                op: naga::UnaryOperator::LogicalNot,
                expr: cond,
            },
            span,
        );
        let mut brk = naga::Block::new();
        brk.push(S::Break, span);
        self.push(
            S::If {
                condition: stop,
                accept: brk,
                reject: naga::Block::new(),
            },
            span,
        );
    }

    fn for_loop(
        &mut self,
        var: LocalId,
        start: ExprId,
        end: ExprId,
        inclusive: bool,
        body: &hir::Block,
        span: naga::Span,
    ) {
        let l = &self.body.locals[var.index()];
        let ty = self.l.ty(l.ty);
        let s = self.l.ty.scalar_of(l.ty).unwrap_or(Scalar::I32);
        let name = Some(self.l.names.get(l.name).to_string());
        let v = self.func.local_variables.append(
            naga::LocalVariable {
                name,
                ty,
                init: None,
            },
            span,
        );
        let first = self.value(start);
        // The range is evaluated once, before the loop, as in Rust.
        let last = self.value(end);
        let ptr = self.append(E::LocalVariable(v), span);
        self.push(
            S::Store {
                pointer: ptr,
                value: first,
            },
            span,
        );
        self.slots.insert(var, Slot::Var(v));

        self.flush();
        self.blocks.push(naga::Block::new());
        let ptr = self.append(E::LocalVariable(v), span);
        let i = self.append(E::Load { pointer: ptr }, span);
        let op = if inclusive {
            naga::BinaryOperator::LessEqual
        } else {
            naga::BinaryOperator::Less
        };
        let ok = self.append(
            E::Binary {
                op,
                left: i,
                right: last,
            },
            span,
        );
        self.break_unless(ok, span);
        self.block(body);
        self.flush();
        let loop_body = self.blocks.pop().expect("for body");

        self.blocks.push(naga::Block::new());
        let ptr = self.append(E::LocalVariable(v), span);
        let i = self.append(E::Load { pointer: ptr }, span);
        let one = self.append(
            E::Literal(literal(&hir::Lit::Int(1), Type::Scalar(s))),
            span,
        );
        let next = self.append(
            E::Binary {
                op: naga::BinaryOperator::Add,
                left: i,
                right: one,
            },
            span,
        );
        self.push(
            S::Store {
                pointer: ptr,
                value: next,
            },
            span,
        );
        self.flush();
        let continuing = self.blocks.pop().expect("for continuing");
        self.push(
            S::Loop {
                body: loop_body,
                continuing,
                break_if: None,
            },
            span,
        );
    }

    /// A `match`. Rust's `break` and `continue` in an arm leave the enclosing loop, but in a
    /// naga switch `break` leaves the switch, so such a match becomes an `if` chain.
    fn switch(
        &mut self,
        selector: ExprId,
        cases: &[(Option<Vec<ExprId>>, hir::Block)],
        span: naga::Span,
    ) {
        let sel = self.value(selector);
        let sel_ty = self.body.expr(selector).ty;
        let literal_cases = cases.iter().all(|(v, _)| {
            v.as_ref().is_none_or(|vs| {
                vs.iter()
                    .all(|&e| matches!(self.body.expr(e).kind, ExprKind::Lit(_)))
            })
        });
        let jumps = cases.iter().any(|(_, b)| leaves_loop(b));
        if literal_cases && !jumps {
            let unsigned = self.l.ty.scalar_of(sel_ty) == Some(Scalar::U32);
            let mut out = Vec::new();
            let mut default = None;
            for (values, body) in cases {
                let block = self.nested(body);
                match values {
                    None => default = Some(block),
                    Some(vs) => {
                        for (i, &e) in vs.iter().enumerate() {
                            let ExprKind::Lit(hir::Lit::Int(n)) = self.body.expr(e).kind else {
                                continue;
                            };
                            let value = if unsigned {
                                naga::SwitchValue::U32(n as u32)
                            } else {
                                naga::SwitchValue::I32(n as i32)
                            };
                            // Several values share one body by falling through to the last.
                            let last = i + 1 == vs.len();
                            let body = if last {
                                block.clone()
                            } else {
                                naga::Block::new()
                            };
                            out.push(naga::SwitchCase {
                                value,
                                body,
                                fall_through: !last,
                            });
                        }
                    }
                }
            }
            out.push(naga::SwitchCase {
                value: naga::SwitchValue::Default,
                body: default.unwrap_or_default(),
                fall_through: false,
            });
            self.push(
                S::Switch {
                    selector: sel,
                    cases: out,
                },
                span,
            );
            return;
        }
        self.if_chain(sel, sel_ty, cases, span);
    }

    fn if_chain(
        &mut self,
        sel: Handle<E>,
        sel_ty: crate::types::TypeId,
        cases: &[(Option<Vec<ExprId>>, hir::Block)],
        span: naga::Span,
    ) {
        let Some(((values, body), rest)) = cases.split_first() else {
            return;
        };
        let Some(values) = values else {
            let block = self.nested(body);
            self.push(S::Block(block), span);
            return;
        };
        let mut cond: Option<Handle<E>> = None;
        for &v in values {
            let x = self.value(v);
            let vt = self.body.expr(v).ty;
            let eq = self.binary(hir::BinOp::Eq, sel, sel_ty, x, vt, span);
            cond = Some(match cond {
                None => eq,
                Some(c) => self.append(
                    E::Binary {
                        op: naga::BinaryOperator::LogicalOr,
                        left: c,
                        right: eq,
                    },
                    span,
                ),
            });
        }
        let condition =
            cond.unwrap_or_else(|| self.append(E::Literal(naga::Literal::Bool(false)), span));
        let accept = self.nested(body);
        self.flush();
        self.blocks.push(naga::Block::new());
        self.if_chain(sel, sel_ty, rest, span);
        self.flush();
        let reject = self.blocks.pop().expect("else");
        self.push(
            S::If {
                condition,
                accept,
                reject,
            },
            span,
        );
    }

    /// A call statement, and its result if the function returns one.
    fn call(&mut self, f: FnId, args: &[ExprId], span: naga::Span) -> Option<Handle<E>> {
        let arguments: Vec<_> = args.iter().map(|&a| self.value(a)).collect();
        let function = self.l.fns[&f];
        let returns = self.l.module.functions[function].result.is_some();
        let result = returns.then(|| self.append(E::CallResult(function), span));
        self.push(
            S::Call {
                function,
                arguments,
                result,
            },
            span,
        );
        result
    }

    /// A pointer to a place, or `None` for a value.
    fn place(&mut self, e: ExprId) -> Option<Handle<E>> {
        let expr = self.body.expr(e);
        let span = naga_span(expr.span);
        match &expr.kind {
            ExprKind::Local(l) => match self.slots.get(l).copied()? {
                Slot::Var(v) => Some(self.append(E::LocalVariable(v), span)),
                Slot::Value(_) => None,
            },
            ExprKind::Global(g) => {
                let global = &self.l.hir.globals[g.index()];
                if global.space == hir::Space::Handle {
                    return None;
                }
                let h = self.l.global(*g);
                Some(self.append(E::GlobalVariable(h), span))
            }
            ExprKind::Field(base, i) => {
                let base = self.place(*base)?;
                Some(self.append(E::AccessIndex { base, index: *i }, span))
            }
            ExprKind::Index(base, index) => {
                let base = self.place(*base)?;
                let index = self.value(*index);
                Some(self.append(E::Access { base, index }, span))
            }
            _ => None,
        }
    }

    /// A texture operation; `vals` holds the image first, as the analyzer checked them.
    fn texture(
        &mut self,
        op: Tex,
        image_ty: crate::types::TypeId,
        vals: &[Handle<E>],
        span: naga::Span,
    ) -> Handle<E> {
        let Type::Texture { dim, kind } = self.l.ty.get(image_ty) else {
            unreachable!("checked a texture")
        };
        let layered = dim == TextureDim::D2Array;
        let image = vals[0];
        let last = *vals.last().expect("the image");
        let expr = match op {
            Tex::Sample | Tex::SampleLevel => E::ImageSample {
                image,
                sampler: vals[1],
                gather: None,
                coordinate: vals[2],
                array_index: layered.then(|| vals[3]),
                offset: None,
                level: if op == Tex::Sample {
                    naga::SampleLevel::Auto
                } else {
                    naga::SampleLevel::Exact(last)
                },
                depth_ref: None,
                clamp_to_edge: false,
            },
            Tex::Load => {
                let multi = matches!(kind, TextureKind::Multisampled(_));
                E::ImageLoad {
                    image,
                    coordinate: vals[1],
                    array_index: layered.then(|| vals[2]),
                    sample: multi.then_some(last),
                    level: (!multi).then_some(last),
                }
            }
            Tex::Dimensions => E::ImageQuery {
                image,
                query: naga::ImageQuery::Size { level: None },
            },
        };
        self.append(expr, span)
    }

    fn value(&mut self, e: ExprId) -> Handle<E> {
        if let Some(ptr) = self.place(e) {
            let span = naga_span(self.body.expr(e).span);
            return self.append(E::Load { pointer: ptr }, span);
        }
        let expr = self.body.expr(e);
        let span = naga_span(expr.span);
        let ty = expr.ty;
        match expr.kind.clone() {
            ExprKind::Lit(lit) => {
                let v = self.append(E::Literal(literal(&lit, self.l.ty.get(ty))), span);
                if self.l.ty.scalar_of(ty) == Some(Scalar::F16) {
                    return self.append(
                        E::As {
                            expr: v,
                            kind: naga::ScalarKind::Float,
                            convert: Some(2),
                        },
                        span,
                    );
                }
                v
            }
            ExprKind::Local(l) => match self.slots[&l] {
                Slot::Value(v) => v,
                Slot::Var(_) => unreachable!("handled as a place"),
            },
            ExprKind::Const(c) => match self.l.constant(c) {
                ConstRef::Const(h) => self.append(E::Constant(h), span),
                ConstRef::Override(h) => self.append(E::Override(h), span),
            },
            ExprKind::Global(g) => {
                let h = self.l.global(g);
                self.append(E::GlobalVariable(h), span)
            }
            ExprKind::Unary(op, a) => {
                let at = self.body.expr(a).ty;
                let a = self.value(a);
                self.append(
                    E::Unary {
                        op: unary(op, self.l.ty.scalar_of(at)),
                        expr: a,
                    },
                    span,
                )
            }
            ExprKind::Binary(op, a, b) => {
                let (at, bt) = (self.body.expr(a).ty, self.body.expr(b).ty);
                let (a, b) = (self.value(a), self.value(b));
                self.binary(op, a, at, b, bt, span)
            }
            ExprKind::Call(f, args) => self
                .call(f, &args, span)
                .expect("a call used as a value returns one"),
            ExprKind::Builtin(b, args) => {
                let tys: Vec<_> = args.iter().map(|&a| self.body.expr(a).ty).collect();
                let mut vals: Vec<_> = args.iter().map(|&a| self.value(a)).collect();
                match b {
                    Builtin::Math(fun) => {
                        // A scalar weight for a vector `mix` is spread over the vector.
                        if fun == naga::MathFunction::Mix
                            && let (Type::Vector(n, _), Type::Scalar(_)) =
                                (self.l.ty.get(tys[0]), self.l.ty.get(tys[2]))
                        {
                            vals[2] = self.append(
                                E::Splat {
                                    size: vector_size(n),
                                    value: vals[2],
                                },
                                span,
                            );
                        }
                        self.append(
                            E::Math {
                                fun,
                                arg: vals[0],
                                arg1: vals.get(1).copied(),
                                arg2: vals.get(2).copied(),
                                arg3: None,
                            },
                            span,
                        )
                    }
                    Builtin::Any | Builtin::All => {
                        if matches!(self.l.ty.get(tys[0]), Type::Scalar(_)) {
                            return vals[0];
                        }
                        let fun = if b == Builtin::Any {
                            naga::RelationalFunction::Any
                        } else {
                            naga::RelationalFunction::All
                        };
                        self.append(
                            E::Relational {
                                fun,
                                argument: vals[0],
                            },
                            span,
                        )
                    }
                    Builtin::Derivative(axis) => self.append(
                        E::Derivative {
                            axis,
                            ctrl: naga::DerivativeControl::None,
                            expr: vals[0],
                        },
                        span,
                    ),
                    Builtin::Texture(op) => self.texture(op, tys[0], &vals, span),
                }
            }
            ExprKind::Construct(parts) => {
                let components: Vec<_> = parts.iter().map(|&p| self.value(p)).collect();
                let ty = self.l.ty(ty);
                self.append(E::Compose { ty, components }, span)
            }
            ExprKind::Splat(a) => {
                let Type::Vector(n, _) = self.l.ty.get(ty) else {
                    unreachable!("a vector")
                };
                let value = self.value(a);
                self.append(
                    E::Splat {
                        size: vector_size(n),
                        value,
                    },
                    span,
                )
            }
            ExprKind::Zero => {
                let ty = self.l.ty(ty);
                self.append(E::ZeroValue(ty), span)
            }
            ExprKind::Field(base, index) => {
                let base = self.value(base);
                self.append(E::AccessIndex { base, index }, span)
            }
            ExprKind::Swizzle(base, comps) => {
                let vector = self.value(base);
                let mut pattern = [naga::SwizzleComponent::X; 4];
                for (slot, &c) in pattern.iter_mut().zip(&comps) {
                    *slot = [
                        naga::SwizzleComponent::X,
                        naga::SwizzleComponent::Y,
                        naga::SwizzleComponent::Z,
                        naga::SwizzleComponent::W,
                    ][c as usize];
                }
                self.append(
                    E::Swizzle {
                        size: vector_size(comps.len() as u8),
                        vector,
                        pattern,
                    },
                    span,
                )
            }
            ExprKind::Index(base, index) => {
                let base = self.value(base);
                let index = self.value(index);
                self.append(E::Access { base, index }, span)
            }
            ExprKind::Cast(a) => {
                let from = self.body.expr(a).ty;
                let v = self.value(a);
                if from == ty {
                    return v;
                }
                let s = scalar(self.l.ty.scalar_of(ty).expect("checked numeric"));
                self.append(
                    E::As {
                        expr: v,
                        kind: s.kind,
                        convert: Some(s.width),
                    },
                    span,
                )
            }
            ExprKind::Bitcast(a) => {
                let v = self.value(a);
                let s = scalar(self.l.ty.scalar_of(ty).expect("checked scalar"));
                self.append(
                    E::As {
                        expr: v,
                        kind: s.kind,
                        convert: None,
                    },
                    span,
                )
            }
            ExprKind::Select {
                cond,
                then,
                otherwise,
            } => {
                let condition = self.value(cond);
                let accept = self.value(then);
                let reject = self.value(otherwise);
                self.append(
                    E::Select {
                        condition,
                        accept,
                        reject,
                    },
                    span,
                )
            }
            ExprKind::Member(..) | ExprKind::Error => {
                unreachable!("resolved or reported before lowering")
            }
        }
    }

    /// A binary operation. naga takes a scalar operand only for multiplication, so for the
    /// others it is spread over the vector.
    fn binary(
        &mut self,
        op: hir::BinOp,
        a: Handle<E>,
        at: crate::types::TypeId,
        b: Handle<E>,
        bt: crate::types::TypeId,
        span: naga::Span,
    ) -> Handle<E> {
        let (mut a, mut b) = (a, b);
        if op != hir::BinOp::Mul && !matches!(op, hir::BinOp::Shl | hir::BinOp::Shr) {
            match (self.l.ty.get(at), self.l.ty.get(bt)) {
                (Type::Vector(n, _), Type::Scalar(_)) => {
                    b = self.append(
                        E::Splat {
                            size: vector_size(n),
                            value: b,
                        },
                        span,
                    )
                }
                (Type::Scalar(_), Type::Vector(n, _)) => {
                    a = self.append(
                        E::Splat {
                            size: vector_size(n),
                            value: a,
                        },
                        span,
                    )
                }
                _ => {}
            }
        }
        self.append(
            E::Binary {
                op: binary(op),
                left: a,
                right: b,
            },
            span,
        )
    }
}

/// Whether a block holds a `break` or `continue` that leaves an enclosing loop (rather than
/// one nested inside it).
fn leaves_loop(block: &hir::Block) -> bool {
    block.iter().any(|s| match &s.kind {
        StmtKind::Break | StmtKind::Continue => true,
        StmtKind::If {
            then, otherwise, ..
        } => leaves_loop(then) || otherwise.as_ref().is_some_and(leaves_loop),
        StmtKind::Block(b) => leaves_loop(b),
        StmtKind::Switch { cases, .. } => cases.iter().any(|c| leaves_loop(&c.1)),
        _ => false,
    })
}
