mod empty {
    russl::russl! {}
}

#[test]
fn the_macro_expands_to_every_target() {
    naga::front::wgsl::parse_str(empty::WGSL).expect("WGSL from russl! parses");
    assert!(empty::MSL.contains("metal_stdlib"));
    assert_eq!(empty::SPIRV.first(), Some(&0x0723_0203));
    assert!(empty::GLSL.is_empty());
    let _ = empty::HLSL;
}
