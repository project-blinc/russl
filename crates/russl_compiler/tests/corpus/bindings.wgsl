struct G { tint: vec4<f32> }
@group(0) @binding(0) var<uniform> globals: G;
@group(0) @binding(1) var<storage, read> items: array<vec4<f32>>;
@group(1) @binding(0) var atlas: texture_2d<f32>;
@group(1) @binding(1) var smp: sampler;
@group(2) @binding(0) var shadow: texture_depth_2d;
@group(2) @binding(1) var cmp: sampler_comparison;
@id(7) override gain: f32 = 1.0;
override soft: bool = true;

@vertex
fn vs(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    return items[vi] * globals.tint.x;
}

@fragment
fn fs(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    let lit = textureSampleCompare(shadow, cmp, p.xy, 0.5);
    var g = gain;
    if (!soft) { g = 1.0; }
    return globals.tint * textureSample(atlas, smp, p.xy) * lit * g;
}
