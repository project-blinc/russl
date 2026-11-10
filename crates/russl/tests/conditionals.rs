//! One shader and its library written once, built as different variants by specialising a
//! pipeline constant, with `#if` choosing what each variant declares.

mod access {
    russl::russl! {
        #[pipeline] const TIER: u32 = 0;

        #if TIER == 2
            #[group(0)] #[binding(1)] pub static DATA: Texture2D<f32>;
            pub fn load(i: u32) -> Vec4 { DATA.load(IVec2::new(i as i32, 0), 0) }
        #else
            #[group(0)] #[binding(1)] pub static DATA: Storage<[Vec4]>;
            pub fn load(i: u32) -> Vec4 { DATA[i] }
        #endif
    }
}

mod shader {
    russl::russl! {
        use super::access::load;

        #[pipeline] const TIER: u32 = 0;

        #[vertex]
        fn vs(
            #[builtin(vertex_index)] vi: u32,
            #if TIER >= 1
                #[location(0)] bounds: Vec4,
            #endif
        ) -> Out {
            #if TIER >= 1
                return Out { pos: bounds + load(vi) };
            #else
                return Out { pos: load(vi) };
            #endif
        }

        struct Out { #[builtin(position)] pos: Vec4 }

        #[fragment]
        #[location(0)]
        fn fs(input: Out) -> Vec4 { input.pos }
    }
}

mod vertex_buffer {
    russl::russl! { #![specialize(super::shader)] const TIER: u32 = 1; }
}

mod data_texture {
    russl::russl! { #![specialize(super::shader)] const TIER: u32 = 2; }
}

#[test]
fn each_variant_declares_its_own_bindings_and_inputs() {
    let storage = shader::WGSL;
    assert!(storage.contains("var<storage> DATA"), "{storage}");
    assert!(!storage.contains("@location(0) bounds"), "{storage}");

    let attributes = vertex_buffer::WGSL;
    assert!(attributes.contains("var<storage> DATA"), "{attributes}");
    assert!(attributes.contains("@location(0) bounds"), "{attributes}");

    let textured = data_texture::WGSL;
    assert!(textured.contains("var DATA: texture_2d<f32>"), "{textured}");
    assert!(!textured.contains("var<storage>"), "{textured}");
    assert!(textured.contains("@location(0) bounds"), "{textured}");
}
