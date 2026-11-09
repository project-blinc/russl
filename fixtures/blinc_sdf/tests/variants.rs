//! The SDF core shader and its simple specialisation, built by rustc through `russl!`.

use russl_fixture_blinc_sdf::{sdf_core, simple};

fn module(wgsl: &str) -> naga::Module {
    naga::front::wgsl::parse_str(wgsl).unwrap_or_else(|e| panic!("{}", e.emit_to_string(wgsl)))
}

#[test]
fn both_pipelines_have_their_constants_fixed() {
    for wgsl in [sdf_core::WGSL, simple::WGSL] {
        let m = module(wgsl);
        assert!(m.overrides.is_empty());
        let entries: Vec<_> = m.entry_points.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(entries, ["vs_main", "fs_main"]);
    }
    assert!(!sdf_core::MSL.is_empty() && !simple::HLSL.is_empty() && !simple::SPIRV.is_empty());
}

#[test]
fn the_simple_pipeline_switches_the_optional_blocks_off() {
    for feature in [
        "FEAT_GRADIENT",
        "FEAT_MASK",
        "FEAT_FILTER",
        "FEAT_SHADOW",
        "FEAT_3D",
        "FEAT_CLIP_SHAPE",
    ] {
        assert!(sdf_core::WGSL.contains(&format!("const {feature}: bool = true;")));
        assert!(simple::WGSL.contains(&format!("const {feature}: bool = false;")));
    }
    assert!(simple::WGSL.contains("const FEAT_BORDER: bool = true;"));
}

#[test]
fn storage_buffers_keep_it_off_webgl2() {
    assert!(sdf_core::GLSL.is_empty());
}
