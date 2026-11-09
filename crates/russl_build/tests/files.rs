//! Programs spanning files: imports by path through the module tree, errors in the file they
//! are in.

use std::path::{Path, PathBuf};

/// A fresh module tree holding `files`.
fn tree(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("russl_build_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for (path, text) in files {
        let p = dir.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    dir
}

const COMMON: &str = "pub fn sd_circle(p: Vec2, r: f32) -> f32 { p.length() - r }\n";
const RING: &str = "use super::super::common::sd_circle;\n\
pub fn sd_ring(p: Vec2, r: f32, w: f32) -> f32 { sd_circle(p, r).abs() - w }\n";

fn main_with(body: &str) -> String {
    format!(
        "use crate::common::sd_circle;\nuse crate::shapes::ring::sd_ring;\n\n\
#[fragment]\n#[location(0)]\nfn fs(#[builtin(position)] p: Vec4) -> Vec4 {{\n{body}\n}}\n"
    )
}

#[test]
fn a_program_compiles_from_its_files() {
    let main = main_with("    Vec4::splat(sd_ring(p.xy, 4.0, 1.0) + sd_circle(p.xy, 2.0))");
    let dir = tree(
        "ok",
        &[
            ("common.russl", COMMON),
            ("shapes/ring.russl", RING),
            ("main.russl", &main),
        ],
    );
    let a = russl_build::compile(dir.join("main.russl")).unwrap_or_else(|e| panic!("{e}"));
    assert!(
        a.wgsl.contains("fn sd_ring") && a.wgsl.contains("fn sd_circle"),
        "{}",
        a.wgsl
    );
    assert!(
        a.wgsl.matches("fn sd_circle").count() == 1,
        "a diamond loads once"
    );
}

#[test]
fn a_module_directory_holds_mod_russl() {
    let main = main_with("    Vec4::splat(sd_ring(p.xy, 4.0, 1.0) + sd_circle(p.xy, 2.0))");
    let dir = tree(
        "mod",
        &[
            ("common/mod.russl", COMMON),
            ("shapes/ring.russl", RING),
            ("main.russl", &main),
        ],
    );
    russl_build::compile(dir.join("main.russl")).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn a_missing_module_names_the_files_it_looked_for() {
    let main = main_with("    Vec4::splat(sd_ring(p.xy, 4.0, 1.0) + sd_circle(p.xy, 2.0))");
    let dir = tree(
        "missing",
        &[("common.russl", COMMON), ("main.russl", &main)],
    );
    let e = russl_build::compile(dir.join("main.russl")).unwrap_err();
    let ring = Path::new("shapes").join("ring");
    assert!(
        e.contains("no file for module `crate::shapes::ring`"),
        "{e}"
    );
    assert!(
        e.contains(&ring.with_extension("russl").display().to_string()),
        "{e}"
    );
    assert!(
        e.contains("main.russl:2:"),
        "the error is at the use item:\n{e}"
    );
}

#[test]
fn an_error_in_an_imported_file_is_shown_in_that_file() {
    let main = main_with("    Vec4::splat(sd_ring(p.xy, 4.0, 1.0) + sd_circle(p.xy, 2.0))");
    let broken = "use super::super::common::sd_circle;\n\
pub fn sd_ring(p: Vec2, r: f32, w: f32) -> f32 {\n    sd_circle(p, r).abs() - true\n}\n";
    let dir = tree(
        "imported",
        &[
            ("common.russl", COMMON),
            ("shapes/ring.russl", broken),
            ("main.russl", &main),
        ],
    );
    let e = russl_build::compile(dir.join("main.russl")).unwrap_err();
    assert!(e.contains("ring.russl:3:"), "{e}");
    assert!(
        e.contains("sd_circle(p, r).abs() - true"),
        "the snippet is from ring.russl:\n{e}"
    );
}

#[test]
fn a_file_imports_by_crate_super_or_self() {
    let main =
        "use common::sd_circle;\n#[fragment]\n#[location(0)]\nfn fs() -> Vec4 { Vec4::ZERO }\n";
    let dir = tree("bare", &[("common.russl", COMMON), ("main.russl", main)]);
    let e = russl_build::compile(dir.join("main.russl")).unwrap_err();
    assert!(e.contains("starts at `crate`, `super` or `self`"), "{e}");
}
