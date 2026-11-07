// The gaussian blur of the material pass, at a reduced resolution.
//
// `extract` reads a region of the frame drawn so far from the target's luma and chroma planes,
// box filtered down by `scale`, as RGB: the backdrop of frosted glass. `gaussian` blurs a texture
// along one axis; a horizontal then a vertical pass make the 2D gaussian.

struct Blur {
    // Canvas position of the destination's top left, and canvas pixels per destination texel.
    origin: vec2<f32>,
    scale: f32,
    // 1 when the target is video range, 0 when full range.
    video_range: f32,
    // Luma weights Kr and Kb of the target matrix.
    weights: vec2<f32>,
    // One texel of the source along the blurred axis, in texture coordinates.
    step: vec2<f32>,
    // Standard deviation in source texels.
    sigma: f32,
    canvas_width: f32,
    canvas_height: f32,
    _pad: f32,
}

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var chroma: texture_2d<f32>;
@group(0) @binding(2) var linear: sampler;
@group(0) @binding(3) var<uniform> blur: Blur;

// A triangle that covers the destination.
@vertex
fn vertex(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4<f32>(uv * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), 0.0, 1.0);
}

fn to_rgb(at: vec2<f32>) -> vec3<f32> {
    let y = textureSampleLevel(source, linear, at, 0.0).r;
    let c = textureSampleLevel(chroma, linear, at, 0.0).rg - vec2<f32>(128.0 / 255.0);
    var yn = y;
    var cn = c;
    if blur.video_range > 0.5 {
        yn = (y - 16.0 / 255.0) * (255.0 / 219.0);
        cn = c * (255.0 / 224.0);
    }
    let k = blur.weights;
    let r = yn + 2.0 * (1.0 - k.x) * cn.y;
    let b = yn + 2.0 * (1.0 - k.y) * cn.x;
    let g = (yn - k.x * r - k.y * b) / (1.0 - k.x - k.y);
    return clamp(vec3<f32>(r, g, b), vec3<f32>(0.0), vec3<f32>(1.0));
}

// Four bilinear taps spread over the texel's footprint average it as a box.
@fragment
fn extract(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let canvas = vec2<f32>(blur.canvas_width, blur.canvas_height);
    let center = blur.origin + position.xy * blur.scale;
    let spread = blur.scale * 0.25;
    var sum = vec3<f32>(0.0);
    for (var i = 0; i < 4; i++) {
        let offset = vec2<f32>(f32(i & 1) - 0.5, f32(i >> 1u) - 0.5) * 2.0 * spread;
        sum += to_rgb((center + offset) / canvas);
    }
    return vec4<f32>(sum / 4.0, 1.0);
}

@fragment
fn gaussian(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let size = vec2<f32>(textureDimensions(source));
    let uv = position.xy / size;
    let reach = i32(ceil(blur.sigma * 3.0));
    var sum = vec4<f32>(0.0);
    var total = 0.0;
    for (var i = -reach; i <= reach; i++) {
        let x = f32(i);
        let w = exp(-x * x / (2.0 * blur.sigma * blur.sigma));
        sum += textureSampleLevel(source, linear, uv + blur.step * x, 0.0) * w;
        total += w;
    }
    return sum / total;
}
