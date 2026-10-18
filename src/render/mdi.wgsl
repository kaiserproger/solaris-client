// Authored implementation. Match visibility::Frustum's WebGPU 0..w planes
// and conservative epsilon; unknown source bounds must never hide terrain.
struct Candidate {
    min: vec4<f32>,
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
struct Frustum {
    planes: array<vec4<f32>, 6>,
    count: vec4<u32>,
}
@group(0) @binding(0) var<storage, read> candidates: array<Candidate>;
@group(0) @binding(1) var<storage, read_write> draws: array<DrawIndexed>;
@group(0) @binding(2) var<uniform> frustum: Frustum;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x;
    if i >= frustum.count.x { return; }
    let c = candidates[i];
    var visible = true;
    // !(x <= y) also admits NaN conservatively, like the CPU fallback.
    let finite = all(abs(c.min.xyz) <= vec3<f32>(3.402823e38))
        && all(abs(c.max.xyz) <= vec3<f32>(3.402823e38));
    if finite && all(c.min.xyz <= c.max.xyz) {
        for (var p = 0u; p < 6u; p += 1u) {
            let plane = frustum.planes[p];
            let positive = select(c.min.xyz, c.max.xyz, plane.xyz >= vec3<f32>(0.));
            if dot(plane.xyz, positive) + plane.w < -0.01 { visible = false; }
        }
    }
    draws[i] = DrawIndexed(c.draw.x, select(0u, 1u, visible), c.draw.y, bitcast<i32>(c.draw.z), c.draw.w);
}
