// Draws one source layer into the luma or the chroma plane of a biplanar 4:2:0 target.
//
// The source's YCbCr planes are sampled and turned into RGB with the source's matrix, the layer's
// rounded corners give coverage, and the color goes back to YCbCr in the target's range and
// matrix. Blending in YCbCr equals blending in RGB, because the conversion is affine.

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
    // Luma weights Kr and Kb of the source matrix, then of the target matrix.
    weights: vec4<f32>,
}

// RGB from normalized luma and centered chroma with luma weights Kr and Kb.
fn to_rgb(y: f32, c: vec2<f32>, k: vec2<f32>) -> vec3<f32> {
    let kg = 1.0 - k.x - k.y;
    let r = y + 2.0 * (1.0 - k.x) * c.y;
    let b = y + 2.0 * (1.0 - k.y) * c.x;
    return vec3<f32>(r, (y - k.x * r - k.y * b) / kg, b);
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
    return clamp(to_rgb(yn, cn, layer.weights.xy), vec3<f32>(0.0), vec3<f32>(1.0));
}

// Luma, Cb, Cr in the target's range.
fn target_ycbcr(rgb: vec3<f32>) -> vec3<f32> {
    let k = layer.weights.zw;
    let y = dot(rgb, vec3<f32>(k.x, 1.0 - k.x - k.y, k.y));
    let c = vec2<f32>((rgb.b - y) / (2.0 * (1.0 - k.y)), (rgb.r - y) / (2.0 * (1.0 - k.x)));
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
