//! What the host learns of a compiled module, checked against naga's own layout of the same
//! WGSL.

use russl_compiler::{BindingKind, ConstValue, HostType, compile};

const SRC: &str = r#"
struct Light { dir: Vec3, power: f32 }
struct Scene {
    tint: Vec3,
    scale: f32,
    uv: Vec2,
    basis: Mat3,
    lights: [Light; 2],
    points: [Vec3; 3],
    count: u32,
    half: HVec2,
    last: f16,
}
#[group(0)] #[binding(0)] static SCENE: Storage<Scene>;
#[group(1)] #[binding(2)] static VALUES: Uniform<Vec4>;
#[pipeline] const SOFT: bool = true;
#[pipeline] const GAIN: f32 = 1.5;

struct VsIn {
    #[location(0)] pos: Vec2,
    #[location(2)] color: Vec4,
}

#[vertex]
#[builtin(position)]
fn vs(input: VsIn, #[location(1)] id: u32) -> Vec4 {
    Vec4::new(input.pos, id as f32, 1.0) * input.color.x
}

#[fragment]
#[location(0)]
fn fs() -> Vec4 {
    let g = if SOFT { GAIN } else { 1.0 };
    VALUES * g + Vec4::splat(SCENE.scale + SCENE.lights[1].power + SCENE.points[2].x + SCENE.last as f32 + SCENE.count as f32)
}
"#;

#[test]
fn struct_layouts_match_naga() {
    let a = compile(SRC.parse().unwrap()).unwrap_or_else(|d| panic!("{d:?}"));
    let parsed = naga::front::wgsl::parse_str(&a.wgsl).expect("WGSL parses");
    for s in &a.reflection.structs {
        let (_, ty) = parsed
            .types
            .iter()
            .find(|(_, t)| t.name.as_deref() == Some(s.name.as_str()))
            .unwrap_or_else(|| panic!("`{}` in the WGSL", s.name));
        let naga::TypeInner::Struct { members, span } = &ty.inner else {
            panic!("a struct")
        };
        assert_eq!(s.size, *span, "size of `{}`", s.name);
        let ours: Vec<u32> = s.fields.iter().map(|f| f.offset).collect();
        let theirs: Vec<u32> = members.iter().map(|m| m.offset).collect();
        assert_eq!(ours, theirs, "offsets of `{}`", s.name);
    }
    assert!(a.reflection.structs.iter().any(|s| s.name == "Scene"));
}

#[test]
fn bindings_constants_and_vertex_inputs_are_reported() {
    let a = compile(SRC.parse().unwrap()).unwrap();
    let r = &a.reflection;
    let values = r.bindings.iter().find(|b| b.name == "VALUES").unwrap();
    assert_eq!(
        (values.group, values.binding, values.kind, values.size),
        (1, 2, BindingKind::Uniform, Some(16))
    );
    let scene = r.bindings.iter().find(|b| b.name == "SCENE").unwrap();
    assert_eq!(scene.kind, BindingKind::Storage { writable: false });
    assert!(
        r.constants
            .contains(&("SOFT".into(), ConstValue::Bool(true)))
    );
    assert!(
        r.constants
            .contains(&("GAIN".into(), ConstValue::Float(1.5)))
    );
    let vs = &r.vertex_inputs[0];
    let got: Vec<_> = vs
        .inputs
        .iter()
        .map(|i| (i.name.as_str(), i.location, i.format))
        .collect();
    assert_eq!(
        got,
        [
            ("pos", 0, "Float32x2"),
            ("id", 1, "Uint32"),
            ("color", 2, "Float32x4")
        ]
    );
    let lights = &r.structs.iter().find(|s| s.name == "Scene").unwrap().fields[4];
    assert!(
        matches!(
            &lights.ty,
            HostType::Array {
                len: Some(2),
                stride: 16,
                ..
            }
        ),
        "{:?}",
        lights.ty
    );
}

#[test]
fn native_bindings_follow_each_backends_model() {
    use russl_compiler::{HlslSlot, MslSlot};
    let a = compile(
        r#"
#[group(0)] #[binding(0)] static GLOBALS: Uniform<Vec4>;
#[group(0)] #[binding(1)] static ITEMS: Storage<[Vec4]>;
#[group(1)] #[binding(0)] static ATLAS: Texture2D<f32>;
#[group(1)] #[binding(1)] static SAMPLER: Sampler;

#[vertex]
#[builtin(position)]
fn vs(#[builtin(vertex_index)] vi: u32) -> Vec4 { ITEMS[vi] * GLOBALS.x }

#[fragment]
#[location(0)]
fn fs(#[builtin(position)] p: Vec4) -> Vec4 { GLOBALS * ATLAS.sample(SAMPLER, p.xy) }
"#
        .parse()
        .unwrap(),
    )
    .unwrap();
    let native = &a.reflection.native;
    let fs = native.msl.iter().find(|e| e.entry_point == "fs").unwrap();
    let slots: Vec<_> = fs
        .resources
        .iter()
        .map(|r| ((r.group, r.binding), r.slot))
        .collect();
    assert_eq!(
        slots,
        [
            ((0, 0), MslSlot::Buffer(0)),
            ((0, 1), MslSlot::Buffer(1)),
            ((1, 0), MslSlot::Texture(0)),
            ((1, 1), MslSlot::Sampler(0)),
        ]
    );
    assert_eq!(
        fs.sizes_buffer,
        Some(2),
        "a storage buffer is bound, so the sizes buffer follows"
    );
    assert!(a.msl.contains("[[buffer(0)]]") && a.msl.contains("[[texture(0)]]"));
    assert!(a.msl.contains("[[sampler(0)]]") && !a.msl.contains("fake"));
    let hlsl: Vec<_> = native
        .hlsl
        .iter()
        .map(|b| ((b.group, b.binding), b.slot))
        .collect();
    assert_eq!(
        hlsl[0].1,
        HlslSlot::Register {
            class: 'b',
            index: 0,
            space: 0
        }
    );
    assert_eq!(
        hlsl[1].1,
        HlslSlot::Register {
            class: 't',
            index: 1,
            space: 0
        }
    );
    assert_eq!(hlsl[3].1, HlslSlot::SamplerIndex { index: 1 });
    assert!(a.hlsl.contains("register(t0, space1)"));
}
