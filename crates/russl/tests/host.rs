//! The host side of a module: Rust twins of its structs with the bytes a buffer holds, and
//! constants for its bindings, pipeline constants and vertex inputs.

mod types {
    russl::russl! {
        pub struct Light { pub dir: Vec3, pub power: f32 }
    }
}

mod scene {
    russl::russl! {
        #![bytemuck]
        use super::types::Light;

        struct Scene {
            tint: Vec3,
            scale: f32,
            lights: [Light; 2],
            points: [Vec3; 2],
            basis: Mat3,
        }

        #[group(0)] #[binding(0)] static SCENE: Uniform<Scene>;
        #[group(0)] #[binding(1)] static ATLAS: Texture2D<f32>;
        #[pipeline] const GAIN: f32 = 1.5;

        #[vertex]
        #[builtin(position)]
        fn vs(#[location(0)] pos: Vec2, #[location(1)] id: u32) -> Vec4 {
            Vec4::new(pos * SCENE.scale * GAIN, id as f32, 1.0)
        }

        #[fragment]
        #[location(0)]
        fn fs() -> Vec4 {
            Vec4::new(SCENE.tint, SCENE.lights[1].power + SCENE.points[0].x + SCENE.basis[0].x)
                + ATLAS.load(IVec2::ZERO, 0)
        }
    }
}

mod scene_dim {
    russl::russl! {
        #![specialize(super::scene)]
        const GAIN: f32 = 0.5;
    }
}

#[test]
fn twins_hold_the_bytes_naga_lays_out() {
    use scene::Scene;
    let parsed = naga::front::wgsl::parse_str(scene::WGSL).unwrap();
    let (_, ty) = parsed
        .types
        .iter()
        .find(|(_, t)| t.name.as_deref() == Some("Scene"))
        .unwrap();
    let naga::TypeInner::Struct { members, span } = &ty.inner else {
        panic!()
    };
    assert_eq!(Scene::SIZE, *span as usize);
    assert_eq!(std::mem::size_of::<Scene>(), Scene::SIZE);
    let offsets = [
        Scene::TINT_OFFSET,
        Scene::SCALE_OFFSET,
        Scene::LIGHTS_OFFSET,
        Scene::POINTS_OFFSET,
        Scene::BASIS_OFFSET,
    ];
    let theirs: Vec<usize> = members.iter().map(|m| m.offset as usize).collect();
    assert_eq!(offsets.to_vec(), theirs);

    let light = types::Light::new([0.0, 1.0, 0.0], 2.0);
    let s = Scene::new(
        [1.0, 0.5, 0.25],
        3.0,
        [light; 2],
        [[1.0; 4]; 2],
        [[0.0; 4]; 3],
    );
    let bytes = s.as_bytes();
    assert_eq!(bytes.len(), Scene::SIZE);
    assert_eq!(&bytes[Scene::SCALE_OFFSET..][..4], &3.0f32.to_le_bytes());
    let power_at = Scene::LIGHTS_OFFSET + types::Light::SIZE + types::Light::POWER_OFFSET;
    assert_eq!(&bytes[power_at..][..4], &2.0f32.to_le_bytes());
    assert_eq!(bytemuck::bytes_of(&s), bytes, "#![bytemuck] makes it Pod");
}

#[test]
fn bindings_constants_and_inputs_are_consts() {
    assert_eq!((scene::SCENE_GROUP, scene::SCENE_BINDING), (0, 0));
    assert_eq!(scene::SCENE_SIZE, scene::Scene::SIZE as u64);
    assert_eq!((scene::ATLAS_GROUP, scene::ATLAS_BINDING), (0, 1));
    assert_eq!(scene::CONSTANTS, &[("GAIN", 1.5)]);
    assert_eq!(scene_dim::CONSTANTS, &[("GAIN", 0.5)]);
    assert_eq!(
        scene::VS_INPUTS,
        &[("pos", 0, "Float32x2"), ("id", 1, "Uint32")]
    );
}

mod generic {
    russl::russl! {
        struct Pair<T> { a: T, b: T }
        #[group(0)] #[binding(0)] static P: Storage<Pair<Vec3>>;

        #[fragment]
        #[location(0)]
        fn fs() -> Vec4 { Vec4::new(P.a, P.b.x) }
    }
}

#[test]
fn a_generic_struct_instance_has_a_twin() {
    let p = generic::PairVec3::new([1.0; 3], [2.0; 3]);
    assert_eq!(generic::PairVec3::SIZE, 32);
    assert_eq!(generic::PairVec3::B_OFFSET, 16);
    assert_eq!(p.as_bytes().len(), 32);
}
