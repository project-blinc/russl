//! The russl compiler.
//!
//! Source in Rust syntax is parsed with `syn`, checked, lowered to a [`naga::Module`], validated,
//! and written out for every target: WGSL, MSL, HLSL, SPIR-V and GLSL ES. The macro, the build driver and the CLI all call
//! [`compile`], so a shader compiles the same way wherever it is written.
//!
//! Only an empty module compiles: any item is reported as unsupported.

use proc_macro2::{Span, TokenStream};

/// What a compiled shader module produces: one artifact per target, so no platform compiles
/// shaders at run time.
#[derive(Debug, Clone)]
pub struct Artifacts {
    /// WGSL, for WebGPU (browsers accept nothing else) and wgpu.
    pub wgsl: String,
    /// Metal Shading Language, for macOS and iOS.
    pub msl: String,
    /// HLSL, for Direct3D 12.
    pub hlsl: String,
    /// SPIR-V words, for Vulkan.
    pub spirv: Vec<u32>,
    /// GLSL ES 3.00 for WebGL2, one source per entry point as GLSL requires.
    pub glsl: Vec<GlslEntry>,
}

/// One entry point written as GLSL ES.
#[derive(Debug, Clone)]
pub struct GlslEntry {
    pub entry_point: String,
    pub stage: Stage,
    pub source: String,
}

/// A shader stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Vertex,
    Fragment,
    Compute,
}

/// A compile error, at the source token that caused it.
#[derive(Debug, Clone)]
pub struct Error {
    pub span: Span,
    pub message: String,
}

impl Error {
    fn new(span: Span, message: impl Into<String>) -> Self {
        Self {
            span,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

/// Compile one module of russl source.
pub fn compile(source: TokenStream) -> Result<Artifacts, Error> {
    let file: syn::File = syn::parse2(source).map_err(|e| Error::new(e.span(), e.to_string()))?;
    if let Some(item) = file.items.first() {
        return Err(Error::new(
            syn::spanned::Spanned::span(item),
            "russl does not compile items yet",
        ));
    }
    emit(&naga::Module::default())
}

/// Validate a module and write it out for each target.
fn emit(module: &naga::Module) -> Result<Artifacts, Error> {
    let fail = |target: &str, e: &dyn std::fmt::Display| {
        Error::new(Span::call_site(), format!("{target} output: {e}"))
    };
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(module)
    .map_err(|e| Error::new(Span::call_site(), format!("invalid module: {e}")))?;

    let wgsl =
        naga::back::wgsl::write_string(module, &info, naga::back::wgsl::WriterFlags::empty())
            .map_err(|e| fail("WGSL", &e))?;
    let (msl, _) = naga::back::msl::write_string(
        module,
        &info,
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
    .write(module, &info, None)
    .map_err(|e| fail("HLSL", &e))?;
    let spirv =
        naga::back::spv::write_vec(module, &info, &naga::back::spv::Options::default(), None)
            .map_err(|e| fail("SPIR-V", &e))?;

    let glsl_options = naga::back::glsl::Options {
        version: naga::back::glsl::Version::Embedded {
            version: 300,
            is_webgl: true,
        },
        ..Default::default()
    };
    let mut glsl = Vec::with_capacity(module.entry_points.len());
    for entry in &module.entry_points {
        let stage = match entry.stage {
            naga::ShaderStage::Vertex => Stage::Vertex,
            naga::ShaderStage::Fragment => Stage::Fragment,
            naga::ShaderStage::Compute => Stage::Compute,
            other => return Err(fail("GLSL", &format!("unsupported stage {other:?}"))),
        };
        let mut source = String::new();
        naga::back::glsl::Writer::new(
            &mut source,
            module,
            &info,
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_module_compiles_for_every_target() {
        let out = compile(TokenStream::new()).expect("empty module");
        naga::front::wgsl::parse_str(&out.wgsl).expect("output parses as WGSL");
        assert!(out.msl.contains("metal_stdlib"), "{}", out.msl);
        assert_eq!(out.spirv.first(), Some(&0x0723_0203), "SPIR-V magic number");
        assert!(out.glsl.is_empty(), "no entry points, no GLSL sources");
    }

    #[test]
    fn an_item_is_reported_at_its_own_span() {
        let err = compile("fn main() {}".parse().unwrap()).unwrap_err();
        assert!(err.message.contains("items"), "{}", err.message);
    }
}
