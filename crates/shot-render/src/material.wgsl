// Composites materials into the luma or the chroma plane of a biplanar 4:2:0 target.
//
// A sheet (mode 0) is a texture drawn flat and placed through a perspective transform per motion
// blur sample: each fragment maps back into the sheet with the sample's inverse, averages the
// content premultiplied, and lays it over frosted glass, the blurred backdrop tinted and lit, in
// the sheet's rounded shape. A shadow (mode 1) is a blurred coverage texture in one color.

struct Material {
    // The canvas rectangle this draw covers: left, top, right, bottom.
    bounds: vec4<f32>,
    canvas: vec2<f32>,
    // The sheet's size in its own pixels.
    size: vec2<f32>,
    // The content texture's size in texels.
    content: vec2<f32>,
    radius: f32,
    samples: f32,
    // Glass tint, or for a shadow its color.
    tint: vec4<f32>,
    // The canvas rectangle the backdrop or shadow texture covers.
    backdrop: vec4<f32>,
    weights: vec2<f32>,
    video_range: f32,
    // 1 when the sheet sits on glass.
    glass: f32,
    mode: f32,
    // How much the light brightens (positive) or darkens the sheet.
    sheen: f32,
    _pad: vec2<f32>,
    // Per sample.
    opacity: array<vec4<f32>, 2>,
    // Per sample, the rows of the map from canvas to sheet pixels, homogeneous.
    inverse: array<vec4<f32>, 24>,
}

@group(0) @binding(0) var content: texture_2d<f32>;
@group(0) @binding(1) var backdrop: texture_2d<f32>;
@group(0) @binding(2) var linear: sampler;
@group(0) @binding(3) var<uniform> material: Material;

struct Varyings {
    @builtin(position) position: vec4<f32>,
    @location(0) canvas: vec2<f32>,
}

@vertex
fn vertex(@builtin(vertex_index) index: u32) -> Varyings {
    let corner = vec2<f32>(f32(index & 1u), f32((index >> 1u) & 1u));
    let canvas = mix(material.bounds.xy, material.bounds.zw, corner);
    let ndc = vec2<f32>(canvas.x / material.canvas.x * 2.0 - 1.0, 1.0 - canvas.y / material.canvas.y * 2.0);
    return Varyings(vec4<f32>(ndc, 0.0, 1.0), canvas);
}

fn target_ycbcr(rgb: vec3<f32>) -> vec3<f32> {
    let k = material.weights;
    let y = dot(rgb, vec3<f32>(k.x, 1.0 - k.x - k.y, k.y));
    let c = vec2<f32>((rgb.b - y) / (2.0 * (1.0 - k.y)), (rgb.r - y) / (2.0 * (1.0 - k.x)));
    if material.video_range > 0.5 {
        return vec3<f32>(16.0 / 255.0 + y * (219.0 / 255.0), 128.0 / 255.0 + c * (224.0 / 255.0));
    }
    return vec3<f32>(y, c + vec2<f32>(128.0 / 255.0));
}

// Antialiased coverage of the sheet's rounded rectangle at a point in its own pixels.
fn coverage(local: vec2<f32>) -> f32 {
    let half_size = material.size * 0.5;
    let q = abs(local - half_size) - half_size + vec2<f32>(material.radius);
    let distance = length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - material.radius;
    return clamp(0.5 - distance, 0.0, 1.0);
}

// The content at a point in texels, bilinear over premultiplied texels: vello writes straight
// alpha, whose color is noise where alpha is near zero and must not bleed into its neighbors.
fn premultiplied(at: vec2<f32>) -> vec4<f32> {
    let p = at - vec2<f32>(0.5);
    let base = floor(p);
    let f = p - base;
    let last = vec2<i32>(material.content) - vec2<i32>(1);
    var texels: array<vec4<f32>, 4>;
    for (var i = 0; i < 4; i++) {
        let corner = vec2<i32>(base) + vec2<i32>(i & 1, i >> 1u);
        let t = textureLoad(content, clamp(corner, vec2<i32>(0), last), 0);
        texels[i] = vec4<f32>(t.rgb * t.a, t.a);
    }
    return mix(mix(texels[0], texels[1], f.x), mix(texels[2], texels[3], f.x), f.y);
}

fn backdrop_at(canvas: vec2<f32>) -> vec4<f32> {
    let uv = (canvas - material.backdrop.xy) / (material.backdrop.zw - material.backdrop.xy);
    return textureSampleLevel(backdrop, linear, uv, 0.0);
}

// Premultiplied color and coverage of the material at a canvas position.
fn shade(canvas: vec2<f32>) -> vec4<f32> {
    if material.mode > 0.5 {
        return vec4<f32>(material.tint.rgb, 1.0) * backdrop_at(canvas).a * material.tint.a;
    }
    let count = i32(material.samples);
    var ink = vec4<f32>(0.0);
    var shape = 0.0;
    for (var i = 0; i < count; i++) {
        let row = i * 3;
        let p = vec3<f32>(canvas, 1.0);
        let q = vec3<f32>(
            dot(material.inverse[row].xyz, p),
            dot(material.inverse[row + 1].xyz, p),
            dot(material.inverse[row + 2].xyz, p),
        );
        if q.z <= 0.0 {
            continue;
        }
        let local = q.xy / q.z;
        let opacity = material.opacity[i / 4][i % 4];
        ink += premultiplied(local + vec2<f32>(1.0)) * opacity;
        shape += coverage(local) * opacity;
    }
    ink /= f32(count);
    shape /= f32(count);
    let light = material.sheen;
    ink = vec4<f32>(clamp(ink.rgb * (1.0 + light * 0.6), vec3<f32>(0.0), vec3<f32>(ink.a)), ink.a);
    if material.glass < 0.5 {
        return ink;
    }
    let behind = backdrop_at(canvas).rgb;
    let tinted = mix(behind, material.tint.rgb, material.tint.a);
    let lit = clamp(tinted + vec3<f32>(light * 0.5), vec3<f32>(0.0), vec3<f32>(1.0));
    return ink + vec4<f32>(lit, 1.0) * shape * (1.0 - ink.a);
}

@fragment
fn luma_plane(input: Varyings) -> @location(0) vec4<f32> {
    let color = shade(input.canvas);
    if color.a <= 0.0 {
        return vec4<f32>(0.0);
    }
    return vec4<f32>(target_ycbcr(color.rgb / color.a).x, 0.0, 0.0, color.a);
}

// Runs once per 2x2 block of canvas pixels, at the block's center.
@fragment
fn chroma_plane(input: Varyings) -> @location(0) vec4<f32> {
    let color = shade(input.canvas);
    if color.a <= 0.0 {
        return vec4<f32>(0.5, 0.5, 0.0, 0.0);
    }
    let ycbcr = target_ycbcr(color.rgb / color.a);
    return vec4<f32>(ycbcr.y, ycbcr.z, 0.0, color.a);
}
