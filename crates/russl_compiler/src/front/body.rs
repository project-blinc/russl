//! Function bodies and constant initializers: syntax to resolved expressions and statements.
//! Locals resolve through block scopes; a shadowing `let` is a new local, as in Rust.

use super::{Front, Item};
use crate::builtins;
use crate::hir::*;
use crate::intern::Symbol;
use crate::types::{Scalar, Type, TypeId};
use std::collections::HashMap;
use syn::spanned::Spanned;

struct Builder<'f, 'a> {
    f: &'f mut Front<'a>,
    body: Body,
    scopes: Vec<HashMap<Symbol, LocalId>>,
}

/// A function's parameters and body.
pub fn function(f: &mut Front<'_>, func: &syn::ItemFn) -> (Vec<Param>, Body) {
    let mut b = Builder {
        f,
        body: Body::default(),
        scopes: vec![HashMap::new()],
    };
    let mut params = Vec::new();
    for arg in &func.sig.inputs {
        let syn::FnArg::Typed(arg) = arg else {
            b.f.sink
                .error(arg.span(), "shader functions take no `self`");
            continue;
        };
        let syn::Pat::Ident(pat) = &*arg.pat else {
            b.f.sink
                .error(arg.pat.span(), "a parameter is a plain name");
            continue;
        };
        let ty = b.f.ty(&arg.ty);
        let local = b.declare(&pat.ident, ty, pat.mutability.is_some(), true);
        let io = super::attrs::io(&arg.attrs, b.f.sink);
        params.push(Param { local, io });
    }
    let block = b.block(&func.block.stmts, true);
    b.body.block = block;
    (params, b.body)
}

/// A constant's or private static's initializer.
pub fn constant(f: &mut Front<'_>, e: &syn::Expr, _ty: TypeId) -> Body {
    let mut b = Builder {
        f,
        body: Body::default(),
        scopes: vec![HashMap::new()],
    };
    let value = b.expr(e);
    b.body.value = Some(value);
    b.body
}

impl Builder<'_, '_> {
    fn span(&mut self, s: proc_macro2::Span) -> crate::diag::SpanId {
        self.f.spans.add(s)
    }

    fn push(&mut self, kind: ExprKind, ty: TypeId, span: proc_macro2::Span) -> ExprId {
        let span = self.span(span);
        self.body.exprs.push(Expr { kind, ty, span });
        ExprId(self.body.exprs.len() as u32 - 1)
    }

    fn error(&mut self, span: proc_macro2::Span, message: impl Into<String>) -> ExprId {
        self.f.sink.error(span, message);
        let ty = self.f.types.error;
        self.push(ExprKind::Error, ty, span)
    }

    fn declare(
        &mut self,
        ident: &syn::Ident,
        ty: TypeId,
        mutable: bool,
        is_param: bool,
    ) -> LocalId {
        let name = self.f.names.intern(&ident.to_string());
        let span = self.span(ident.span());
        self.body.locals.push(Local {
            name,
            ty,
            mutable,
            is_param,
            used: false,
            span,
        });
        let id = LocalId(self.body.locals.len() as u32 - 1);
        self.scopes.last_mut().expect("a scope").insert(name, id);
        id
    }

    fn lookup(&self, sym: Symbol) -> Option<LocalId> {
        self.scopes.iter().rev().find_map(|s| s.get(&sym).copied())
    }

    fn stmt(&mut self, kind: StmtKind, span: proc_macro2::Span) -> Stmt {
        Stmt {
            kind,
            span: self.span(span),
        }
    }

    /// A block in its own scope. A function body's tail expression is its return value.
    fn block(&mut self, stmts: &[syn::Stmt], returns: bool) -> Block {
        self.scopes.push(HashMap::new());
        let mut out = Vec::new();
        for (i, stmt) in stmts.iter().enumerate() {
            let last = i + 1 == stmts.len();
            match stmt {
                syn::Stmt::Local(local) => {
                    if let Some(s) = self.local(local) {
                        out.push(s);
                    }
                }
                syn::Stmt::Expr(e, semi) => {
                    if semi.is_none() && last && returns {
                        out.extend(self.tail(e));
                    } else if let Some(s) = self.statement(e) {
                        out.push(s);
                    }
                }
                syn::Stmt::Item(item) => {
                    self.f
                        .sink
                        .error(item.span(), "items cannot be declared inside a function");
                }
                syn::Stmt::Macro(m) => {
                    if let Some(s) = self.discard(&m.mac) {
                        out.push(s);
                    }
                }
            }
        }
        self.scopes.pop();
        out
    }

