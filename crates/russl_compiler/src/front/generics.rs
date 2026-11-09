//! Generic functions, monomorphised. A generic function keeps its syntax as a template; each
//! distinct set of arguments builds the function again with its parameters bound, so a type
//! parameter resolves like any type (`T::splat(x)`, `[T; 4]`) and a const parameter like any
//! constant (`[f32; N]`, `N` as a value). The analyzer infers the arguments at each call.

use super::{Front, Item};
use crate::diag::{Sink, Spans};
use crate::hir::{self, GenericArg};
use crate::intern::{Interner, Symbol};
use crate::types::{Scalar, Type, TypeId, Types};
use std::collections::{HashMap, HashSet};
use syn::spanned::Spanned;

/// What a type parameter must be: `T: Float` and the like.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Bound {
    Float,
    Int,
    Numeric,
    Scalar,
    Vector,
}

impl Bound {
    fn parse(name: &str) -> Option<Bound> {
        Some(match name {
            "Float" => Bound::Float,
            "Int" => Bound::Int,
            "Numeric" => Bound::Numeric,
            "Scalar" => Bound::Scalar,
            "Vector" => Bound::Vector,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Bound::Float => "Float",
            Bound::Int => "Int",
            Bound::Numeric => "Numeric",
            Bound::Scalar => "Scalar",
            Bound::Vector => "Vector",
        }
    }

    /// Whether `ty` meets the bound. A number bound holds for scalars and vectors of it.
    pub fn holds(self, types: &Types, ty: TypeId) -> bool {
        let t = types.get(ty);
        let s = match t {
            Type::Scalar(s) | Type::Vector(_, s) => Some(s),
            _ => None,
        };
        match self {
            Bound::Float => s.is_some_and(Scalar::is_float),
            Bound::Int => s.is_some_and(Scalar::is_int),
            Bound::Numeric => s.is_some_and(Scalar::is_numeric),
            Bound::Scalar => matches!(t, Type::Scalar(_)),
            Bound::Vector => matches!(t, Type::Vector(..)),
        }
    }
}

/// One generic parameter, in declaration order.
#[derive(Clone, Debug)]
pub enum Param {
    Type(String, Vec<Bound>),
    Const(String, TypeId),
}

pub struct Template {
    item: syn::ItemFn,
    module: usize,
    pub params: Vec<Param>,
}

/// What instantiating a template needs from the front end, kept after it has built the
/// program: every module's names, the constants array lengths read, and the templates.
pub struct Generics {
    scopes: Vec<HashMap<Symbol, Item>>,
    public: Vec<HashSet<Symbol>>,
    const_src: Vec<Option<(syn::Expr, usize)>>,
    pub templates: Vec<Template>,
}

impl Front<'_> {
    /// A generic function's parameters, or `None` after reporting why it cannot be one.
    pub(super) fn template(&mut self, func: &syn::ItemFn) -> Option<Template> {
        let g = &func.sig.generics;
        if let Some(w) = &g.where_clause {
            self.sink.error(
                w.span(),
                "put a parameter's bounds in its declaration: `<T: Float>`",
            );
        }
        if func.attrs.iter().any(|a| {
            ["vertex", "fragment", "compute"]
                .iter()
                .any(|s| a.path().is_ident(s))
        }) {
            self.sink
                .error(g.span(), "an entry point cannot be generic");
            return None;
        }
        let mut params = Vec::new();
        for p in &g.params {
            match p {
                syn::GenericParam::Type(t) => {
                    if let Some(d) = &t.default {
                        self.sink
                            .error(d.span(), "a type parameter takes no default");
                    }
                    let mut bounds = Vec::new();
                    for b in &t.bounds {
                        let name = match b {
                            syn::TypeParamBound::Trait(tr) => {
                                tr.path.get_ident().map(|i| i.to_string())
                            }
                            _ => None,
                        };
                        match name.as_deref().and_then(Bound::parse) {
                            Some(bound) => bounds.push(bound),
                            None => self.sink.error(
                                b.span(),
                                "a bound is `Float`, `Int`, `Numeric`, `Scalar` or `Vector`",
                            ),
                        }
                    }
                    params.push(Param::Type(t.ident.to_string(), bounds));
                }
                syn::GenericParam::Const(c) => {
                    let ty = self.ty(&c.ty);
                    if !matches!(
                        self.types.get(ty),
                        Type::Scalar(Scalar::U32 | Scalar::I32) | Type::Error
                    ) {
                        self.sink
                            .error(c.ty.span(), "a const parameter is `u32` or `i32`");
                    }
                    params.push(Param::Const(c.ident.to_string(), ty));
                }
                syn::GenericParam::Lifetime(l) => {
                    self.sink
                        .error(l.span(), "shaders have no references, so no lifetimes");
                }
            }
        }
        Some(Template {
            item: func.clone(),
            module: self.current,
            params,
        })
    }

    pub(super) fn into_generics(self, templates: Vec<Template>) -> Generics {
        Generics {
            scopes: self.scopes,
            public: self.public,
            const_src: self.const_src,
            templates,
        }
    }

    /// A type parameter bound for the function being built, else a builtin type by name.
    pub fn named_type(&self, name: &str) -> Option<Type> {
        match self.generic_types.get(name) {
            Some(&t) => Some(self.types.get(t)),
            None => super::ty::builtin_type(name),
        }
    }
}

