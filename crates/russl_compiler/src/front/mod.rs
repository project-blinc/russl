//! syn to the resolved IR: module-scope names first, so items may refer to each other in any
//! order, then signatures and bodies.

mod attrs;
mod body;
mod generics;
mod ty;

pub use generics::{Generics, Param};

use crate::diag::{Sink, Spans};
use crate::hir::{self, ConstId, FnId, GlobalId};
use crate::intern::{Interner, Symbol};
use crate::types::{StructDef, StructId, Type, TypeId, Types};
use std::collections::{HashMap, HashSet};
use syn::spanned::Spanned;

/// What a module-scope name refers to.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Item {
    Struct(StructId),
    Const(ConstId),
    Global(GlobalId),
    Fn(FnId),
}

/// One module of a program: its syntax, and which module each of its `use` paths names.
pub struct Source<'s> {
    pub file: &'s syn::File,
    /// `use` module paths as [`use_key`] spells them, to indices into the program's sources.
    pub imports: &'s [(String, usize)],
}

/// State shared by the whole program while it is built.
pub struct Front<'a> {
    pub names: &'a mut Interner,
    pub types: &'a mut Types,
    pub spans: &'a mut Spans,
    pub sink: &'a mut Sink,
    /// Each module's names: its own items and what it imports.
    scopes: Vec<HashMap<Symbol, Item>>,
    /// The names each module exports: its `pub` items and `pub use` re-exports.
    public: Vec<HashSet<Symbol>>,
    /// The module whose items are being built.
    current: usize,
    /// Each constant's initializer and module, by `ConstId`, for array lengths; `None` for a
    /// pipeline constant, which no length may use.
    const_src: Vec<Option<(syn::Expr, usize)>>,
    /// While a generic function's instance is built: its type and const parameters' values.
    generic_types: HashMap<String, TypeId>,
    generic_consts: HashMap<String, (i128, TypeId)>,
    /// The extension's builtins by name, in `hir::Module::ext_builtins` order.
    ext_names: Vec<String>,
}