    fn local(&mut self, local: &syn::Local) -> Option<Stmt> {
        let (pat, ty) = match &local.pat {
            syn::Pat::Type(t) => (&*t.pat, Some(&*t.ty)),
            other => (other, None),
        };
        let syn::Pat::Ident(ident) = pat else {
            self.f.sink.error(pat.span(), "a `let` binds a plain name");
            return None;
        };
        let ty = match ty {
            Some(t) => self.f.ty(t),
            None => self.f.types.error,
        };
        // The initializer resolves before the name exists: `let x = x + 1.0` reads the outer x.
        let init = match &local.init {
            Some(i) if i.diverge.is_some() => {
                self.f
                    .sink
                    .error(i.expr.span(), "`let .. else` has no shader equivalent");
                None
            }
            Some(i) => Some(self.expr(&i.expr)),
            None => None,
        };
        let id = self.declare(&ident.ident, ty, ident.mutability.is_some(), false);
        Some(self.stmt(StmtKind::Let { local: id, init }, local.span()))
    }

    fn statement(&mut self, e: &syn::Expr) -> Option<Stmt> {
        let span = e.span();
        let kind = match e {
            syn::Expr::Assign(a) => {
                let target = self.expr(&a.left);
                let value = self.expr(&a.right);
                StmtKind::Assign {
                    target,
                    op: None,
                    value,
                }
            }
            syn::Expr::Binary(b) if compound(&b.op).is_some() => {
                let target = self.expr(&b.left);
                let value = self.expr(&b.right);
                StmtKind::Assign {
                    target,
                    op: compound(&b.op),
                    value,
                }
            }
            syn::Expr::If(i) => return self.if_statement(i, false),
            syn::Expr::While(w) => {
                self.no_label(w.label.as_ref());
                let cond = self.expr(&w.cond);
                let body = self.block(&w.body.stmts, false);
                StmtKind::While { cond, body }
            }
            syn::Expr::Loop(l) => {
                self.no_label(l.label.as_ref());
                StmtKind::Loop {
                    body: self.block(&l.body.stmts, false),
                }
            }
            syn::Expr::ForLoop(f) => return self.for_loop(f),
            syn::Expr::Match(m) => return self.switch(m, false),
            syn::Expr::Block(b) => StmtKind::Block(self.block(&b.block.stmts, false)),
            syn::Expr::Break(b) => {
                if b.label.is_some() || b.expr.is_some() {
                    self.f
                        .sink
                        .error(span, "`break` takes no label or value in shaders");
                }
                StmtKind::Break
            }
            syn::Expr::Continue(c) => {
                if let Some(l) = &c.label {
                    self.f.sink.error(l.span(), "loop labels are not supported");
                }
                StmtKind::Continue
            }
            syn::Expr::Return(r) => StmtKind::Return(r.expr.as_ref().map(|v| self.expr(v))),
            syn::Expr::Macro(m) => return self.discard(&m.mac),
            other => StmtKind::Expr(self.expr(other)),
        };
        Some(self.stmt(kind, span))
    }

    /// `discard!()`, the one macro a shader body takes.
    fn discard(&mut self, mac: &syn::Macro) -> Option<Stmt> {
        if !mac.path.is_ident("discard") {
            self.f
                .sink
                .error(mac.span(), "the only macro in a shader is `discard!()`");
            return None;
        }
        if !mac.tokens.is_empty() {
            self.f
                .sink
                .error(mac.tokens.span(), "`discard!()` takes no arguments");
        }
        Some(self.stmt(StmtKind::Discard, mac.span()))
    }

    fn no_label(&mut self, label: Option<&syn::Label>) {
        if let Some(l) = label {
            self.f.sink.error(l.span(), "loop labels are not supported");
        }
    }

