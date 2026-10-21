// Authored implementation. Max-depth (farthest) pyramid: level 0 reduces the
// scene depth buffer 2x2, each next level reduces the previous one. The last
// texel of an odd-sized source also absorbs the tail row/column so no pixel is
// ever left out of a covering texel (the cull test relies on that).
@group(0) @binding(0) var source_depth: texture_depth_2d;
@group(0) @binding(1) var destination: texture_storage_2d<r32float, write>;

@compute @workgroup_size(8, 8)
fn from_depth(@builtin(global_invocation_id) id: vec3<u32>) {
    let dst = textureDimensions(destination);
    if any(id.xy >= dst) { return; }
    let src = textureDimensions(source_depth);
    let lo = id.xy * 2u;
    var hi = lo + vec2<u32>(1u);
    if id.x == dst.x - 1u { hi.x = src.x - 1u; }
    if id.y == dst.y - 1u { hi.y = src.y - 1u; }
    var farthest = 0.0;
    for (var y = lo.y; y <= hi.y; y += 1u) {
        for (var x = lo.x; x <= hi.x; x += 1u) {
            farthest = max(farthest, textureLoad(source_depth, vec2<u32>(x, y), 0));
        }
    }
    textureStore(destination, id.xy, vec4<f32>(farthest));
}

@group(0) @binding(0) var source_level: texture_2d<f32>;
@group(0) @binding(1) var reduced: texture_storage_2d<r32float, write>;

@compute @workgroup_size(8, 8)
fn reduce(@builtin(global_invocation_id) id: vec3<u32>) {
    let dst = textureDimensions(reduced);
    if any(id.xy >= dst) { return; }
    let src = textureDimensions(source_level);
    let lo = id.xy * 2u;
    var hi = lo + vec2<u32>(1u);
    if id.x == dst.x - 1u { hi.x = src.x - 1u; }
    if id.y == dst.y - 1u { hi.y = src.y - 1u; }
    var farthest = 0.0;
    for (var y = lo.y; y <= hi.y; y += 1u) {
        for (var x = lo.x; x <= hi.x; x += 1u) {
            farthest = max(farthest, textureLoad(source_level, vec2<u32>(x, y), 0).r);
        }
    }
    textureStore(reduced, id.xy, vec4<f32>(farthest));
}
