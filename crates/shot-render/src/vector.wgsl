// Composites vello's RGBA rendering of a frame's drawing into the luma or the chroma plane of a
// biplanar 4:2:0 target, like compose.wgsl does for source layers.
//
// The texture holds straight-alpha sRGB with its origin at the quad's top left. Luma reads one
// texel per pixel; chroma averages the 2x2 block premultiplied, so edges keep their color.

struct Quad {
    // Destination in canvas pixels: left, top, right, bottom.
    rect: vec4<f32>,
    canvas: vec2<f32>,
    target_video_range: f32,
    _pad: f32,
    // Luma weights Kr and Kb of the target matrix.
    weights: vec2<f32>,
}

@group(0) @binding(0) var drawing: texture_2d<f32>;
@group(0) @binding(1) var<uniform> quad: Quad;

struct Varyings {
    @builtin(position) position: vec4<f32>,
    // Texel coordinates inside the drawing.
    @location(0) texel: vec2<f32>,
}

@vertex
fn vertex(@builtin(vertex_index) index: u32) -> Varyings {
    let corner = vec2<f32>(f32(index & 1u), f32((index >> 1u) & 1u));
    let canvas = mix(quad.rect.xy, quad.rect.zw, corner);
    let ndc = vec2<f32>(canvas.x / quad.canvas.x * 2.0 - 1.0, 1.0 - canvas.y / quad.canvas.y * 2.0);
    return Varyings(vec4<f32>(ndc, 0.0, 1.0), canvas - quad.rect.xy);
}

fn load(texel: vec2<f32>) -> vec4<f32> {
    let size = vec2<f32>(textureDimensions(drawing));
    let at = vec2<i32>(clamp(texel, vec2<f32>(0.0), size - vec2<f32>(1.0)));
    return textureLoad(drawing, at, 0);
}

fn target_ycbcr(rgb: vec3<f32>) -> vec3<f32> {
    let k = quad.weights;
    let y = dot(rgb, vec3<f32>(k.x, 1.0 - k.x - k.y, k.y));
    let c = vec2<f32>((rgb.b - y) / (2.0 * (1.0 - k.y)), (rgb.r - y) / (2.0 * (1.0 - k.x)));
    if quad.target_video_range > 0.5 {
        return vec3<f32>(16.0 / 255.0 + y * (219.0 / 255.0), 128.0 / 255.0 + c * (224.0 / 255.0));
    }
    return vec3<f32>(y, c + vec2<f32>(128.0 / 255.0));
}

@fragment
fn luma_plane(input: Varyings) -> @location(0) vec4<f32> {
    let texel = load(floor(input.texel));
    return vec4<f32>(target_ycbcr(texel.rgb).x, 0.0, 0.0, texel.a);
}

// Runs once per 2x2 block of canvas pixels; the block's texels start one below and left.
@fragment
fn chroma_plane(input: Varyings) -> @location(0) vec4<f32> {
    let base = floor(input.texel - vec2<f32>(0.5));
    var sum = vec4<f32>(0.0);
    for (var i = 0; i < 4; i++) {
        let t = load(base + vec2<f32>(f32(i & 1), f32(i >> 1u)));
        sum += vec4<f32>(t.rgb * t.a, t.a);
    }
    if sum.a <= 0.0 {
        return vec4<f32>(0.5, 0.5, 0.0, 0.0);
    }
    let ycbcr = target_ycbcr(sum.rgb / sum.a);
    return vec4<f32>(ycbcr.y, ycbcr.z, 0.0, sum.a / 4.0);
}