    /// A returning function's tail: a value is returned, and a branch returns its own tail.
    fn tail(&mut self, e: &syn::Expr) -> Option<Stmt> {
        match e {
            syn::Expr::If(i) => self.if_statement(i, true),
            syn::Expr::Match(m) => self.switch(m, true),
            syn::Expr::Block(b) => {
                let block = self.block(&b.block.stmts, true);
                Some(self.stmt(StmtKind::Block(block), e.span()))
            }
            e if is_control(e) => self.statement(e),
            e => {
                let value = self.expr(e);
                Some(self.stmt(StmtKind::Return(Some(value)), e.span()))
            }
        }
    }

    /// `returns` when the `if` is a returning function's tail, so each branch returns its own.
    fn if_statement(&mut self, i: &syn::ExprIf, returns: bool) -> Option<Stmt> {
        if matches!(&*i.cond, syn::Expr::Let(_)) {
            self.f
                .sink
                .error(i.cond.span(), "`if let` has no shader equivalent");
            return None;
        }
        let cond = self.expr(&i.cond);
        let then = self.block(&i.then_branch.stmts, returns);
        let otherwise = match i.else_branch.as_ref().map(|(_, e)| &**e) {
            None => None,
            Some(syn::Expr::If(nested)) => {
                Some(self.if_statement(nested, returns).into_iter().collect())
            }
            Some(syn::Expr::Block(b)) => Some(self.block(&b.block.stmts, returns)),
            Some(other) => {
                self.f.sink.error(other.span(), "expected a block");
                None
            }
        };
        Some(self.stmt(
            StmtKind::If {
                cond,
                then,
                otherwise,
            },
            i.span(),
        ))
    }

    fn for_loop(&mut self, f: &syn::ExprForLoop) -> Option<Stmt> {
        self.no_label(f.label.as_ref());
        let syn::Pat::Ident(var) = &*f.pat else {
            self.f
                .sink
                .error(f.pat.span(), "a `for` loop binds a plain name");
            return None;
        };
        let syn::Expr::Range(range) = &*f.expr else {
            self.f
                .sink
                .error(f.expr.span(), "shader `for` loops run over a range, `a..b`");
            return None;
        };
        let (Some(start), Some(end)) = (&range.start, &range.end) else {
            self.f.sink.error(range.span(), "the range needs both ends");
            return None;
        };
        let inclusive = matches!(range.limits, syn::RangeLimits::Closed(_));
        let start = self.expr(start);
        let end = self.expr(end);
        self.scopes.push(HashMap::new());
        let error = self.f.types.error;
        let var = self.declare(&var.ident, error, false, false);
        let body = self.block(&f.body.stmts, false);
        self.scopes.pop();
        Some(self.stmt(
            StmtKind::For {
                var,
                start,
                end,
                inclusive,
                body,
            },
            f.span(),
        ))
    }

    /// `returns` as for [`Self::if_statement`].
    fn switch(&mut self, m: &syn::ExprMatch, returns: bool) -> Option<Stmt> {
        let selector = self.expr(&m.expr);
        let mut cases = Vec::new();
        for arm in &m.arms {
            if let Some((_, guard)) = &arm.guard {
                self.f
                    .sink
                    .error(guard.span(), "match guards are not supported");
            }
            let values = self.case_values(&arm.pat);
            let body = match &*arm.body {
                syn::Expr::Block(b) => self.block(&b.block.stmts, returns),
                other => {
                    self.scopes.push(HashMap::new());
                    let s = if returns {
                        self.tail(other)
                    } else {
                        self.statement(other)
                    };
                    let s = s.into_iter().collect();
                    self.scopes.pop();
                    s
                }
            };
            cases.push((values, body));
        }
        Some(self.stmt(StmtKind::Switch { selector, cases }, m.span()))
    }

