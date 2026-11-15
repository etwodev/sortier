// Prepares uploaded textures: premultiplies straight RGBA into the first mip level, then fills each
// smaller level from the one above with a 2x2 box filter (one bilinear sample at the centre of each
// 2x2 block). Both read and write through sRGB views, so the maths happens in linear light.

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var source_sampler: sampler;

struct Vertex {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> Vertex {
    let corner = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    var out: Vertex;
    out.position = vec4<f32>(corner * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2<f32>(corner.x, 1.0 - corner.y);
    return out;
}

@fragment
fn fs_premultiply(in: Vertex) -> @location(0) vec4<f32> {
    let texel = textureLoad(source, vec2<i32>(in.position.xy), 0);
    return vec4<f32>(texel.rgb * texel.a, texel.a);
}

@fragment
fn fs_downsample(in: Vertex) -> @location(0) vec4<f32> {
    return textureSample(source, source_sampler, in.uv);
}
