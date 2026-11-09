//! Whole-pipeline tests: russl source in, every target out, or diagnostics at the right token.

use russl_compiler::{Diagnostic, Severity, compile};

fn ok(src: &str) -> russl_compiler::Artifacts {
    match compile(src.parse().expect("tokens")) {
        Ok(a) => {
            naga::front::wgsl::parse_str(&a.wgsl)
                .unwrap_or_else(|e| panic!("WGSL does not parse back: {e}\n{}", a.wgsl));
            assert!(a.msl.contains("metal_stdlib"));
            assert_eq!(a.spirv.first(), Some(&0x0723_0203));
            a
        }
        Err(d) => panic!("expected success, got:\n{}", show(&d)),
    }
}

fn errors(src: &str) -> Vec<Diagnostic> {
    match compile(src.parse().expect("tokens")) {
        Ok(_) => panic!("expected errors"),
        Err(d) => d
            .into_iter()
            .filter(|d| d.severity == Severity::Error)
            .collect(),
    }
}

fn show(d: &[Diagnostic]) -> String {
    d.iter()
        .map(|d| d.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The first error's message contains `text`, at `line` (from 1) and `column` (from 1).
fn error_at(src: &str, text: &str, line: usize, column: usize) {
    let d = errors(src);
    let first = d.first().unwrap_or_else(|| panic!("no error"));
    assert!(first.message.contains(text), "message: {}", show(&d));
    let at = first.span.start();
    assert_eq!((at.line, at.column + 1), (line, column), "{}", show(&d));
}

const TRIANGLE: &str = r#"
struct Globals { tint: Vec4, scale: f32 }

struct VsOut {
    #[builtin(position)] pos: Vec4,
    #[location(0)] uv: Vec2,
}

#[group(0)] #[binding(0)] static GLOBALS: Uniform<Globals>;
#[pipeline] const FEAT_FADE: bool = true;
const CORNERS: [Vec2; 3] = [Vec2::new(-1.0, -1.0), Vec2::new(3.0, -1.0), Vec2::new(-1.0, 3.0)];

fn rounded(p: Vec2, size: Vec2, r: f32) -> f32 {
    let q = p.abs() - size + r;
    q.max(Vec2::ZERO).length() + q.x.max(q.y).min(0.0) - r
}

#[vertex]
fn vs(#[builtin(vertex_index)] vi: u32) -> VsOut {
    let corner = CORNERS[vi];
    VsOut { pos: Vec4::new(corner * GLOBALS.scale, 0.0, 1), uv: corner * 0.5 + 0.5 }
}

#[fragment]
#[location(0)]
fn fs(input: VsOut) -> Vec4 {
    let d = rounded(input.uv - 0.5, Vec2::splat(0.4), 0.1);
    let alpha = if FEAT_FADE { 1.0 - d.clamp(0.0, 1.0) } else { 1.0 };
    let mut color = GLOBALS.tint;
    color.w *= alpha;
    color
}
"#;

#[test]
fn a_triangle_compiles_for_every_target() {
    let a = ok(TRIANGLE);
    assert!(a.wgsl.contains("fn rounded"), "{}", a.wgsl);
    assert!(
        !a.wgsl.contains("override"),
        "pipeline constants are fixed at their defaults: {}",
        a.wgsl
    );
    assert_eq!(
        a.glsl.len(),
        2,
        "one GLSL source per vertex or fragment entry point"
    );
    assert!(a.hlsl.contains("vs"), "{}", a.hlsl);
    assert!(a.warnings.is_empty(), "{}", show(&a.warnings));
}

const COMPUTE: &str = r#"
#[group(0)] #[binding(0)] static DATA: StorageMut<[u32]>;
#[group(0)] #[binding(1)] static PARAMS: Uniform<UVec4>;
static TILE: Workgroup<[u32; 64]>;

fn collatz(n: u32) -> u32 {
    let mut x = n;
    let mut steps = 0u32;
    while x != 1 {
        if x % 2 == 0 { x /= 2; } else { x = 3 * x + 1; }
        steps += 1;
    }
    steps
}

#[compute(workgroup_size(64))]
fn main(#[builtin(global_invocation_id)] id: UVec3, #[builtin(local_invocation_index)] li: u32) {
    let i = id.x;
    TILE[li] = collatz(i + 1);
    let mut total = 0u32;
    for k in 0..PARAMS.x {
        match k % 4 {
            0 => { total += 1; }
            1 | 2 => { total += 2; }
            _ => { break; }
        }
    }
    DATA[i] = TILE[li] + total;
}
"#;

#[test]
fn a_compute_shader_with_loops_compiles() {
    let a = ok(COMPUTE);
    assert!(a.glsl.is_empty(), "WebGL2 has no compute stage");
    assert!(a.wgsl.contains("@workgroup_size(64"), "{}", a.wgsl);
}

#[test]
fn unsuffixed_literals_take_their_context() {
    ok(r#"
        #[fragment] #[location(0)]
        fn fs() -> Vec4 {
            let half = 0.5;
            let v = Vec3::new(1, 2, 3) * half;
            let n: u32 = 7;
            Vec4::new(v, (n as f32).max(0.0))
        }
    "#);
}

#[test]
fn a_type_mismatch_points_at_the_value() {
    error_at(
        "fn f() -> f32 {\n    let x: f32 = true;\n    x\n}",
        "expected `f32`, found `bool`",
        2,
        18,
    );
}

#[test]
fn assigning_an_immutable_binding_is_an_error() {
    error_at(
        "fn f() -> f32 {\n    let x = 1.0;\n    x = 2.0;\n    x\n}",
        "cannot assign twice to immutable variable `x`",
        3,
        5,
    );
}

#[test]
fn an_unknown_function_is_reported_at_its_name() {
    error_at(
        "fn f() -> f32 {\n    shimmer(1.0)\n}",
        "cannot find function `shimmer`",
        2,
        5,
    );
}

#[test]
fn a_vertex_shader_must_output_a_position() {
    let d = errors("#[vertex]\n#[location(0)]\nfn vs() -> Vec4 { Vec4::ZERO }");
    assert!(
        d.iter()
            .any(|d| d.message.contains("returns `#[builtin(position)]`")),
        "{}",
        show(&d)
    );
}

#[test]
fn a_binding_used_twice_is_an_error() {
    let d = errors(
        "#[group(0)] #[binding(0)] static A: Uniform<Vec4>;\n#[group(0)] #[binding(0)] static B: Uniform<Vec4>;",
    );
    assert!(
        d.iter().any(|d| d.message.contains("is already `A`")),
        "{}",
        show(&d)
    );
}

#[test]
fn recursion_is_an_error() {
    let d = errors("fn f(n: u32) -> u32 { if n == 0 { return 0; } f(n - 1) }");
    assert!(
        d.iter().any(|d| d.message.contains("calls itself")),
        "{}",
        show(&d)
    );
}

#[test]
fn bool_cannot_be_shared_with_the_host() {
    let d = errors("#[group(0)] #[binding(0)] static FLAGS: Uniform<bool>;");
    assert!(
        d.iter()
            .any(|d| d.message.contains("cannot be shared with the host")),
        "{}",
        show(&d)
    );
}

#[test]
fn conventions_and_unused_bindings_are_warnings() {
    let a = ok(r#"
        fn Helper(x: f32) -> f32 { x }
        #[fragment] #[location(0)]
        fn fs() -> Vec4 {
            let unused = 1.0;
            let v = Helper(0.5);
            return Vec4::splat(v);
            let after = 2.0;
        }
    "#);
    let w = show(&a.warnings);
    assert!(
        w.contains("function `Helper` should have a snake case name"),
        "{w}"
    );
    assert!(w.contains("unused variable: `unused`"), "{w}");
    assert!(w.contains("unreachable statement"), "{w}");
}

#[test]
fn a_tail_if_or_match_returns_each_branch() {
    let a = ok(r#"
fn pick(kind: u32, x: f32) -> f32 {
    match kind {
        0 => x,
        1 => { x * 2.0 }
        _ => if x > 0.0 { x } else { -x },
    }
}

#[fragment]
#[location(0)]
fn fs() -> Vec4 {
    Vec4::splat(pick(1, 0.5))
}
"#);
    assert!(a.wgsl.contains("switch"), "{}", a.wgsl);
}

#[test]
fn a_shader_webgl2_cannot_run_has_no_glsl() {
    let a = ok(r#"
#[group(0)] #[binding(0)] static COLORS: Storage<[Vec4]>;

#[fragment]
#[location(0)]
fn fs() -> Vec4 {
    COLORS[0]
}
"#);
    assert!(a.glsl.is_empty());
    assert!(a.glsl_missing.is_some_and(|m| m.contains("BUFFER_STORAGE")));
}
