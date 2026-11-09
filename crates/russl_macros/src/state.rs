//! The state an import chain carries from one macro expansion to the next, as tokens.
//!
//! ```text
//! root { .. }
//! loaded [ { id "…" path { … } imports [ ("key" "id") … ] source { … } } … ]
//! root_imports [ ("key" "id") … ]
//! pending [ { path { … } root_key "…" } { path { … } dep "…" } … ]
//! module { id "…" deps [ "…" … ] imports [ ("key" "id") … ] source { … } }   // appended by a source macro
//! ```

use proc_macro2::TokenStream as Tokens;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::{LitStr, braced, bracketed, parenthesized};

/// A module of the program, fetched through its source macro.
pub struct Loaded {
    pub id: String,
    /// The macro path it was fetched through, from the root's module.
    pub path: Tokens,
    pub imports: Vec<(String, String)>,
    pub source: Tokens,
}

/// A module still to fetch: through a `use` of the root (`root_key`), or as a dependency of a
/// module already loaded (`dep`, the id expected).
pub struct Pending {
    pub path: Tokens,
    pub root_key: Option<String>,
    pub dep: Option<String>,
}

pub struct State {
    pub root: Tokens,
    pub loaded: Vec<Loaded>,
    pub root_imports: Vec<(String, String)>,
    pub pending: Vec<Pending>,
}

/// What a source macro appends.
pub struct Delivered {
    id: String,
    deps: Vec<String>,
    imports: Vec<(String, String)>,
    source: Tokens,
}

pub struct Continue {
    pub state: State,
    pub module: Delivered,
}

