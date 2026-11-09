//! The language features through rustc's expansion: generics, swizzle assignment, array
//! lengths from constants.

mod kernel {
    russl::russl! {
        pub const TAPS: u32 = 3;
        pub fn total<const N: u32>(w: [f32; N]) -> f32 {
            let mut s = 0.0;
            for i in 0..N { s += w[i]; }
            s
        }
        pub fn spread<T: Float>(x: T, by: f32) -> T { x * by }
    }
}

mod shader {
    russl::russl! {
        use super::kernel::{spread, total, TAPS};
        const WEIGHTS: [f32; TAPS] = [0.25, 0.5, 0.25];

        #[fragment]
        #[location(0)]
        fn fs(#[builtin(position)] p: Vec4) -> Vec4 {
            let mut c = Vec4::ONE;
            c.xyz = spread(p.xyz, total(WEIGHTS));
            c.w = spread(c.w, 0.5);
            c
        }
    }
}

#[test]
fn the_language_builds_through_the_macro() {
    naga::front::wgsl::parse_str(shader::WGSL).expect("WGSL parses");
    assert!(shader::WGSL.contains("fn spread_vec3") && shader::WGSL.contains("fn total_3"));
}
