// Far terrain: tiles of 16x16 column samples pulled from one storage buffer.
// The CPU meshes each tile into face words (far.rs `mesh`): tops merged along
// x, a wall from a sample down to each lower neighbour, and skirts at the
// tile edge, so neighbouring tiles of another level never show cracks. Each
// slot owns MAX_FACES face words; six vertices per face, and the vertex index
// (offset by the draw's first vertex) selects slot and face. Positions are
// camera-relative; the far projection has its own near/far planes and the
// pass clears its own depth.

struct Far {
    view_projection: mat4x4<f32>,
    // x: camera y, yz: camera x/z inside its chunk (0..16), w: skirt depth.
    camera: vec4<f32>,
    // xy: mask texel of the camera chunk, z: mask side in chunks, w: 1 = mask on.
    mask: vec4<i32>,
    // x, y: environmental fog start/end (spherical distance), z: daylight
    // brightness, w: 1 = linear output (Fancy: fog dissolves into the sky).
    fog: vec4<f32>,
    // x, y: render-distance fog start/end (cylindrical distance), z: fog
    // alpha, w: Fancy aerial-perspective coefficient (0 = none).
    edge: vec4<f32>,
    // rgb: fog colour, the non-Fancy fog target.
    fog_color: vec4<f32>,
    // xy: camera block x/z; slot origins subtract it in integers.
    anchor: vec4<i32>,
    // xy: camera position inside its block.
    fraction: vec4<f32>,
};

@group(0) @binding(0) var<uniform> far: Far;
@group(0) @binding(1) var<storage, read> samples: array<u32>;
@group(0) @binding(2) var mask: texture_2d<f32>;
@group(0) @binding(3) var<storage, read> faces: array<u32>;
// xy: tile origin (blocks), z: level.
@group(0) @binding(4) var<storage, read> slots: array<vec4<i32>>;

struct FarOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) relative: vec3<f32>,
    @location(1) color: vec3<f32>,
};

const EDGE: u32 = 16u;
// Face words per slot, set by the pipeline (far.rs `MAX_FACES`).
override MAX_FACES: u32;

fn word(slot: u32, i: u32, j: u32) -> u32 {
    return samples[(slot * EDGE * EDGE + j * EDGE + i) * 2u];
}

fn present(w: u32) -> bool {
    return ((w >> 16u) & 15u) != 0u;
}

// Top surface of the sample's top block.
fn top(w: u32) -> f32 {
    return f32(bitcast<i32>(w << 16u) >> 16u) + 1.0;
}

@vertex
fn far_vertex(@builtin(vertex_index) vertex: u32) -> FarOut {
    var out: FarOut;
    // Outside the clip volume: a whole quad of these is discarded.
    out.clip = vec4<f32>(0.0, 0.0, 2.0, 1.0);
    out.relative = vec3<f32>(0.0);
    out.color = vec3<f32>(0.0);
    let index = vertex / 6u;
    let corner = vertex % 6u;
    let slot = index / MAX_FACES;
    let packed = faces[index];
    let i = packed & 15u;
    let j = (packed >> 4u) & 15u;
    let face = (packed >> 8u) & 7u;
    let run = f32(((packed >> 11u) & 15u) + 1u);
    let info = slots[slot];
    let spacing = f32(1u << u32(info.z));
    let origin = vec2<f32>(info.xy - far.anchor.xy) - far.fraction.xy;
    let w = word(slot, i, j);
    let h = top(w);
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0),
    );
    let c = corners[corner];
    let x0 = origin.x + f32(i) * spacing;
    let z0 = origin.y + f32(j) * spacing;
    var p = vec3<f32>(x0 + c.x * spacing * run, h, z0 + c.y * spacing);
    var shade = 1.0;
    if face != 0u {
        var step = vec2<i32>(0, 0);
        switch face {
            case 1u: { step = vec2<i32>(1, 0); }
            case 2u: { step = vec2<i32>(-1, 0); }
            case 3u: { step = vec2<i32>(0, 1); }
            default: { step = vec2<i32>(0, -1); }
        }
        let n = vec2<i32>(i32(i), i32(j)) + step;
        var low = h - far.camera.w * max(1.0, spacing / 4.0);
        if n.x >= 0 && n.x < i32(EDGE) && n.y >= 0 && n.y < i32(EDGE) {
            let nw = word(slot, u32(n.x), u32(n.y));
            if present(nw) {
                low = top(nw);
            }
        }
        if low >= h {
            return out;
        }
        let y = mix(low, h, c.y);
        switch face {
            case 1u: { p = vec3<f32>(x0 + spacing, y, z0 + c.x * spacing); shade = 0.6; }
            case 2u: { p = vec3<f32>(x0, y, z0 + c.x * spacing); shade = 0.6; }
            case 3u: { p = vec3<f32>(x0 + c.x * spacing, y, z0 + spacing); shade = 0.8; }
            default: { p = vec3<f32>(x0 + c.x * spacing, y, z0); shade = 0.8; }
        }
    }
    let relative = vec3<f32>(p.x, p.y - far.camera.x, p.z);
    out.clip = far.view_projection * vec4<f32>(relative, 1.0);
    out.relative = relative;
    let rgba = unpack4x8unorm(samples[(slot * EDGE * EDGE + j * EDGE + i) * 2u + 1u]);
    out.color = rgba.rgb * shade;
    return out;
}

fn srgb_to_linear(color: vec3<f32>) -> vec3<f32> {
    return select(pow((max(color, vec3<f32>(0.0)) + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4)),
        color / 12.92, color <= vec3<f32>(0.04045));
}

fn linear_fog(distance: f32, start: f32, end: f32) -> f32 {
    if distance <= start { return 0.0; }
    if distance >= end { return 1.0; }
    return (distance - start) / (end - start);
}

@fragment
fn far_fragment(in: FarOut) -> @location(0) vec4<f32> {
    // Loaded chunk columns are the real terrain's: the far tile steps aside.
    if far.mask.w != 0 {
        let chunk = vec2<i32>(floor((far.camera.yz + in.relative.xz) / 16.0)) + far.mask.xy;
        if all(chunk >= vec2<i32>(0)) && all(chunk < vec2<i32>(far.mask.z)) {
            if textureLoad(mask, chunk, 0).r > 0.5 {
                discard;
            }
        }
    }
    var color = in.color * far.fog.z;
    // The loaded terrain's fog (shader.wgsl `fogged`), so both meet seamlessly.
    let spherical = length(in.relative);
    let cylindrical = max(length(in.relative.xz), abs(in.relative.y));
    let fog = max(linear_fog(spherical, far.fog.x, far.fog.y),
        linear_fog(cylindrical, far.edge.x, far.edge.y)) * far.edge.z;
    if far.fog.w > 0.5 {
        let aerial = (1.0 - exp(-spherical * far.edge.w)) * far.edge.z;
        let haze = max(max(fog, smoothstep(far.fog.y * 0.24, far.fog.y, spherical) * 0.72), aerial);
        // Fancy terrain fades into the atmosphere sky along its ray, which is
        // the sky pixel already drawn behind this pass.
        return vec4<f32>(srgb_to_linear(color), 1.0 - haze);
    }
    return vec4<f32>(mix(color, far.fog_color.rgb, fog), 1.0);
}
