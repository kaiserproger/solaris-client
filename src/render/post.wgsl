// Solaris HDR postprocess: Off passthrough copy; Fancy adds TAA resolve (High),
// half-resolution screen-space god rays, light bloom, ACES and sRGB encode.
//
// The world target is always Rgba16Float. Off mode writes display-encoded
// color into it and copies those texels through unchanged. Fancy mode writes
// linear HDR. Passes, in order (each optional per preset):
//   fs_taa      scene (+history, depth) -> resolved   temporal anti-aliasing
//   fs_rays     depth                   -> rays       radial shafts toward the sun, half res
//   fs_bloom_h  scene|resolved          -> bloom A    bright extract + horizontal blur
//   fs_bloom_v  bloom A                 -> bloom B
//   fs_combine  scene|resolved + bloom B + rays -> display (ACES + sRGB)
// UVs are derived from the fragment position (not NDC) so sampling matches
// texel orientation exactly; scene texels are fetched with textureLoad so HDR
// is never filtered or clamped before the tonemap.

struct Params {
    scene_texel: vec2<f32>,
    bloom_texel: vec2<f32>,
    // Unjittered inverse view-projection of this frame (camera-relative).
    inverse_vp: mat4x4<f32>,
    // Unjittered view-projection of the previous frame (previous-camera-relative).
    previous_vp: mat4x4<f32>,
    // xyz: current camera - previous camera (world); w: 1 when history is valid.
    camera_delta: vec4<f32>,
    // rgb: god-ray colour * strength; w: tap count (0 = pass off).
    ray_color: vec4<f32>,
    // xy: sun screen uv; z: 1 when the sun is in front of the camera.
    sun_uv: vec4<f32>,
    // xy: this frame's projection jitter in NDC.
    jitter: vec4<f32>,
};

@group(0) @binding(0) var scene_tex: texture_2d<f32>;
@group(0) @binding(1) var bloom_h_tex: texture_2d<f32>;
@group(0) @binding(2) var post_sampler: sampler;
@group(0) @binding(3) var<uniform> params: Params;
@group(0) @binding(4) var bloom_v_tex: texture_2d<f32>;
@group(0) @binding(5) var history_tex: texture_2d<f32>;
@group(0) @binding(6) var depth_tex: texture_depth_2d;
@group(0) @binding(7) var rays_tex: texture_2d<f32>;

const GAUSS_NORM_9: f32 = 4.89803;
const BLOOM_STRENGTH: f32 = 0.25;

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    var corners = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    return vec4<f32>(corners[vi], 0.0, 1.0);
}

// Off mode: exact texel copy, no filtering, no tonemap, no bloom draws.
@fragment
fn fs_copy(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    return textureLoad(scene_tex, vec2<i32>(floor(frag.xy)), 0);
}

fn luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

// ---- TAA ------------------------------------------------------------------

// Resolve in a luminance-compressed space so HDR highlights (sun disc, water
// glints) cannot dominate the neighbourhood statistics or leave fireflies.
fn compress(c: vec3<f32>) -> vec3<f32> { return c / (1.0 + luma(c)); }
fn decompress(c: vec3<f32>) -> vec3<f32> { return c / max(1.0 - luma(c), 0.001); }

fn history_tap(uv: vec2<f32>) -> vec3<f32> {
    return textureSampleLevel(history_tex, post_sampler, uv, 0.0).rgb;
}

