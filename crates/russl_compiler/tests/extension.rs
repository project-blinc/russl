//! A framework's extension: a prelude every module sees, extern functions it defines, builtins
//! it lowers, and rules over the checked program.

use russl_compiler::{
    BindingKind, Builtin, Diagnostic, Emit, Extension, Module, ProgramView, Severity,
    compile_program_ext,
};

struct Toy;

impl Extension for Toy {
    fn name(&self) -> &str {
        "toy"
    }

    fn prelude(&self) -> Option<proc_macro2::TokenStream> {
        Some(
            "pub struct Frame { pub time: f32, pub size: Vec2 }
             #[group(0)] #[binding(0)] pub static FRAME: Uniform<Frame>;
             pub fn aspect() -> f32 { FRAME.size.x / FRAME.size.y }"
                .parse()
                .unwrap(),
        )
    }

    fn extern_fn(&self, name: &str) -> Option<proc_macro2::TokenStream> {
        (name == "wave").then(|| {
            "fn wave(t: f32) -> f32 { t.sin() * 0.5 + 0.5 }"
                .parse()
                .unwrap()
        })
    }

    fn builtins(&self) -> Vec<Builtin> {
        vec![Builtin {
            name: "twice".into(),
            params: vec!["f32".into()],
            ret: "f32".into(),
            lower: Box::new(
                |e: &mut Emit<'_>, args: &[naga::Handle<naga::Expression>]| {
                    e.append(naga::Expression::Binary {
                        op: naga::BinaryOperator::Add,
                        left: args[0],
                        right: args[0],
                    })
                },
            ),
        }]
    }

    fn check(&self, program: &ProgramView, report: &mut Vec<Diagnostic>) {
        for b in &program.bindings {
            if b.group != 0 {
                report.push(Diagnostic::error(
                    b.span,
                    format!(
                        "toy binds group 0 only; `{}` is in group {}",
                        b.name, b.group
                    ),
                ));
            }
            if b.name == "FRAME" && b.kind != BindingKind::Uniform {
                report.push(Diagnostic::error(b.span, "FRAME is a uniform"));
            }
        }
    }
}

fn compile(src: &str) -> Result<russl_compiler::Artifacts, Vec<Diagnostic>> {
    let m = Module {
        source: src.parse().expect("tokens"),
        imports: Vec::new(),
    };
    compile_program_ext(&[m], &[], &Toy)
}

fn first_error(src: &str) -> Diagnostic {
    compile(src)
        .expect_err("errors")
        .into_iter()
        .find(|d| d.severity == Severity::Error)
        .expect("an error")
}

const SKETCH: &str = r#"
extern { fn wave(t: f32) -> f32; }

#[fragment]
#[location(0)]
fn fs() -> Vec4 {
    Vec4::splat(twice(wave(FRAME.time)) * aspect())
}
"#;

#[test]
fn a_shader_sees_the_prelude_links_externs_and_calls_builtins() {
    let a = compile(SKETCH).unwrap_or_else(|d| panic!("{d:?}"));
    naga::front::wgsl::parse_str(&a.wgsl).expect("WGSL parses");
    assert!(
        a.wgsl.contains("fn wave(") && a.wgsl.contains("fn aspect("),
        "{}",
        a.wgsl
    );
    assert!(a.wgsl.contains("var<uniform> FRAME"), "{}", a.wgsl);
    assert!(a.warnings.is_empty(), "{:?}", a.warnings);
}

#[test]
fn a_module_item_shadows_the_prelude() {
    let a = compile(
        "fn aspect() -> f32 { 2.0 }\n#[fragment]\n#[location(0)]\nfn fs() -> Vec4 { Vec4::splat(aspect()) }",
    )
    .unwrap_or_else(|d| panic!("{d:?}"));
    assert!(
        !a.wgsl.contains("FRAME"),
        "the prelude's aspect is not reached:\n{}",
        a.wgsl
    );
}

#[test]
fn an_extern_the_extension_lacks_is_an_error_at_its_name() {
    let d = first_error(
        "extern { fn noise(p: Vec2) -> f32; }\n#[fragment]\n#[location(0)]\nfn fs() -> Vec4 { Vec4::ZERO }",
    );
    assert!(
        d.message
            .contains("`noise` is extern, and the extension `toy` does not define it"),
        "{}",
        d.message
    );
    assert_eq!((d.span.start().line, d.span.start().column), (1, 12));
}

#[test]
fn an_extern_must_match_the_definition() {
    let d = first_error(
        "extern { fn wave(t: Vec2) -> f32; }\n#[fragment]\n#[location(0)]\nfn fs() -> Vec4 { Vec4::ZERO }",
    );
    assert!(
        d.message
            .contains("defines `wave` as `fn wave (t : f32) -> f32`"),
        "{}",
        d.message
    );
}

#[test]
fn the_extension_rules_the_bindings() {
    let d = first_error(
        "#[group(1)] #[binding(0)] static EXTRA: Uniform<Vec4>;\n#[fragment]\n#[location(0)]\nfn fs() -> Vec4 { EXTRA }",
    );
    assert!(
        d.message
            .contains("toy binds group 0 only; `EXTRA` is in group 1"),
        "{}",
        d.message
    );
    assert_eq!(d.span.start().line, 1);
}

#[test]
fn without_an_extension_an_extern_has_no_definition() {
    let d = russl_compiler::compile(
        "extern { fn wave(t: f32) -> f32; }\n#[fragment]\n#[location(0)]\nfn fs() -> Vec4 { Vec4::ZERO }"
            .parse()
            .unwrap(),
    )
    .expect_err("errors")
    .remove(0);
    assert!(
        d.message.contains("no extension is in use"),
        "{}",
        d.message
    );
}
