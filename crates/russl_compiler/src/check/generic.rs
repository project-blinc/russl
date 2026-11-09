//! Calls of generic functions. Each distinct set of arguments builds an instance once, through
//! the front end, and the call becomes a call of that instance. Instances are checked like any
//! function; an error in one names the instance it was found in.

use super::{Checker, Target};
use crate::front::Param;
use crate::hir::*;
use crate::types::{Type, TypeId};

/// More instances of one template than this means it instantiates itself without end.
const MAX_INSTANCES: usize = 64;

impl Checker<'_> {
    pub(super) fn check_function(&mut self, id: FnId) {
        self.target = Target::Fn(id);
        self.current = id;
        self.loop_depth = 0;
        self.ret = self.module.functions[id.index()].ret.map(|r| r.0);
        self.function(id);
    }

    /// Check every instance built so far, and those their bodies build in turn.
    pub(super) fn check_instances(&mut self) {
        while let Some((id, t)) = self.unchecked.pop() {
            self.root = self.module.functions[id.index()].module == 0 && self.warned.insert(t);
            let before = self.sink.items.len();
            self.check_function(id);
            let label = self.module.functions[id.index()]
                .instance
                .clone()
                .unwrap_or_default();
            for d in &mut self.sink.items[before..] {
                if d.severity == crate::Severity::Error {
                    d.message = format!("{} (in `{label}`)", d.message);
                }
            }
        }
    }

    /// A call of the generic function `f`, with the generic arguments it names.
    pub(super) fn instance_call(
        &mut self,
        e: ExprId,
        f: FnId,
        explicit: &[GenericArg],
        args: &[ExprId],
    ) -> TypeId {
        let error = self.types.error;
        let Some(t) = self.module.functions[f.index()].template else {
            let name = self
                .names
                .get(self.module.functions[f.index()].name)
                .to_string();
            self.error_at(e, format!("`{name}` takes no generic arguments"));
            return error;
        };
        let mut arg_types = Vec::with_capacity(args.len());
        for &a in args {
            let ty = self.infer(a);
            if self.types.get(ty) == Type::Error {
                return error;
            }
            arg_types.push(if self.types.is_abstract(ty) {
                self.types.concrete(ty)
            } else {
                ty
            });
        }
        let subst = match self
            .generics
            .infer(t as usize, explicit, &arg_types, self.types, self.names)
        {
            Ok(s) => s,
            Err(message) => {
                self.error_at(e, message);
                return error;
            }
        };
        let name = self
            .names
            .get(self.module.functions[f.index()].name)
            .to_string();
        let params = self.generics.templates[t as usize].params.clone();
        for (p, a) in params.iter().zip(&subst) {
            if let (Param::Type(n, bounds), GenericArg::Type(ty)) = (p, a) {
                for b in bounds {
                    if !b.holds(self.types, *ty) {
                        let have = self.ty_name(*ty);
                        self.error_at(
                            e,
                            format!(
                                "`{name}` needs `{n}: {}`, and `{n}` is `{have}` here",
                                b.name()
                            ),
                        );
                        return error;
                    }
                }
            }
        }
        let id = match self.instances.get(&(t, subst.clone())) {
            Some(&id) => id,
            None => {
                let count = self.instances.keys().filter(|k| k.0 == t).count();
                if count >= MAX_INSTANCES {
                    self.error_at(
                        e,
                        format!("`{name}` instantiates itself without end: {count} instances"),
                    );
                    return error;
                }
                let label = self
                    .generics
                    .display(t as usize, &subst, self.types, self.names);
                let sym = self.module.functions[f.index()].name;
                let mut func = self.generics.instantiate(
                    t as usize, &subst, sym, self.names, self.types, self.spans, self.sink,
                );
                func.instance = Some(label);
                let built = self.generics.take_structs();
                self.module.structs.extend(built);
                let id = FnId(self.module.functions.len() as u32);
                self.module.functions.push(func);
                self.instances.insert((t, subst), id);
                self.unchecked.push((id, t));
                id
            }
        };
        self.instantiated.insert(f);
        self.body_mut().exprs[e.index()].kind = ExprKind::Call(id, args.to_vec());
        self.call(e, id, args)
    }
}
