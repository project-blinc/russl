//! `#if`, `#elif`, `#else` and `#endif` over pipeline constants.

use russl_compiler::{ConstValue, Constant, Module, compile_program_with};

fn module(source: &str) -> Module {
    Module {
        source: source.parse().unwrap(),
        imports: Vec::new(),
    }
}

fn values(values: &[(&str, ConstValue)]) -> Vec<Constant> {
    values
        .iter()
        .map(|(n, v)| Constant {
            name: n.to_string(),
            value: *v,
            span: proc_macro2::Span::call_site(),
        })
        .collect()
}

fn wgsl(modules: &[Module], v: &[(&str, ConstValue)]) -> String {
    compile_program_with(modules, &values(v))
        .unwrap_or_else(|d| panic!("{:?}", d.iter().map(|d| &d.message).collect::<Vec<_>>()))
        .wgsl
}

fn first_error(modules: &[Module]) -> (String, (usize, usize)) {
    let d = compile_program_with(modules, &[]).expect_err("an error");
    let d = &d[0];
    (
        d.message.clone(),
        (d.span.start().line, d.span.start().column),
    )
}

/// A binding of a different kind at the same slot in each branch, as the tiers need.
const TIERS: &str = r#"
    #[pipeline] const TIER: u32 = 0;

    #if TIER == 2
        #[group(0)] #[binding(1)] static DATA: Texture2D<f32>;
        fn fetch(i: u32) -> Vec4 { DATA.load(IVec2::new(i as i32, 0), 0) }
    #else
        #[group(0)] #[binding(1)] static DATA: Storage<[Vec4]>;
        fn fetch(i: u32) -> Vec4 { DATA[i] }
    #endif

    #[fragment] #[location(0)]
    fn fs(#[builtin(position)] p: Vec4) -> Vec4 { fetch(p.x as u32) }
"#;

#[test]
fn a_branch_declares_a_binding_the_other_does_not() {
    let storage = wgsl(&[module(TIERS)], &[]);
    assert!(storage.contains("var<storage> DATA"), "{storage}");
    assert!(!storage.contains("texture_2d"), "{storage}");
    let textured = wgsl(&[module(TIERS)], &[("TIER", ConstValue::Int(2))]);
    assert!(textured.contains("var DATA: texture_2d<f32>"), "{textured}");
    assert!(!textured.contains("var<storage>"), "{textured}");
}

#[test]
fn elif_and_else_choose_the_first_branch_that_holds() {
    let source = r#"
        #[pipeline] const LEVEL: u32 = 0;
        #if LEVEL == 1
            const K: f32 = 1.0;
        #elif LEVEL >= 2 && LEVEL < 5
            const K: f32 = 2.0;
        #elif LEVEL == 7 || !(LEVEL != 9)
            const K: f32 = 3.0;
        #else
            const K: f32 = 4.0;
        #endif
        #[fragment] #[location(0)]
        fn fs() -> Vec4 { Vec4::splat(K) }
    "#;
    let k = |level| wgsl(&[module(source)], &[("LEVEL", ConstValue::Int(level))]);
    for (level, want) in [(1, "1f"), (3, "2f"), (7, "3f"), (9, "3f"), (0, "4f")] {
        let out = k(level);
        assert!(
            out.contains(&format!("const K: f32 = {want};")),
            "{level}: {out}"
        );
    }
}

#[test]
fn a_condition_can_choose_inside_a_parameter_list_and_a_body() {
    let source = r#"
        #[pipeline] const ATTRIBUTES: bool = false;

        struct Out { #[builtin(position)] pos: Vec4 }

        #[vertex]
        fn vs(
            #[builtin(vertex_index)] vi: u32,
            #if ATTRIBUTES
                #[location(0)] bounds: Vec4,
            #endif
        ) -> Out {
            #if ATTRIBUTES
                let b = bounds;
            #else
                let b = Vec4::splat(vi as f32);
            #endif
            Out { pos: b }
        }
    "#;
    let storage = wgsl(&[module(source)], &[]);
    assert!(!storage.contains("@location(0) bounds"), "{storage}");
    let attributes = wgsl(&[module(source)], &[("ATTRIBUTES", ConstValue::Bool(true))]);
    assert!(attributes.contains("@location(0) bounds"), "{attributes}");
}

#[test]
fn conditionals_nest() {
    let source = r#"
        #[pipeline] const A: bool = true;
        #[pipeline] const B: bool = false;
        #if A
            #if B
                const K: f32 = 1.0;
            #else
                const K: f32 = 2.0;
            #endif
        #else
            const K: f32 = 3.0;
        #endif
        #[fragment] #[location(0)]
        fn fs() -> Vec4 { Vec4::splat(K) }
    "#;
    assert!(wgsl(&[module(source)], &[]).contains("const K: f32 = 2f;"));
    let both = wgsl(&[module(source)], &[("B", ConstValue::Bool(true))]);
    assert!(both.contains("const K: f32 = 1f;"), "{both}");
    let neither = wgsl(&[module(source)], &[("A", ConstValue::Bool(false))]);
    assert!(neither.contains("const K: f32 = 3f;"), "{neither}");
}

#[test]
fn a_library_sees_the_value_the_shader_is_specialised_with() {
    let library = r#"
        #[pipeline] const TIER: u32 = 0;
        #if TIER == 2
            pub fn pick() -> f32 { 2.0 }
        #else
            pub fn pick() -> f32 { 0.0 }
        #endif
    "#;
    let shader = r#"
        use crate::lib::pick;
        #[pipeline] const TIER: u32 = 0;
        #[fragment] #[location(0)]
        fn fs() -> Vec4 { Vec4::splat(pick()) }
    "#;
    let mut root = module(shader);
    root.imports = vec![("crate::lib".to_string(), 1)];
    let program = [root, module(library)];
    let storage = wgsl(&program, &[]);
    assert!(!storage.contains("2f"), "{storage}");
    let textured = wgsl(&program, &[("TIER", ConstValue::Int(2))]);
    assert!(textured.contains("2f"), "{textured}");
}

#[test]
fn a_condition_that_names_no_constant_is_an_error_at_the_name() {
    let (message, at) = first_error(&[module(
        "#if MISSING\nconst K: f32 = 1.0;\n#endif\n#[fragment] #[location(0)] fn fs() -> Vec4 { Vec4::ZERO }",
    )]);
    assert!(
        message.contains("`MISSING` is not a pipeline constant"),
        "{message}"
    );
    assert_eq!(at, (1, 4));
}

#[test]
fn a_chain_must_close_and_stay_in_order() {
    let (message, _) = first_error(&[module(
        "#[pipeline] const A: bool = true;\n#if A\nconst K: f32 = 1.0;\n",
    )]);
    assert!(message.contains("no `#endif`"), "{message}");
    let (message, _) = first_error(&[module("#endif\n")]);
    assert!(message.contains("without a matching `#if`"), "{message}");
    let (message, _) = first_error(&[module(
        "#[pipeline] const A: bool = true;\n#if A\n#else\n#else\n#endif\n",
    )]);
    assert!(message.contains("after `#else`"), "{message}");
}

#[test]
fn a_condition_is_a_bool() {
    let (message, _) = first_error(&[module(
        "#[pipeline] const N: u32 = 1;\n#if N\nconst K: f32 = 1.0;\n#endif\n",
    )]);
    assert!(message.contains("a condition is a bool"), "{message}");
}

#[test]
fn two_defaults_for_one_constant_are_an_error() {
    let a = module(
        "#[pipeline] const TIER: u32 = 0;\n#[fragment] #[location(0)] fn fs() -> Vec4 { Vec4::ZERO }",
    );
    let b = module("#[pipeline] const TIER: u32 = 1;");
    let d = compile_program_with(&[a, b], &[]).expect_err("an error");
    assert!(
        d[0].message.contains("two different defaults"),
        "{}",
        d[0].message
    );
}

#[test]
fn source_without_directives_is_unchanged() {
    let source = r#"
        #[pipeline] const K: f32 = 1.0;
        #[fragment] #[location(0)]
        fn fs() -> Vec4 { Vec4::splat(K) }
    "#;
    assert!(wgsl(&[module(source)], &[]).contains("const K: f32 = 1f;"));
}
