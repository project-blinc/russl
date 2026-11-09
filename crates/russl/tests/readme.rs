//! The README's usage example, so it keeps compiling.

mod sdf {
    russl::russl! {
        /// The distance from `p` to a box of half-size `b` with corner radius `r`.
        pub fn rounded_box(p: Vec2, b: Vec2, r: f32) -> f32 {
            let q = p.abs() - b + r;
            q.max(Vec2::ZERO).length() + q.x.max(q.y).min(0.0) - r
        }
    }
}

mod card {
    russl::russl! {
        use super::sdf::rounded_box;

        struct Card { size: Vec2, radius: f32, color: Vec4 }

        struct VsOut {
            #[builtin(position)] pos: Vec4,
            #[location(0)] local: Vec2,
        }

        #[group(0)] #[binding(0)] static CARD: Uniform<Card>;
        #[pipeline] const SOFT_EDGE: bool = true;

        const CORNERS: [Vec2; 6] = [
            Vec2::new(-1.0, -1.0), Vec2::new(1.0, -1.0), Vec2::new(1.0, 1.0),
            Vec2::new(-1.0, -1.0), Vec2::new(1.0, 1.0), Vec2::new(-1.0, 1.0),
        ];

        #[vertex]
        fn vs(#[builtin(vertex_index)] vi: u32) -> VsOut {
            let corner = CORNERS[vi];
            VsOut { pos: Vec4::new(corner, 0.0, 1.0), local: corner * CARD.size * 0.5 }
        }

        #[fragment]
        #[location(0)]
        fn fs(input: VsOut) -> Vec4 {
            let d = rounded_box(input.local, CARD.size * 0.5, CARD.radius);
            if d > 1.0 {
                discard!();
            }
            let aa = if SOFT_EDGE { d.fwidth() } else { 0.0001 };
            let alpha = 1.0 - d.smoothstep(-aa, aa);
            CARD.color * alpha
        }
    }
}

mod card_hard {
    russl::russl! {
        #![specialize(super::card)]
        const SOFT_EDGE: bool = false;
    }
}

#[test]
fn the_readme_example_builds_every_target() {
    for wgsl in [card::WGSL, card_hard::WGSL] {
        naga::front::wgsl::parse_str(wgsl).expect("WGSL parses");
    }
    assert!(!card::MSL.is_empty() && !card::HLSL.is_empty() && !card::SPIRV.is_empty());
    assert_eq!(card::GLSL.len(), 2);
}
