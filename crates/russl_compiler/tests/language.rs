//! Language features: swizzle assignment, array lengths from constants, uniform layout, f16
//! literals.

use russl_compiler::{Diagnostic, Severity, compile};

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

fn show(d: &[Diagnostic]) -> String {
    d.iter()
        .map(|d| d.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The first error contains `text`, at `line` and `column` (both from 1).
fn error_at(src: &str, text: &str, line: usize, column: usize) {
    let d: Vec<_> = match compile(src.parse().expect("tokens")) {
        Ok(_) => panic!("expected errors"),
        Err(d) => d
            .into_iter()
            .filter(|d| d.severity == Severity::Error)
            .collect(),
    };
    let first = d.first().unwrap_or_else(|| panic!("no error"));
    assert!(first.message.contains(text), "message: {}", show(&d));
    let at = first.span.start();
    assert_eq!((at.line, at.column + 1), (line, column), "{}", show(&d));
}

#[test]
fn a_swizzle_is_assigned_component_by_component() {
    let a = ok(r#"
#[fragment]
#[location(0)]
fn fs() -> Vec4 {
    let mut c = Vec4::ZERO;
    c.xz = Vec2::new(1.0, 0.5);
    c.wy += Vec2::splat(0.25);
    c
}
"#);
    for store in ["c.x = ", "c.z = ", "c.w = ", "c.y = "] {
        assert!(a.wgsl.contains(store), "{store} in:\n{}", a.wgsl);
    }
}

#[test]
fn a_swizzle_naming_a_component_twice_cannot_be_assigned() {
    error_at(
        r#"
#[fragment]
#[location(0)]
fn fs() -> Vec4 {
    let mut c = Vec4::ZERO;
    c.xx = Vec2::ZERO;
    c
}
"#,
        "names `x` twice",
        6,
        5,
    );
}

#[test]
fn an_array_length_is_built_from_constants() {
    let a = ok(r#"
const TAPS: u32 = 4;
const SIDE: u32 = TAPS * 2 + 1;
const WEIGHTS: [f32; SIDE as u32] = [0.1, 0.1, 0.1, 0.1, 0.2, 0.1, 0.1, 0.1, 0.1];
struct Kernel { offsets: [Vec4; TAPS] }
#[group(0)] #[binding(0)] static KERNEL: Uniform<Kernel>;

#[fragment]
#[location(0)]
fn fs() -> Vec4 {
    let mut sum = Vec4::ZERO;
    for i in 0..TAPS {
        sum += KERNEL.offsets[i] * WEIGHTS[i];
    }
    sum
}
"#);
    assert!(a.wgsl.contains("array<f32, 9>"), "{}", a.wgsl);
    assert!(a.wgsl.contains("array<vec4<f32>, 4>"), "{}", a.wgsl);
}

#[test]
fn a_pipeline_constant_cannot_size_an_array() {
    error_at(
        r#"
#[pipeline] const TAPS: u32 = 4;
const WEIGHTS: [f32; TAPS] = [0.1, 0.1, 0.1, 0.1];
"#,
        "`TAPS` is a pipeline constant",
        3,
        22,
    );
}

fn uniform_of(structs: &str) -> String {
    format!(
        "{structs}\n#[group(0)] #[binding(0)] static P: Uniform<Params>;\n\
#[fragment]\n#[location(0)]\nfn fs() -> Vec4 {{ let _p = P; Vec4::ZERO }}\n"
    )
}

#[test]
fn uniform_arrays_need_a_16_byte_stride() {
    error_at(
        &uniform_of("struct Params {\n    weights: [f32; 4],\n}"),
        "places array elements 16 bytes apart, and the `f32` elements of `[f32; 4]` are 4",
        2,
        5,
    );
}

#[test]
fn a_uniform_struct_member_starts_on_16_bytes() {
    error_at(
        &uniform_of("struct Inner { a: f32 }\nstruct Params {\n    x: f32,\n    inner: Inner,\n}"),
        "struct member `inner` starts on a 16-byte boundary, and here it starts at byte 4",
        4,
        5,
    );
}

#[test]
fn the_member_after_a_uniform_struct_leaves_room() {
    error_at(
        &uniform_of("struct Inner { a: f32 }\nstruct Params {\n    inner: Inner,\n    y: f32,\n}"),
        "starts at least 16 bytes after it",
        4,
        5,
    );
}

#[test]
fn layouts_that_follow_the_rules_compile() {
    ok(&uniform_of(
        "struct Inner { a: f32 }\nstruct Params {\n    x: Vec4,\n    inner: Inner,\n    _pad: Vec3,\n    y: f32,\n    w: [Vec4; 2],\n}",
    ));
    // Storage buffers have no such rules.
    ok(
        "struct Params { weights: [f32; 4] }\n#[group(0)] #[binding(0)] static P: Storage<Params>;\n\
#[fragment]\n#[location(0)]\nfn fs() -> Vec4 { Vec4::splat(P.weights[0]) }\n",
    );
}

#[test]
fn f16_literals_are_half_precision() {
    let a = ok(r#"
const HALF: f16 = 0.5;

#[fragment]
#[location(0)]
fn fs() -> Vec4 {
    let k = 2.0f16 * HALF;
    let v = HVec2::new(HALF, k + 1);
    Vec4::splat(v.y as f32)
}
"#);
    assert!(a.wgsl.contains("enable f16;"), "{}", a.wgsl);
    assert!(a.wgsl.contains("2h") && a.wgsl.contains("1h"), "{}", a.wgsl);
    assert!(a.msl.contains("half"), "{}", a.msl);
    assert!(a.glsl.is_empty() && a.glsl_missing.is_some_and(|m| m.contains("f16")));
}

#[test]
fn an_array_of_unsuffixed_literals_takes_its_context() {
    ok(r#"
const IDS: [u32; 3] = [1, 2, 3];
fn sum(w: [u32; 3]) -> u32 { w[0] + w[1] + w[2] }

#[fragment]
#[location(0)]
fn fs() -> Vec4 {
    Vec4::splat((sum([4, 5, 6]) + IDS[0]) as f32)
}
"#);
}
