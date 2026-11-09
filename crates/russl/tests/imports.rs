//! Imports between russl modules, through rustc's own macro expansion.

mod common {
    russl::russl! {
        pub fn sd_circle(p: Vec2, r: f32) -> f32 { p.length() - r }
        pub fn sd_box(p: Vec2, b: Vec2) -> f32 {
            let d = p.abs() - b;
            outside(d) + d.x.max(d.y).min(0.0)
        }
        fn outside(d: Vec2) -> f32 { d.max(Vec2::ZERO).length() }
        pub const EDGE: f32 = 0.01;
    }
}

mod rings {
    russl::russl! {
        pub use super::common::sd_circle;
        pub fn ring(p: Vec2, r: f32, w: f32) -> f32 { sd_circle(p, r).abs() - w }
    }
}

mod shader {
    russl::russl! {
        use super::rings::{ring, sd_circle};
        use super::common::{sd_box, EDGE};

        #[fragment]
        #[location(0)]
        fn fs(#[builtin(position)] p: Vec4) -> Vec4 {
            let q = p.xy * 0.01 - 0.5;
            let d = ring(q, 0.3, 0.02).min(sd_box(q, Vec2::splat(0.1))).min(sd_circle(q, 0.05));
            Vec4::splat(1.0 - d.smoothstep(0.0, EDGE))
        }
    }
}

#[test]
fn a_shader_compiles_against_imported_modules() {
    naga::front::wgsl::parse_str(shader::WGSL).expect("WGSL parses");
    for f in ["fn ring", "fn sd_circle", "fn sd_box", "fn outside"] {
        assert!(shader::WGSL.contains(f), "{f} missing:\n{}", shader::WGSL);
    }
    assert!(shader::MSL.contains("fs"));
    assert_eq!(shader::GLSL.len(), 1);
}