// Catmull-Rom history fetch in five bilinear taps (keeps the history sharp).
fn history_catmull_rom(uv: vec2<f32>) -> vec3<f32> {
    let size = 1.0 / params.scene_texel;
    let position = uv * size;
    let center = floor(position - 0.5) + 0.5;
    let f = position - center;
    let w0 = f * (-0.5 + f * (1.0 - 0.5 * f));
    let w1 = 1.0 + f * f * (-2.5 + 1.5 * f);
    let w2 = f * (0.5 + f * (2.0 - 1.5 * f));
    let w3 = f * f * (-0.5 + 0.5 * f);
    let w12 = w1 + w2;
    let tc12 = (center + w2 / w12) * params.scene_texel;
    let tc0 = (center - 1.0) * params.scene_texel;
    let tc3 = (center + 2.0) * params.scene_texel;
    let weights = array<f32, 5>(w12.x * w0.y, w0.x * w12.y, w12.x * w12.y, w3.x * w12.y, w12.x * w3.y);
    var result = history_tap(vec2<f32>(tc12.x, tc0.y)) * weights[0]
        + history_tap(vec2<f32>(tc0.x, tc12.y)) * weights[1]
        + history_tap(tc12) * weights[2]
        + history_tap(vec2<f32>(tc3.x, tc12.y)) * weights[3]
        + history_tap(vec2<f32>(tc12.x, tc3.y)) * weights[4];
    result /= weights[0] + weights[1] + weights[2] + weights[3] + weights[4];
    return max(result, vec3<f32>(0.0));
}

@fragment
fn fs_taa(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let pixel = vec2<i32>(floor(frag.xy));
    let dims = vec2<i32>(textureDimensions(scene_tex));
    let current = textureLoad(scene_tex, pixel, 0).rgb;
    if params.camera_delta.w < 0.5 { return vec4<f32>(current, 1.0); }
    var m1 = vec3<f32>(0.0);
    var m2 = vec3<f32>(0.0);
    var closest = 1.0;
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let q = clamp(pixel + vec2<i32>(x, y), vec2<i32>(0), dims - vec2<i32>(1));
            let c = compress(textureLoad(scene_tex, q, 0).rgb);
            m1 += c;
            m2 += c * c;
            closest = min(closest, textureLoad(depth_tex, q, 0));
        }
    }
    let mean = m1 / 9.0;
    let sigma = sqrt(max(m2 / 9.0 - mean * mean, vec3<f32>(0.0)));
    // Reproject the nearest surface of the neighbourhood. The pixel sampled the
    // scene at its NDC minus this frame's jitter.
    let uv = frag.xy * params.scene_texel;
    let ndc = vec2<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0) - params.jitter.xy;
    let world = params.inverse_vp * vec4<f32>(ndc, closest, 1.0);
    let previous = params.previous_vp * vec4<f32>(world.xyz / world.w + params.camera_delta.xyz, 1.0);
    if previous.w <= 0.0 { return vec4<f32>(current, 1.0); }
    let previous_uv = (previous.xy / previous.w) * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5);
    if any(previous_uv < vec2<f32>(0.0)) || any(previous_uv > vec2<f32>(1.0)) {
        return vec4<f32>(current, 1.0);
    }
    let history = clamp(compress(history_catmull_rom(previous_uv)),
        mean - sigma * 1.25, mean + sigma * 1.25);
    // Faster convergence on moving pixels hides disocclusion trails.
    let speed = length((previous_uv - uv) / params.scene_texel);
    let blend = mix(0.1, 0.4, clamp(speed / 4.0, 0.0, 1.0));
    return vec4<f32>(decompress(mix(history, compress(current), blend)), 1.0);
}

// ---- God rays -----------------------------------------------------------------

fn interleaved_noise(position: vec2<f32>) -> f32 {
    return fract(52.9829189 * fract(dot(position, vec2<f32>(0.06711056, 0.00583715))));
}

