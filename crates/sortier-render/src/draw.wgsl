// Draws one texture (or one tile of it) as a quad onto the target, optionally over a checkerboard,
// as premultiplied linear colour; the pipeline blends it "over" what's already there.

struct Params {
    // Corners (x0, y0, x1, y1) in target pixels, already clipped to the target.
    dest: vec4<f32>,
    // Texture coordinates (u0, v0, u1, v1) at those corners.
    uv: vec4<f32>,
    target_size: vec2<f32>,
    opacity: f32,
    // Zero draws no checkerboard.
    checker_size: f32,
    checker_origin: vec2<f32>,
    padding: vec2<f32>,
    // Linear colours.
    checker_light: vec4<f32>,
    checker_dark: vec4<f32>,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var image_sampler: sampler;
@group(1) @binding(0) var image: texture_2d<f32>;

struct Vertex {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> Vertex {
    // Two triangles: corners (0,0) (1,0) (0,1), (0,1) (1,0) (1,1).
    let right = index == 1u || index == 4u || index == 5u;
    let bottom = index == 2u || index == 3u || index == 5u;
    let corner = vec2<bool>(right, bottom);
    // `select` rather than `mix`, so tiles that share an edge get bit-identical vertices there.
    let pixel = select(params.dest.xy, params.dest.zw, corner);
    let ndc = pixel / params.target_size * 2.0 - 1.0;
    var out: Vertex;
    out.position = vec4<f32>(ndc.x, -ndc.y, 0.0, 1.0);
    out.uv = select(params.uv.xy, params.uv.zw, corner);
    return out;
}

@fragment
fn fs_main(in: Vertex) -> @location(0) vec4<f32> {
    let color = textureSample(image, image_sampler, in.uv);
    var out = color;
    if params.checker_size > 0.0 {
        let cell = vec2<i32>(floor((in.position.xy - params.checker_origin) / params.checker_size));
        var back = params.checker_light.rgb;
        if ((cell.x + cell.y) & 1) != 0 {
            back = params.checker_dark.rgb;
        }
        out = vec4<f32>(color.rgb + back * (1.0 - color.a), 1.0);
    }
    return out * params.opacity;
}
