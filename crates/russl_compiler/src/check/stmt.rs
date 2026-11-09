//! Statement checks: bindings, assignment to places, control flow and return paths.

use super::Checker;
use crate::hir::*;
use crate::types::{Scalar, Type};

impl Checker<'_> {
    pub(super) fn block(&mut self, block: Block) -> Block {
        let mut out = Vec::with_capacity(block.len());
        let mut ended = false;
        let mut warned = false;
        for stmt in block {
            if ended && !warned {
                self.sink
                    .warning(self.spans.get(stmt.span), "unreachable statement");
                warned = true;
            }
            ended |= matches!(
                stmt.kind,
                StmtKind::Return(_) | StmtKind::Break | StmtKind::Continue
            );
            out.push(self.stmt(stmt));
        }
        out
    }

    fn stmt(&mut self, mut stmt: Stmt) -> Stmt {
        let span = stmt.span;
        stmt.kind = match stmt.kind {
            StmtKind::Let { local, init } => {
                let annotated = self.body().locals[local.index()].ty;
                let annotated = (annotated != self.types.error).then_some(annotated);
                let ty = match (init, annotated) {
                    (Some(i), Some(t)) => {
                        self.check(i, Some(t));
                        t
                    }
                    (Some(i), None) => self.check_concrete(i),
                    (None, Some(t)) => t,
                    (None, None) => {
                        self.sink
                            .error(self.spans.get(span), "a `let` without a value needs a type");
                        self.types.error
                    }
                };
                self.body_mut().locals[local.index()].ty = ty;
                StmtKind::Let { local, init }
            }
            StmtKind::Assign { target, op, value } => {
                let t = self.check_concrete(target);
                self.place(target);
                match op {
                    None => {
                        self.check(value, Some(t));
                    }
                    Some(BinOp::Shl | BinOp::Shr) => {
                        let amount = match self.types.get(t) {
                            Type::Vector(n, _) => self.types.add(Type::Vector(n, Scalar::U32)),
                            _ => self.types.u32,
                        };
                        self.check(value, Some(amount));
                    }
                    Some(_) => {
                        // `a op= b` is `a = a op b`: a scalar operand may scale a vector.
                        let vt = self.check_concrete_or(value, t);
                        let ok = self.types.converts_to(vt, t)
                            || matches!((self.types.get(t), self.types.get(vt)), (Type::Vector(_, s), Type::Scalar(x)) if s == x)
                            || matches!(
                                (self.types.get(t), self.types.get(vt)),
                                (Type::Matrix { .. }, Type::Scalar(_))
                                    | (Type::Vector(..), Type::Matrix { .. })
                            );
                        if !ok {
                            let (a, b) = (self.ty_name(t), self.ty_name(vt));
                            self.error_at(
                                value,
                                format!(
                                    "cannot apply `{:?}=` to `{a}` and `{b}`",
                                    op.expect("compound")
                                ),
                            );
                        }
                    }
                }
                StmtKind::Assign { target, op, value }
            }
            StmtKind::Expr(e) => {
                self.check_concrete(e);
                StmtKind::Expr(e)
            }
            StmtKind::If {
                cond,
                then,
                otherwise,
            } => {
                let b = self.types.bool;
                self.check(cond, Some(b));
                let then = self.block(then);
                let otherwise = otherwise.map(|o| self.block(o));
                StmtKind::If {
                    cond,
                    then,
                    otherwise,
                }
            }
            StmtKind::While { cond, body } => {
                let b = self.types.bool;
                self.check(cond, Some(b));
                self.loop_depth += 1;
                let body = self.block(body);
                self.loop_depth -= 1;
                StmtKind::While { cond, body }
            }
            StmtKind::Loop { body } => {
                self.loop_depth += 1;
                let body = self.block(body);
                self.loop_depth -= 1;
                StmtKind::Loop { body }
            }
            StmtKind::For {
                var,
                start,
                end,
                inclusive,
                body,
            } => {
                let t = self.unify_ints(start, end);
                self.body_mut().locals[var.index()].ty = t;
                self.loop_depth += 1;
                let body = self.block(body);
                self.loop_depth -= 1;
                StmtKind::For {
                    var,
                    start,
                    end,
                    inclusive,
                    body,
                }
            }
            StmtKind::Switch { selector, cases } => {
                let t = self.check_concrete(selector);
                if !matches!(
                    self.types.get(t),
                    Type::Scalar(Scalar::I32 | Scalar::U32) | Type::Error
                ) {
                    let name = self.ty_name(t);
                    self.error_at(
                        selector,
                        format!("`match` in a shader is on `i32` or `u32`, not `{name}`"),
                    );
                }
                let mut cases_out = Vec::with_capacity(cases.len());
                for (values, body) in cases {
                    if let Some(values) = &values {
                        for &v in values {
                            self.check(v, Some(t));
                            self.require_const(v);
                        }
                    }
                    cases_out.push((values, self.block(body)));
                }
                StmtKind::Switch {
                    selector,
                    cases: cases_out,
                }
            }
            StmtKind::Break | StmtKind::Continue => {
                if self.loop_depth == 0 {
                    self.sink.error(
                        self.spans.get(span),
                        "`break` and `continue` belong inside a loop",
                    );
                }
                stmt.kind
            }
            StmtKind::Return(value) => {
                match (value, self.ret) {
                    (Some(v), Some(t)) => {
                        self.check(v, Some(t));
                    }
                    (Some(v), None) => self.error_at(v, "this function returns nothing"),
                    (None, Some(t)) if t != self.types.unit => {
                        self.sink
                            .error(self.spans.get(span), "`return` needs a value here");
                    }
                    _ => {}
                }
                StmtKind::Return(value)
            }
            StmtKind::Block(b) => StmtKind::Block(self.block(b)),
        };
        stmt
    }

    /// The value type of a compound assignment's right side, abstract literals taking the
    /// target's scalar.
    fn check_concrete_or(
        &mut self,
        value: crate::hir::ExprId,
        target: crate::types::TypeId,
    ) -> crate::types::TypeId {
        let s = self.types.scalar_of(target);
        let vt = self.check(value, None);
        if self.types.is_abstract(vt)
            && let Some(s) = s
        {
            let to = self.types.with_scalar(vt, s);
            self.concretise(value, to);
            return to;
        }
        vt
    }

    /// A `for` range: both ends one integer type.
    fn unify_ints(&mut self, a: ExprId, b: ExprId) -> crate::types::TypeId {
        let ta = self.check(a, None);
        let tb = self.check(b, None);
        let t = match (self.types.is_abstract(ta), self.types.is_abstract(tb)) {
            (true, false) => tb,
            (false, _) => ta,
            (true, true) => self.types.i32,
        };
        self.check(a, Some(t));
        self.check(b, Some(t));
        if !matches!(
            self.types.get(t),
            Type::Scalar(Scalar::I32 | Scalar::U32) | Type::Error
        ) {
            let name = self.ty_name(t);
            self.error_at(
                a,
                format!("a `for` range is over `i32` or `u32`, not `{name}`"),
            );
        }
        t
    }

    /// An assignment target must be a mutable place.
    fn place(&mut self, e: ExprId) {
        match self.body().expr(e).kind.clone() {
            ExprKind::Local(l) => {
                let local = &self.body().locals[l.index()];
                if !local.mutable {
                    let name = self.names.get(local.name).to_string();
                    let what = if local.is_param {
                        "parameter"
                    } else {
                        "variable"
                    };
                    self.error_at(
                        e,
                        format!(
                            "cannot assign twice to immutable {what} `{name}`; declare it `mut`"
                        ),
                    );
                }
            }
            ExprKind::Global(g) => {
                let global = &self.module.globals[g.index()];
                let ok = matches!(
                    global.space,
                    Space::Private | Space::Workgroup | Space::Storage { writable: true }
                );
                if !ok {
                    let name = self.names.get(global.name).to_string();
                    self.error_at(e, format!("`{name}` is read-only; only `static mut`, `Workgroup` and `StorageMut` can be assigned"));
                }
            }
            ExprKind::Field(base, _) | ExprKind::Index(base, _) => self.place(base),
            ExprKind::Swizzle(..) => self.error_at(
                e,
                "assign one component at a time; a swizzle is not a place",
            ),
            ExprKind::Const(_) => self.error_at(e, "a constant cannot be assigned"),
            ExprKind::Error => {}
            _ => self.error_at(e, "this is a value, not a place that can be assigned"),
        }
    }
}

/// Whether every path through a block ends in `return` (or never leaves an endless loop).
pub fn returns(block: &Block) -> bool {
    block.iter().any(|s| match &s.kind {
        StmtKind::Return(_) => true,
        StmtKind::If {
            then,
            otherwise: Some(o),
            ..
        } => returns(then) && returns(o),
        StmtKind::Block(b) => returns(b),
        StmtKind::Loop { body } => !breaks(body),
        StmtKind::Switch { cases, .. } => {
            cases.iter().any(|c| c.0.is_none()) && cases.iter().all(|c| returns(&c.1))
        }
        _ => false,
    })
}

/// Whether a loop body can `break` out of its loop.
fn breaks(block: &Block) -> bool {
    block.iter().any(|s| match &s.kind {
        StmtKind::Break => true,
        StmtKind::If {
            then, otherwise, ..
        } => breaks(then) || otherwise.as_ref().is_some_and(breaks),
        StmtKind::Block(b) => breaks(b),
        StmtKind::Switch { cases, .. } => cases.iter().any(|c| breaks(&c.1)),
        _ => false,
    })
}
