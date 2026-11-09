//! How each native target binds the module's resources, so a backend that takes the native
//! artifacts directly binds what the shader reads.
//!
//! - Metal: wgpu's assignment, for a layout holding exactly the module's bindings and visible
//!   to every stage. Per stage, groups in order and bindings in order within one, buffers,
//!   textures and samplers each count from 0; the sizes buffer (lengths of runtime-sized
//!   arrays) follows the buffers when a storage buffer is bound, and always for a vertex stage.
//!   wgpu's passthrough shader modules take this MSL as it is.
//! - HLSL: naga's: a resource's register is its binding and its space its group. A sampler is
//!   an index (its binding) into its group's sampler index buffer at `t<group>, space255`, which
//!   selects from the sampler heap at `s0, space0` (comparison samplers `s0, space1`).
//! - SPIR-V: the descriptor set is the group and the binding is the binding.
//! - GLSL ES: combined texture-samplers and uniform blocks, bound by the names given here.

use naga::back::msl;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default)]
pub struct NativeBindings {
    /// Per entry point, as Metal sees each stage.
    pub msl: Vec<MslEntry>,
    pub hlsl: Vec<HlslBinding>,
    /// Per GLSL entry point.
    pub glsl: Vec<GlslBindings>,
}

#[derive(Debug, Clone)]
pub struct MslEntry {
    pub entry_point: String,
    pub resources: Vec<MslResource>,
    /// The buffer index of the sizes buffer, when the stage has one.
    pub sizes_buffer: Option<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MslResource {
    pub group: u32,
    pub binding: u32,
    pub slot: MslSlot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MslSlot {
    Buffer(u8),
    Texture(u8),
    Sampler(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HlslBinding {
    pub group: u32,
    pub binding: u32,
    pub slot: HlslSlot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HlslSlot {
    /// `register(<class><index>, space<space>)`: `b` constant buffers, `t` read-only
    /// resources, `u` writable ones.
    Register { class: char, index: u32, space: u8 },
    /// An index into the group's sampler index buffer.
    SamplerIndex { index: u32 },
}

#[derive(Debug, Clone, Default)]
pub struct GlslBindings {
    pub entry_point: String,
    /// Each combined sampler the GLSL declares.
    pub textures: Vec<GlslTexture>,
    /// Each uniform block: its name and the buffer's (group, binding).
    pub uniform_blocks: Vec<(String, (u32, u32))>,
}

/// A combined texture-sampler: the GLSL name, the texture's (group, binding), and the
/// sampler's when it has one.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct GlslTexture {
    pub name: String,
    pub texture: (u32, u32),
    pub sampler: Option<(u32, u32)>,
}

enum Class {
    Buffer { writable: bool, storage: bool },
    Texture { writable: bool },
    Sampler,
}

/// Every bound resource, by (group, binding), with what it is.
fn resources(module: &naga::Module) -> BTreeMap<(u32, u32), Class> {
    let mut out = BTreeMap::new();
    for (_, g) in module.global_variables.iter() {
        let Some(b) = &g.binding else { continue };
        let class = match (&module.types[g.ty].inner, g.space) {
            (naga::TypeInner::Sampler { .. }, _) => Class::Sampler,
            (naga::TypeInner::Image { class, .. }, _) => Class::Texture {
                writable: matches!(class, naga::ImageClass::Storage { access, .. } if access.contains(naga::StorageAccess::STORE)),
            },
            (_, naga::AddressSpace::Storage { access }) => Class::Buffer {
                writable: access.contains(naga::StorageAccess::STORE),
                storage: true,
            },
            _ => Class::Buffer {
                writable: false,
                storage: false,
            },
        };
        out.insert((b.group, b.binding), class);
    }
    out
}

/// The Metal binding map naga writes with, and the same as reflection.
pub(crate) fn msl_map(module: &naga::Module) -> (msl::EntryPointResourceMap, Vec<MslEntry>) {
    let all = resources(module);
    let storage = all
        .values()
        .any(|c| matches!(c, Class::Buffer { storage: true, .. }));
    let mut map = msl::EntryPointResourceMap::default();
    let mut entries = Vec::new();
    for ep in &module.entry_points {
        let (mut buffers, mut textures, mut samplers) = (0u8, 0u8, 0u8);
        let mut resources = msl::EntryPointResources::default();
        let mut listed = Vec::new();
        for (&(group, binding), class) in &all {
            let mut target = msl::BindTarget::default();
            let slot = match class {
                Class::Buffer { writable, .. } => {
                    target.buffer = Some(buffers);
                    target.mutable = *writable;
                    buffers += 1;
                    MslSlot::Buffer(buffers - 1)
                }
                Class::Texture { writable } => {
                    target.texture = Some(textures);
                    target.mutable = *writable;
                    textures += 1;
                    MslSlot::Texture(textures - 1)
                }
                Class::Sampler => {
                    target.sampler = Some(msl::BindSamplerTarget::Resource(samplers));
                    samplers += 1;
                    MslSlot::Sampler(samplers - 1)
                }
            };
            resources
                .resources
                .insert(naga::ResourceBinding { group, binding }, target);
            listed.push(MslResource {
                group,
                binding,
                slot,
            });
        }
        let sizes = (storage || ep.stage == naga::ShaderStage::Vertex).then_some(buffers);
        resources.sizes_buffer = sizes;
        map.insert(ep.name.clone(), resources);
        entries.push(MslEntry {
            entry_point: ep.name.clone(),
            resources: listed,
            sizes_buffer: sizes,
        });
    }
    (map, entries)
}

/// Where naga's HLSL puts each resource, with no binding map given.
pub(crate) fn hlsl(module: &naga::Module) -> Vec<HlslBinding> {
    resources(module)
        .into_iter()
        .map(|((group, binding), class)| {
            let register = |class| HlslSlot::Register {
                class,
                index: binding,
                space: group as u8,
            };
            let slot = match class {
                Class::Buffer { storage: false, .. } => register('b'),
                Class::Buffer {
                    writable: false, ..
                }
                | Class::Texture { writable: false } => register('t'),
                Class::Buffer { writable: true, .. } | Class::Texture { writable: true } => {
                    register('u')
                }
                Class::Sampler => HlslSlot::SamplerIndex { index: binding },
            };
            HlslBinding {
                group,
                binding,
                slot,
            }
        })
        .collect()
}

/// The names a GLSL entry point's textures and uniform blocks take.
pub(crate) fn glsl(
    module: &naga::Module,
    entry_point: &str,
    info: &naga::back::glsl::ReflectionInfo,
) -> GlslBindings {
    let at = |h: naga::Handle<naga::GlobalVariable>| {
        module.global_variables[h]
            .binding
            .as_ref()
            .map(|b| (b.group, b.binding))
    };
    let mut textures: Vec<_> = info
        .texture_mapping
        .iter()
        .filter_map(|(name, m)| {
            Some(GlslTexture {
                name: name.clone(),
                texture: at(m.texture)?,
                sampler: m.sampler.and_then(at),
            })
        })
        .collect();
    textures.sort();
    let mut uniform_blocks: Vec<_> = info
        .uniforms
        .iter()
        .filter_map(|(h, name)| Some((name.clone(), at(*h)?)))
        .collect();
    uniform_blocks.sort();
    GlslBindings {
        entry_point: entry_point.to_string(),
        textures,
        uniform_blocks,
    }
}
