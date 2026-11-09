//! Programs of several modules, as the macro and the build driver assemble them.

use russl_compiler::{Module, Severity, compile_program};

fn module(src: &str, imports: &[(&str, usize)]) -> Module {
    Module {
        source: src.parse().expect("tokens"),
        imports: imports.iter().map(|(k, i)| (k.to_string(), *i)).collect(),
    }
}

const COMMON: &str = r#"
    pub fn sd_circle(p: Vec2, r: f32) -> f32 { p.length() - r }
    pub fn sd_box(p: Vec2, b: Vec2) -> f32 {
        let d = p.abs() - b;
        outside(d) + d.x.max(d.y).min(0.0)
    }
    fn outside(d: Vec2) -> f32 { d.max(Vec2::ZERO).length() }
    pub fn never_called() -> f32 { 1.0 }
    pub struct Shape { pub center: Vec2, pub radius: f32 }
    pub const EDGE: f32 = 0.01;
"#;

const ROOT: &str = r#"
    use crate::common::{sd_box, sd_circle as circle, Shape, EDGE};

    #[fragment] #[location(0)]
    fn fs(#[builtin(position)] p: Vec4) -> Vec4 {
        let s = Shape { center: Vec2::splat(0.5), radius: 0.25 };
        let d = circle(p.xy - s.center, s.radius).min(sd_box(p.xy, Vec2::splat(0.1)));
        Vec4::splat(1.0 - d.smoothstep(0.0, EDGE))
    }
"#;

#[test]
fn imported_functions_types_and_constants_compile() {
    let a = compile_program(&[module(ROOT, &[("crate::common", 1)]), module(COMMON, &[])])
        .unwrap_or_else(|d| {
            panic!(
                "{}",
                d.iter()
                    .map(|d| d.to_string())
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        });
    assert!(!a.library);
    assert!(
        a.wgsl.contains("fn sd_circle") && a.wgsl.contains("fn outside"),
        "{}",
        a.wgsl
    );
    assert!(
        !a.wgsl.contains("never_called"),
        "only what the entry points reach is emitted: {}",
        a.wgsl
    );
    assert!(a.warnings.is_empty(), "{:?}", a.warnings);
}

#[test]
fn a_glob_imports_every_public_item() {
    let root = ROOT
        .replace("{sd_box, sd_circle as circle, Shape, EDGE}", "*")
        .replace("circle(", "sd_circle(");
    let r = compile_program(&[module(&root, &[("crate::common", 1)]), module(COMMON, &[])]);
    assert!(r.is_ok(), "{:?}", r.err());
}

#[test]
fn a_private_item_cannot_be_imported() {
    let root = "use crate::common::outside;\n#[fragment] #[location(0)] fn fs() -> Vec4 { Vec4::splat(outside(Vec2::ZERO)) }";
    let d =
        compile_program(&[module(root, &[("crate::common", 1)]), module(COMMON, &[])]).unwrap_err();
    assert!(
        d.iter()
            .any(|d| d.message.contains("has no public item `outside`")),
        "{d:?}"
    );
}

#[test]
fn a_module_the_macro_did_not_find_is_reported() {
    let d = compile_program(&[module("use crate::missing::f;", &[])]).unwrap_err();
    assert!(
        d.iter()
            .any(|d| d.message.contains("cannot find module `crate::missing`")),
        "{d:?}"
    );
}

#[test]
fn a_pub_use_re_exports_through_a_middle_module() {
    let mid = "pub use crate::common::sd_circle;";
    let root = "use crate::mid::sd_circle;\n#[fragment] #[location(0)] fn fs() -> Vec4 { Vec4::splat(sd_circle(Vec2::ZERO, 1.0)) }";
    let r = compile_program(&[
        module(root, &[("crate::mid", 1)]),
        module(mid, &[("crate::common", 2)]),
        module(COMMON, &[]),
    ]);
    assert!(r.is_ok(), "{:?}", r.err());
}

#[test]
fn a_module_without_entry_points_is_a_library() {
    let a = compile_program(&[module(COMMON, &[])]).expect("a library checks");
    assert!(a.library && a.wgsl.is_empty());
    assert!(a.warnings.iter().all(|w| w.severity == Severity::Warning));
}

#[test]
fn an_imported_modules_warnings_are_not_repeated() {
    let noisy = "pub fn Loud(x: f32) -> f32 { let unused = 1.0; x }";
    let root = "use crate::noisy::Loud;\n#[fragment] #[location(0)] fn fs() -> Vec4 { Vec4::splat(Loud(1.0)) }";
    let a = compile_program(&[module(root, &[("crate::noisy", 1)]), module(noisy, &[])])
        .expect("compiles");
    assert!(a.warnings.is_empty(), "{:?}", a.warnings);
}
