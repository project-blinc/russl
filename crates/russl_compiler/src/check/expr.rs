//! Expression typing. Inference runs bottom-up; an expected type flows down so unsuffixed
//! literals take the type their context needs, as WGSL's abstract numbers do.

use super::Checker;
use crate::builtins::{Builtin, Rule};
use crate::hir::*;
use crate::types::{Scalar, Type, TypeId};

impl Checker<'_> {
    /// The type of `e`, with abstract literals converted to `expected` where it allows.
    pub fn check(&mut self, e: ExprId, expected: Option<TypeId>) -> TypeId {
        let ty = self.infer(e);
        match expected {
            Some(want) if self.types.converts_to(ty, want) => {
                if self.types.is_abstract(ty) && !self.types.is_abstract(want) {
                    self.concretise(e, want);
                    return want;
                }
                ty
            }
            Some(want) => {
                let (have, want_name) = (self.ty_name(ty), self.ty_name(want));
                self.error_at(e, format!("expected `{want_name}`, found `{have}`"));
                self.types.error
            }
            None => ty,
        }
    }

    /// The type of `e` with any abstract literal made concrete: a value that must exist at run
    /// time.
    pub fn check_concrete(&mut self, e: ExprId) -> TypeId {
        let ty = self.infer(e);
        if self.types.is_abstract(ty) {
            let c = self.types.concrete(ty);
            self.concretise(e, c);
            return c;
        }
        ty
    }

    /// Give an abstract expression a concrete type, down through its operands.
    pub fn concretise(&mut self, e: ExprId, to: TypeId) {
        let have = self.body().expr(e).ty;
        if !self.types.is_abstract(have) {
            return;
        }
        let Some(scalar) = self
            .types
            .scalar_of(to)
            .or_else(|| match self.types.get(to) {
                Type::Array { elem, .. } => self.types.scalar_of(elem),
                _ => None,
            })
        else {
            return;
        };
        let new = self.types.with_scalar(have, scalar);
        self.body_mut().exprs[e.index()].ty = new;
        let kids: Vec<ExprId> = match &self.body().expr(e).kind {
            ExprKind::Unary(_, a) | ExprKind::Splat(a) => vec![*a],
            ExprKind::Binary(op, a, b)
                if !op.is_comparison() && !matches!(op, BinOp::Shl | BinOp::Shr) =>
            {
                vec![*a, *b]
            }
            ExprKind::Binary(BinOp::Shl | BinOp::Shr, a, _) => vec![*a],
            ExprKind::Construct(parts) => parts.clone(),
            ExprKind::Builtin(_, args) => args.clone(),
            ExprKind::Select {
                then, otherwise, ..
            } => vec![*then, *otherwise],
            ExprKind::Index(base, _) => vec![*base],
            _ => Vec::new(),
        };
        for k in kids {
            let kt = self.body().expr(k).ty;
            let target = self.types.with_scalar(kt, scalar);
            self.concretise(k, target);
        }
    }

    fn set(&mut self, e: ExprId, ty: TypeId) -> TypeId {
        self.body_mut().exprs[e.index()].ty = ty;
        ty
    }

    fn infer(&mut self, e: ExprId) -> TypeId {
        let error = self.types.error;
        let preset = self.body().expr(e).ty;
        let kind = self.body().expr(e).kind.clone();
        let ty = match kind {
            ExprKind::Error => error,
            ExprKind::Lit(_) | ExprKind::Zero => preset,
            ExprKind::Local(l) => {
                let local = &mut self.body_mut().locals[l.index()];
                local.used = true;
                local.ty
            }
            ExprKind::Const(c) => {
                self.used_consts.insert(c);
                self.module.consts[c.index()].ty
            }
            ExprKind::Global(g) => {
                self.used_globals.insert(g);
                self.module.globals[g.index()].ty
            }
            ExprKind::Unary(op, a) => self.unary(e, op, a),
            ExprKind::Binary(op, a, b) => self.binary(e, op, a, b),
            ExprKind::Call(f, args) => self.call(e, f, &args),
            ExprKind::Builtin(b, args) => self.builtin(e, b, &args),
            ExprKind::Construct(parts) => self.construct(e, preset, &parts),
            ExprKind::Splat(a) => match self.types.get(preset) {
                Type::Vector(_, s) => {
                    let want = self.types.scalar(s);
                    self.check(a, Some(want));
                    preset
                }
                _ => error,
            },
            ExprKind::Member(base, name) => self.member(e, base, name),
            ExprKind::Field(..) | ExprKind::Swizzle(..) => preset,
            ExprKind::Index(base, index) => self.index(e, base, index),
            ExprKind::Cast(a) => self.cast(e, a, preset),
            ExprKind::Bitcast(a) => {
                let from = self.check_concrete(a);
                let ok = matches!(
                    (self.types.get(from), self.types.get(preset)),
                    (Type::Scalar(x), Type::Scalar(y)) if x != Scalar::Bool && y != Scalar::Bool && x != Scalar::F16 && y != Scalar::F16
                );
                if !ok && self.types.get(from) != Type::Error {
                    let (f, t) = (self.ty_name(from), self.ty_name(preset));
                    self.error_at(e, format!("cannot reinterpret `{f}` as `{t}`"));
                }
                preset
            }
            ExprKind::Select {
                cond,
                then,
                otherwise,
            } => {
                let b = self.types.bool;
                self.check(cond, Some(b));
                self.unify(e, then, otherwise)
            }
        };
        self.set(e, ty)
    }

    /// The common type of two operands, converting an abstract one to the other's scalar.
    fn unify(&mut self, at: ExprId, a: ExprId, b: ExprId) -> TypeId {
        let ta = self.infer(a);
        let tb = self.infer(b);
        if self.types.converts_to(ta, tb) {
            if self.types.is_abstract(ta) && !self.types.is_abstract(tb) {
                self.concretise(a, tb);
            }
            return tb;
        }
        if self.types.converts_to(tb, ta) {
            if self.types.is_abstract(tb) {
                self.concretise(b, ta);
            }
            return ta;
        }
        let (x, y) = (self.ty_name(ta), self.ty_name(tb));
        self.error_at(at, format!("mismatched types `{x}` and `{y}`"));
        self.types.error
    }

    fn unary(&mut self, e: ExprId, op: UnOp, a: ExprId) -> TypeId {
        let t = self.infer(a);
        let s = self.types.scalar_of(t);
        let ok = match (op, s) {
            (_, None) if self.types.get(t) == Type::Error => true,
            (UnOp::Neg, Some(s)) => s.is_numeric() && s != Scalar::U32,
            // `!` is logical on bools and bitwise on integers, as in Rust.
            (UnOp::Not, Some(s)) => s == Scalar::Bool || s.is_int(),
            _ => false,
        };
        if !ok {
            let name = self.ty_name(t);
            let sym = if op == UnOp::Neg { "-" } else { "!" };
            self.error_at(e, format!("cannot apply unary `{sym}` to `{name}`"));
            return self.types.error;
        }
        t
    }

    fn binary(&mut self, e: ExprId, op: BinOp, a: ExprId, b: ExprId) -> TypeId {
        let error = self.types.error;
        if matches!(op, BinOp::Shl | BinOp::Shr) {
            let t = self.infer(a);
            let amount = match self.types.get(t) {
                Type::Vector(n, _) => self.types.add(Type::Vector(n, Scalar::U32)),
                _ => self.types.u32,
            };
            if !self.types.scalar_of(t).is_some_and(Scalar::is_int)
                && self.types.get(t) != Type::Error
            {
                let name = self.ty_name(t);
                self.error_at(e, format!("cannot shift `{name}`"));
                return error;
            }
            self.check(b, Some(amount));
            return t;
        }
        let ta = self.infer(a);
        let tb = self.infer(b);
        if self.types.get(ta) == Type::Error || self.types.get(tb) == Type::Error {
            return error;
        }
        // Matrix products and vector-scalar mixing keep their shapes; everything else needs
        // one type on both sides.
        if let Some(t) = self.mixed_shapes(op, a, ta, b, tb) {
            return t;
        }
        let t = self.unify(e, a, b);
        if self.types.get(t) == Type::Error {
            return error;
        }
        let s = self.types.scalar_of(t);
        let ok = match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => {
                s.is_some_and(Scalar::is_numeric)
                    && !matches!(self.types.get(t), Type::Matrix { .. } if matches!(op, BinOp::Div | BinOp::Rem))
            }
            BinOp::And | BinOp::Or => self.types.get(t) == Type::Scalar(Scalar::Bool),
            BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor => {
                s.is_some_and(|s| s.is_int() || s == Scalar::Bool)
            }
            BinOp::Eq | BinOp::Ne => {
                s.is_some_and(|_| !matches!(self.types.get(t), Type::Matrix { .. }))
            }
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                s.is_some_and(Scalar::is_numeric)
                    && !matches!(self.types.get(t), Type::Matrix { .. })
            }
            BinOp::Shl | BinOp::Shr => unreachable!(),
        };
        if !ok {
            let name = self.ty_name(t);
            self.error_at(e, format!("no `{op:?}` for `{name}`"));
            return error;
        }
        if op.is_comparison() {
            if self.types.is_abstract(t) {
                let c = self.types.concrete(t);
                self.concretise(a, c);
                self.concretise(b, c);
            }
            return match self.types.get(t) {
                Type::Vector(n, _) => self.types.add(Type::Vector(n, Scalar::Bool)),
                _ => self.types.bool,
            };
        }
        t
    }

    /// `vector op scalar`, `scalar op vector`, and the matrix products.
    fn mixed_shapes(
        &mut self,
        op: BinOp,
        a: ExprId,
        ta: TypeId,
        b: ExprId,
        tb: TypeId,
    ) -> Option<TypeId> {
        let arith = matches!(
            op,
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem
        );
        if !arith {
            return None;
        }
        let (ga, gb) = (self.types.get(ta), self.types.get(tb));
        let scalar_pair = |x: Scalar, y: Scalar| x.converts_to(y) || y.converts_to(x);
        let result = match (ga, gb) {
            (Type::Vector(n, x), Type::Scalar(y)) if scalar_pair(x, y) && x.is_numeric() => {
                Some((Type::Vector(n, pick(x, y)), true))
            }
            (Type::Scalar(x), Type::Vector(n, y)) if scalar_pair(x, y) && y.is_numeric() => {
                Some((Type::Vector(n, pick(x, y)), true))
            }
            (Type::Matrix { cols, rows, scalar }, Type::Vector(n, y))
                if op == BinOp::Mul && n == cols && scalar_pair(scalar, y) =>
            {
                Some((Type::Vector(rows, pick(scalar, y)), true))
            }
            (Type::Vector(n, x), Type::Matrix { cols, rows, scalar })
                if op == BinOp::Mul && n == rows && scalar_pair(x, scalar) =>
            {
                Some((Type::Vector(cols, pick(x, scalar)), true))
            }
            (
                Type::Matrix {
                    cols: c1,
                    rows: r1,
                    scalar: x,
                },
                Type::Matrix {
                    cols: c2,
                    rows: r2,
                    scalar: y,
                },
            ) if op == BinOp::Mul && c1 == r2 && scalar_pair(x, y) => Some((
                Type::Matrix {
                    cols: c2,
                    rows: r1,
                    scalar: pick(x, y),
                },
                true,
            )),
            (Type::Matrix { cols, rows, scalar }, Type::Scalar(y))
            | (Type::Scalar(y), Type::Matrix { cols, rows, scalar })
                if op == BinOp::Mul && scalar_pair(scalar, y) =>
            {
                Some((
                    Type::Matrix {
                        cols,
                        rows,
                        scalar: pick(scalar, y),
                    },
                    true,
                ))
            }
            _ => None,
        };
        let (t, _) = result?;
        let t = self.types.add(t);
        let s = self.types.scalar_of(t).expect("numeric");
        for (x, tx) in [(a, ta), (b, tb)] {
            if self.types.is_abstract(tx) {
                let target = self.types.with_scalar(tx, s);
                self.concretise(x, target);
            }
        }
        Some(t)
    }

    fn call(&mut self, e: ExprId, f: FnId, args: &[ExprId]) -> TypeId {
        self.calls.push((self.current, f));
        let callee = &self.module.functions[f.index()];
        if callee.stage.is_some() {
            let name = self.names.get(callee.name).to_string();
            self.error_at(
                e,
                format!("`{name}` is an entry point and cannot be called"),
            );
        }
        let callee = &self.module.functions[f.index()];
        let params: Vec<TypeId> = callee
            .params
            .iter()
            .map(|p| callee.body.locals[p.local.index()].ty)
            .collect();
        let ret = callee.ret.map(|r| r.0).unwrap_or(self.types.unit);
        if params.len() != args.len() {
            let name = self.names.get(callee.name).to_string();
            self.error_at(
                e,
                format!(
                    "`{name}` takes {} arguments, {} given",
                    params.len(),
                    args.len()
                ),
            );
        }
        for (&a, &p) in args.iter().zip(&params) {
            self.check(a, Some(p));
        }
        for &a in args.iter().skip(params.len()) {
            self.check_concrete(a);
        }
        ret
    }

    fn builtin(&mut self, e: ExprId, b: Builtin, args: &[ExprId]) -> TypeId {
        let error = self.types.error;
        let def = crate::builtins::DEFS
            .iter()
            .find(|d| d.builtin == b)
            .expect("a builtin from the table");
        if args.len() != def.arity {
            self.error_at(
                e,
                format!(
                    "`{}` takes {} arguments, {} given",
                    def.function,
                    def.arity,
                    args.len()
                ),
            );
            return error;
        }
        // Every rule but `Mix` and `Refract` takes operands of one type; unify them.
        let operands = match def.rule {
            Rule::Mix | Rule::Refract => &args[..2],
            _ => args,
        };
        let mut t = self.infer(operands[0]);
        for &o in &operands[1..] {
            t = self.unify(e, operands[0], o);
        }
        if self.types.get(t) == Type::Error {
            return error;
        }
        if self.types.is_abstract(t) {
            let c = self.types.concrete(t);
            for &o in operands {
                self.concretise(o, c);
            }
            t = c;
        }
        let ty = self.types.get(t);
        let s = self.types.scalar_of(t);
        let float = s.is_some_and(Scalar::is_float);
        let result = match def.rule {
            Rule::Float if float && !matches!(ty, Type::Matrix { .. }) => Some(t),
            Rule::Numeric
                if s.is_some_and(Scalar::is_numeric) && !matches!(ty, Type::Matrix { .. }) =>
            {
                Some(t)
            }
            Rule::Signed
                if s.is_some_and(|s| s.is_numeric() && s != Scalar::U32)
                    && !matches!(ty, Type::Matrix { .. }) =>
            {
                Some(t)
            }
            Rule::FloatVector if float && matches!(ty, Type::Vector(..)) => Some(t),
            Rule::Dot if matches!(ty, Type::Vector(_, s) if s.is_numeric()) => {
                s.map(|s| self.types.scalar(s))
            }
            Rule::Cross if float && matches!(ty, Type::Vector(3, _)) => Some(t),
            Rule::Length | Rule::Distance if float && !matches!(ty, Type::Matrix { .. }) => {
                s.map(|s| self.types.scalar(s))
            }
            Rule::Mix | Rule::Refract if float && !matches!(ty, Type::Matrix { .. }) => {
                let weight = args[2];
                let scalar = self.types.scalar(s.expect("float"));
                let w = self.infer(weight);
                if def.rule == Rule::Mix && self.types.converts_to(w, t) {
                    self.check(weight, Some(t));
                } else {
                    self.check(weight, Some(scalar));
                }
                Some(t)
            }
            Rule::Transpose => match ty {
                Type::Matrix { cols, rows, scalar } => Some(self.types.add(Type::Matrix {
                    cols: rows,
                    rows: cols,
                    scalar,
                })),
                _ => None,
            },
            Rule::Determinant => match ty {
                Type::Matrix { cols, rows, scalar } if cols == rows => {
                    Some(self.types.scalar(scalar))
                }
                _ => None,
            },
            Rule::Reduce if s == Some(Scalar::Bool) => Some(self.types.bool),
            _ => None,
        };
        result.unwrap_or_else(|| {
            let name = self.ty_name(t);
            self.error_at(e, format!("`{}` does not take `{name}`", def.method));
            error
        })
    }

    fn construct(&mut self, e: ExprId, preset: TypeId, parts: &[ExprId]) -> TypeId {
        let error = self.types.error;
        match self.types.get(preset) {
            Type::Vector(n, s) => {
                let mut count = 0;
                for &p in parts {
                    let t = self.infer(p);
                    match self.types.get(t) {
                        Type::Scalar(x) if x.converts_to(s) => count += 1,
                        Type::Vector(m, x) if x.converts_to(s) => count += m,
                        Type::Error => return error,
                        _ => {
                            let name = self.ty_name(t);
                            self.error_at(
                                p,
                                format!(
                                    "a `{}` is built from `{}` values, not `{name}`",
                                    self.ty_name(preset),
                                    s.name()
                                ),
                            );
                            return error;
                        }
                    }
                    if self.types.is_abstract(t) {
                        let target = self.types.with_scalar(t, s);
                        self.concretise(p, target);
                    }
                }
                if count != n {
                    self.error_at(
                        e,
                        format!(
                            "`{}::new` needs {n} components, found {count}",
                            self.ty_name(preset)
                        ),
                    );
                    return error;
                }
                preset
            }
            Type::Matrix { cols, rows, scalar } => {
                let col = self.types.add(Type::Vector(rows, scalar));
                if parts.len() != cols as usize {
                    self.error_at(
                        e,
                        format!("`{}::from_cols` needs {cols} columns", self.ty_name(preset)),
                    );
                }
                for &p in parts {
                    self.check(p, Some(col));
                }
                preset
            }
            Type::Struct(s) => {
                let fields: Vec<TypeId> = self.types.structs[s.0 as usize]
                    .fields
                    .iter()
                    .map(|f| f.1)
                    .collect();
                for (&p, &f) in parts.iter().zip(&fields) {
                    self.check(p, Some(f));
                }
                preset
            }
            // An array literal: one element type.
            _ => {
                let first = parts[0];
                let mut t = self.infer(first);
                for &p in &parts[1..] {
                    t = self.unify(e, first, p);
                }
                let t = self.types.concrete(t);
                for &p in parts {
                    self.concretise(p, t);
                }
                self.types.add(Type::Array {
                    elem: t,
                    len: Some(parts.len() as u32),
                })
            }
        }
    }

    fn member(&mut self, e: ExprId, base: ExprId, name: crate::intern::Symbol) -> TypeId {
        let error = self.types.error;
        let bt = self.check_concrete(base);
        let text = self.names.get(name).to_string();
        match self.types.get(bt) {
            Type::Struct(s) => {
                let fields = &self.types.structs[s.0 as usize].fields;
                match fields.iter().position(|f| f.0 == name) {
                    Some(i) => {
                        let ty = fields[i].1;
                        self.body_mut().exprs[e.index()].kind = ExprKind::Field(base, i as u32);
                        ty
                    }
                    None => {
                        let st = self.ty_name(bt);
                        self.error_at(e, format!("no field `{text}` on `{st}`"));
                        error
                    }
                }
            }
            Type::Vector(n, s) => match swizzle(&text, n) {
                Some(comps) if comps.len() == 1 => {
                    self.body_mut().exprs[e.index()].kind = ExprKind::Field(base, comps[0] as u32);
                    self.types.scalar(s)
                }
                Some(comps) => {
                    let len = comps.len() as u8;
                    self.body_mut().exprs[e.index()].kind = ExprKind::Swizzle(base, comps);
                    self.types.add(Type::Vector(len, s))
                }
                None => {
                    let vt = self.ty_name(bt);
                    self.error_at(e, format!("no field or swizzle `{text}` on `{vt}`"));
                    error
                }
            },
            Type::Error => error,
            _ => {
                let t = self.ty_name(bt);
                self.error_at(e, format!("`{t}` has no fields"));
                error
            }
        }
    }

    fn index(&mut self, e: ExprId, base: ExprId, index: ExprId) -> TypeId {
        let error = self.types.error;
        let bt = self.check_concrete(base);
        let it = self.infer(index);
        match self.types.get(it) {
            Type::Scalar(Scalar::AbstractInt) => {
                let u = self.types.u32;
                self.concretise(index, u);
            }
            Type::Scalar(Scalar::I32 | Scalar::U32) | Type::Error => {}
            _ => {
                let name = self.ty_name(it);
                self.error_at(index, format!("an index is `u32` or `i32`, not `{name}`"));
            }
        }
        match self.types.get(bt) {
            Type::Array { elem, .. } => elem,
            Type::Vector(_, s) => self.types.scalar(s),
            Type::Matrix { rows, scalar, .. } => self.types.add(Type::Vector(rows, scalar)),
            Type::Error => error,
            _ => {
                let name = self.ty_name(bt);
                self.error_at(e, format!("cannot index `{name}`"));
                error
            }
        }
    }

    fn cast(&mut self, e: ExprId, a: ExprId, to: TypeId) -> TypeId {
        let from = self.infer(a);
        let shape = |t: Type| match t {
            Type::Scalar(s) => Some((1, s)),
            Type::Vector(n, s) => Some((n, s)),
            _ => None,
        };
        match (shape(self.types.get(from)), shape(self.types.get(to))) {
            (Some((n, x)), Some((m, y))) if n == m => {
                if x.is_abstract() {
                    let target = if x.converts_to(y) {
                        to
                    } else {
                        self.types.concrete(from)
                    };
                    self.concretise(a, target);
                }
                to
            }
            _ if self.types.get(from) == Type::Error => self.types.error,
            _ => {
                let (f, t) = (self.ty_name(from), self.ty_name(to));
                self.error_at(e, format!("cannot cast `{f}` as `{t}`"));
                self.types.error
            }
        }
    }
}

fn pick(a: Scalar, b: Scalar) -> Scalar {
    if a.is_abstract() { b } else { a }
}

/// Component indices of a swizzle like `xy` or `rgba`, within a vector of `n`.
fn swizzle(text: &str, n: u8) -> Option<Vec<u8>> {
    if text.is_empty() || text.len() > 4 {
        return None;
    }
    let xyzw = text.chars().all(|c| "xyzw".contains(c));
    let rgba = text.chars().all(|c| "rgba".contains(c));
    if !xyzw && !rgba {
        return None;
    }
    let set = if xyzw { "xyzw" } else { "rgba" };
    let comps: Vec<u8> = text
        .chars()
        .map(|c| set.find(c).expect("checked") as u8)
        .collect();
    comps.iter().all(|&c| c < n).then_some(comps)
}
