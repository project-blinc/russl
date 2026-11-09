//! Generic functions over type and const parameters, monomorphised per call's arguments.

use russl_compiler::{Diagnostic, Module, Severity, compile, compile_program};

fn show(d: &[Diagnostic]) -> String {
    d.iter()
        .map(|d| d.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

fn ok(src: &str) -> russl_compiler::Artifacts {
    match compile(src.parse().expect("tokens")) {
        Ok(a) => {
            naga::front::wgsl::parse_str(&a.wgsl)
                .unwrap_or_else(|e| panic!("WGSL does not parse back: {e}\n{}", a.wgsl));
            a
        }
        Err(d) => panic!("expected success, got:\n{}", show(&d)),
    }
}

fn first_error(src: &str) -> Diagnostic {
    match compile(src.parse().expect("tokens")) {
        Ok(_) => panic!("expected errors"),
        Err(d) => d
            .into_iter()
            .find(|d| d.severity == Severity::Error)
            .expect("an error"),
    }
}

fn fragment(body: &str) -> String {
    format!("#[fragment]\n#[location(0)]\nfn fs() -> Vec4 {{\n{body}\n}}\n")
}

#[test]
fn each_argument_type_gets_its_own_instance() {
    let a = ok(&format!(
        "fn mix_by<T: Float>(a: T, b: T, t: f32) -> T {{ a + (b - a) * t }}\n{}",
        fragment(
            "    let s = mix_by(0.0, 1.0, 0.25);\n    let v = mix_by(Vec3::ZERO, Vec3::ONE, s);\n    Vec4::new(v, s)"
        )
    ));
    assert!(a.wgsl.contains("fn mix_by_f32"), "{}", a.wgsl);
    assert!(a.wgsl.contains("fn mix_by_vec3"), "{}", a.wgsl);
    assert!(a.warnings.is_empty(), "{}", show(&a.warnings));
}

#[test]
fn a_const_parameter_sizes_arrays_and_reads_as_a_value() {
    let a = ok(&format!(
        "fn total<const N: u32>(w: [f32; N]) -> f32 {{\n    let mut s = 0.0;\n    for i in 0..N {{ s += w[i]; }}\n    s / (N as f32)\n}}\n{}",
        fragment(
            "    let a = total([1.0, 2.0, 3.0]);\n    let b = total([1.0, 2.0, 3.0, 4.0, 5.0]);\n    Vec4::splat(a + b)"
        )
    ));
    assert!(
        a.wgsl.contains("fn total_3") && a.wgsl.contains("fn total_5"),
        "{}",
        a.wgsl
    );
}

#[test]
fn a_turbofish_names_what_the_arguments_cannot() {
    let a = ok(&format!(
        "const LANES: u32 = 4;\nfn filled<T: Vector>(x: f32) -> T {{ T::splat(x) }}\nfn zeros<const N: u32>() -> [f32; N] {{ [0.0; N] }}\n{}",
        fragment(
            "    let z = zeros::<LANES>();\n    let w = zeros::<2>();\n    filled::<Vec4>(z[3] + w[1] + 0.5)"
        )
    ));
    assert!(a.wgsl.contains("fn filled_vec4"), "{}", a.wgsl);
    assert!(
        a.wgsl.contains("fn zeros_4") && a.wgsl.contains("fn zeros_2"),
        "{}",
        a.wgsl
    );
}

#[test]
fn a_bound_is_checked_at_the_call() {
    let d = first_error(&format!(
        "fn half<T: Float>(x: T) -> T {{ x * 0.5 }}\n{}",
        fragment("    let h = half(3i32);\n    Vec4::splat(h as f32)")
    ));
    assert!(
        d.message
            .contains("`half` needs `T: Float`, and `T` is `i32` here"),
        "{}",
        d.message
    );
    assert_eq!(d.span.start().line, 5);
}

#[test]
fn an_argument_the_call_does_not_decide_must_be_named() {
    let d = first_error(&format!(
        "fn none<T: Vector>() -> T {{ T::ZERO }}\n{}",
        fragment("    none()")
    ));
    assert!(d.message.contains("cannot tell `T`"), "{}", d.message);
    assert!(d.message.contains("none::<f32>(..)"), "{}", d.message);
}

#[test]
fn an_error_in_a_generic_body_names_the_instance() {
    let d = first_error(&format!(
        "fn broken<T>(x: T) -> T {{\n    x + true\n}}\n{}",
        fragment("    Vec4::splat(broken(1.0))")
    ));
    assert!(d.message.contains("(in `broken::<f32>`)"), "{}", d.message);
    assert_eq!(d.span.start().line, 2, "{}", d.message);
}

#[test]
fn an_entry_point_cannot_be_generic() {
    let d = first_error("#[fragment]\n#[location(0)]\nfn fs<T>() -> Vec4 { Vec4::ZERO }\n");
    assert!(
        d.message.contains("an entry point cannot be generic"),
        "{}",
        d.message
    );
}

#[test]
fn a_generic_function_is_imported_like_any_other() {
    let lib = Module {
        source: "pub fn twice<T: Numeric>(x: T) -> T { x + x }"
            .parse()
            .unwrap(),
        imports: Vec::new(),
    };
    let root = Module {
        source: format!(
            "use crate::lib::twice;\n{}",
            fragment("    Vec4::splat(twice(0.25)) + twice(Vec4::ONE)")
        )
        .parse()
        .unwrap(),
        imports: vec![("crate::lib".into(), 1)],
    };
    let a = compile_program(&[root, lib]).unwrap_or_else(|d| panic!("{}", show(&d)));
    assert!(
        a.wgsl.contains("fn twice_f32") && a.wgsl.contains("fn twice_vec4"),
        "{}",
        a.wgsl
    );
}

#[test]
fn a_generic_never_called_is_an_unused_function() {
    let a = ok(&format!(
        "fn idle<T>(x: T) -> T {{ x }}\n{}",
        fragment("    Vec4::ZERO")
    ));
    assert!(
        a.warnings
            .iter()
            .any(|w| w.message.contains("function `idle` is never used")),
        "{}",
        show(&a.warnings)
    );
}

#[test]
fn generics_call_generics() {
    let a = ok(&format!(
        "fn sq<T: Numeric>(x: T) -> T {{ x * x }}\nfn quad<T: Numeric>(x: T) -> T {{ sq(sq(x)) }}\n{}",
        fragment("    Vec4::splat(quad(2.0)) + quad(Vec4::ONE)")
    ));
    assert!(
        a.wgsl.contains("fn sq_vec4") && a.wgsl.contains("fn quad_f32"),
        "{}",
        a.wgsl
    );
}

#[test]
fn a_generic_that_calls_itself_is_recursion() {
    let d = first_error(&format!(
        "fn again<T>(x: T) -> T {{ again(x) }}\n{}",
        fragment("    Vec4::splat(again(1.0))")
    ));
    assert!(d.message.contains("calls itself"), "{}", d.message);
}

const PAIRS: &str = r#"
struct Pair<T> { a: T, b: T }
struct Ring<const N: u32> { taps: [f32; N] }
#[group(0)] #[binding(0)] static RING: Storage<Ring<4>>;
#[group(0)] #[binding(1)] static OFFSETS: Storage<Pair<Vec4>>;

fn sum<T: Numeric>(p: Pair<T>) -> T { p.a + p.b }
"#;

#[test]
fn generic_structs_build_one_instance_per_argument_set() {
    let a = ok(&format!(
        "{PAIRS}\n{}",
        fragment(
            "    let p = Pair::<f32> { a: 1.0, b: RING.taps[3] };\n    let q = Pair::<Vec4> { a: OFFSETS.a, b: OFFSETS.b };\n    sum(q) * sum(p)"
        )
    ));
    assert!(
        a.wgsl.contains("struct PairF32") && a.wgsl.contains("struct PairVec4"),
        "{}",
        a.wgsl
    );
    assert!(
        a.wgsl.contains("struct Ring4") && a.wgsl.contains("array<f32, 4>"),
        "{}",
        a.wgsl
    );
    assert!(
        a.wgsl.contains("fn sum_f32") && a.wgsl.contains("fn sum_vec4"),
        "{}",
        a.wgsl
    );
}

#[test]
fn a_generic_struct_type_names_its_arguments() {
    let d = first_error(&format!(
        "struct Pair<T> {{ a: T, b: T }}\n#[group(0)] #[binding(0)] static P: Storage<Pair>;\n{}",
        fragment("    Vec4::ZERO")
    ));
    assert!(
        d.message
            .contains("`Pair` takes generic arguments: `Pair<f32>`"),
        "{}",
        d.message
    );
}

#[test]
fn a_generic_struct_literal_names_its_arguments() {
    let d = first_error(&format!(
        "struct Pair<T> {{ a: T, b: T }}\n{}",
        fragment("    let p = Pair { a: 1.0, b: 2.0 };\n    Vec4::splat(p.a)")
    ));
    assert!(
        d.message.contains("`Pair` is generic: name its arguments"),
        "{}",
        d.message
    );
}

#[test]
fn a_generic_struct_checks_its_bounds() {
    let d = first_error(&format!(
        "struct Unit<T: Float> {{ v: T }}\n#[group(0)] #[binding(0)] static U: Storage<Unit<u32>>;\n{}",
        fragment("    Vec4::ZERO")
    ));
    assert!(
        d.message
            .contains("`Unit` needs `T: Float`, and `T` is `u32` here"),
        "{}",
        d.message
    );
}
