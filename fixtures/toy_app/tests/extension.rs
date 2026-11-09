//! The framework macro's shader, built by rustc.

#[test]
fn a_framework_macro_compiles_with_its_extension() {
    let wgsl = russl_fixture_toy_app::pulse::WGSL;
    naga::front::wgsl::parse_str(wgsl).expect("WGSL parses");
    assert!(
        wgsl.contains("fn wave(") && wgsl.contains("fn sd_circle("),
        "{wgsl}"
    );
    assert!(wgsl.contains("var<uniform> FRAME"), "{wgsl}");
}
