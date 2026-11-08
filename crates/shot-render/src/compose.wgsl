// Draws one source layer into the luma or the chroma plane of a biplanar 4:2:0 target.
//
// The source's YCbCr planes are sampled and turned into RGB with the source's matrix, the layer's
// rounded corners give coverage, and the color goes back to YCbCr in the target's range and
// matrix. Blending in YCbCr equals blending in RGB, because the conversion is affine. A layer can
// carry a border inside its edge and a drop shadow outside it, and an opacity over everything.

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
    // Block size in canvas pixels of a blurred layer, 0 for a sharp one.
    blur: f32,
    // Motion blur samples, 1 when the layer holds still.
    samples: f32,
    opacity: f32,
    // Luma weights Kr and Kb of the source matrix, then of the target matrix.
    weights: vec4<f32>,
    // How the crop changes over the shutter, in texture coordinates.
    shutter: vec4<f32>,
    // The border's gamma-encoded RGB, and its width in canvas pixels in w, 0 for none.
    border: vec4<f32>,
    // The drop shadow's offset in canvas pixels, its standard deviation and its alpha, 0 for none.
    shadow: vec4<f32>,
    // What the draw covers in canvas pixels: the frame, grown to hold the shadow.
    quad: vec4<f32>,
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
    // How this point's texture coordinate moves over the shutter.
    @location(2) motion: vec2<f32>,
}

// A triangle strip over the layer's quad. Texture coordinates run on past the frame's edges.
@vertex
fn vertex(@builtin(vertex_index) index: u32) -> Varyings {
    let corner = vec2<f32>(f32(index & 1u), f32((index >> 1u) & 1u));
    let canvas = mix(layer.quad.xy, layer.quad.zw, corner);
    let ndc = vec2<f32>(canvas.x / layer.canvas.x * 2.0 - 1.0, 1.0 - canvas.y / layer.canvas.y * 2.0);
    let at = (canvas - layer.frame.xy) / (layer.frame.zw - layer.frame.xy);
    let uv = mix(layer.crop.xy, layer.crop.zw, at);
    return Varyings(vec4<f32>(ndc, 0.0, 1.0), canvas, uv, mix(layer.shutter.xy, layer.shutter.zw, at));
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

// The mean of a block of the source, from 4x4 samples spread over it.
fn block_rgb(block: vec2<f32>, size: vec2<f32>) -> vec3<f32> {
    var sum = vec3<f32>(0.0);
    for (var i = 0; i < 4; i++) {
        for (var j = 0; j < 4; j++) {
            sum += source_rgb(block + (vec2<f32>(f32(i), f32(j)) + 0.5) / 4.0 * size);
        }
    }
    return sum / 16.0;
}

// The source averaged over blocks of `layer.blur` canvas pixels and eased between them, so no
// detail smaller than a block survives.
fn blurred_rgb(uv: vec2<f32>) -> vec3<f32> {
    let size = (layer.crop.zw - layer.crop.xy) / (layer.frame.zw - layer.frame.xy) * layer.blur;
    let cell = uv / size - 0.5;
    let base = floor(cell);
    let t = smoothstep(vec2<f32>(0.0), vec2<f32>(1.0), cell - base);
    let a = block_rgb(base * size, size);
    let b = block_rgb((base + vec2<f32>(1.0, 0.0)) * size, size);
    let c = block_rgb((base + vec2<f32>(0.0, 1.0)) * size, size);
    let d = block_rgb((base + vec2<f32>(1.0, 1.0)) * size, size);
    return mix(mix(a, b, t.x), mix(c, d, t.x), t.y);
}

// The source averaged over the shutter: evenly spaced samples along the point's path.
fn layer_rgb(uv: vec2<f32>, motion: vec2<f32>) -> vec3<f32> {
    if layer.blur > 0.0 {
        return blurred_rgb(uv);
    }
    let count = i32(layer.samples);
    if count <= 1 {
        return source_rgb(uv);
    }
    var sum = vec3<f32>(0.0);
    for (var i = 0; i < count; i++) {
        let at = f32(i) / f32(count - 1) - 0.5;
        sum += source_rgb(uv + motion * at);
    }
    return sum / f32(count);
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

// Signed distance from a canvas position to the rounded frame's edge, negative inside.
fn edge_distance(canvas: vec2<f32>) -> f32 {
    let center = (layer.frame.xy + layer.frame.zw) * 0.5;
    let half_size = (layer.frame.zw - layer.frame.xy) * 0.5;
    let q = abs(canvas - center) - half_size + vec2<f32>(layer.radius);
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - layer.radius;
}

// The layer's RGB at a point and its alpha: the source with its border, over its drop shadow,
// with antialiased edges. The shadow is black, its edge a gaussian of the shadow's deviation,
// through the logistic approximation of the normal distribution.
fn shaded(input: Varyings) -> vec4<f32> {
    let distance = edge_distance(input.canvas);
    let inside = clamp(0.5 - distance, 0.0, 1.0);
    var rgb = layer_rgb(input.uv, input.motion);
    if layer.border.w > 0.0 {
        rgb = mix(layer.border.rgb, rgb, clamp(0.5 - (distance + layer.border.w), 0.0, 1.0));
    }
    var alpha = inside;
    if layer.shadow.w > 0.0 {
        let away = edge_distance(input.canvas - layer.shadow.xy);
        let shadow = layer.shadow.w / (1.0 + exp(1.702 * away / max(layer.shadow.z, 0.5)));
        alpha = inside + shadow * (1.0 - inside);
        rgb = rgb * inside / max(alpha, 1e-5);
    }
    return vec4<f32>(rgb, alpha * layer.opacity);
}

@fragment
fn luma_plane(input: Varyings) -> @location(0) vec4<f32> {
    let color = shaded(input);
    return vec4<f32>(target_ycbcr(color.rgb).x, 0.0, 0.0, color.a);
}

// Runs once per 2x2 block of canvas pixels, at the block's center.
@fragment
fn chroma_plane(input: Varyings) -> @location(0) vec4<f32> {
    let color = shaded(input);
    let ycbcr = target_ycbcr(color.rgb);
    return vec4<f32>(ycbcr.y, ycbcr.z, 0.0, color.a);
}
