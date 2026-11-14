struct G { tint: vec4<f32> }
@group(0) @binding(0) var<uniform> globals: G;
@group(1) @binding(0) var atlas: texture_2d<f32>;
@group(1) @binding(1) var smp: sampler;
@vertex fn vs(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> { return vec4<f32>(f32(vi)); }
@fragment fn fs(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    return globals.tint * textureSample(atlas, smp, p.xy);
}