/// Build the resolved IR of a program. `sources[0]` is the module being compiled; the rest are
/// what it imports, directly or through other imports.
pub fn program(
    sources: &[Source<'_>],
    names: &mut Interner,
    types: &mut Types,
    spans: &mut Spans,
    sink: &mut Sink,
    builtins: &[crate::extension::Builtin],
) -> (hir::Module, Generics) {
    let n = sources.len();
    let mut f = Front {
        names,
        types,
        spans,
        sink,
        scopes: vec![HashMap::new(); n],
        public: vec![HashSet::new(); n],
        current: 0,
        const_src: Vec::new(),
        generic_types: HashMap::new(),
        generic_consts: HashMap::new(),
        ext_names: builtins.iter().map(|b| b.name.clone()).collect(),
    };
    let mut m = hir::Module::default();
    let mut templates = Vec::new();

    // Declare every name of every module, so order does not matter. Ids run across modules in
    // this order, and the passes below visit items in the same order.
    let (mut nconst, mut nglobal, mut nfn) = (0u32, 0u32, 0u32);
    for (mi, src) in sources.iter().enumerate() {
        for item in &src.file.items {
            let named = |ident: &syn::Ident, public: bool, kind| {
                Some((ident.to_string(), ident.span(), public, kind))
            };
            let is_pub = |v: &syn::Visibility| matches!(v, syn::Visibility::Public(_));
            let decl = match item {
                syn::Item::Struct(s) => {
                    let id = StructId(f.types.structs.len() as u32);
                    let name = f.names.intern(&s.ident.to_string());
                    f.types.structs.push(StructDef {
                        name,
                        fields: Vec::new(),
                    });
                    named(&s.ident, is_pub(&s.vis), Item::Struct(id))
                }
                syn::Item::Const(c) => {
                    nconst += 1;
                    let pipeline = c.attrs.iter().any(|a| a.path().is_ident("pipeline"));
                    f.const_src
                        .push((!pipeline).then(|| ((*c.expr).clone(), mi)));
                    named(&c.ident, is_pub(&c.vis), Item::Const(ConstId(nconst - 1)))
                }
                syn::Item::Static(s) => {
                    nglobal += 1;
                    named(
                        &s.ident,
                        is_pub(&s.vis),
                        Item::Global(GlobalId(nglobal - 1)),
                    )
                }
                syn::Item::Fn(func) => {
                    nfn += 1;
                    named(&func.sig.ident, is_pub(&func.vis), Item::Fn(FnId(nfn - 1)))
                }
                syn::Item::Verbatim(tokens) => match syn::parse2::<attrs::Decl>(tokens.clone()) {
                    Ok(d) if d.is_const => {
                        nconst += 1;
                        f.const_src.push(None);
                        named(&d.ident, is_pub(&d.vis), Item::Const(ConstId(nconst - 1)))
                    }
                    Ok(d) => {
                        nglobal += 1;
                        named(
                            &d.ident,
                            is_pub(&d.vis),
                            Item::Global(GlobalId(nglobal - 1)),
                        )
                    }
                    Err(e) => {
                        f.sink.error(e.span(), e.to_string());
                        None
                    }
                },
                syn::Item::Use(_) => None,
                other => {
                    f.sink
                        .error(other.span(), "this item has no shader equivalent");
                    None
                }
            };
            if let Some((name, span, public, kind)) = decl {
                let sym = f.names.intern(&name);
                if f.scopes[mi].insert(sym, kind).is_some() {
                    f.sink
                        .error(span, format!("`{name}` is defined more than once"));
                }
                if public {
                    f.public[mi].insert(sym);
                }
            }
        }
    }
    f.resolve_uses(sources);

    // Struct members, now that every struct has an id.
    for (mi, src) in sources.iter().enumerate() {
        f.current = mi;
        for item in &src.file.items {
            if let syn::Item::Struct(s) = item {
                let st = f.structure(s);
                m.structs.push(st);
            }
        }
    }
    // The extension's builtins, typed as the root module reads type names.
    f.current = 0;
    for b in builtins {
        let mut ty = |text: &str| match syn::parse_str::<syn::Type>(text) {
            Ok(t) => f.ty(&t),
            Err(e) => {
                f.sink.error(
                    proc_macro2::Span::call_site(),
                    format!(
                        "the extension's builtin `{}` has a type `{text}` that does not parse: {e}",
                        b.name
                    ),
                );
                f.types.error
            }
        };
        let params = b.params.iter().map(|p| ty(p)).collect();
        let ret = ty(&b.ret);
        m.ext_builtins.push((params, ret));
    }
    for (mi, src) in sources.iter().enumerate() {
        f.current = mi;
        for item in &src.file.items {
            match item {
                syn::Item::Const(c) => {
                    let k = f.constant(&c.attrs, &c.ident, &c.ty, Some(&c.expr), c.span());
                    m.consts.push(k);
                }
                syn::Item::Static(s) => m.globals.push(f.private_static(s)),
                syn::Item::Verbatim(tokens) => {
                    if let Ok(d) = syn::parse2::<attrs::Decl>(tokens.clone()) {
                        if d.is_const {
                            let k = f.constant(&d.attrs, &d.ident, &d.ty, None, tokens.span());
                            m.consts.push(k);
                        } else {
                            m.globals.push(f.resource(&d, tokens.span()));
                        }
                    }
                }
                _ => {}
            }
        }
    }
    for (mi, src) in sources.iter().enumerate() {
        f.current = mi;
        for item in &src.file.items {
            if let syn::Item::Fn(func) = item {
                let built = if func.sig.generics.params.is_empty() {
                    f.function(func)
                } else {
                    let k = templates.len() as u32;
                    if let Some(t) = f.template(func) {
                        templates.push(t);
                    }
                    let span = f.spans.add(func.sig.ident.span());
                    hir::Function {
                        module: mi as u32,
                        name: f.names.intern(&func.sig.ident.to_string()),
                        template: (templates.len() as u32 > k).then_some(k),
                        instance: None,
                        stage: None,
                        params: Vec::new(),
                        ret: None,
                        ret_span: span,
                        body: hir::Body::default(),
                        span,
                    }
                };
                m.functions.push(built);
            }
        }
    }
    (m, f.into_generics(templates))
}

/// One name a `use` item brings in.
struct UseLeaf {
    module: String,
    /// The module path as written, with its spans.
    path: Vec<syn::Ident>,
    /// `None` for a glob.
    name: Option<syn::Ident>,
    alias: Option<syn::Ident>,
    public: bool,
    span: proc_macro2::Span,
}

/// How a `use` module path is spelled as a key: its segments joined by `::`.
pub fn use_key(segments: &[String]) -> String {
    segments.join("::")
}

/// The module paths a file imports from, each once, in order: the key and the path's tokens.
pub fn use_paths(file: &syn::File) -> Vec<(String, Vec<syn::Ident>)> {
    let mut out: Vec<(String, Vec<syn::Ident>)> = Vec::new();
    for leaf in use_leaves(file) {
        if !out.iter().any(|(k, _)| *k == leaf.module) {
            out.push((leaf.module, leaf.path));
        }
    }
    out
}

/// Each module path a file imports from, once, with the span of the first name taken from it.
pub fn use_modules(file: &syn::File) -> Vec<(String, proc_macro2::Span)> {
    let mut out: Vec<(String, proc_macro2::Span)> = Vec::new();
    for leaf in use_leaves(file) {
        if !out.iter().any(|(k, _)| *k == leaf.module) {
            out.push((leaf.module, leaf.span));
        }
    }
    out
}

fn use_leaves(file: &syn::File) -> Vec<UseLeaf> {
    fn walk(
        tree: &syn::UseTree,
        prefix: &mut Vec<syn::Ident>,
        public: bool,
        out: &mut Vec<UseLeaf>,
    ) {
        let key = |p: &[syn::Ident]| use_key(&p.iter().map(|i| i.to_string()).collect::<Vec<_>>());
        match tree {
            syn::UseTree::Path(p) => {
                prefix.push(p.ident.clone());
                walk(&p.tree, prefix, public, out);
                prefix.pop();
            }
            syn::UseTree::Name(n) => out.push(UseLeaf {
                module: key(prefix),
                path: prefix.clone(),
                name: Some(n.ident.clone()),
                alias: None,
                public,
                span: n.ident.span(),
            }),
            syn::UseTree::Rename(r) => out.push(UseLeaf {
                module: key(prefix),
                path: prefix.clone(),
                name: Some(r.ident.clone()),
                alias: Some(r.rename.clone()),
                public,
                span: r.ident.span(),
            }),
            syn::UseTree::Glob(g) => out.push(UseLeaf {
                module: key(prefix),
                path: prefix.clone(),
                name: None,
                alias: None,
                public,
                span: g.star_token.span,
            }),
            syn::UseTree::Group(g) => {
                for t in &g.items {
                    walk(t, prefix, public, out);
                }
            }
        }
    }
    let mut out = Vec::new();
    for item in &file.items {
        if let syn::Item::Use(u) = item {
            walk(
                &u.tree,
                &mut Vec::new(),
                matches!(u.vis, syn::Visibility::Public(_)),
                &mut out,
            );
        }
    }
    out
}

impl Front<'_> {
    /// The names visible in the module being built.
    pub fn scope(&self) -> &HashMap<Symbol, Item> {
        &self.scopes[self.current]
    }

    /// Bring imported names into scope. Repeats until nothing changes, so `pub use`
    /// re-exports chain through modules in any order.
    fn resolve_uses(&mut self, sources: &[Source<'_>]) {
        let leaves: Vec<Vec<UseLeaf>> = sources.iter().map(|s| use_leaves(s.file)).collect();
        let mut done: Vec<Vec<bool>> = leaves.iter().map(|l| vec![false; l.len()]).collect();
        loop {
            let mut progress = false;
            for (mi, list) in leaves.iter().enumerate() {
                for (li, leaf) in list.iter().enumerate() {
                    if done[mi][li] {
                        continue;
                    }
                    let Some(target) = sources[mi]
                        .imports
                        .iter()
                        .find(|(k, _)| *k == leaf.module)
                        .map(|(_, t)| *t)
                    else {
                        continue;
                    };
                    let found: Vec<(Symbol, Item)> = match &leaf.name {
                        None => self.public[target]
                            .iter()
                            .map(|s| (*s, self.scopes[target][s]))
                            .collect(),
                        Some(name) => {
                            let sym = self.names.intern(&name.to_string());
                            match (
                                self.scopes[target].get(&sym),
                                self.public[target].contains(&sym),
                            ) {
                                (Some(item), true) => vec![(sym, *item)],
                                _ => continue,
                            }
                        }
                    };
                    for (sym, item) in found {
                        let alias = leaf
                            .alias
                            .as_ref()
                            .map(|a| self.names.intern(&a.to_string()))
                            .unwrap_or(sym);
                        match self.scopes[mi].get(&alias) {
                            Some(existing) if *existing != item && leaf.name.is_some() => {
                                let n = self.names.get(alias).to_string();
                                self.sink.error(
                                    leaf.span,
                                    format!("`{n}` is already defined in this module"),
                                );
                            }
                            Some(_) => {}
                            None => {
                                self.scopes[mi].insert(alias, item);
                                progress = true;
                            }
                        }
                        if leaf.public && self.public[mi].insert(alias) {
                            progress = true;
                        }
                    }
                    done[mi][li] = leaf.name.is_some();
                    progress |= leaf.name.is_some();
                }
            }
            if !progress {
                break;
            }
        }
        for (mi, list) in leaves.iter().enumerate() {
            for (li, leaf) in list.iter().enumerate() {
                if done[mi][li] || leaf.name.is_none() {
                    continue;
                }
                let name = leaf.name.as_ref().expect("checked");
                let known = sources[mi].imports.iter().any(|(k, _)| *k == leaf.module);
                let message = if known {
                    format!("`{}` has no public item `{name}`", leaf.module)
                } else {
                    format!("cannot find module `{}`", leaf.module)
                };
                self.sink.error(leaf.span, message);
            }
        }
    }
}

impl Front<'_> {
    fn structure(&mut self, s: &syn::ItemStruct) -> hir::Struct {
        let name = self.names.intern(&s.ident.to_string());
        let Some(Item::Struct(id)) = self.scope().get(&name).copied() else {
            unreachable!("declared above");
        };
        if !s.generics.params.is_empty() {
            self.sink
                .error(s.generics.span(), "generic structs are not supported yet");
        }
        let mut fields = Vec::new();
        for field in &s.fields {
            let Some(ident) = &field.ident else {
                self.sink.error(field.span(), "struct fields need names");
                continue;
            };
            let fname = self.names.intern(&ident.to_string());
            let ty = self.ty(&field.ty);
            let io = attrs::io(&field.attrs, self.sink);
            self.types.structs[id.0 as usize].fields.push((fname, ty));
            let span = self.spans.add(field.span());
            fields.push(hir::Field {
                name: fname,
                ty,
                io,
                span,
            });
        }
        let ty = self.types.add(Type::Struct(id));
        let span = self.spans.add(s.ident.span());
        hir::Struct {
            module: self.current as u32,
            name,
            ty,
            fields,
            span,
        }
    }

    fn constant(
        &mut self,
        attrs: &[syn::Attribute],
        ident: &syn::Ident,
        ty: &syn::Type,
        value: Option<&syn::Expr>,
        span: proc_macro2::Span,
    ) -> hir::Const {
        let name = self.names.intern(&ident.to_string());
        let ty = self.ty(ty);
        let pipeline = attrs::pipeline(attrs, self.sink);
        if pipeline.is_none() && value.is_none() {
            self.sink
                .error(ident.span(), "a const needs a value, or `#[pipeline]`");
        }
        let value = value.map(|v| body::constant(self, v, ty));
        let span = self.spans.add(span);
        hir::Const {
            module: self.current as u32,
            name,
            ty,
            value,
            pipeline,
            span,
        }
    }

    fn private_static(&mut self, s: &syn::ItemStatic) -> hir::Global {
        if !matches!(s.mutability, syn::StaticMutability::Mut(_)) {
            self.sink.error(
                s.span(),
                "a static with a value is `static mut`, private to each invocation",
            );
        }
        let name = self.names.intern(&s.ident.to_string());
        let ty = self.ty(&s.ty);
        let init = Some(body::constant(self, &s.expr, ty));
        let span = self.spans.add(s.ident.span());
        hir::Global {
            module: self.current as u32,
            name,
            space: hir::Space::Private,
            ty,
            binding: None,
            init,
            span,
        }
    }

    fn resource(&mut self, d: &attrs::Decl, span: proc_macro2::Span) -> hir::Global {
        let name = self.names.intern(&d.ident.to_string());
        let (space, inner) = attrs::address_space(&d.ty);
        let ty = self.ty(inner);
        let space = match space {
            Some(s) => s,
            None => match self.types.get(ty) {
                Type::Texture { .. } | Type::Sampler { .. } => hir::Space::Handle,
                _ => {
                    self.sink.error(
                        d.ty.span(),
                        "a resource is `Uniform<T>`, `Storage<T>`, `StorageMut<T>`, `Workgroup<T>`, a texture or a sampler",
                    );
                    hir::Space::Handle
                }
            },
        };
        let group = attrs::int(&d.attrs, "group", self.sink);
        let binding = attrs::int(&d.attrs, "binding", self.sink);
        let binding = match (space, group, binding) {
            (hir::Space::Workgroup, None, None) => None,
            (hir::Space::Workgroup, ..) => {
                self.sink
                    .error(d.ident.span(), "workgroup memory takes no binding");
                None
            }
            (_, Some(g), Some(b)) => Some((g, b)),
            _ => {
                self.sink.error(
                    d.ident.span(),
                    "a resource needs `#[group(n)]` and `#[binding(n)]`",
                );
                None
            }
        };
        let span = self.spans.add(span);
        hir::Global {
            module: self.current as u32,
            name,
            space,
            ty,
            binding,
            init: None,
            span,
        }
    }

    fn function(&mut self, func: &syn::ItemFn) -> hir::Function {
        let name = self.names.intern(&func.sig.ident.to_string());
        let stage = attrs::stage(&func.attrs, self.sink);
        let ret = match &func.sig.output {
            syn::ReturnType::Default => None,
            syn::ReturnType::Type(_, t) => Some((self.ty(t), attrs::io(&func.attrs, self.sink))),
        };
        let ret_span = self.spans.add(match &func.sig.output {
            syn::ReturnType::Default => func.sig.ident.span(),
            syn::ReturnType::Type(_, t) => t.span(),
        });
        let (params, body) = body::function(self, func);
        let span = self.spans.add(func.sig.ident.span());
        hir::Function {
            module: self.current as u32,
            name,
            template: None,
            instance: None,
            stage,
            params,
            ret,
            ret_span,
            body,
            span,
        }
    }
}