impl Generics {
    /// The arguments of a call of template `t`: those named, then those its argument types
    /// imply (`args`, made concrete). An error says what could not be decided.
    pub fn infer(
        &self,
        t: usize,
        explicit: &[GenericArg],
        args: &[TypeId],
        types: &Types,
        names: &Interner,
    ) -> Result<Vec<GenericArg>, String> {
        let template = &self.templates[t];
        let name = template.item.sig.ident.to_string();
        if explicit.len() > template.params.len() {
            return Err(format!(
                "`{name}` takes {} generic arguments, {} given",
                template.params.len(),
                explicit.len()
            ));
        }
        let mut out: Vec<Option<GenericArg>> = vec![None; template.params.len()];
        for (i, (arg, param)) in explicit.iter().zip(&template.params).enumerate() {
            match (arg, param) {
                (GenericArg::Type(_), Param::Type(..))
                | (GenericArg::Const(_), Param::Const(..)) => out[i] = Some(arg.clone()),
                (_, Param::Type(p, _)) => return Err(format!("`{p}` is a type parameter")),
                (_, Param::Const(p, _)) => return Err(format!("`{p}` is a const parameter")),
            }
        }
        let inputs: Vec<&syn::Type> = template
            .item
            .sig
            .inputs
            .iter()
            .filter_map(|i| match i {
                syn::FnArg::Typed(p) => Some(&*p.ty),
                syn::FnArg::Receiver(_) => None,
            })
            .collect();
        for (pat, &ty) in inputs.iter().zip(args) {
            unify(&template.params, pat, ty, types, names, &mut out)?;
        }
        out.into_iter()
            .zip(&template.params)
            .map(|(a, p)| {
                a.ok_or_else(|| {
                    let (p, example) = match p {
                        Param::Type(p, _) => (p, "f32"),
                        Param::Const(p, _) => (p, "4"),
                    };
                    format!(
                        "cannot tell `{p}` for this call of `{name}` from its arguments; name it: `{name}::<{example}>(..)`"
                    )
                })
            })
            .collect()
    }

    /// Build template `t` with `args` bound to its parameters.
    #[allow(clippy::too_many_arguments)]
    pub fn instantiate(
        &mut self,
        t: usize,
        args: &[GenericArg],
        name: Symbol,
        names: &mut Interner,
        types: &mut Types,
        spans: &mut Spans,
        sink: &mut Sink,
    ) -> hir::Function {
        let template = &self.templates[t];
        let (item, module) = (template.item.clone(), template.module);
        let mut generic_types = HashMap::new();
        let mut generic_consts = HashMap::new();
        for (p, a) in template.params.iter().zip(args) {
            match (p, a) {
                (Param::Type(n, _), GenericArg::Type(ty)) => {
                    generic_types.insert(n.clone(), *ty);
                }
                (Param::Const(n, ty), GenericArg::Const(v)) => {
                    generic_consts.insert(n.clone(), (*v, *ty));
                }
                _ => unreachable!("inferred to match"),
            }
        }
        let mut f = Front {
            names,
            types,
            spans,
            sink,
            scopes: std::mem::take(&mut self.scopes),
            public: std::mem::take(&mut self.public),
            current: module,
            const_src: std::mem::take(&mut self.const_src),
            generic_types,
            generic_consts,
        };
        let mut built = f.function(&item);
        built.name = name;
        self.scopes = f.scopes;
        self.public = f.public;
        self.const_src = f.const_src;
        built
    }

    /// The parameter names of template `t`, for messages: `lerp::<Vec3>`.
    pub fn display(
        &self,
        t: usize,
        args: &[GenericArg],
        types: &Types,
        names: &Interner,
    ) -> String {
        let name = self.templates[t].item.sig.ident.to_string();
        let args: Vec<String> = args
            .iter()
            .map(|a| match a {
                GenericArg::Type(ty) => types.name(*ty, names),
                GenericArg::Const(v) => v.to_string(),
            })
            .collect();
        format!("{name}::<{}>", args.join(", "))
    }
}

/// Bind the parameters a parameter's declared type `pat` names, given the argument type `ty`.
fn unify(
    params: &[Param],
    pat: &syn::Type,
    ty: TypeId,
    types: &Types,
    names: &Interner,
    out: &mut [Option<GenericArg>],
) -> Result<(), String> {
    let index = |name: &str, ty: bool| {
        params.iter().position(|p| match p {
            Param::Type(n, _) => ty && n == name,
            Param::Const(n, _) => !ty && n == name,
        })
    };
    let bind = |out: &mut [Option<GenericArg>], i: usize, arg: GenericArg| -> Result<(), String> {
        match &out[i] {
            Some(have) if *have != arg => {
                let show = |a: &GenericArg| match a {
                    GenericArg::Type(t) => format!("`{}`", types.name(*t, names)),
                    GenericArg::Const(v) => v.to_string(),
                };
                let p = match &params[i] {
                    Param::Type(n, _) | Param::Const(n, _) => n,
                };
                Err(format!(
                    "`{p}` is {} by one argument and {} by another",
                    show(have),
                    show(&arg)
                ))
            }
            _ => {
                out[i] = Some(arg);
                Ok(())
            }
        }
    };
    match pat {
        syn::Type::Paren(p) => unify(params, &p.elem, ty, types, names, out),
        syn::Type::Path(p) if p.qself.is_none() => {
            if let Some(i) = p
                .path
                .get_ident()
                .and_then(|id| index(&id.to_string(), true))
                && types.get(ty) != Type::Error
            {
                bind(out, i, GenericArg::Type(ty))?;
            }
            Ok(())
        }
        syn::Type::Array(a) => {
            if let Type::Array { elem, len } = types.get(ty) {
                unify(params, &a.elem, elem, types, names, out)?;
                if let (syn::Expr::Path(p), Some(n)) = (&a.len, len)
                    && let Some(i) = p
                        .path
                        .get_ident()
                        .and_then(|id| index(&id.to_string(), false))
                {
                    bind(out, i, GenericArg::Const(n as i128))?;
                }
            }
            Ok(())
        }
        _ => Ok(()),
    }
}