    /// The values an arm matches, or `None` for `_`.
    fn case_values(&mut self, pat: &syn::Pat) -> Option<Vec<ExprId>> {
        match pat {
            syn::Pat::Wild(_) => None,
            syn::Pat::Or(or) => {
                let mut all = Vec::new();
                for case in &or.cases {
                    all.extend(self.case_values(case)?);
                }
                Some(all)
            }
            syn::Pat::Lit(l) => Some(vec![self.literal(&l.lit)]),
            syn::Pat::Path(p) => Some(vec![self.path(&p.path, p.span())]),
            other => {
                self.error(other.span(), "match on integer literals and constants");
                Some(Vec::new())
            }
        }
    }

    pub fn expr(&mut self, e: &syn::Expr) -> ExprId {
        let span = e.span();
        let error = self.f.types.error;
        match e {
            syn::Expr::Lit(l) => self.literal(&l.lit),
            syn::Expr::Path(p) if p.qself.is_none() => self.path(&p.path, span),
            syn::Expr::Paren(p) => self.expr(&p.expr),
            syn::Expr::Group(g) => self.expr(&g.expr),
            syn::Expr::Unary(u) => {
                let op = match u.op {
                    syn::UnOp::Neg(_) => UnOp::Neg,
                    syn::UnOp::Not(_) => UnOp::Not,
                    _ => return self.error(span, "pointers have no shader equivalent"),
                };
                let a = self.expr(&u.expr);
                self.push(ExprKind::Unary(op, a), error, span)
            }
            syn::Expr::Binary(b) => {
                let Some(op) = binary(&b.op) else {
                    return self.error(span, "assignment is a statement, not a value");
                };
                let l = self.expr(&b.left);
                let r = self.expr(&b.right);
                self.push(ExprKind::Binary(op, l, r), error, span)
            }
            syn::Expr::Call(c) => self.call(c),
            syn::Expr::MethodCall(m) => self.method(m),
            syn::Expr::Field(f) => {
                let base = self.expr(&f.base);
                match &f.member {
                    syn::Member::Named(name) => {
                        let sym = self.f.names.intern(&name.to_string());
                        self.push(ExprKind::Member(base, sym), error, span)
                    }
                    syn::Member::Unnamed(_) => self.error(span, "tuples have no shader equivalent"),
                }
            }
            syn::Expr::Index(i) => {
                let base = self.expr(&i.expr);
                let index = self.expr(&i.index);
                self.push(ExprKind::Index(base, index), error, span)
            }
            syn::Expr::Cast(c) => {
                let inner = self.expr(&c.expr);
                let ty = self.f.ty(&c.ty);
                self.push(ExprKind::Cast(inner), ty, span)
            }
            syn::Expr::Struct(s) => self.struct_literal(s),
            syn::Expr::Array(a) => {
                let parts: Vec<_> = a.elems.iter().map(|e| self.expr(e)).collect();
                if parts.is_empty() {
                    return self.error(span, "an array literal needs elements");
                }
                self.push(ExprKind::Construct(parts), error, span)
            }
            syn::Expr::If(i) => self.select(i),
            // `[x; N]`: N copies of one value, with N known when the shader is built.
            syn::Expr::Repeat(r) => {
                let n = match self.f.const_value(&r.len) {
                    Ok(n @ 1..=4096) => n as usize,
                    Ok(n) => {
                        return self.error(
                            r.len.span(),
                            format!("an array repeats its value 1 to 4096 times, not {n}"),
                        );
                    }
                    Err((s, m)) => return self.error(s, m),
                };
                let value = self.expr(&r.expr);
                self.push(ExprKind::Construct(vec![value; n]), error, span)
            }
            _ => self.error(span, "this expression has no shader equivalent"),
        }
    }

