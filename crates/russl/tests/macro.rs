mod empty {
    russl::russl! {}
}

mod quad {
    russl::russl! {
        struct VsOut {
            #[builtin(position)] pos: Vec4,
            #[location(0)] uv: Vec2,
        }

        #[group(0)] #[binding(0)] static TINT: Uniform<Vec4>;

        #[vertex]
        fn vs(#[builtin(vertex_index)] vi: u32) -> VsOut {
            let uv = Vec2::new((vi & 1) as f32, (vi >> 1) as f32);
            VsOut { pos: Vec4::new(uv * 2.0 - 1.0, 0.0, 1.0), uv }
        }

        #[fragment]
        #[location(0)]
        fn fs(input: VsOut) -> Vec4 {
            TINT * input.uv.x.smoothstep(0.0, 1.0)
        }
    }
}

#[test]
fn an_empty_module_expands_to_every_target() {
    naga::front::wgsl::parse_str(empty::WGSL).expect("WGSL from russl! parses");
    assert!(empty::MSL.contains("metal_stdlib"));
    assert_eq!(empty::SPIRV.first(), Some(&0x0723_0203));
    assert!(empty::GLSL.is_empty());
    let _ = empty::HLSL;
}

#[test]
fn a_shader_expands_to_every_target() {
    naga::front::wgsl::parse_str(quad::WGSL).expect("WGSL from russl! parses");
    assert!(quad::MSL.contains("vertex"), "{}", quad::MSL);
    assert!(quad::HLSL.contains("fs"), "{}", quad::HLSL);
    assert_eq!(quad::SPIRV.first(), Some(&0x0723_0203));
    let entries: Vec<&str> = quad::GLSL.iter().map(|g| g.0).collect();
    assert_eq!(entries, ["vs", "fs"]);
}
