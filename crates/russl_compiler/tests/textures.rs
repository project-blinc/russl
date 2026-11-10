//! Textures, derivatives and `discard!()`, and the rule that only fragment shaders run the
//! last two and implicit-level sampling.

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

/// The first error's message contains `text`, at `line` and `column` (both from 1).
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

const GLYPHS: &str = r#"
struct VsOut {
    #[builtin(position)] pos: Vec4,
    #[location(0)] uv: Vec2,
}

#[group(0)] #[binding(0)] static ATLAS: Texture2D<f32>;
#[group(0)] #[binding(1)] static SAMPLER: Sampler;
#[group(0)] #[binding(2)] static LAYERS: Texture2DArray<f32>;
#[group(0)] #[binding(3)] static IDS: Texture2D<u32>;

#[vertex]
fn vs(#[builtin(vertex_index)] vi: u32) -> VsOut {
    let size = ATLAS.dimensions().as_vec2();
    let x = (vi & 1) as f32;
    let y = (vi >> 1) as f32;
    let id = IDS.load(IVec2::new(0, 0), 0).x;
    let layer = LAYERS.sample_level(SAMPLER, Vec2::new(x, y), id, 0.0).x;
    VsOut { pos: Vec4::new(x, y, layer, 1.0), uv: Vec2::new(x, y) / size }
}

#[fragment]
#[location(0)]
fn fs(input: VsOut) -> Vec4 {
    let coverage = ATLAS.sample_level(SAMPLER, input.uv, 0.0).x;
    let aa = coverage.fwidth();
    let soft = ATLAS.sample(SAMPLER, input.uv).w;
    if coverage + soft < aa {
        discard!();
    }
    if coverage > 2.0 {
        discard!()
    } else {
        Vec4::new(1.0, 1.0, 1.0, coverage)
    }
}
"#;

#[test]
fn textures_derivatives_and_discard_compile_for_every_target() {
    let a = ok(GLYPHS);
    for needle in [
        "textureSampleLevel(",
        "textureSample(",
        "textureDimensions(",
        "textureLoad(",
        "fwidth(",
        "discard",
    ] {
        assert!(a.wgsl.contains(needle), "{needle} in:\n{}", a.wgsl);
    }
    assert!(a.msl.contains("discard_fragment"), "{}", a.msl);
    assert!(a.hlsl.contains("discard"), "{}", a.hlsl);
    assert_eq!(a.glsl.len(), 2);
    assert!(a.warnings.is_empty(), "{}", show(&a.warnings));
}

#[test]
fn discard_in_a_vertex_shader_is_an_error() {
    error_at(
        r#"
#[vertex]
#[builtin(position)]
fn vs() -> Vec4 {
    discard!();
    Vec4::ZERO
}
"#,
        "runs only in a fragment shader, and `vs` is a vertex entry point",
        5,
        5,
    );
}

#[test]
fn a_derivative_a_vertex_shader_reaches_is_an_error() {
    error_at(
        r#"
fn edge(x: f32) -> f32 {
    x.fwidth()
}

#[vertex]
#[builtin(position)]
fn vs() -> Vec4 {
    Vec4::splat(edge(1.0))
}
"#,
        "a derivative runs only in a fragment shader",
        3,
        5,
    );
}

#[test]
fn sampling_takes_float_coordinates_of_the_texture_dimension() {
    error_at(
        r#"
#[group(0)] #[binding(0)] static ATLAS: Texture2D<f32>;
#[group(0)] #[binding(1)] static SAMPLER: Sampler;

#[fragment]
#[location(0)]
fn fs() -> Vec4 {
    ATLAS.sample_level(SAMPLER, Vec3::ZERO, 0.0)
}
"#,
        "expected `Vec2`, found `Vec3`",
        8,
        33,
    );
}

#[test]
fn an_integer_texture_is_loaded_not_sampled() {
    error_at(
        r#"
#[group(0)] #[binding(0)] static IDS: Texture2D<u32>;
#[group(0)] #[binding(1)] static SAMPLER: Sampler;

#[fragment]
#[location(0)]
fn fs() -> Vec4 {
    let id = IDS.sample_level(SAMPLER, Vec2::ZERO, 0.0);
    id.as_vec4()
}
"#,
        "read this one with `load`",
        8,
        14,
    );
}

const SHADOW: &str = r#"
#[group(0)] #[binding(0)] static MAP: TextureDepth2D;
#[group(0)] #[binding(1)] static COMPARE: SamplerComparison;

#[fragment]
#[location(0)]
fn fs(#[location(0)] uv: Vec2, #[location(1)] depth: f32) -> Vec4 {
    let lit = MAP.sample_compare(COMPARE, uv, depth);
    let soft = MAP.sample_compare_level(COMPARE, uv + Vec2::splat(0.001), depth - 0.002);
    Vec4::splat((lit + soft) * 0.5)
}
"#;

#[test]
fn a_depth_texture_is_compared_with_a_comparison_sampler() {
    let a = ok(SHADOW);
    assert!(a.wgsl.contains("textureSampleCompare("), "{}", a.wgsl);
    assert!(a.wgsl.contains("textureSampleCompareLevel("), "{}", a.wgsl);
    assert!(a.wgsl.contains("sampler_comparison"), "{}", a.wgsl);
    assert!(a.wgsl.contains("texture_depth_2d"), "{}", a.wgsl);
}

#[test]
fn comparing_needs_a_depth_texture_and_a_comparison_sampler() {
    error_at(
        r#"
#[group(0)] #[binding(0)] static ATLAS: Texture2D<f32>;
#[group(0)] #[binding(1)] static COMPARE: SamplerComparison;

#[fragment]
#[location(0)]
fn fs() -> Vec4 {
    Vec4::splat(ATLAS.sample_compare(COMPARE, Vec2::ZERO, 0.5))
}
"#,
        "only a depth texture is compared against a reference",
        8,
        17,
    );
    error_at(
        r#"
#[group(0)] #[binding(0)] static MAP: TextureDepth2D;
#[group(0)] #[binding(1)] static SAMPLER: Sampler;

#[fragment]
#[location(0)]
fn fs() -> Vec4 {
    Vec4::splat(MAP.sample_compare(SAMPLER, Vec2::ZERO, 0.5))
}
"#,
        "expected",
        8,
        36,
    );
}

#[test]
fn comparing_at_an_implicit_level_is_for_fragment_shaders() {
    error_at(
        r#"
#[group(0)] #[binding(0)] static MAP: TextureDepth2D;
#[group(0)] #[binding(1)] static COMPARE: SamplerComparison;

#[vertex]
#[builtin(position)]
fn vs() -> Vec4 {
    Vec4::splat(MAP.sample_compare(COMPARE, Vec2::ZERO, 0.5))
}
"#,
        "runs only in a fragment shader",
        8,
        17,
    );
}