    fn literal(&mut self, lit: &syn::Lit) -> ExprId {
        let span = lit.span();
        match lit {
            syn::Lit::Int(i) => {
                let scalar = match i.suffix() {
                    "" => Scalar::AbstractInt,
                    "i32" => Scalar::I32,
                    "u32" => Scalar::U32,
                    "f32" => Scalar::F32,
                    other => {
                        return self.error(
                            span,
                            format!("no `{other}` in shaders: use i32, u32 or f32"),
                        );
                    }
                };
                match i.base10_parse::<i128>() {
                    Ok(v) if scalar == Scalar::F32 => {
                        let ty = self.f.types.f32;
                        self.push(ExprKind::Lit(Lit::Float(v as f64)), ty, span)
                    }
                    Ok(v) => {
                        let ty = self.f.types.scalar(scalar);
                        self.push(ExprKind::Lit(Lit::Int(v)), ty, span)
                    }
                    Err(e) => self.error(span, e.to_string()),
                }
            }
            syn::Lit::Float(x) => {
                let scalar = match x.suffix() {
                    "" => Scalar::AbstractFloat,
                    "f32" => Scalar::F32,
                    "f16" => Scalar::F16,
                    other => {
                        return self
                            .error(span, format!("no `{other}` in shaders: use f32 or f16"));
                    }
                };
                match x.base10_parse::<f64>() {
                    Ok(v) => {
                        let ty = self.f.types.scalar(scalar);
                        self.push(ExprKind::Lit(Lit::Float(v)), ty, span)
                    }
                    Err(e) => self.error(span, e.to_string()),
                }
            }
            syn::Lit::Bool(b) => {
                let ty = self.f.types.bool;
                self.push(ExprKind::Lit(Lit::Bool(b.value)), ty, span)
            }
            _ => self.error(span, "this literal has no shader equivalent"),
        }
    }

    fn path(&mut self, path: &syn::Path, span: proc_macro2::Span) -> ExprId {
        let error = self.f.types.error;
        if let Some(ident) = path.get_ident() {
            let sym = self.f.names.intern(&ident.to_string());
            if let Some(local) = self.lookup(sym) {
                return self.push(ExprKind::Local(local), error, span);
            }
            if let Some(&(v, ty)) = self.f.generic_consts.get(&ident.to_string()) {
                return self.push(ExprKind::Lit(Lit::Int(v)), ty, span);
            }
            return match self.f.scope().get(&sym).copied() {
                Some(Item::Const(c)) => self.push(ExprKind::Const(c), error, span),
                Some(Item::Global(g)) => self.push(ExprKind::Global(g), error, span),
                Some(Item::Fn(_)) => self.error(span, format!("`{ident}` is a function: call it")),
                Some(Item::Struct(_)) => self.error(span, format!("`{ident}` is a type")),
                None => self.error(span, format!("cannot find `{ident}` in this scope")),
            };
        }
        // `Type::CONST`: zero and one vectors, and the scalar limits.
        let segs: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
        if let [ty, name] = segs.as_slice()
            && let Some(t) = self.f.named_type(ty)
        {
            let id = self.f.types.add(t);
            return match (t, name.as_str()) {
                (Type::Vector(..) | Type::Matrix { .. }, "ZERO") => {
                    self.push(ExprKind::Zero, id, span)
                }
                (Type::Vector(_, s), "ONE") => {
                    let one = self.scalar_lit(s, 1.0, span);
                    self.push(ExprKind::Splat(one), id, span)
                }
                (Type::Scalar(Scalar::F32), "MAX") => {
                    self.push(ExprKind::Lit(Lit::Float(f32::MAX as f64)), id, span)
                }
                (Type::Scalar(Scalar::F32), "MIN") => {
                    self.push(ExprKind::Lit(Lit::Float(f32::MIN as f64)), id, span)
                }
                (Type::Scalar(Scalar::F32), "EPSILON") => {
                    self.push(ExprKind::Lit(Lit::Float(f32::EPSILON as f64)), id, span)
                }
                (Type::Scalar(Scalar::I32), "MAX") => {
                    self.push(ExprKind::Lit(Lit::Int(i32::MAX as i128)), id, span)
                }
                (Type::Scalar(Scalar::I32), "MIN") => {
                    self.push(ExprKind::Lit(Lit::Int(i32::MIN as i128)), id, span)
                }
                (Type::Scalar(Scalar::U32), "MAX") => {
                    self.push(ExprKind::Lit(Lit::Int(u32::MAX as i128)), id, span)
                }
                _ => self.error(span, format!("no `{ty}::{name}`")),
            };
        }
        if let [.., f, consts, name] = segs.as_slice()
            && f == "f32"
            && consts == "consts"
        {
            let v = match name.as_str() {
                "PI" => std::f64::consts::PI,
                "TAU" => std::f64::consts::TAU,
                "E" => std::f64::consts::E,
                "FRAC_PI_2" => std::f64::consts::FRAC_PI_2,
                "SQRT_2" => std::f64::consts::SQRT_2,
                "LN_2" => std::f64::consts::LN_2,
                _ => return self.error(span, format!("no `f32::consts::{name}`")),
            };
            let ty = self.f.types.f32;
            return self.push(ExprKind::Lit(Lit::Float(v)), ty, span);
        }
        self.error(
            span,
            "paths name a local, a module item or a constant like `Vec3::ZERO`",
        )
    }

