//! Module rules: entry-point inputs and outputs, resource bindings, Rust's naming conventions
//! and unused items.

use super::Checker;
use crate::diag::SpanId;
use crate::hir::*;
use crate::intern::Symbol;
use crate::types::{Scalar, Type, TypeId};
use std::collections::HashMap;

#[derive(Copy, Clone, PartialEq, Eq)]
enum Dir {
    In,
    Out,
}

impl Checker<'_> {
    pub(super) fn rules(&mut self, reachable: &std::collections::HashSet<FnId>) {
        self.entry_points();
        self.stages();
        self.bindings(reachable);
        self.conventions();
        self.unused_items();
    }

    fn entry_points(&mut self) {
        for i in 0..self.module.functions.len() {
            let f = &self.module.functions[i];
            if f.module != 0 {
                continue;
            }
            let Some(stage) = f.stage else {
                // IO attributes mean nothing outside an entry point.
                let stray =
                    f.params.iter().any(|p| p.io.is_some()) || f.ret.is_some_and(|r| r.1.is_some());
                if stray {
                    let span = f.span;
                    self.warn_or_error(
                        span,
                        true,
                        "`#[location]` and `#[builtin]` apply only to entry points",
                    );
                }
                continue;
            };
            let mut seen: HashMap<(bool, u32), ()> = HashMap::new();
            let params: Vec<(Option<Io>, TypeId, SpanId)> = f
                .params
                .iter()
                .map(|p| {
                    let l = &f.body.locals[p.local.index()];
                    (p.io, l.ty, l.span)
                })
                .collect();
            let ret = f.ret;
            let ret_span = f.ret_span;
            let mut has_position = false;
            for (io, ty, span) in params {
                self.io(stage, Dir::In, io, ty, span, &mut seen, &mut has_position);
            }
            match (stage, ret) {
                (Stage::Compute(_), Some(_)) => {
                    self.warn_or_error(ret_span, true, "a compute entry point returns nothing")
                }
                (Stage::Compute(_), None) => {}
                (_, Some((ty, io))) => {
                    has_position = false;
                    self.io(
                        stage,
                        Dir::Out,
                        io,
                        ty,
                        ret_span,
                        &mut seen,
                        &mut has_position,
                    );
                    if stage == Stage::Vertex && !has_position {
                        self.warn_or_error(
                            ret_span,
                            true,
                            "a vertex entry point returns `#[builtin(position)]`",
                        );
                    }
                }
                (Stage::Vertex, None) => self.warn_or_error(
                    ret_span,
                    true,
                    "a vertex entry point returns `#[builtin(position)]`",
                ),
                (Stage::Fragment, None) => {}
            }
        }
    }

    /// A vertex or compute entry point must not reach an operation only fragments run.
    fn stages(&mut self) {
        let mut reported = std::collections::HashSet::new();
        for i in 0..self.module.functions.len() {
            let f = &self.module.functions[i];
            let stage = match f.stage {
                Some(Stage::Vertex) => "vertex",
                Some(Stage::Compute(_)) => "compute",
                _ => continue,
            };
            if f.module != 0 {
                continue;
            }
            let entry = self.names.get(f.name).to_string();
            let mut seen = std::collections::HashSet::from([FnId(i as u32)]);
            let mut stack = vec![FnId(i as u32)];
            while let Some(g) = stack.pop() {
                for &(from, to) in &self.calls {
                    if from == g && seen.insert(to) {
                        stack.push(to);
                    }
                }
            }
            let ops: Vec<_> = self
                .fragment_ops
                .iter()
                .filter(|(f, span, _)| seen.contains(f) && reported.insert(*span))
                .map(|&(_, span, what)| (span, what))
                .collect();
            for (span, what) in ops {
                self.sink.error(
                    self.spans.get(span),
                    format!("{what} runs only in a fragment shader, and `{entry}` is a {stage} entry point"),
                );
            }
        }
    }

    /// One entry-point input or output: bound directly, or a struct whose fields are all bound.
    #[allow(clippy::too_many_arguments)]
    fn io(
        &mut self,
        stage: Stage,
        dir: Dir,
        io: Option<Io>,
        ty: TypeId,
        span: SpanId,
        seen: &mut HashMap<(bool, u32), ()>,
        has_position: &mut bool,
    ) {
        match (io, self.types.get(ty)) {
            (Some(io), _) => self.binding(stage, dir, io, ty, span, seen, has_position),
            (None, Type::Struct(s)) => {
                let fields: Vec<(Option<Io>, TypeId, SpanId)> = self
                    .module
                    .structs
                    .iter()
                    .find(|st| st.ty == ty)
                    .map(|st| st.fields.iter().map(|f| (f.io, f.ty, f.span)).collect())
                    .unwrap_or_default();
                let _ = s;
                for (fio, fty, fspan) in fields {
                    match fio {
                        Some(fio) => self.binding(stage, dir, fio, fty, fspan, seen, has_position),
                        None => self.warn_or_error(fspan, true, "every field of an entry point's input or output struct needs `#[location]` or `#[builtin]`"),
                    }
                }
            }
            (None, Type::Error) => {}
            (None, _) => self.warn_or_error(
                span,
                true,
                "an entry point's input or output needs `#[location(n)]` or `#[builtin(name)]`",
            ),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn binding(
        &mut self,
        stage: Stage,
        dir: Dir,
        io: Io,
        ty: TypeId,
        span: SpanId,
        seen: &mut HashMap<(bool, u32), ()>,
        has_position: &mut bool,
    ) {
        match io {
            Io::Location { location, .. } => {
                let ok = matches!(self.types.get(ty), Type::Scalar(s) | Type::Vector(_, s) if s.is_numeric() && !s.is_abstract());
                if !ok && self.types.get(ty) != Type::Error {
                    let name = self.ty_name(ty);
                    self.warn_or_error(
                        span,
                        true,
                        &format!(
                            "a `#[location]` carries a number or numeric vector, not `{name}`"
                        ),
                    );
                }
                if seen.insert((dir == Dir::Out, location), ()).is_some() {
                    self.warn_or_error(
                        span,
                        true,
                        &format!("`#[location({location})]` is used twice"),
                    );
                }
            }
            Io::Builtin(b) => {
                let (want, allowed) = builtin_rule(b, stage, dir);
                if !allowed {
                    self.warn_or_error(span, true, &format!("`{b:?}` is not available here"));
                }
                if matches!(b, Builtin::Position { .. }) && dir == Dir::Out {
                    *has_position = true;
                }
                let want = self.types.add(want);
                if want != ty && self.types.get(ty) != Type::Error {
                    let (w, h) = (self.ty_name(want), self.ty_name(ty));
                    self.warn_or_error(span, true, &format!("`{b:?}` is `{w}`, not `{h}`"));
                }
            }
        }
    }

    /// The root's globals and those its entry points reach, which are the ones emitted.
    fn bindings(&mut self, reachable: &std::collections::HashSet<FnId>) {
        let emitted = self.emitted(reachable);
        let mut seen: HashMap<(u32, u32), Symbol> = HashMap::new();
        for g in 0..self.module.globals.len() {
            if !emitted.contains(&g) {
                continue;
            }
            let global = &self.module.globals[g];
            let (name, space, ty, binding, span) = (
                global.name,
                global.space,
                global.ty,
                global.binding,
                global.span,
            );
            if let Some(b) = binding
                && let Some(other) = seen.insert(b, name)
            {
                let other = self.names.get(other).to_string();
                self.warn_or_error(
                    span,
                    true,
                    &format!(
                        "`#[group({})] #[binding({})]` is already `{other}`",
                        b.0, b.1
                    ),
                );
            }
            match space {
                Space::Uniform | Space::Storage { .. } => {
                    if !self.types.host_shareable(ty) {
                        let n = self.ty_name(ty);
                        self.warn_or_error(span, true, &format!("`{n}` cannot be shared with the host: no `bool` in uniform or storage buffers"));
                    }
                    let runtime = self.runtime_array_misplaced(ty, space == Space::Uniform);
                    if runtime {
                        self.warn_or_error(span, true, "a runtime-sized array is a storage buffer's whole type or its struct's last field");
                    } else if space == Space::Uniform {
                        self.uniform_layout(ty, span);
                    }
                }
                Space::Handle
                    if !matches!(
                        self.types.get(ty),
                        Type::Texture { .. } | Type::Sampler { .. } | Type::Error
                    ) =>
                {
                    self.warn_or_error(
                        span,
                        true,
                        "only textures and samplers bind without an address space",
                    );
                }
                _ => {}
            }
        }
    }

    /// Whether a runtime-sized array appears where WGSL forbids it.
    fn runtime_array_misplaced(&self, ty: TypeId, uniform: bool) -> bool {
        let inner = |t: TypeId| self.contains_runtime_array(t);
        match self.types.get(ty) {
            Type::Array { len: None, elem } => uniform || inner(elem),
            Type::Struct(s) => {
                let fields = &self.types.structs[s.0 as usize].fields;
                fields
                    .iter()
                    .enumerate()
                    .any(|(i, &(_, t))| match self.types.get(t) {
                        Type::Array { len: None, elem } => {
                            uniform || i + 1 != fields.len() || inner(elem)
                        }
                        _ => inner(t),
                    })
            }
            _ => inner(ty),
        }
    }

    fn contains_runtime_array(&self, ty: TypeId) -> bool {
        match self.types.get(ty) {
            Type::Array { len: None, .. } => true,
            Type::Array { elem, .. } => self.contains_runtime_array(elem),
            Type::Struct(s) => self.types.structs[s.0 as usize]
                .fields
                .iter()
                .any(|&(_, t)| self.contains_runtime_array(t)),
            _ => false,
        }
    }

    /// Rust's naming conventions, as warnings.
    fn conventions(&mut self) {
        let mut warn = Vec::new();
        for s in self.module.structs.iter().filter(|s| s.module == 0) {
            let n = self.names.get(s.name);
            if !is_camel(n) {
                warn.push((
                    s.span,
                    format!("type `{n}` should have an upper camel case name"),
                ));
            }
            for f in &s.fields {
                let fname = self.names.get(f.name);
                if !is_snake(fname) {
                    warn.push((
                        f.span,
                        format!("field `{fname}` should have a snake case name"),
                    ));
                }
            }
        }
        for k in self.module.consts.iter().filter(|k| k.module == 0) {
            let n = self.names.get(k.name);
            if !is_screaming(n) {
                warn.push((
                    k.span,
                    format!("constant `{n}` should have an upper case name"),
                ));
            }
        }
        for g in self.module.globals.iter().filter(|g| g.module == 0) {
            let n = self.names.get(g.name);
            if !is_screaming(n) {
                warn.push((
                    g.span,
                    format!("static `{n}` should have an upper case name"),
                ));
            }
        }
        for f in self.module.functions.iter().filter(|f| f.module == 0) {
            let n = self.names.get(f.name);
            if !is_snake(n) {
                warn.push((
                    f.span,
                    format!("function `{n}` should have a snake case name"),
                ));
            }
            for l in &f.body.locals {
                let ln = self.names.get(l.name);
                if !is_snake(ln) {
                    warn.push((
                        l.span,
                        format!("variable `{ln}` should have a snake case name"),
                    ));
                }
            }
        }
        for (span, message) in warn {
            self.sink.warning(self.spans.get(span), message);
        }
    }

    /// Items no entry point reaches. A module without entry points is a library, so nothing in
    /// it is unused.
    fn unused_items(&mut self) {
        if !self
            .module
            .functions
            .iter()
            .any(|f| f.module == 0 && f.stage.is_some())
        {
            return;
        }
        let called: std::collections::HashSet<FnId> = self.calls.iter().map(|c| c.1).collect();
        let mut warn = Vec::new();
        for (i, f) in self.module.functions.iter().enumerate() {
            let used =
                called.contains(&FnId(i as u32)) || self.instantiated.contains(&FnId(i as u32));
            if f.module == 0 && f.stage.is_none() && f.instance.is_none() && !used {
                warn.push((
                    f.span,
                    format!("function `{}` is never used", self.names.get(f.name)),
                ));
            }
        }
        for (i, k) in self.module.consts.iter().enumerate() {
            if k.module == 0
                && k.pipeline.is_none()
                && !self.used_consts.contains(&ConstId(i as u32))
            {
                warn.push((
                    k.span,
                    format!("constant `{}` is never used", self.names.get(k.name)),
                ));
            }
        }
        for (i, g) in self.module.globals.iter().enumerate() {
            if g.module == 0 && !self.used_globals.contains(&GlobalId(i as u32)) {
                warn.push((
                    g.span,
                    format!("static `{}` is never used", self.names.get(g.name)),
                ));
            }
        }
        for (span, message) in warn {
            self.sink.warning(self.spans.get(span), message);
        }
    }

    fn warn_or_error(&mut self, span: SpanId, error: bool, message: &str) {
        let span = self.spans.get(span);
        if error {
            self.sink.error(span, message);
        } else {
            self.sink.warning(span, message);
        }
    }
}

/// A builtin's type, and whether it is available to this stage in this direction.
fn builtin_rule(b: Builtin, stage: Stage, dir: Dir) -> (Type, bool) {
    use Stage::*;
    let u = Type::Scalar(Scalar::U32);
    let uvec3 = Type::Vector(3, Scalar::U32);
    match b {
        Builtin::Position { .. } => (
            Type::Vector(4, Scalar::F32),
            matches!((stage, dir), (Vertex, Dir::Out) | (Fragment, Dir::In)),
        ),
        Builtin::VertexIndex | Builtin::InstanceIndex => (u, stage == Vertex && dir == Dir::In),
        Builtin::FrontFacing => (
            Type::Scalar(Scalar::Bool),
            stage == Fragment && dir == Dir::In,
        ),
        Builtin::FragDepth => (
            Type::Scalar(Scalar::F32),
            stage == Fragment && dir == Dir::Out,
        ),
        Builtin::SampleIndex => (u, stage == Fragment && dir == Dir::In),
        Builtin::SampleMask => (u, stage == Fragment),
        Builtin::LocalInvocationIndex => (u, matches!(stage, Compute(_)) && dir == Dir::In),
        Builtin::LocalInvocationId
        | Builtin::GlobalInvocationId
        | Builtin::WorkgroupId
        | Builtin::NumWorkgroups => (uvec3, matches!(stage, Compute(_)) && dir == Dir::In),
    }
}

fn is_snake(s: &str) -> bool {
    let t = s.trim_start_matches('_');
    t.chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && !t.contains("__")
}

fn is_camel(s: &str) -> bool {
    s.chars().next().is_some_and(|c| c.is_ascii_uppercase()) && !s.contains('_')
}

fn is_screaming(s: &str) -> bool {
    let t = s.trim_start_matches('_');
    t.chars()
        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn naming_conventions_follow_rustc() {
        assert!(is_snake("sd_rounded_rect") && is_snake("_unused") && !is_snake("sdRect"));
        assert!(is_camel("VsOut") && !is_camel("vs_out") && !is_camel("Vs_Out"));
        assert!(is_screaming("FEAT_SHADOW") && is_screaming("MAX_2") && !is_screaming("Feat"));
    }
}
