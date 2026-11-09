//! The analyzer: types every expression, checks statements and places, orders functions by
//! their calls, and applies the entry-point, binding and naming rules.

mod expr;
mod rules;
mod stmt;
mod texture;

use crate::diag::{Sink, Spans};
use crate::hir::*;
use crate::intern::Interner;
use crate::types::{TypeId, Types};
use std::collections::HashSet;

/// What lowering needs from the analysis.
pub struct Analysis {
    /// Functions that are not entry points, callees before callers.
    pub order: Vec<FnId>,
    /// Functions the root module's entry points reach.
    pub reachable: HashSet<FnId>,
}

/// Which body is being checked.
#[derive(Copy, Clone)]
enum Target {
    Fn(FnId),
    Const(ConstId),
    Global(GlobalId),
}

pub struct Checker<'a> {
    names: &'a Interner,
    types: &'a mut Types,
    module: &'a mut Module,
    spans: &'a Spans,
    sink: &'a mut Sink,
    target: Target,
    current: FnId,
    calls: Vec<(FnId, FnId)>,
    used_consts: HashSet<ConstId>,
    used_globals: HashSet<GlobalId>,
    /// Which globals each function reads or writes, for the bindings a program really uses.
    fn_globals: Vec<(FnId, GlobalId)>,
    /// Operations only a fragment shader runs, with the function they are in.
    fragment_ops: Vec<(FnId, crate::diag::SpanId, &'static str)>,
    /// Whether the body being checked is the root module's; only its warnings are reported,
    /// since an imported module reports its own when it is compiled.
    root: bool,
    loop_depth: u32,
    ret: Option<TypeId>,
}

pub fn module(
    names: &Interner,
    types: &mut Types,
    module: &mut Module,
    spans: &Spans,
    sink: &mut Sink,
) -> Analysis {
    let mut c = Checker {
        names,
        types,
        module,
        spans,
        sink,
        target: Target::Const(ConstId(0)),
        current: FnId(0),
        calls: Vec::new(),
        used_consts: HashSet::new(),
        used_globals: HashSet::new(),
        fn_globals: Vec::new(),
        fragment_ops: Vec::new(),
        root: true,
        loop_depth: 0,
        ret: None,
    };
    for i in 0..c.module.consts.len() {
        let id = ConstId(i as u32);
        c.target = Target::Const(id);
        c.root = c.module.consts[i].module == 0;
        let ty = c.module.consts[i].ty;
        if let Some(value) = c.module.consts[i].value.as_ref().and_then(|b| b.value) {
            c.check(value, Some(ty));
            c.require_const(value);
        }
    }
    for i in 0..c.module.globals.len() {
        let id = GlobalId(i as u32);
        c.target = Target::Global(id);
        c.root = c.module.globals[i].module == 0;
        let ty = c.module.globals[i].ty;
        if let Some(value) = c.module.globals[i].init.as_ref().and_then(|b| b.value) {
            c.check(value, Some(ty));
            c.require_const(value);
        }
    }
    for i in 0..c.module.functions.len() {
        let id = FnId(i as u32);
        c.target = Target::Fn(id);
        c.current = id;
        c.root = c.module.functions[i].module == 0;
        c.loop_depth = 0;
        c.ret = c.module.functions[i].ret.map(|r| r.0);
        c.function(id);
    }
    let order = c.order();
    let reachable = c.reachable();
    c.rules(&reachable);
    Analysis { order, reachable }
}

