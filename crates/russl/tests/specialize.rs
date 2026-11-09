//! A variant module specialising a shader's pipeline constants, through rustc's expansion.

mod fade {
    russl::russl! {
        #[pipeline] const FEAT_FADE: bool = true;

        #[fragment]
        #[location(0)]
        fn fs(#[builtin(position)] p: Vec4) -> Vec4 {
            let a = if FEAT_FADE { p.x * 0.001 } else { 1.0 };
            Vec4::splat(a)
        }
    }
}

mod fade_off {
    russl::russl! {
        #![specialize(super::fade)]
        const FEAT_FADE: bool = false;
    }
}

#[test]
fn a_variant_specialises_its_shader() {
    naga::front::wgsl::parse_str(fade_off::WGSL).expect("WGSL parses");
    assert_ne!(fade::WGSL, fade_off::WGSL);
    assert!(fade_off::MSL.contains("fs"));
}
