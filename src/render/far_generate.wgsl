// Far-terrain generation on the GPU: the server's LOD program
// (`far/program.rs`, the CPU reference) evaluated in f32, one invocation per
// column. Output per column: top solid block Y (i32 bits), biome name index.
//
// `data` holds every table as 32-bit words (offsets in `params`):
// octave = 64 packed permutation words + xo, yo, zo; normal = value factor,
// then two Perlin sets of (input, value, level offset, level count); level =
// amplitude, octave (0xffffffff = none); blended = 40 octaves + 5 scales;
// instruction = op, 4 args, 2 constants, pad; point = location, derivative,
// register flag, value; biome row = 14 bounds + biome.

struct Params {
    // min_y, height, cell_width, cell_height
    geometry: vec4<i32>,
    // instructions offset, instruction count, points offset, flat_len
    surface: vec4<u32>,
    // instructions offset, instruction count, points offset, surface root
    climate: vec4<u32>,
    // climate roots 0..3
    roots_a: vec4<u32>,
    // climate roots 4, 5, rows offset, row count
    roots_b: vec4<u32>,
    // octaves offset, normals offset, blended offset, job count
    tables: vec4<u32>,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read> data: array<u32>;
// Tile level, x, z per job.
@group(0) @binding(2) var<storage, read> jobs: array<vec4<i32>>;
@group(0) @binding(3) var<storage, read_write> out: array<u32>;

const REGISTERS: u32 = REGISTER_COUNTu;
var<private> regs: array<f32, REGISTERS>;
// Quantized climate of the column; the offset target is 0.
var<private> climate_target: array<i32, 7>;

var<private> GRADIENTS: array<vec3<f32>, 16> = array<vec3<f32>, 16>(
    vec3(1.0, 1.0, 0.0), vec3(-1.0, 1.0, 0.0), vec3(1.0, -1.0, 0.0), vec3(-1.0, -1.0, 0.0),
    vec3(1.0, 0.0, 1.0), vec3(-1.0, 0.0, 1.0), vec3(1.0, 0.0, -1.0), vec3(-1.0, 0.0, -1.0),
    vec3(0.0, 1.0, 1.0), vec3(0.0, -1.0, 1.0), vec3(0.0, 1.0, -1.0), vec3(0.0, -1.0, -1.0),
    vec3(1.0, 1.0, 0.0), vec3(0.0, -1.0, 1.0), vec3(-1.0, 1.0, 0.0), vec3(0.0, -1.0, -1.0),
);

fn f(index: u32) -> f32 {
    return bitcast<f32>(data[index]);
}

fn perm(octave: u32, n: i32) -> i32 {
    let i = u32(n & 255);
    return i32((data[octave + (i >> 2u)] >> ((i & 3u) * 8u)) & 255u);
}

fn grad(hash: i32, x: f32, y: f32, z: f32) -> f32 {
    let g = GRADIENTS[hash & 15];
    return g.x * x + g.y * y + g.z * z;
}

fn fade(v: f32) -> f32 {
    return v * v * v * (v * (v * 6.0 - 15.0) + 10.0);
}

fn wrap(v: f32) -> f32 {
    return v - floor(v / 33554432.0 + 0.5) * 33554432.0;
}

// Vanilla ImprovedNoise with its Y smear; `octave` is the word offset.
fn improved(octave: u32, sx: f32, sy: f32, sz: f32, y_scale: f32, y_max: f32) -> f32 {
    let x = sx + f(octave + 64u);
    let y = sy + f(octave + 65u);
    let z = sz + f(octave + 66u);
    let fx0 = floor(x);
    let fy0 = floor(y);
    let fz0 = floor(z);
    let ix = i32(fx0);
    let iy = i32(fy0);
    let iz = i32(fz0);
    let xr = x - fx0;
    let yr = y - fy0;
    let zr = z - fz0;
    var delta = 0.0;
    if y_scale != 0.0 {
        var limited = yr;
        if y_max >= 0.0 && y_max < yr {
            limited = y_max;
        }
        delta = floor(limited / y_scale + 1.0000000116860974e-7) * y_scale;
    }
    let gy = yr - delta;
    let x0 = perm(octave, ix);
    let x1 = perm(octave, ix + 1);
    let a = perm(octave, x0 + iy);
    let b = perm(octave, x0 + iy + 1);
    let c = perm(octave, x1 + iy);
    let d = perm(octave, x1 + iy + 1);
    let fx = fade(xr);
    let fy = fade(yr);
    let fz = fade(zr);
    let lower = lerp(
        fy,
        lerp(fx, grad(perm(octave, a + iz), xr, gy, zr), grad(perm(octave, c + iz), xr - 1.0, gy, zr)),
        lerp(fx, grad(perm(octave, b + iz), xr, gy - 1.0, zr), grad(perm(octave, d + iz), xr - 1.0, gy - 1.0, zr)),
    );
    let upper = lerp(
        fy,
        lerp(fx, grad(perm(octave, a + iz + 1), xr, gy, zr - 1.0), grad(perm(octave, c + iz + 1), xr - 1.0, gy, zr - 1.0)),
        lerp(fx, grad(perm(octave, b + iz + 1), xr, gy - 1.0, zr - 1.0), grad(perm(octave, d + iz + 1), xr - 1.0, gy - 1.0, zr - 1.0)),
    );
    return lerp(fz, lower, upper);
}

// Vanilla `Mth.lerp` operand order (not WGSL `mix`).
fn lerp(t: f32, a: f32, b: f32) -> f32 {
    return a + t * (b - a);
}

fn octave_offset(id: u32) -> u32 {
    return params.tables.x + id * 67u;
}

// One Perlin set at word `base`: input, value, level offset, level count.
fn perlin(base: u32, x: f32, y: f32, z: f32) -> f32 {
    var input = f(base);
    var weight = f(base + 1u);
    let levels = data[base + 2u];
    let count = data[base + 3u];
    var sum = 0.0;
    for (var i = 0u; i < count; i++) {
        let id = data[levels + i * 2u + 1u];
        if id != 0xffffffffu {
            sum += f(levels + i * 2u) * improved(
                octave_offset(id), wrap(x * input), wrap(y * input), wrap(z * input), 0.0, 0.0,
            ) * weight;
        }
        input *= 2.0;
        weight /= 2.0;
    }
    return sum;
}

fn normal(id: u32, x: f32, y: f32, z: f32) -> f32 {
    let base = params.tables.y + id * 9u;
    let factor = 1.0181268882175227;
    return (perlin(base + 1u, x, y, z) + perlin(base + 5u, x * factor, y * factor, z * factor)) * f(base);
}

fn blended(id: u32, px: i32, py: i32, pz: i32) -> f32 {
    let base = params.tables.z + id * 45u;
    let xz = f(base + 40u);
    let ys = f(base + 41u);
    let xz_factor = f(base + 42u);
    let y_factor = f(base + 43u);
    let smear = ys * f(base + 44u);
    let x = f32(px) * xz;
    let y = f32(py) * ys;
    let z = f32(pz) * xz;
    let mx = x / xz_factor;
    let my = y / y_factor;
    let mz = z / xz_factor;
    let main_smear = smear / y_factor;
    var main = 0.0;
    var scale = 1.0;
    for (var i = 32u; i < 40u; i++) {
        main += improved(
            octave_offset(data[base + i]), wrap(mx * scale), wrap(my * scale), wrap(mz * scale),
            main_smear * scale, my * scale,
        ) / scale;
        scale /= 2.0;
    }
    let alpha = (main / 10.0 + 1.0) / 2.0;
    var low = 0.0;
    var high = 0.0;
    scale = 1.0;
    for (var i = 0u; i < 16u; i++) {
        let sx = wrap(x * scale);
        let sy = wrap(y * scale);
        let sz = wrap(z * scale);
        if alpha < 1.0 {
            low += improved(octave_offset(data[base + i]), sx, sy, sz, smear * scale, y * scale) / scale;
        }
        if alpha > 0.0 {
            high += improved(octave_offset(data[base + 16u + i]), sx, sy, sz, smear * scale, y * scale) / scale;
        }
        scale /= 2.0;
    }
    let t = clamp(alpha, 0.0, 1.0);
    return (low / 512.0 + t * (high / 512.0 - low / 512.0)) / 128.0;
}

fn point_value(at: u32) -> f32 {
    if data[at + 2u] != 0u {
        return regs[data[at + 3u]];
    }
    return f(at + 3u);
}

// Vanilla's f32 cubic spline step over already evaluated point values.
fn spline(points: u32, input: f32, first: u32, count: u32) -> f32 {
    let base = points + first * 4u;
    var next = 0u;
    loop {
        if next >= count || f(base + next * 4u) > input {
            break;
        }
        next++;
    }
    if next == 0u || next == count {
        var edge = base;
        if next != 0u {
            edge = base + (count - 1u) * 4u;
        }
        let v = point_value(edge);
        let derivative = f(edge + 1u);
        if derivative == 0.0 {
            return v;
        }
        return v + derivative * (input - f(edge));
    }
    let a = base + (next - 1u) * 4u;
    let b = base + next * 4u;
    let width = f(b) - f(a);
    let t = (input - f(a)) / width;
    let y1 = point_value(a);
    let y2 = point_value(b);
    let da = f(a + 1u) * width - (y2 - y1);
    let db = -f(b + 1u) * width + (y2 - y1);
    return y1 + t * (y2 - y1) + t * (1.0 - t) * (da + t * (db - da));
}

fn rarity(v: f32, type_2: bool) -> f32 {
    if type_2 {
        if v < -0.75 { return 0.5; }
        if v < -0.5 { return 0.75; }
        if v < 0.5 { return 1.0; }
        if v < 0.75 { return 2.0; }
        return 3.0;
    }
    if v < -0.5 { return 0.75; }
    if v < 0.0 { return 1.0; }
    if v < 0.5 { return 1.5; }
    return 2.0;
}

// Instructions [start, end) of the code at `insts` (points at `points`).
fn run(insts: u32, points: u32, start: u32, end: u32, p: vec3<i32>) {
    for (var i = start; i < end; i++) {
        let w = insts + i * 8u;
        let op = data[w];
        let a = data[w + 1u];
        let b = data[w + 2u];
        let c = data[w + 3u];
        let d = data[w + 4u];
        let c0 = f(w + 5u);
        let c1 = f(w + 6u);
        var v = 0.0;
        switch op {
            case 0u: { v = c0; }
            case 1u: { v = regs[a] + regs[b]; }
            case 2u: {
                let left = regs[a];
                if left != 0.0 { v = left * regs[b]; }
            }
            case 3u: { v = min(regs[a], regs[b]); }
            case 4u: { v = max(regs[a], regs[b]); }
            case 5u: { v = abs(regs[a]); }
            case 6u: { v = regs[a] * regs[a]; }
            case 7u: { v = regs[a] * regs[a] * regs[a]; }
            case 8u: { v = select(regs[a] * 0.5, regs[a], regs[a] > 0.0); }
            case 9u: { v = select(regs[a] * 0.25, regs[a], regs[a] > 0.0); }
            case 10u: { v = 1.0 / regs[a]; }
            case 11u: {
                let s = clamp(regs[a], -1.0, 1.0);
                v = s / 2.0 - s * s * s / 24.0;
            }
            case 12u: { v = clamp(regs[a], c0, c1); }
            case 13u: {
                let from_y = f32(bitcast<i32>(a));
                let to_y = f32(bitcast<i32>(b));
                let t = clamp((f32(p.y) - from_y) / (to_y - from_y), 0.0, 1.0);
                v = c0 + t * (c1 - c0);
            }
            case 14u: { v = normal(a, f32(p.x) * c0, f32(p.y) * c1, f32(p.z) * c0); }
            case 15u: {
                var q = p;
                if b == 1u { q = vec3(p.x, 0, p.z); }
                if b == 2u { q = vec3(p.z, p.x, 0); }
                v = normal(a, f32(q.x) * 0.25, f32(q.y) * 0.25, f32(q.z) * 0.25) * 4.0;
            }
            case 16u: {
                v = normal(a, f32(p.x) * c0 + regs[b], f32(p.y) * c1 + regs[c], f32(p.z) * c0 + regs[d]);
            }
            case 17u: {
                let r = rarity(regs[b], c != 0u);
                v = r * abs(normal(a, f32(p.x) / r, f32(p.y) / r, f32(p.z) / r));
            }
            case 18u: {
                let input = regs[a];
                v = select(regs[c], regs[b], input >= c0 && input < c1);
            }
            case 19u: { v = spline(points, regs[a], b, c); }
            case 20u: { v = blended(a, p.x, p.y, p.z); }
            default: {}
        }
        regs[i] = v;
    }
}

// Squared climate distance to one row as two 32-bit halves (no 64-bit
// integers in WGSL); every term is below 2^32 for |values| < 65536.
fn climate_distance(row: u32) -> vec2<u32> {
    var high = 0u;
    var low = 0u;
    for (var i = 0u; i < 7u; i++) {
        let lo = bitcast<i32>(data[row + i * 2u]);
        let hi = bitcast<i32>(data[row + i * 2u + 1u]);
        let d = u32(max(max(climate_target[i] - hi, lo - climate_target[i]), 0));
        let square = d * d;
        let sum = low + square;
        high += select(0u, 1u, sum < low);
        low = sum;
    }
    return vec2(high, low);
}

@compute @workgroup_size(64)
fn generate(@builtin(global_invocation_id) id: vec3<u32>) {
    let job = id.x / 256u;
    let column = id.x % 256u;
    if job >= params.tables.w {
        return;
    }
    let key = jobs[job];
    let spacing = 1 << u32(key.x);
    let x = key.y * 16 * spacing + i32(column % 16u) * spacing;
    let z = key.z * 16 * spacing + i32(column / 16u) * spacing;
    let width = params.geometry.z;
    let height = params.geometry.w;
    let cx = (x - (((x % width) + width) % width));
    let cz = (z - (((z % width) + width) % width));
    let flat_x = cx - (((cx % 4) + 4) % 4);
    let flat_z = cz - (((cz % 4) + 4) % 4);
    let surface = params.surface;
    run(surface.x, surface.z, 0u, surface.w, vec3(flat_x, 0, flat_z));
    let min_y = params.geometry.x;
    let max_y = min_y + params.geometry.y;
    let root = params.climate.w;
    var top = min_y - 1;
    var have_upper = false;
    var upper = 0.0;
    let floor_y = min_y - (((min_y % height) + height) % height);
    let top_cell = max_y - 1 - (((max_y - 1) % height + height) % height);
    var y = top_cell + height;
    loop {
        if y < floor_y {
            break;
        }
        run(surface.x, surface.z, surface.w, surface.y, vec3(cx, y, cz));
        let value = regs[root];
        if value > 0.0 {
            if !have_upper {
                top = max_y - 1;
            } else {
                var dy = height - 1;
                loop {
                    if dy <= 0 || value + f32(dy) / f32(height) * (upper - value) > 0.0 {
                        break;
                    }
                    dy--;
                }
                top = min(y + max(dy, 0), max_y - 1);
            }
            break;
        }
        upper = value;
        have_upper = true;
        y -= height;
    }
    top = clamp(top, min_y, max_y - 1);
    let quart = vec3(
        x - (((x % 4) + 4) % 4),
        top - (((top % 4) + 4) % 4),
        z - (((z % 4) + 4) % 4),
    );
    let climate = params.climate;
    run(climate.x, climate.z, 0u, climate.y, quart);
    var roots = array<u32, 6>(
        params.roots_a.x, params.roots_a.y, params.roots_a.z, params.roots_a.w,
        params.roots_b.x, params.roots_b.y,
    );
    for (var i = 0u; i < 6u; i++) {
        climate_target[i] = i32(regs[roots[i]] * 10000.0);
    }
    climate_target[6] = 0;
    var best = vec2(0xffffffffu, 0xffffffffu);
    var biome = 0u;
    for (var row = 0u; row < params.roots_b.w; row++) {
        let base = params.roots_b.z + row * 15u;
        let d = climate_distance(base);
        if d.x < best.x || (d.x == best.x && d.y < best.y) {
            best = d;
            biome = data[base + 14u];
        }
    }
    out[id.x * 2u] = bitcast<u32>(top);
    out[id.x * 2u + 1u] = biome;
}
