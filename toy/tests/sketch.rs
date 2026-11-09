//! Sketches compile for Blinc's flow pipeline: its entry points, its uniform block, and errors
//! on the sketch's own lines.

use russl_compiler::{ConstValue, Constant};
use russl_toy::sketch::{STARTER, Source, compile as compile_sources};

fn compile(
    text: &str,
    constants: &[Constant],
    generation: u64,
) -> Result<
    (russl_toy::sketch::Compiled, Vec<russl_toy::sketch::Problem>),
    Vec<russl_toy::sketch::Problem>,
> {
    compile_sources(
        &[Source {
            name: "main".into(),
            text: text.into(),
        }],
        constants,
        generation,
    )
}

#[test]
fn the_starter_sketch_compiles_for_the_preview() {
    let (c, warnings) = compile(STARTER, &[], 1).unwrap_or_else(|p| panic!("{p:?}"));
    assert!(warnings.is_empty(), "{warnings:?}");
    let module = naga::front::wgsl::parse_str(&c.shader.wgsl).expect("WGSL parses");
    let mut entries: Vec<_> = module
        .entry_points
        .iter()
        .map(|e| e.name.as_str())
        .collect();
    entries.sort();
    assert_eq!(entries, ["fs_main", "vs_main"]);
    assert_eq!(c.shader.name, "russl_toy_1");
    let names: Vec<_> = c
        .artifacts
        .reflection
        .constants
        .iter()
        .map(|(n, _)| n.as_str())
        .collect();
    assert_eq!(names, ["RINGS", "SPEED"]);
}

/// Blinc's flow pipeline fills this block as `FlowUniforms`: viewport_size, time, frame_index,
/// element_bounds, pointer, corner_radius and padding.
#[test]
fn the_frame_uniform_is_laid_out_as_blinc_fills_it() {
    let (c, _) = compile(STARTER, &[], 1).unwrap();
    let module = naga::front::wgsl::parse_str(&c.shader.wgsl).unwrap();
    let (_, ty) = module
        .types
        .iter()
        .find(|(_, t)| t.name.as_deref() == Some("Toy"))
        .unwrap();
    let naga::TypeInner::Struct { members, span } = &ty.inner else {
        panic!()
    };
    let offsets: Vec<u32> = members.iter().map(|m| m.offset).collect();
    assert_eq!((offsets, *span), (vec![0, 8, 12, 16, 32, 40, 44], 48));
    let binding = c
        .artifacts
        .reflection
        .bindings
        .iter()
        .find(|b| b.name == "TOY")
        .unwrap();
    assert_eq!((binding.group, binding.binding), (0, 0));
}

#[test]
fn an_error_is_on_the_sketchs_own_line() {
    let src = "#[fragment]\n#[location(0)]\nfn fs_main(input: Frag) -> Vec4 {\n    Vec4::new(input.uv, true)\n}\n";
    let problems = compile(src, &[], 1).err().expect("an error");
    assert_eq!(problems[0].line, Some(4), "{problems:?}");
    assert!(
        problems[0].text().starts_with("4:"),
        "{}",
        problems[0].text()
    );
}

#[test]
fn a_sketch_binds_only_the_frame() {
    let src = "#[group(0)] #[binding(1)] static EXTRA: Uniform<Vec4>;\n#[fragment]\n#[location(0)]\nfn fs_main(input: Frag) -> Vec4 { EXTRA }\n";
    let problems = compile(src, &[], 1).err().expect("an error");
    assert!(
        problems[0].message.contains("binds only `TOY`"),
        "{problems:?}"
    );
}

#[test]
fn a_sketch_without_fs_main_says_what_it_needs() {
    let problems = compile("fn helper() -> f32 { 1.0 }\n", &[], 1)
        .err()
        .expect("an error");
    assert!(
        problems
            .iter()
            .any(|p| p.message.contains("fn fs_main(input: Frag) -> Vec4")),
        "{problems:?}"
    );
}

#[test]
fn a_pipeline_constant_takes_the_value_chosen() {
    let off = Constant {
        name: "RINGS".into(),
        value: ConstValue::Bool(false),
        span: proc_macro2::Span::call_site(),
    };
    let (c, _) = compile(STARTER, &[off], 2).unwrap();
    assert!(
        c.artifacts
            .reflection
            .constants
            .contains(&("RINGS".into(), ConstValue::Bool(false)))
    );
}

#[test]
fn a_module_tab_is_imported_by_name_and_its_errors_stay_in_it() {
    let main = Source {
        name: "main".into(),
        text: "use crate::shapes::ring;\n#[fragment]\n#[location(0)]\nfn fs_main(input: Frag) -> Vec4 { Vec4::splat(ring(input.uv)) }\n".into(),
    };
    let shapes = Source {
        name: "shapes".into(),
        text: "pub fn ring(p: Vec2) -> f32 {\n    (p.length() - 0.3).abs()\n}\n".into(),
    };
    let (c, _) =
        compile_sources(&[main.clone(), shapes], &[], 1).unwrap_or_else(|p| panic!("{p:?}"));
    assert!(c.shader.wgsl.contains("fn ring("));
    let broken = Source {
        name: "shapes".into(),
        text: "pub fn ring(p: Vec2) -> f32 {\n    p.length() + true\n}\n".into(),
    };
    let problems = compile_sources(&[main, broken], &[], 2)
        .err()
        .expect("an error");
    assert_eq!(
        (problems[0].module, problems[0].line),
        (1, Some(2)),
        "{problems:?}"
    );
}