    /// One argument of a turbofish: a type, or a const parameter's value. A bare name is a
    /// constant when one by that name is in scope, as Rust reads it.
    fn generic_arg(&mut self, a: &syn::GenericArgument) -> Option<GenericArg> {
        let value = |this: &mut Self, e: &syn::Expr| match this.f.const_value(e) {
            Ok(v) => Some(GenericArg::Const(v)),
            Err((span, message)) => {
                this.f.sink.error(span, message);
                None
            }
        };
        match a {
            syn::GenericArgument::Type(syn::Type::Path(p))
                if p.path.get_ident().is_some_and(|i| self.f.is_constant(i)) =>
            {
                let e = syn::Expr::Path(syn::ExprPath {
                    attrs: Vec::new(),
                    qself: None,
                    path: p.path.clone(),
                });
                value(self, &e)
            }
            syn::GenericArgument::Type(t) => Some(GenericArg::Type(self.f.ty(t))),
            syn::GenericArgument::Const(e) => value(self, e),
            other => {
                self.f
                    .sink
                    .error(other.span(), "a generic argument is a type or a constant");
                None
            }
        }
    }

    fn scalar_lit(&mut self, s: Scalar, v: f64, span: proc_macro2::Span) -> ExprId {
        let ty = self.f.types.scalar(s);
        let lit = if s.is_float() {
            Lit::Float(v)
        } else if s == Scalar::Bool {
            Lit::Bool(v != 0.0)
        } else {
            Lit::Int(v as i128)
        };
        self.push(ExprKind::Lit(lit), ty, span)
    }

    fn call(&mut self, c: &syn::ExprCall) -> ExprId {
        let span = c.span();
        let error = self.f.types.error;
        let syn::Expr::Path(p) = &*c.func else {
            return self.error(c.func.span(), "only named functions can be called");
        };
        let args: Vec<ExprId> = c.args.iter().map(|a| self.expr(a)).collect();
        // `f::<A, B>(..)`: a generic function named with its arguments.
        if let [seg] = p.path.segments.iter().collect::<Vec<_>>().as_slice()
            && let syn::PathArguments::AngleBracketed(g) = &seg.arguments
        {
            let sym = self.f.names.intern(&seg.ident.to_string());
            let Some(Item::Fn(id)) = self.f.scope().get(&sym).copied() else {
                return self.error(
                    seg.ident.span(),
                    format!("cannot find function `{}`", seg.ident),
                );
            };
            let generics = g.args.iter().filter_map(|a| self.generic_arg(a)).collect();
            return self.push(ExprKind::GenericCall(id, generics, args), error, span);
        }
        if let Some(ident) = p.path.get_ident() {
            let name = ident.to_string();
            let sym = self.f.names.intern(&name);
            if let Some(Item::Fn(id)) = self.f.scope().get(&sym).copied() {
                return self.push(ExprKind::Call(id, args), error, span);
            }
            if let Some(def) = builtins::function(&name) {
                return self.push(ExprKind::Builtin(def.builtin, args), error, span);
            }
            if let Some(i) = self.f.ext_names.iter().position(|n| *n == name) {
                return self.push(ExprKind::ExtBuiltin(i as u32, args), error, span);
            }
            if let Some(op) = builtins::texture_function(&name) {
                return self.push(
                    ExprKind::Builtin(builtins::Builtin::Texture(op), args),
                    error,
                    span,
                );
            }
            return self.error(ident.span(), format!("cannot find function `{name}`"));
        }
        let segs: Vec<String> = p
            .path
            .segments
            .iter()
            .map(|s| s.ident.to_string())
            .collect();
        let [ty, ctor] = segs.as_slice() else {
            return self.error(span, "unknown function path");
        };
        let Some(t) = self.f.named_type(ty) else {
            return self.error(span, format!("no constructors on `{ty}`"));
        };
        let id = self.f.types.add(t);
        match (t, ctor.as_str()) {
            (Type::Vector(..), "new") | (Type::Matrix { .. }, "from_cols") => {
                self.push(ExprKind::Construct(args), id, span)
            }
            (Type::Vector(..), "splat") if args.len() == 1 => {
                self.push(ExprKind::Splat(args[0]), id, span)
            }
            (Type::Scalar(Scalar::F32), "from_bits") if args.len() == 1 => {
                self.push(ExprKind::Bitcast(args[0]), id, span)
            }
            (Type::Scalar(Scalar::I32), "from_bits") if args.len() == 1 => {
                self.push(ExprKind::Bitcast(args[0]), id, span)
            }
            _ => self.error(
                span,
                format!("no `{ty}::{ctor}` with {} arguments", args.len()),
            ),
        }
    }

