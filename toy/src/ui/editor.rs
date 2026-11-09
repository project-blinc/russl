//! The editor: one tab per module of the sketch, and a button that adds one. `main` holds the
//! entry point; another module is imported from it by name, `use crate::<name>::f;`.

use super::{HEADER_H, ModuleTab, Toy, bordered, editor_height, icon_button, log, recompile, toy};
use crate::highlight::ShaderHighlighter;
use blinc_app::prelude::*;
use blinc_cn::prelude::*;

/// The editor panel: its tabs, rebuilt when a module is added, with the add button at the
/// header's end.
pub(super) fn editor(t: &'static Toy) -> impl ElementBuilder {
    bordered()
        .relative()
        .child(
            stateful_with_key::<NoState>("toy-editor-tabs")
                .deps([
                    t.modules_rev.id(),
                    t.window_h.id(),
                    t.bottom_h.id(),
                    t.panel_open.id(),
                ])
                .on_state(move |_| {
                    let h = editor_height(t);
                    let mut tabs = cn::tabs(t.active);
                    for m in t.modules.lock().unwrap().iter() {
                        let state = m.editor.clone();
                        tabs = tabs.tab(m.name.clone(), format!("{}.russl", m.name), move || {
                            div().w_full().h(h).child(
                                code_editor(&state)
                                    .w_full()
                                    .h(h)
                                    .font_size(14.0)
                                    .line_numbers(true)
                                    .syntax(SyntaxConfig::new(ShaderHighlighter::russl()))
                                    .on_change(|_| recompile(toy())),
                            )
                        });
                    }
                    div().w_full().h_full().child(tabs)
                }),
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
                    true,
                    add_module,
                )),
        )
}

/// Add a module tab with a function `main` can import, and show it.
fn add_module() {
    let t = toy();
    let name = {
        let mut modules = t.modules.lock().unwrap();
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
        name
    };
    log(t, format!("added {name}.russl"));
    t.modules_rev.update(|r| r + 1);
    t.active.set(name);
    recompile(t);
}