impl Checker<'_> {
    fn body(&self) -> &Body {
        match self.target {
            Target::Fn(f) => &self.module.functions[f.index()].body,
            Target::Const(k) => self.module.consts[k.index()]
                .value
                .as_ref()
                .expect("checked only with a value"),
            Target::Global(g) => self.module.globals[g.index()]
                .init
                .as_ref()
                .expect("checked only with an initializer"),
        }
    }

    fn body_mut(&mut self) -> &mut Body {
        match self.target {
            Target::Fn(f) => &mut self.module.functions[f.index()].body,
            Target::Const(k) => self.module.consts[k.index()]
                .value
                .as_mut()
                .expect("checked only with a value"),
            Target::Global(g) => self.module.globals[g.index()]
                .init
                .as_mut()
                .expect("checked only with an initializer"),
        }
    }

    fn error_at(&mut self, e: ExprId, message: impl Into<String>) {
        let span = self.spans.get(self.body().expr(e).span);
        self.sink.error(span, message);
    }

    fn ty_name(&self, t: TypeId) -> String {
        self.types.name(t, self.names)
    }

    /// A constant's value must be known when the shader is built.
    fn require_const(&mut self, e: ExprId) {
        let kids: Vec<ExprId> = match self.body().expr(e).kind.clone() {
            ExprKind::Lit(_) | ExprKind::Zero | ExprKind::Const(_) | ExprKind::Error => Vec::new(),
            ExprKind::Unary(_, a)
            | ExprKind::Splat(a)
            | ExprKind::Cast(a)
            | ExprKind::Bitcast(a) => vec![a],
            ExprKind::Binary(_, a, b) => vec![a, b],
            ExprKind::Construct(parts) | ExprKind::Builtin(_, parts) => parts,
            ExprKind::Field(a, _) | ExprKind::Swizzle(a, _) => vec![a],
            _ => {
                self.error_at(
                    e,
                    "a constant's value must be known when the shader is built",
                );
                Vec::new()
            }
        };
        for k in kids {
            self.require_const(k);
        }
    }

    fn function(&mut self, id: FnId) {
        let f = &self.module.functions[id.index()];
        let has_ret = f.ret.is_some_and(|(t, _)| t != self.types.unit);
        let ret_span = f.ret_span;
        let block = std::mem::take(&mut self.module.functions[id.index()].body.block);
        let block = self.block(block);
        if has_ret && !stmt::returns(&block) {
            let name = self
                .names
                .get(self.module.functions[id.index()].name)
                .to_string();
            self.sink.error(
                self.spans.get(ret_span),
                format!("`{name}` does not return a value on every path"),
            );
        }
        self.module.functions[id.index()].body.block = block;
        let locals: Vec<(String, bool, crate::diag::SpanId)> = self.module.functions[id.index()]
            .body
            .locals
            .iter()
            .map(|l| (self.names.get(l.name).to_string(), l.used, l.span))
            .collect();
        for (name, used, span) in locals {
            if self.root && !used && !name.starts_with('_') {
                self.sink.warning(
                    self.spans.get(span),
                    format!("unused variable: `{name}`; if this is intentional, prefix it with an underscore: `_{name}`"),
                );
            }
        }
    }

    /// Functions reachable from the root module's entry points.
    fn reachable(&self) -> HashSet<FnId> {
        let mut seen: HashSet<FnId> = (0..self.module.functions.len())
            .map(|i| FnId(i as u32))
            .filter(|f| {
                let func = &self.module.functions[f.index()];
                func.module == 0 && func.stage.is_some()
            })
            .collect();
        let mut stack: Vec<FnId> = seen.iter().copied().collect();
        while let Some(f) = stack.pop() {
            for &(from, to) in &self.calls {
                if from == f && seen.insert(to) {
                    stack.push(to);
                }
            }
        }
        seen
    }

    /// Callees before callers; recursion, which no target supports, is an error.
    fn order(&mut self) -> Vec<FnId> {
        let n = self.module.functions.len();
        let mut state = vec![0u8; n];
        let mut out = Vec::new();
        let calls = self.calls.clone();
        fn visit(
            f: usize,
            calls: &[(FnId, FnId)],
            state: &mut [u8],
            out: &mut Vec<FnId>,
            cycle: &mut Option<usize>,
        ) {
            match state[f] {
                2 => return,
                1 => {
                    cycle.get_or_insert(f);
                    return;
                }
                _ => {}
            }
            state[f] = 1;
            for &(from, to) in calls {
                if from.index() == f {
                    visit(to.index(), calls, state, out, cycle);
                }
            }
            state[f] = 2;
            out.push(FnId(f as u32));
        }
        let mut cycle = None;
        for f in 0..n {
            visit(f, &calls, &mut state, &mut out, &mut cycle);
        }
        if let Some(f) = cycle {
            let func = &self.module.functions[f];
            let name = self.names.get(func.name).to_string();
            self.sink.error(
                self.spans.get(func.span),
                format!("`{name}` calls itself; shaders cannot recurse"),
            );
        }
        out.retain(|f| self.module.functions[f.index()].stage.is_none());
        out
    }
}