    fn method(&mut self, m: &syn::ExprMethodCall) -> ExprId {
        let span = m.span();
        let error = self.f.types.error;
        let recv = self.expr(&m.receiver);
        let mut args: Vec<ExprId> = m.args.iter().map(|a| self.expr(a)).collect();
        let name = m.method.to_string();
        if name == "to_bits" && args.is_empty() {
            let u = self.f.types.u32;
            return self.push(ExprKind::Bitcast(recv), u, span);
        }
        if let Some(ty) = name.strip_prefix("as_").and_then(cast_target)
            && args.is_empty()
        {
            let id = self.f.types.add(ty);
            return self.push(ExprKind::Cast(recv), id, span);
        }
        if let Some(op) = builtins::texture_method(&name) {
            args.insert(0, recv);
            return self.push(
                ExprKind::Builtin(builtins::Builtin::Texture(op), args),
                error,
                span,
            );
        }
        let Some(def) = builtins::method(&name) else {
            return self.error(m.method.span(), format!("no method `{name}` in shaders"));
        };
        if def.receiver_last {
            args.push(recv);
        } else {
            args.insert(0, recv);
        }
        self.push(ExprKind::Builtin(def.builtin, args), error, span)
    }

    fn struct_literal(&mut self, s: &syn::ExprStruct) -> ExprId {
        let span = s.span();
        if let Some(rest) = &s.rest {
            return self.error(
                rest.span(),
                "`..` in a struct literal has no shader equivalent",
            );
        }
        let Some(ident) = s.path.get_ident() else {
            return self.error(s.path.span(), "expected a struct name");
        };
        let sym = self.f.names.intern(&ident.to_string());
        let Some(Item::Struct(id)) = self.f.scope().get(&sym).copied() else {
            return self.error(ident.span(), format!("`{ident}` is not a struct"));
        };
        let order: Vec<Symbol> = self.f.types.structs[id.0 as usize]
            .fields
            .iter()
            .map(|f| f.0)
            .collect();
        let mut parts: Vec<Option<ExprId>> = vec![None; order.len()];
        for field in &s.fields {
            let syn::Member::Named(name) = &field.member else {
                self.error(field.span(), "fields are named");
                continue;
            };
            let fsym = self.f.names.intern(&name.to_string());
            let value = self.expr(&field.expr);
            match order.iter().position(|&o| o == fsym) {
                Some(i) if parts[i].is_none() => parts[i] = Some(value),
                Some(_) => {
                    self.error(name.span(), format!("field `{name}` given twice"));
                }
                None => {
                    self.error(name.span(), format!("`{ident}` has no field `{name}`"));
                }
            }
        }
        let missing: Vec<&str> = order
            .iter()
            .zip(&parts)
            .filter(|(_, p)| p.is_none())
            .map(|(s, _)| self.f.names.get(*s))
            .collect();
        if !missing.is_empty() {
            let message = format!("missing fields: {}", missing.join(", "));
            return self.error(span, message);
        }
        let ty = self.f.types.add(Type::Struct(id));
        let parts = parts.into_iter().flatten().collect();
        self.push(ExprKind::Construct(parts), ty, span)
    }

