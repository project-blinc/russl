//! A validated naga module written for every target.

use crate::{Artifacts, GlslEntry, Stage};

/// Write `module` as WGSL, MSL, HLSL, SPIR-V and GLSL ES. An error names the target that
/// failed.
///
/// naga's writers take no pipeline constants, so each one is fixed at its default first: the
/// artifacts are the module as those defaults specialise it.
pub fn emit(module: &naga::Module, info: &naga::valid::ModuleInfo) -> Result<Artifacts, String> {
    let fail = |target: &str, e: &dyn std::fmt::Display| format!("{target} output: {e}");
    let (module, _) = naga::back::pipeline_constants::process_overrides(
        module,
        info,
        None,
        &naga::back::PipelineConstants::default(),
    )
    .map_err(|e| format!("pipeline constants: {e}"))?;
    // Resolving leaves the overrides declared but unreferenced, and naga's writers refuse a
    // module that declares any. Validating again proves nothing still refers to one.
    let mut module = module.into_owned();
    module.overrides = naga::Arena::new();
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .map_err(|e| format!("specialised module: {}", e.as_inner()))?;
    let (module, info) = (&module, &info);
    let wgsl = naga::back::wgsl::write_string(module, info, naga::back::wgsl::WriterFlags::empty())
        .map_err(|e| fail("WGSL", &e))?;
    let (msl, _) = naga::back::msl::write_string(
        module,
        info,
        &naga::back::msl::Options::default(),
        &naga::back::msl::PipelineOptions::default(),
    )
    .map_err(|e| fail("MSL", &e))?;
    let mut hlsl = String::new();
    naga::back::hlsl::Writer::new(
        &mut hlsl,
        &naga::back::hlsl::Options::default(),
        &naga::back::hlsl::PipelineOptions::default(),
    )
    .write(module, info, None)
    .map_err(|e| fail("HLSL", &e))?;
    let spirv =
        naga::back::spv::write_vec(module, info, &naga::back::spv::Options::default(), None)
            .map_err(|e| fail("SPIR-V", &e))?;

    // GLSL ES 3.00 is WebGL2's language: one source per entry point, and no compute stage.
    let glsl_options = naga::back::glsl::Options {
        version: naga::back::glsl::Version::Embedded {
            version: 300,
            is_webgl: true,
        },
        ..Default::default()
    };
    let mut glsl = Vec::new();
    for entry in &module.entry_points {
        let stage = match entry.stage {
            naga::ShaderStage::Vertex => Stage::Vertex,
            naga::ShaderStage::Fragment => Stage::Fragment,
            _ => continue,
        };
        let mut source = String::new();
        naga::back::glsl::Writer::new(
            &mut source,
            module,
            info,
            &glsl_options,
            &naga::back::glsl::PipelineOptions {
                shader_stage: entry.stage,
                entry_point: entry.name.clone(),
                multiview: None,
            },
            naga::proc::BoundsCheckPolicies::default(),
        )
        .and_then(|mut w| w.write())
        .map_err(|e| fail("GLSL", &e))?;
        glsl.push(GlslEntry {
            entry_point: entry.name.clone(),
            stage,
            source,
        });
    }

    Ok(Artifacts {
        wgsl,
        msl,
        hlsl,
        spirv,
        glsl,
        warnings: Vec::new(),
    })
}
