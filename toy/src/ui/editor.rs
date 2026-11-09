//! The editor: one tab per module of the sketch, and a button that adds one. `main` holds the
//! entry point; another module is imported from it by name, `use crate::<name>::f;`. Each
//! editor fills its tab, which fills the panel.

use super::{HEADER_H, ModuleTab, bordered, icon_button, store};
use crate::highlight::ShaderHighlighter;
use blinc_app::prelude::*;
use blinc_cn::prelude::*;
use blinc_core::reactive::computed;

/// The editor panel: its tabs, made again only when the set of modules changes, with the add
/// button at the header's end.
pub(super) fn editor() -> impl ElementBuilder {
    bordered()
        .relative()
        .for_each(
            // One tabs element for the whole set: made again when a module comes or goes.
            computed(move |g| vec![g.get(store().names).unwrap_or_default()]),
            |names: &Vec<String>| names.join("/"),
            move |names: Vec<String>| {
                let mut tabs = cn::tabs(store().active);
                for name in names {
                    let state = store()
                        .modules
                        .lock()
                        .unwrap()
                        .iter()
                        .find(|m| m.name == name)
                        .map(|m| m.editor.clone());
                    let Some(state) = state else { continue };
                    tabs = tabs.tab(name.clone(), format!("{name}.russl"), move || {
                        div().w_full().h_full().child(
                            code_editor(&state)
                                .w_full()
                                .font_size(14.0)
                                .line_numbers(true)
                                .code_folding(true)
                                .syntax(SyntaxConfig::new(ShaderHighlighter::russl()))
                                .on_change(|_| store().recompile()),
                        )
                    });
                }
                div().w_full().h_full().child(tabs)
            },
        )
        .child(
            div()
                .absolute()
                .top((HEADER_H - 24.0) / 2.0)
                .right(10.0)
                .child(icon_button(
                    "toy-add-module",
                    icons::PLUS,
                    "Add a module",
                    add_module,
                )),
        )
}

/// Add a module tab with a function `main` can import, and show it.
fn add_module() {
    let (name, names) = {
        let mut modules = store().modules.lock().unwrap();
        let n = (2..)
            .find(|n| !modules.iter().any(|m| m.name == format!("module{n}")))
            .unwrap();
        let name = format!("module{n}");
        let text = format!(
            "// Import it in main with: use crate::{name}::helper;\npub fn helper(x: f32) -> f32 {{\n    x * 0.5\n}}\n"
        );
        modules.push(ModuleTab {
            name: name.clone(),
            editor: code_editor_state(text),
        });
        let names = modules.iter().map(|m| m.name.clone()).collect::<Vec<_>>();
        (name, names)
    };
    store().log(format!("added {name}.russl"));
    store().names.set(names);
    store().active.set(name);
    store().recompile();
}
