// Draws one source layer into the luma or the chroma plane of a biplanar 4:2:0 target.
//
// The source's YCbCr planes are sampled and turned into BT.709 RGB, the layer's rounded corners
// give coverage, and the color goes back to BT.709 YCbCr in the target's range. Blending in
// YCbCr equals blending in RGB, because the conversion is affine.

struct Layer {
    // Destination in canvas pixels: left, top, right, bottom.
    frame: vec4<f32>,
    // Source rectangle in texture coordinates.
    crop: vec4<f32>,
    canvas: vec2<f32>,
    radius: f32,
    // 1 when the source is video range, 0 when full range.
    source_video_range: f32,
    // Same for the target.
    target_video_range: f32,
}

@group(0) @binding(0) var luma: texture_2d<f32>;
@group(0) @binding(1) var chroma: texture_2d<f32>;
@group(0) @binding(2) var bilinear: sampler;
@group(0) @binding(3) var<uniform> layer: Layer;

struct Varyings {
    @builtin(position) position: vec4<f32>,
    @location(0) canvas: vec2<f32>,
    @location(1) uv: vec2<f32>,
}

// A triangle strip over the layer's frame.
@vertex
fn vertex(@builtin(vertex_index) index: u32) -> Varyings {
    let corner = vec2<f32>(f32(index & 1u), f32((index >> 1u) & 1u));
    let canvas = mix(layer.frame.xy, layer.frame.zw, corner);
    let ndc = vec2<f32>(canvas.x / layer.canvas.x * 2.0 - 1.0, 1.0 - canvas.y / layer.canvas.y * 2.0);
    return Varyings(vec4<f32>(ndc, 0.0, 1.0), canvas, mix(layer.crop.xy, layer.crop.zw, corner));
}

fn source_rgb(uv: vec2<f32>) -> vec3<f32> {
    let y = textureSample(luma, bilinear, uv).r;
    let c = textureSample(chroma, bilinear, uv).rg - vec2<f32>(128.0 / 255.0);
    var yn = y;
    var cn = c;
    if layer.source_video_range > 0.5 {
        yn = (y - 16.0 / 255.0) * (255.0 / 219.0);
        cn = c * (255.0 / 224.0);
    }
    let rgb = vec3<f32>(
        yn + 1.5748 * cn.y,
        yn - 0.187324 * cn.x - 0.468124 * cn.y,
        yn + 1.8556 * cn.x,
    );
    return clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0));
}

// Luma, Cb, Cr in the target's range.
fn target_ycbcr(rgb: vec3<f32>) -> vec3<f32> {
    let y = dot(rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
    let c = vec2<f32>((rgb.b - y) / 1.8556, (rgb.r - y) / 1.5748);
    if layer.target_video_range > 0.5 {
        return vec3<f32>(16.0 / 255.0 + y * (219.0 / 255.0), 128.0 / 255.0 + c * (224.0 / 255.0));
    }
    return vec3<f32>(y, c + vec2<f32>(128.0 / 255.0));
}

// Antialiased coverage of the rounded frame at a canvas position.
fn coverage(canvas: vec2<f32>) -> f32 {
    let center = (layer.frame.xy + layer.frame.zw) * 0.5;
    let half_size = (layer.frame.zw - layer.frame.xy) * 0.5;
    let q = abs(canvas - center) - half_size + vec2<f32>(layer.radius);
    let distance = length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - layer.radius;
    return clamp(0.5 - distance, 0.0, 1.0);
}

@fragment
fn luma_plane(input: Varyings) -> @location(0) vec4<f32> {
    let ycbcr = target_ycbcr(source_rgb(input.uv));
    return vec4<f32>(ycbcr.x, 0.0, 0.0, coverage(input.canvas));
}

// Runs once per 2x2 block of canvas pixels, at the block's center.
@fragment
fn chroma_plane(input: Varyings) -> @location(0) vec4<f32> {
    let ycbcr = target_ycbcr(source_rgb(input.uv));
    return vec4<f32>(ycbcr.y, ycbcr.z, 0.0, coverage(input.canvas));
}
