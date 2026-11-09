//! A validated naga module written for every target.

use crate::{Artifacts, GlslEntry, Stage};

/// Write `module` as WGSL, MSL, HLSL, SPIR-V and GLSL ES. An error names the target that
/// failed.
///
/// naga's writers take no pipeline constants, so each is fixed first: at the value in
/// `constants`, else at its default.
pub fn emit(
    module: &naga::Module,
    info: &naga::valid::ModuleInfo,
    constants: &naga::back::PipelineConstants,
) -> Result<Artifacts, String> {
    let fail = |target: &str, e: &dyn std::fmt::Display| format!("{target} output: {e}");
    let (module, _) =
        naga::back::pipeline_constants::process_overrides(module, info, None, constants)
            .map_err(|e| format!("pipeline constants: {e}"))?;
    // Resolving leaves the overrides declared but unreferenced, and naga's writers refuse a
    // module that declares any. Validating again proves nothing still refers to one.
    let mut module = module.into_owned();
    let built = module
        .overrides
        .iter()
        .filter_map(|(_, o)| {
            let value = match module.global_expressions[o.init?] {
                naga::Expression::Literal(naga::Literal::Bool(b)) => crate::ConstValue::Bool(b),
                naga::Expression::Literal(naga::Literal::F32(v)) => {
                    crate::ConstValue::Float(v as f64)
                }
                naga::Expression::Literal(naga::Literal::F16(v)) => {
                    crate::ConstValue::Float(v.to_f64())
                }
                naga::Expression::Literal(naga::Literal::I32(v)) => {
                    crate::ConstValue::Int(v as i64)
                }
                naga::Expression::Literal(naga::Literal::U32(v)) => {
                    crate::ConstValue::Int(v as i64)
                }
                _ => return None,
            };
            Some((o.name.clone()?, value))
        })
        .collect();
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
    // One version for every device: MSL 2.1 runs on macOS 10.14 and iOS 12 onwards. Resources
    // take wgpu's Metal indices (see `native`), never naga's placeholders.
    let (per_entry_point_map, msl_bindings) = crate::native::msl_map(module);
    let msl_options = naga::back::msl::Options {
        lang_version: (2, 1),
        per_entry_point_map,
        fake_missing_bindings: false,
        ..Default::default()
    };
    let (msl, _) = naga::back::msl::write_string(
        module,
        info,
        &msl_options,
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
    let spirv = crate::spirv::strip_unneeded_layout(&spirv);

    // GLSL ES 3.00 is WebGL2's language: one source per entry point, and no compute stage.
    let glsl_options = naga::back::glsl::Options {
        version: naga::back::glsl::Version::Embedded {
            version: 300,
            is_webgl: true,
        },
        ..Default::default()
    };
    let mut glsl = Vec::new();
    let half = module.types.iter().any(|(_, t)| {
        matches!(
            t.inner,
            naga::TypeInner::Scalar(naga::Scalar::F16)
                | naga::TypeInner::Vector {
                    scalar: naga::Scalar::F16,
                    ..
                }
                | naga::TypeInner::Matrix {
                    scalar: naga::Scalar::F16,
                    ..
                }
        )
    });
    let mut glsl_missing = half.then(|| "GLSL ES 3.00 has no f16".to_string());
    let mut glsl_bindings = Vec::new();
    let entries = if half {
        &[][..]
    } else {
        &module.entry_points[..]
    };
    for entry in entries {
        let stage = match entry.stage {
            naga::ShaderStage::Vertex => Stage::Vertex,
            naga::ShaderStage::Fragment => Stage::Fragment,
            _ => continue,
        };
        let mut source = String::new();
        let written: Result<naga::back::glsl::ReflectionInfo, _> = naga::back::glsl::Writer::new(
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
        .and_then(|mut w| w.write());
        match written {
            Ok(info) => glsl_bindings.push(crate::native::glsl(module, &entry.name, &info)),
            // WebGL2 cannot run this module at all, which is no fault of its source.
            Err(naga::back::glsl::Error::MissingFeatures(f)) => {
                glsl.clear();
                glsl_bindings.clear();
                glsl_missing = Some(format!("GLSL ES 3.00 has no {f:?}"));
                break;
            }
            Err(e) => return Err(fail("GLSL", &e)),
        }
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
        glsl_missing,
        warnings: Vec::new(),
        library: false,
        reflection: crate::Reflection {
            constants: built,
            native: crate::native::NativeBindings {
                msl: msl_bindings,
                hlsl: crate::native::hlsl(module),
                glsl: glsl_bindings,
            },
            ..Default::default()
        },
    })
}