    /// `if c { a } else { b }` as a value: both branches are single expressions.
    fn select(&mut self, i: &syn::ExprIf) -> ExprId {
        let span = i.span();
        let single = |b: &syn::Block| match b.stmts.as_slice() {
            [syn::Stmt::Expr(e, None)] => Some(e.clone()),
            _ => None,
        };
        let then = single(&i.then_branch);
        let otherwise = match i.else_branch.as_ref().map(|(_, e)| &**e) {
            Some(syn::Expr::Block(b)) => single(&b.block),
            Some(nested @ syn::Expr::If(_)) => Some(nested.clone()),
            _ => None,
        };
        let (Some(then), Some(otherwise)) = (then, otherwise) else {
            return self.error(
                span,
                "`if` as a value needs an `else` and a single expression in each branch",
            );
        };
        let cond = self.expr(&i.cond);
        let then = self.expr(&then);
        let otherwise = self.expr(&otherwise);
        let error = self.f.types.error;
        self.push(
            ExprKind::Select {
                cond,
                then,
                otherwise,
            },
            error,
            span,
        )
    }
}

/// The vector type of `as_vec3`, `as_uvec2` and the like, glam's conversion methods.
fn cast_target(name: &str) -> Option<Type> {
    let (s, n) = match name.as_bytes() {
        [b'v', b'e', b'c', n] => (Scalar::F32, *n),
        [b'i', b'v', b'e', b'c', n] => (Scalar::I32, *n),
        [b'u', b'v', b'e', b'c', n] => (Scalar::U32, *n),
        [b'b', b'v', b'e', b'c', n] => (Scalar::Bool, *n),
        _ => return None,
    };
    matches!(n, b'2'..=b'4').then(|| Type::Vector(n - b'0', s))
}

fn is_control(e: &syn::Expr) -> bool {
    matches!(
        e,
        syn::Expr::If(_)
            | syn::Expr::While(_)
            | syn::Expr::Loop(_)
            | syn::Expr::ForLoop(_)
            | syn::Expr::Match(_)
            | syn::Expr::Block(_)
            | syn::Expr::Macro(_)
    )
}

fn binary(op: &syn::BinOp) -> Option<BinOp> {
    use syn::BinOp as B;
    Some(match op {
        B::Add(_) => BinOp::Add,
        B::Sub(_) => BinOp::Sub,
        B::Mul(_) => BinOp::Mul,
        B::Div(_) => BinOp::Div,
        B::Rem(_) => BinOp::Rem,
        B::And(_) => BinOp::And,
        B::Or(_) => BinOp::Or,
        B::BitAnd(_) => BinOp::BitAnd,
        B::BitOr(_) => BinOp::BitOr,
        B::BitXor(_) => BinOp::BitXor,
        B::Shl(_) => BinOp::Shl,
        B::Shr(_) => BinOp::Shr,
        B::Eq(_) => BinOp::Eq,
        B::Ne(_) => BinOp::Ne,
        B::Lt(_) => BinOp::Lt,
        B::Le(_) => BinOp::Le,
        B::Gt(_) => BinOp::Gt,
        B::Ge(_) => BinOp::Ge,
        _ => return None,
    })
}

fn compound(op: &syn::BinOp) -> Option<BinOp> {
    use syn::BinOp as B;
    Some(match op {
        B::AddAssign(_) => BinOp::Add,
        B::SubAssign(_) => BinOp::Sub,
        B::MulAssign(_) => BinOp::Mul,
        B::DivAssign(_) => BinOp::Div,
        B::RemAssign(_) => BinOp::Rem,
        B::BitAndAssign(_) => BinOp::BitAnd,
        B::BitOrAssign(_) => BinOp::BitOr,
        B::BitXorAssign(_) => BinOp::BitXor,
        B::ShlAssign(_) => BinOp::Shl,
        B::ShrAssign(_) => BinOp::Shr,
        _ => return None,
    })
}
