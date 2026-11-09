#[test]
fn a_shader_imports_modules_from_another_crate() {
    let wgsl = russl_fixture_consumer::badge::WGSL;
    naga::front::wgsl::parse_str(wgsl).expect("WGSL parses");
    assert!(
        wgsl.contains("fn sd_ring") && wgsl.contains("fn sd_circle"),
        "{wgsl}"
    );
}
