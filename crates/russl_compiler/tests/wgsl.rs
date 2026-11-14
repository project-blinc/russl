//! A module given as WGSL: written for every target, with each target's native bindings
//! following the models `native` documents, as for russl source.

use russl_compiler::{HlslSlot, MslSlot, compile, compile_wgsl};

/// A uniform and a storage buffer, a texture with a filtering sampler, a depth texture with a
/// comparison sampler, and two pipeline constants, one with an id.
const SHADER: &str = include_str!("corpus/bindings.wgsl");

/// An entry point's Metal slots by (group, binding), and its sizes buffer.
type Slots = (Vec<((u32, u32), MslSlot)>, Option<u8>);

fn slots(a: &russl_compiler::Artifacts, entry: &str) -> Slots {
    let e = a
        .reflection
        .native
        .msl
        .iter()
        .find(|e| e.entry_point == entry)
        .unwrap();
    (
        e.resources
            .iter()
            .map(|r| ((r.group, r.binding), r.slot))
            .collect(),
        e.sizes_buffer,
    )
}

#[test]
fn every_target_is_written() {
    let a = compile_wgsl(SHADER, &[]).unwrap_or_else(|e| panic!("{e}"));
    assert!(a.wgsl.contains("fn fs"));
    assert!(a.msl.contains("[[buffer(0)]]") && a.msl.contains("[[sampler(1)]]"));
    assert!(!a.hlsl.is_empty() && !a.spirv.is_empty());
    assert!(!a.library);
}

#[test]
fn metal_counts_each_kind_from_zero_in_every_stage_with_the_sizes_buffer_last() {
    let a = compile_wgsl(SHADER, &[]).unwrap();
    let expected = vec![
        ((0, 0), MslSlot::Buffer(0)),
        ((0, 1), MslSlot::Buffer(1)),
        ((1, 0), MslSlot::Texture(0)),
        ((1, 1), MslSlot::Sampler(0)),
        ((2, 0), MslSlot::Texture(1)),
        ((2, 1), MslSlot::Sampler(1)),
    ];
    // Every bound global is numbered for both stages, used there or not.
    for entry in ["vs", "fs"] {
        let (resources, sizes) = slots(&a, entry);
        assert_eq!(resources, expected, "{entry}");
        assert_eq!(
            sizes,
            Some(2),
            "{entry}: a storage buffer is bound, so the sizes buffer follows the buffers"
        );
    }
}

#[test]
fn a_vertex_stage_has_a_sizes_buffer_even_with_no_storage_buffer() {
    let a = compile_wgsl(
        r#"
@group(0) @binding(0) var<uniform> tint: vec4<f32>;
@vertex fn vs() -> @builtin(position) vec4<f32> { return tint; }
@fragment fn fs() -> @location(0) vec4<f32> { return tint; }
"#,
        &[],
    )
    .unwrap();
    assert_eq!(slots(&a, "vs").1, Some(1));
    assert_eq!(slots(&a, "fs").1, None);
}

#[test]
fn hlsl_takes_binding_as_register_and_group_as_space_and_samplers_by_index() {
    let a = compile_wgsl(SHADER, &[]).unwrap();
    let hlsl: Vec<_> = a
        .reflection
        .native
        .hlsl
        .iter()
        .map(|b| ((b.group, b.binding), b.slot))
        .collect();
    let register = |class, index, space| HlslSlot::Register {
        class,
        index,
        space,
    };
    assert_eq!(
        hlsl,
        [
            ((0, 0), register('b', 0, 0)),
            ((0, 1), register('t', 1, 0)),
            ((1, 0), register('t', 0, 1)),
            ((1, 1), HlslSlot::SamplerIndex { index: 1 }),
            ((2, 0), register('t', 0, 2)),
            ((2, 1), HlslSlot::SamplerIndex { index: 1 }),
        ]
    );
}

#[test]
fn glsl_names_each_combined_sampler_and_uniform_block_by_its_bindings() {
    let a = compile_wgsl(include_str!("corpus/sampled.wgsl"), &[]).unwrap();
    assert_eq!(a.glsl_missing, None);
    let fs = a
        .reflection
        .native
        .glsl
        .iter()
        .find(|g| g.entry_point == "fs")
        .unwrap();
    assert_eq!(fs.textures.len(), 1);
    assert_eq!(fs.textures[0].texture, (1, 0));
    assert_eq!(fs.textures[0].sampler, Some((1, 1)));
    let source = &a
        .glsl
        .iter()
        .find(|g| g.entry_point == "fs")
        .unwrap()
        .source;
    assert!(
        source.contains(&fs.textures[0].name),
        "the GLSL declares `{}`",
        fs.textures[0].name
    );
    assert_eq!(fs.uniform_blocks.len(), 1);
    assert_eq!(fs.uniform_blocks[0].1, (0, 0));
    assert!(source.contains(&fs.uniform_blocks[0].0));
}

#[test]
fn a_storage_buffer_leaves_no_glsl_and_says_why() {
    let a = compile_wgsl(SHADER, &[]).unwrap();
    assert!(a.glsl.is_empty());
    assert!(a.glsl_missing.is_some());
}

#[test]
fn pipeline_constants_are_set_by_id_or_by_name() {
    use russl_compiler::ConstValue;
    let a = compile_wgsl(SHADER, &[("7", 2.0), ("soft", 0.0)]).unwrap();
    assert_eq!(
        a.reflection.constants,
        [
            ("gain".to_string(), ConstValue::Float(2.0)),
            ("soft".to_string(), ConstValue::Bool(false))
        ]
    );
    let defaults = compile_wgsl(SHADER, &[]).unwrap();
    assert_eq!(
        defaults.reflection.constants,
        [
            ("gain".to_string(), ConstValue::Float(1.0)),
            ("soft".to_string(), ConstValue::Bool(true))
        ]
    );
}

#[test]
fn a_module_written_in_russl_binds_the_same_from_its_wgsl() {
    let russl = compile(
        r#"
#[group(0)] #[binding(0)] static GLOBALS: Uniform<Vec4>;
#[group(0)] #[binding(1)] static ITEMS: Storage<[Vec4]>;
#[group(1)] #[binding(0)] static ATLAS: Texture2D<f32>;
#[group(1)] #[binding(1)] static SAMPLER: Sampler;

#[vertex]
#[builtin(position)]
fn vs(#[builtin(vertex_index)] vi: u32) -> Vec4 { ITEMS[vi] * GLOBALS.x }

#[fragment]
#[location(0)]
fn fs(#[builtin(position)] p: Vec4) -> Vec4 { GLOBALS * ATLAS.sample(SAMPLER, p.xy) }
"#
        .parse()
        .unwrap(),
    )
    .unwrap();
    let wgsl = compile_wgsl(&russl.wgsl, &[]).unwrap();
    assert_eq!(
        format!("{:?}", wgsl.reflection.native),
        format!("{:?}", russl.reflection.native)
    );
}

#[test]
fn invalid_wgsl_is_reported_against_its_source() {
    let e = compile_wgsl(
        "@fragment fn fs() -> @location(0) vec4<f32> { return 1; }",
        &[],
    )
    .unwrap_err();
    assert!(e.to_string().contains("fs"), "{e}");
    let e = compile_wgsl("fn broken( {", &[]).unwrap_err();
    assert!(
        e.to_string().contains("broken("),
        "the message quotes the source: {e}"
    );
}

#[test]
fn a_module_with_no_entry_points_is_refused() {
    let e = compile_wgsl("fn helper() -> f32 { return 1.0; }", &[]).unwrap_err();
    assert!(e.to_string().contains("no entry points"));
}