impl State {
    /// Take in the module fetched for the first pending entry, and queue what it depends on.
    pub fn deliver(&mut self, m: Delivered) {
        let fetched = self.pending.remove(0);
        if let Some(key) = &fetched.root_key {
            self.root_imports.push((key.clone(), m.id.clone()));
        }
        if self.loaded.iter().any(|l| l.id == m.id) {
            return;
        }
        // A module fetched by its own path lists its dependencies as `__russl_dep_<k>` there;
        // one fetched through an alias has its dependencies in the list that alias came from,
        // which is queued already.
        if let Some(prefix) = fetched
            .root_key
            .as_ref()
            .map(|_| module_part(&fetched.path))
        {
            for (k, dep) in m.deps.iter().enumerate() {
                let known = self.loaded.iter().any(|l| &l.id == dep)
                    || self.pending.iter().any(|p| p.dep.as_ref() == Some(dep));
                if !known && *dep != m.id {
                    let alias = quote::format_ident!("__russl_dep_{}", k);
                    self.pending.push(Pending {
                        path: quote! { #prefix::#alias },
                        root_key: None,
                        dep: Some(dep.clone()),
                    });
                }
            }
        }
        self.loaded.push(Loaded {
            id: m.id,
            path: fetched.path,
            imports: m.imports,
            source: m.source,
        });
    }

    /// The state as tokens. The first pending entry is the module being fetched; `deliver`
    /// removes it when the module arrives.
    pub fn to_tokens(&self) -> Tokens {
        let root = &self.root;
        let loaded = self.loaded.iter().map(|l| {
            let (id, path, source) = (&l.id, &l.path, &l.source);
            let imports = l.imports.iter().map(|(k, i)| quote! { (#k #i) });
            quote! { { id #id path { #path } imports [#(#imports)*] source { #source } } }
        });
        let root_imports = self.root_imports.iter().map(|(k, i)| quote! { (#k #i) });
        let pending = self.pending.iter().map(|p| {
            let path = &p.path;
            match (&p.root_key, &p.dep) {
                (Some(k), _) => quote! { { path { #path } root_key #k } },
                (None, Some(d)) => quote! { { path { #path } dep #d } },
                (None, None) => quote! {},
            }
        });
        quote! { root { #root } loaded [#(#loaded)*] root_imports [#(#root_imports)*] pending [#(#pending)*] }
    }

    pub fn root_id(&self) -> String {
        // FNV-1a over the source and where the macro was called: stable across builds, and
        // distinct for two modules with the same source.
        let at = proc_macro2::Span::call_site().start();
        let text = format!("{}@{}:{}", self.root, at.line, at.column);
        let mut h: u64 = 0xcbf29ce484222325;
        for b in text.bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        format!("{h:016x}")
    }

    /// A module's imports as indices into `order`, the program's modules by id.
    pub fn imports_in(&self, imports: &[(String, String)], order: &[&str]) -> Vec<(String, usize)> {
        imports
            .iter()
            .filter_map(|(k, id)| Some((k.clone(), order.iter().position(|o| o == id)?)))
            .collect()
    }
}

/// `a::b::__russl_source` without its last segment.
fn module_part(path: &Tokens) -> Tokens {
    let parsed: syn::Path = syn::parse2(path.clone()).expect("a macro path");
    let mut segs = parsed.segments.clone();
    segs.pop();
    let leading = parsed.leading_colon;
    let segs: Vec<_> = segs.into_iter().collect();
    quote! { #leading #(#segs)::* }
}

fn tokens_in_braces(input: ParseStream) -> syn::Result<Tokens> {
    let content;
    braced!(content in input);
    content.parse()
}

fn keyword(input: ParseStream, word: &str) -> syn::Result<()> {
    let ident: syn::Ident = input.parse()?;
    if ident != word {
        return Err(syn::Error::new(
            ident.span(),
            format!("russl import state: expected `{word}`"),
        ));
    }
    Ok(())
}

fn pairs(input: ParseStream) -> syn::Result<Vec<(String, String)>> {
    let content;
    bracketed!(content in input);
    let mut out = Vec::new();
    while !content.is_empty() {
        let pair;
        parenthesized!(pair in content);
        let k: LitStr = pair.parse()?;
        let _ = pair.parse::<Option<syn::Token![,]>>()?;
        let v: LitStr = pair.parse()?;
        out.push((k.value(), v.value()));
    }
    Ok(out)
}

impl Parse for Continue {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        keyword(input, "root")?;
        let root = tokens_in_braces(input)?;
        keyword(input, "loaded")?;
        let content;
        bracketed!(content in input);
        let mut loaded = Vec::new();
        while !content.is_empty() {
            let item;
            braced!(item in content);
            keyword(&item, "id")?;
            let id: LitStr = item.parse()?;
            keyword(&item, "path")?;
            let path = tokens_in_braces(&item)?;
            keyword(&item, "imports")?;
            let imports = pairs(&item)?;
            keyword(&item, "source")?;
            let source = tokens_in_braces(&item)?;
            loaded.push(Loaded {
                id: id.value(),
                path,
                imports,
                source,
            });
        }
        keyword(input, "root_imports")?;
        let root_imports = pairs(input)?;
        keyword(input, "pending")?;
        let content;
        bracketed!(content in input);
        let mut pending = Vec::new();
        while !content.is_empty() {
            let item;
            braced!(item in content);
            keyword(&item, "path")?;
            let path = tokens_in_braces(&item)?;
            let kind: syn::Ident = item.parse()?;
            let value: LitStr = item.parse()?;
            let (root_key, dep) = if kind == "root_key" {
                (Some(value.value()), None)
            } else {
                (None, Some(value.value()))
            };
            pending.push(Pending {
                path,
                root_key,
                dep,
            });
        }
        keyword(input, "module")?;
        let m;
        braced!(m in input);
        keyword(&m, "id")?;
        let id: LitStr = m.parse()?;
        keyword(&m, "deps")?;
        let deps_content;
        bracketed!(deps_content in m);
        let mut deps = Vec::new();
        while !deps_content.is_empty() {
            deps.push(deps_content.parse::<LitStr>()?.value());
        }
        keyword(&m, "imports")?;
        let imports = pairs(&m)?;
        keyword(&m, "source")?;
        let source = tokens_in_braces(&m)?;
        let state = State {
            root,
            loaded,
            root_imports,
            pending,
        };
        Ok(Continue {
            state,
            module: Delivered {
                id: id.value(),
                deps,
                imports,
                source,
            },
        })
    }
}