// Half resolution: march from the pixel toward the sun's screen position and
// accumulate how much of that line is open sky (opaque depth at the far
// plane). Occluders such as trunks and leaves therefore carve shafts into the
// glow. Output is premultiplied by the shaft colour and strength.
@fragment
fn fs_rays(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let taps = i32(params.ray_color.w);
    if taps <= 0 || params.sun_uv.z < 0.5 { return vec4<f32>(0.0); }
    let uv = frag.xy * params.bloom_texel;
    let sun = params.sun_uv.xy;
    let delta = (sun - uv) * (0.9 / f32(taps));
    let dims = vec2<f32>(textureDimensions(depth_tex));
    var p = uv + delta * interleaved_noise(frag.xy);
    var open = 0.0;
    var total = 0.0;
    var weight = 1.0;
    for (var i = 0; i < taps; i++) {
        p += delta;
        let q = clamp(p, vec2<f32>(0.001), vec2<f32>(0.999));
        let sky = select(0.0, 1.0, textureLoad(depth_tex, vec2<i32>(q * dims), 0) >= 0.999999);
        open += sky * weight;
        total += weight;
        weight *= 0.965;
    }
    let fraction = open / total;
    let aspect = dims.x / dims.y;
    let falloff = exp(-length((sun - uv) * vec2<f32>(aspect, 1.0)) * 1.5);
    // Open sky keeps a soft glow; partial occlusion (the shafts) reads strongest.
    let shaft = fraction * (1.0 - 0.75 * fraction) * falloff;
    return vec4<f32>(params.ray_color.rgb * shaft, 1.0);
}

// ---- Bloom, tonemap --------------------------------------------------------------

fn bloom_bright(color: vec3<f32>) -> vec3<f32> {
    // HDR-only gate: display-range content never blooms.
    let weight = clamp((luma(color) - 1.0) / 0.5, 0.0, 1.0);
    return color * weight;
}

fn gauss(x: f32) -> f32 {
    return exp(-0.125 * x * x);
}

// Fancy pass (half res): bright-extract the full-res HDR scene with a
// separable horizontal blur into bloom A.
@fragment
fn fs_bloom_h(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let center_uv = frag.xy * params.bloom_texel;
    let step_uv = vec2<f32>(params.scene_texel.x * 2.0, 0.0);
    var acc = vec3<f32>(0.0);
    for (var i: i32 = -4; i <= 4; i = i + 1) {
        let tap = textureSample(scene_tex, post_sampler, center_uv + step_uv * f32(i)).rgb;
        acc = acc + bloom_bright(tap) * gauss(f32(i));
    }
    return vec4<f32>(acc / GAUSS_NORM_9, 1.0);
}

// Fancy pass (half res): vertical blur of bloom A into bloom B.
@fragment
fn fs_bloom_v(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let center_uv = frag.xy * params.bloom_texel;
    let step_uv = vec2<f32>(0.0, params.bloom_texel.y);
    var acc = vec3<f32>(0.0);
    for (var i: i32 = -4; i <= 4; i = i + 1) {
        acc = acc + textureSample(bloom_h_tex, post_sampler, center_uv + step_uv * f32(i)).rgb * gauss(f32(i));
    }
    return vec4<f32>(acc / GAUSS_NORM_9, 1.0);
}

fn aces_tonemap(x: vec3<f32>) -> vec3<f32> {
    // Narkowicz fit: keep positive HDR input, then bound the display-referred result.
    let c = max(x, vec3<f32>(0.0));
    return clamp((c * (c * 2.51 + vec3<f32>(0.03)))
        / (c * (c * 2.43 + vec3<f32>(0.59)) + vec3<f32>(0.14)), vec3<f32>(0.0), vec3<f32>(1.0));
}

fn srgb_encode(c: vec3<f32>) -> vec3<f32> {
    let low = c * 12.92;
    let high = 1.055 * pow(c, vec3<f32>(0.41666666)) - vec3<f32>(0.055);
    return select(high, low, c <= vec3<f32>(0.0031308));
}

// Add bloom (one bilinear tap) and god rays (one bilinear tap of the half-res
// shaft buffer; black when the preset has none) to the exact HDR scene texel in
// linear, then ACES + sRGB display encode. Alpha 1.
@fragment
fn fs_combine(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let base = textureLoad(scene_tex, vec2<i32>(floor(frag.xy)), 0).rgb;
    let center_uv = frag.xy * params.scene_texel;
    let blur = textureSample(bloom_v_tex, post_sampler, center_uv).rgb;
    let rays = textureSample(rays_tex, post_sampler, center_uv).rgb;
    let display = srgb_encode(aces_tonemap(base + blur * BLOOM_STRENGTH + rays));
    return vec4<f32>(display, 1.0);
}
