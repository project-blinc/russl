//! Specialising a module's pipeline constants at build time.

use russl_compiler::{ConstValue, Constant, Module, compile_program_with};

const SHADER: &str = r#"
    #[pipeline] const FEAT_FADE: bool = true;
    #[pipeline] const SCALE: f32 = 1.0;
    const PLAIN: f32 = 2.0;

    #[fragment] #[location(0)]
    fn fs() -> Vec4 {
        let a = if FEAT_FADE { 0.25 } else { 0.75 };
        Vec4::splat(a * SCALE * PLAIN)
    }
"#;

fn compile(values: &[(&str, ConstValue)]) -> Result<String, Vec<String>> {
    let module = Module {
        source: SHADER.parse().unwrap(),
        imports: Vec::new(),
    };
    let constants: Vec<Constant> = values
        .iter()
        .map(|(n, v)| Constant {
            name: n.to_string(),
            value: *v,
            span: proc_macro2::Span::call_site(),
        })
        .collect();
    compile_program_with(&[module], &constants)
        .map(|a| a.wgsl)
        .map_err(|d| d.into_iter().map(|d| d.message).collect())
}

#[test]
fn a_value_replaces_the_default() {
    let base = compile(&[]).unwrap();
    let off = compile(&[
        ("FEAT_FADE", ConstValue::Bool(false)),
        ("SCALE", ConstValue::Int(3)),
    ])
    .unwrap();
    assert_ne!(base, off, "the specialised module differs from the default");
    assert!(!off.contains("override"));
}

#[test]
fn an_unknown_constant_is_an_error() {
    let e = compile(&[("FEAT_GLOW", ConstValue::Bool(false))]).unwrap_err();
    assert!(
        e.iter().any(|m| m.contains("no constant `FEAT_GLOW`")),
        "{e:?}"
    );
}

#[test]
fn only_pipeline_constants_can_be_specialised() {
    let e = compile(&[("PLAIN", ConstValue::Float(1.0))]).unwrap_err();
    assert!(
        e.iter().any(|m| m.contains("not a `#[pipeline]` constant")),
        "{e:?}"
    );
}

#[test]
fn a_value_must_fit_the_constant() {
    let e = compile(&[("FEAT_FADE", ConstValue::Float(0.5))]).unwrap_err();
    assert!(
        e.iter().any(|m| m.contains("`FEAT_FADE` is `bool`")),
        "{e:?}"
    );
}
