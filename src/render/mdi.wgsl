// Authored implementation. Match visibility::Frustum's WebGPU 0..w planes
// and conservative epsilon; unknown source bounds must never hide terrain.
//
// Three entry points share one layout:
//   main   : frustum only (Hi-Z disabled).
//   phase1 : frustum && section was visible last frame (drawn before the
//            pyramid exists, so it can only over-draw, never miss terrain).
//   phase2 : after the pyramid is built from phase-1 depth, re-test every
//            candidate against it. Sections not drawn in phase 1 that pass are
//            drawn now; the verdict becomes next frame's history.
struct Candidate {
    min: vec4<f32>, // w: history slot (bitcast u32)
    max: vec4<f32>,
    draw: vec4<u32>,
}
struct DrawIndexed {
    index_count: u32,
    instance_count: u32,
    first_index: u32,
    base_vertex: i32,
    first_instance: u32,
}
struct Params {
    planes: array<vec4<f32>, 6>,
    // x: candidate count, y: counter base (words), z: history slot mask.
    count: vec4<u32>,
    view_projection: mat4x4<f32>,
    // x,y: depth buffer size in pixels, z: pyramid mip count, w: depth margin.
    screen: vec4<f32>,
}
@group(0) @binding(0) var<storage, read> candidates: array<Candidate>;
@group(0) @binding(1) var<storage, read_write> draws: array<DrawIndexed>;
@group(0) @binding(2) var<uniform> params: Params;
@group(0) @binding(3) var<storage, read> history_prev: array<u32>;
@group(0) @binding(4) var<storage, read_write> history_next: array<atomic<u32>>;
@group(0) @binding(5) var pyramid: texture_2d<f32>;
// Per material, 8 words: phase-1 draws/indices, phase-2 draws/indices,
// hi-z-rejected draws/indices, late (drawn in phase 1 but now hidden) draws/indices.
@group(0) @binding(6) var<storage, read_write> counters: array<atomic<u32>>;

fn frustum_visible(c: Candidate) -> bool {
    // !(x <= y) also admits NaN conservatively, like the CPU fallback.
    let finite = all(abs(c.min.xyz) <= vec3<f32>(3.402823e38))
        && all(abs(c.max.xyz) <= vec3<f32>(3.402823e38));
    if finite && all(c.min.xyz <= c.max.xyz) {
        for (var p = 0u; p < 6u; p += 1u) {
            let plane = params.planes[p];
            let positive = select(c.min.xyz, c.max.xyz, plane.xyz >= vec3<f32>(0.));
            if dot(plane.xyz, positive) + plane.w < -0.01 { return false; }
        }
    }
    return true;
}

fn slot_of(c: Candidate) -> u32 {
    return bitcast<u32>(c.min.w) & params.count.z;
}

// True only when every pixel the box can cover holds geometry strictly nearer
// than the box's nearest point. Anything uncertain (NaN, box reaching the near
// plane or the eye, off-screen rectangle) is "not occluded".
fn hiz_occluded(c: Candidate) -> bool {
    let finite = all(abs(c.min.xyz) <= vec3<f32>(3.402823e38))
        && all(abs(c.max.xyz) <= vec3<f32>(3.402823e38));
    if !(finite && all(c.min.xyz <= c.max.xyz)) { return false; }
    var nearest = 1.0e9;
    var lo = vec2<f32>(1.0e9);
    var hi = vec2<f32>(-1.0e9);
    for (var k = 0u; k < 8u; k += 1u) {
        let p = vec3<f32>(
            select(c.min.x, c.max.x, (k & 1u) != 0u),
            select(c.min.y, c.max.y, (k & 2u) != 0u),
            select(c.min.z, c.max.z, (k & 4u) != 0u),
        );
        let clip = params.view_projection * vec4<f32>(p, 1.0);
        if clip.w <= 0.05 { return false; }
        let ndc = clip.xyz / clip.w;
        nearest = min(nearest, ndc.z);
        let uv = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
        lo = min(lo, uv);
        hi = max(hi, uv);
    }
    if nearest <= 0.0 { return false; }
    let size = params.screen.xy;
    // Half a pixel of slack each side covers rasterisation coverage rules.
    let lo_px = lo * size - vec2<f32>(0.5);
    let hi_px = hi * size + vec2<f32>(0.5);
    if any(hi_px < vec2<f32>(0.)) || any(lo_px > size) { return false; }
    let base = textureDimensions(pyramid, 0);
    let top = vec2<f32>(base - vec2<u32>(1u));
    // Pyramid texel = 2x2 depth pixels; the last texel also owns an odd tail.
    let a = vec2<u32>(clamp(floor(lo_px * 0.5), vec2<f32>(0.), top));
    let b = vec2<u32>(clamp(floor(hi_px * 0.5), vec2<f32>(0.), top));
    let levels = u32(params.screen.z);
    var level = 0u;
    while level + 1u < levels
        && (((b.x >> level) - (a.x >> level)) > 1u || ((b.y >> level) - (a.y >> level)) > 1u) {
        level += 1u;
    }
    let dims = textureDimensions(pyramid, level);
    let x0 = min(a.x >> level, dims.x - 1u);
    let x1 = min(b.x >> level, dims.x - 1u);
    let y0 = min(a.y >> level, dims.y - 1u);
    let y1 = min(b.y >> level, dims.y - 1u);
    var farthest = textureLoad(pyramid, vec2<u32>(x0, y0), level).r;
    farthest = max(farthest, textureLoad(pyramid, vec2<u32>(x1, y0), level).r);
    farthest = max(farthest, textureLoad(pyramid, vec2<u32>(x0, y1), level).r);
    farthest = max(farthest, textureLoad(pyramid, vec2<u32>(x1, y1), level).r);
    return nearest > farthest + params.screen.w;
}

fn write_draw(i: u32, c: Candidate, visible: bool) {
    draws[i] = DrawIndexed(c.draw.x, select(0u, 1u, visible), c.draw.y, bitcast<i32>(c.draw.z), c.draw.w);
}

fn tally(word: u32, indices: u32) {
    let base = params.count.y + word;
    atomicAdd(&counters[base], 1u);
    atomicAdd(&counters[base + 1u], indices);
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x;
    if i >= params.count.x { return; }
    let c = candidates[i];
    let visible = frustum_visible(c);
    if visible { tally(0u, c.draw.x); }
    write_draw(i, c, visible);
}

@compute @workgroup_size(64)
fn phase1(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x;
    if i >= params.count.x { return; }
    let c = candidates[i];
    let visible = history_prev[slot_of(c)] != 0u && frustum_visible(c);
    if visible { tally(0u, c.draw.x); }
    write_draw(i, c, visible);
}

@compute @workgroup_size(64)
fn phase2(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x;
    if i >= params.count.x { return; }
    let c = candidates[i];
    var draw = false;
    if frustum_visible(c) {
        let drawn_in_phase1 = history_prev[slot_of(c)] != 0u;
        if hiz_occluded(c) {
            if drawn_in_phase1 { tally(6u, c.draw.x); } else { tally(4u, c.draw.x); }
        } else {
            atomicOr(&history_next[slot_of(c)], 1u);
            draw = !drawn_in_phase1;
        }
    }
    if draw { tally(2u, c.draw.x); }
    write_draw(i, c, draw);
}
