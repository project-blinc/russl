//! A shader written with `toy_shader!`: it reads the extension's prelude, links its extern,
//! and imports a library module written with plain `russl!`.

pub mod shapes {
    russl::russl! {
        #![export]
        pub fn sd_circle(p: Vec2, r: f32) -> f32 { p.length() - r }
    }
}

pub mod pulse {
    russl_fixture_toy_macro::toy_shader! {
        use super::shapes::sd_circle;
        extern { fn wave(t: f32) -> f32; }

        #[fragment]
        #[location(0)]
        fn fs(#[builtin(position)] p: Vec4) -> Vec4 {
            let d = sd_circle(p.xy / FRAME.size - 0.5, wave(FRAME.time) * 0.25);
            Vec4::splat(1.0 - d.smoothstep(0.0, 0.01))
        }
    }
}
