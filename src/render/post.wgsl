// Solaris HDR postprocess: Off passthrough copy, Fancy ACES + light bloom.
//
// The world target is always Rgba16Float. Off mode writes display-encoded
// color into it and copies those texels through unchanged. Fancy mode writes
// linear HDR, resolves a half-resolution separable bloom (pass 1: bright
// extract + horizontal into bloom A; pass 2: vertical into bloom B; pass 3:
// combine + ACES + sRGB) and display-encodes on the way out. UVs are derived
// from the fragment position (not NDC) so sampling matches texel orientation
// exactly; the scene texel itself is always fetched with textureLoad so HDR
// is never filtered or clamped before the tonemap.

struct Params {
    scene_texel: vec2<f32>,
    bloom_texel: vec2<f32>,
};

@group(0) @binding(0) var scene_tex: texture_2d<f32>;
@group(0) @binding(1) var bloom_h_tex: texture_2d<f32>;
@group(0) @binding(2) var post_sampler: sampler;
@group(0) @binding(3) var<uniform> params: Params;
@group(0) @binding(4) var bloom_v_tex: texture_2d<f32>;

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

fn bloom_bright(color: vec3<f32>) -> vec3<f32> {
    let luma = dot(color, vec3<f32>(0.2126, 0.7152, 0.0722));
    // HDR-only gate: display-range content never blooms.
    let weight = clamp((luma - 1.0) / 0.5, 0.0, 1.0);
    return color * weight;
}

fn gauss(x: f32) -> f32 {
    return exp(-0.125 * x * x);
}

// Fancy pass 1 (half res): bright-extract the full-res HDR scene with a
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

// Fancy pass 2 (half res): vertical blur of bloom A into bloom B.
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

// Fancy pass 3: add the already-separated bloom (one bilinear tap) to the
// exact HDR scene texel in linear, then ACES + sRGB display encode. Alpha 1.
@fragment
fn fs_combine(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let base = textureLoad(scene_tex, vec2<i32>(floor(frag.xy)), 0).rgb;
    let center_uv = frag.xy * params.scene_texel;
    let blur = textureSample(bloom_v_tex, post_sampler, center_uv).rgb;
    let display = srgb_encode(aces_tonemap(base + blur * BLOOM_STRENGTH));
    return vec4<f32>(display, 1.0);
}
