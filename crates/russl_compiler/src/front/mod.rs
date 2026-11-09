//! syn to the resolved IR: module-scope names first, so items may refer to each other in any
//! order, then signatures and bodies.

mod attrs;
mod body;
mod ty;

use crate::diag::{Sink, Spans};
use crate::hir::{self, ConstId, FnId, GlobalId};
use crate::intern::{Interner, Symbol};
use crate::types::{StructDef, StructId, Type, Types};
use std::collections::HashMap;
use syn::spanned::Spanned;

/// What a module-scope name refers to.
#[derive(Copy, Clone, Debug)]
pub enum Item {
    Struct(StructId),
    Const(ConstId),
    Global(GlobalId),
    Fn(FnId),
}

/// State shared by the whole module while it is built.
pub struct Front<'a> {
    pub names: &'a mut Interner,
    pub types: &'a mut Types,
    pub spans: &'a mut Spans,
    pub sink: &'a mut Sink,
    pub items: HashMap<Symbol, Item>,
}

/// Build the resolved IR of a module.
pub fn module(
    file: &syn::File,
    names: &mut Interner,
    types: &mut Types,
    spans: &mut Spans,
    sink: &mut Sink,
) -> hir::Module {
    let mut f = Front {
        names,
        types,
        spans,
        sink,
        items: HashMap::new(),
    };
    let mut m = hir::Module::default();

    // Declare every name, so order does not matter.
    let (mut nconst, mut nglobal, mut nfn) = (0u32, 0u32, 0u32);
    for item in &file.items {
        let named = |ident: &syn::Ident, kind| Some((ident.to_string(), ident.span(), kind));
        let decl = match item {
            syn::Item::Struct(s) => {
                let id = StructId(f.types.structs.len() as u32);
                let name = f.names.intern(&s.ident.to_string());
                f.types.structs.push(StructDef {
                    name,
                    fields: Vec::new(),
                });
                named(&s.ident, Item::Struct(id))
            }
            syn::Item::Const(c) => {
                nconst += 1;
                named(&c.ident, Item::Const(ConstId(nconst - 1)))
            }
            syn::Item::Static(s) => {
                nglobal += 1;
                named(&s.ident, Item::Global(GlobalId(nglobal - 1)))
            }
            syn::Item::Fn(func) => {
                nfn += 1;
                named(&func.sig.ident, Item::Fn(FnId(nfn - 1)))
            }
            syn::Item::Verbatim(tokens) => match syn::parse2::<attrs::Decl>(tokens.clone()) {
                Ok(d) if d.is_const => {
                    nconst += 1;
                    named(&d.ident, Item::Const(ConstId(nconst - 1)))
                }
                Ok(d) => {
                    nglobal += 1;
                    named(&d.ident, Item::Global(GlobalId(nglobal - 1)))
                }
                Err(e) => {
                    f.sink.error(e.span(), e.to_string());
                    None
                }
            },
            syn::Item::Use(u) => {
                f.sink
                    .error(u.span(), "`use` between modules is not supported yet");
                None
            }
            other => {
                f.sink
                    .error(other.span(), "this item has no shader equivalent");
                None
            }
        };
        if let Some((name, span, kind)) = decl {
            let sym = f.names.intern(&name);
            if f.items.insert(sym, kind).is_some() {
                f.sink
                    .error(span, format!("`{name}` is defined more than once"));
            }
        }
    }

    // Struct members, now that every struct has an id.
    for item in &file.items {
        if let syn::Item::Struct(s) = item {
            let st = f.structure(s);
            m.structs.push(st);
        }
    }
    for item in &file.items {
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
    for item in &file.items {
        if let syn::Item::Fn(func) = item {
            let built = f.function(func);
            m.functions.push(built);
        }
    }
    m
}

impl Front<'_> {
    fn structure(&mut self, s: &syn::ItemStruct) -> hir::Struct {
        let name = self.names.intern(&s.ident.to_string());
        let Some(Item::Struct(id)) = self.items.get(&name).copied() else {
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
        if !func.sig.generics.params.is_empty() {
            self.sink.error(
                func.sig.generics.span(),
                "generic functions are not supported yet",
            );
        }
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
            name,
            stage,
            params,
            ret,
            ret_span,
            body,
            span,
        }
    }
}
