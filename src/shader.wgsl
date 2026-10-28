struct Camera {
    view_projection: mat4x4<f32>,
    inverse_view_projection: mat4x4<f32>,
    fog: vec4<f32>,
    sky: vec4<f32>,
    viewport: vec4<f32>,
    celestial: vec4<f32>,
    sun_uv: vec4<f32>,
    moon_uv: vec4<f32>,
    end_uv: vec4<f32>,
    layers: vec4<f32>,
    sunrise: vec4<f32>,
    star: vec4<f32>,
    end_flash: vec4<f32>,
    end_flash_uv: vec4<f32>,
    color_space: vec4<f32>,
    light_direction_0: vec4<f32>,
    light_direction_1: vec4<f32>,
    portal_uv_0: vec4<f32>,
    portal_uv_1: vec4<f32>,
    material_globals: vec4<f32>,
    projection_fog: vec4<f32>,
    shadow_projection: mat4x4<f32>,
    fancy: vec4<f32>,
    sun_direction: vec4<f32>,
    world_origin: vec4<f32>,
    shadow_cascades: array<mat4x4<f32>, 3>,
    cascade_info: vec4<f32>,
    moon_direction: vec4<f32>,
    atmosphere: vec4<f32>,
};
@group(0) @binding(0) var<uniform> camera: Camera;
@group(1) @binding(0) var block_atlas: texture_2d_array<f32>;
@group(1) @binding(1) var block_sampler: sampler;
@group(1) @binding(2) var lightmap_texture: texture_2d<f32>;
@group(1) @binding(3) var lightmap_sampler: sampler;
@group(1) @binding(4) var terrain_sampler: sampler;
@group(1) @binding(5) var entity_overlay_texture: texture_2d<f32>;
@group(2) @binding(0) var shadow_map: texture_depth_2d_array;
@group(2) @binding(1) var shadow_sampler: sampler_comparison;
@group(2) @binding(2) var opaque_depth: texture_depth_2d;
@group(2) @binding(3) var scene_copy: texture_2d<f32>;
@group(2) @binding(4) var scene_sampler: sampler;
@group(1) @binding(6) var tile_table: texture_2d<f32>;
fn srgb_to_linear(color: vec3<f32>) -> vec3<f32> {
    return select(pow((max(color, vec3<f32>(0.0)) + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4)),
        color / 12.92, color <= vec3<f32>(0.04045));
}
fn linear_to_srgb(color: vec3<f32>) -> vec3<f32> {
    return select(1.055 * pow(max(color, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.4)) - vec3<f32>(0.055),
        color * 12.92, color <= vec3<f32>(0.0031308));
}
fn target_color(color: vec4<f32>) -> vec4<f32> {
    if camera.fancy.x > 0.5 { return color; }
    if camera.color_space.x > 0.5 { return color; }
    return vec4<f32>(linear_to_srgb(color.rgb), color.a);
}
// Emissive and sky sprites still arrive display-encoded; decode before HDR blending.
fn hdr_emission(color: vec4<f32>) -> vec4<f32> {
    if camera.fancy.x > 0.5 { return vec4<f32>(srgb_to_linear(color.rgb), color.a); }
    return color;
}
struct SceneOut {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) overlay_uv: vec2<f32>,
    @location(3) overlay_tint: vec3<f32>,
    @location(4) alpha: f32,
    @location(5) @interpolate(flat) layer: u32,
    @location(6) fog_distance: vec2<f32>,
    @location(7) back_color: vec3<f32>,
    @location(8) light_color: vec4<f32>,
    @location(9) entity_overlay: vec4<f32>,
    @location(10) tex_projection: vec4<f32>,
    @location(11) @interpolate(flat) flat_color: vec4<f32>,
    @location(12) relative: vec3<f32>,
    @location(13) normal: vec3<f32>,
    @location(14) @interpolate(flat) block_class: u32,
};
fn waving_kind(position: vec3<f32>, kind: u32, rooted_weight: f32) -> vec3<f32> {
    if camera.fancy.x < 0.5 { return position; }
    if kind != 2u && kind != 3u { return position; }
    let world = position;
    let t = camera.fancy.y;
    let phase = dot(world.xz, vec2<f32>(0.57, 0.39));
    let weight = select(clamp(rooted_weight, 0.0, 1.0), 0.45, kind == 3u);
    let wind = sin(t * 1.7 + phase + camera.fancy.z) * 0.065
        + sin(t * 3.1 + phase * 1.7 + camera.fancy.w) * 0.025;
    return position + vec3<f32>(wind, 0.0, wind * 0.55) * weight;
}
fn waving(position: vec3<f32>, normal: vec3<f32>) -> vec3<f32> {
    let magnitude = length(normal);
    return waving_kind(position, u32(round(magnitude)), (magnitude - 2.0) * 5.0);
}
// Packed terrain vertex (render/packed.rs): the single decode shared by the
// scene and shadow vertex stages.
struct Terrain {
    position: vec3<f32>,
    color: vec3<f32>,
    alpha: f32,
    uv: vec2<f32>,
    layer: u32,
    light: vec2<f32>,
    class_kind: u32,
    weight: f32,
    normal: vec3<f32>,
    tile_origin: vec2<f32>,
    tile: vec3<f32>,
};
fn face_normal(face: u32) -> vec3<f32> {
    var normals = array<vec3<f32>, 6>(
        vec3<f32>(0.0, -1.0, 0.0), vec3<f32>(0.0, 1.0, 0.0),
        vec3<f32>(0.0, 0.0, -1.0), vec3<f32>(0.0, 0.0, 1.0),
        vec3<f32>(-1.0, 0.0, 0.0), vec3<f32>(1.0, 0.0, 0.0));
    return normals[min(face, 5u)];
}
fn decode_terrain(words: vec4<u32>, rgba: vec4<f32>, lanes: vec4<u32>, uv_bits: vec2<u32>) -> Terrain {
    var t: Terrain;
    let flags = words.w;
    t.position = vec3<f32>(words.xyz) / 2048.0 - vec3<f32>(8.0);
    t.class_kind = (flags >> 3u) & 7u;
    t.weight = f32(flags >> 8u) / 128.0;
    t.normal = select(vec3<f32>(0.0), face_normal(flags & 7u), t.class_kind > 0u);
    t.color = rgba.rgb;
    t.alpha = rgba.a;
    t.light = select(vec2<f32>(lanes.xy) / 16.0, vec2<f32>(-1.0), lanes.x == 255u);
    t.layer = lanes.z | (lanes.w << 8u);
    t.tile_origin = vec2<f32>(-1.0);
    t.tile = vec3<f32>(1.0);
    if (flags & 64u) != 0u {
        let texel = i32(t.layer) * 2;
        let width = i32(textureDimensions(tile_table, 0).x);
        let rect = textureLoad(tile_table, vec2<i32>(texel % width, texel / width), 0);
        let extra = textureLoad(tile_table, vec2<i32>((texel + 1) % width, (texel + 1) / width), 0);
        t.uv = vec2<f32>(uv_bits) / 256.0 - vec2<f32>(64.0);
        t.tile_origin = rect.xy;
        t.tile = vec3<f32>(rect.zw, -1.0);
        t.layer = u32(extra.x);
    } else if (flags & 128u) != 0u {
        t.uv = vec2<f32>(-1.0);
    } else {
        t.uv = vec2<f32>(uv_bits) / 32768.0;
    }
    return t;
}
fn cascade_radius(index: i32) -> f32 {
    return camera.cascade_info[index + 1];
}
// One cascade's 3x3 PCF visibility; `inside` is 0 when the point misses the map.
fn cascade_visibility(index: i32, relative: vec3<f32>, normal: vec3<f32>) -> vec2<f32> {
    // World size of one shadow texel; the offset along the normal and the
    // receiver bias scale with it so every cascade is acne-free at its own density.
    let texel = 2.0 * cascade_radius(index) / 2048.0;
    let projected = camera.shadow_cascades[index] * vec4<f32>(relative + normal * (texel * 1.2), 1.0);
    let coord = projected.xyz / projected.w;
    let uv = coord.xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5);
    if all(uv > vec2<f32>(0.002)) && all(uv < vec2<f32>(0.998)) && coord.z > 0.0 && coord.z < 1.0 {
        let bias = (texel * 3.0 + 0.04) / 383.0;
        var visibility = 0.0;
        for (var y = -1; y <= 1; y++) {
            for (var x = -1; x <= 1; x++) {
                visibility += textureSampleCompareLevel(shadow_map, shadow_sampler,
                    uv + vec2<f32>(f32(x), f32(y)) / 2048.0, index, coord.z - bias);
            }
        }
        return vec2<f32>(visibility / 9.0, 1.0);
    }
    return vec2<f32>(1.0, 0.0);
}
// Cascaded directional shadow: nearest cascade covering the receiver, blended
// into the next across its outer 12%, faded out at the last cascade's edge.
fn shadow_visibility(relative: vec3<f32>, normal: vec3<f32>) -> f32 {
    let count = i32(camera.cascade_info.x);
    let distance = length(relative);
    var index = 0;
    while index + 1 < count && distance > cascade_radius(index) * 0.92 { index++; }
    var result = cascade_visibility(index, relative, normal);
    var visibility = result.x;
    let radius = cascade_radius(index);
    if index + 1 < count {
        let blend = smoothstep(radius * 0.8, radius * 0.92, distance);
        if blend > 0.0 {
            visibility = mix(visibility, cascade_visibility(index + 1, relative, normal).x, blend);
        }
    } else {
        visibility = mix(1.0, visibility, 1.0 - smoothstep(radius * 0.75, radius * 0.975, distance));
    }
    return visibility;
}
fn fancy_lighting(in: SceneOut, color: vec4<f32>) -> vec4<f32> {
    if camera.fancy.x < 0.5 || camera.viewport.w != 1.0 { return color; }
    let normal = normalize(in.normal + vec3<f32>(0.0, 0.00001, 0.0));
    var visibility = 1.0;
    if camera.cascade_info.x > 0.5 { visibility = shadow_visibility(in.relative, normal); }
    let diffuse = max(dot(normal, camera.sun_direction.xyz), 0.0);
    let warm = vec3<f32>(1.0, 0.96, 0.88) * dominant_light_tint() + camera.sunrise.rgb * camera.sunrise.a * 0.16;
    let lighting = vec3<f32>(0.66, 0.73, 0.84) + warm * (0.55 * visibility * diffuse * camera.sun_direction.w);
    // Display exposure rises smoothly after sunset; received light still controls dark terrain.
    return vec4<f32>(color.rgb * lighting * mix(1.0, 3.2, camera.color_space.y), color.a);
}
@vertex fn scene_vertex(@location(0) position: vec3<f32>, @location(1) color: vec3<f32>,
    @location(2) uv: vec2<f32>, @location(3) overlay_uv: vec2<f32>, @location(4) overlay_tint: vec3<f32>,
    @location(5) alpha: f32, @location(6) layer: u32, @location(7) origin: vec4<f32>,
    @location(8) light: vec2<f32>, @location(9) normal: vec3<f32>) -> SceneOut {
    var out: SceneOut;
    let relative_position = waving(position + origin.xyz, normal);
    out.relative = relative_position; out.normal = normal;
    if camera.fancy.x > 0.5 { out.block_class = u32(round(length(normal))); }
    out.fog_distance = vec2<f32>(length(relative_position),
        max(length(relative_position.xz), abs(relative_position.y)));
    out.position = camera.view_projection * vec4<f32>(relative_position, 1.0);
    out.color = color; out.uv = uv; out.overlay_uv = overlay_uv; out.overlay_tint = overlay_tint;
    // The pinned terrain.vsh samples light at vertices before interpolation.
    if all(light >= vec2<f32>(0.0)) {
        let coord = (light.yx + vec2<f32>(0.5)) / 16.0;
        out.color *= textureSampleLevel(lightmap_texture, lightmap_sampler, coord, 0.0).rgb;
    }
    if camera.fancy.x > 0.5 { out.color = srgb_to_linear(out.color); }
    out.alpha = alpha; out.layer = layer;
    out.flat_color.w = origin.w;
    return out;
}
@vertex fn terrain_vertex(@location(0) words: vec4<u32>, @location(1) rgba: vec4<f32>,
    @location(2) lanes: vec4<u32>, @location(3) uv_bits: vec2<u32>,
    @location(7) origin: vec4<f32>) -> SceneOut {
    let t = decode_terrain(words, rgba, lanes, uv_bits);
    var out: SceneOut;
    let relative_position = waving_kind(t.position + origin.xyz, t.class_kind, t.weight);
    out.relative = relative_position; out.normal = t.normal;
    if camera.fancy.x > 0.5 { out.block_class = t.class_kind; }
    out.fog_distance = vec2<f32>(length(relative_position),
        max(length(relative_position.xz), abs(relative_position.y)));
    out.position = camera.view_projection * vec4<f32>(relative_position, 1.0);
    out.color = t.color; out.uv = t.uv; out.overlay_uv = t.tile_origin; out.overlay_tint = t.tile;
    // The pinned terrain.vsh samples light at vertices before interpolation.
    if all(t.light >= vec2<f32>(0.0)) {
        let coord = (t.light.yx + vec2<f32>(0.5)) / 16.0;
        out.color *= textureSampleLevel(lightmap_texture, lightmap_sampler, coord, 0.0).rgb;
    }
    if camera.fancy.x > 0.5 { out.color = srgb_to_linear(out.color); }
    out.alpha = t.alpha; out.layer = t.layer;
    out.flat_color.w = origin.w;
    return out;
}
fn terrain_sample(uv: vec2<f32>, layer: u32, du: vec2<f32>, dv: vec2<f32>) -> vec4<f32> {
    // Pinned terrain.fsh sampleNearest; gradient lookup selects sprite-local mips.
    let pixel_size = vec2<f32>(1.0) / vec2<f32>(textureDimensions(block_atlas, 0));
    let screen_size = sqrt(du * du + dv * dv);
    let texel = uv / pixel_size;
    let center = round(texel) - vec2<f32>(0.5);
    let offset = clamp((texel - center - vec2<f32>(0.5)) * pixel_size / screen_size
        + vec2<f32>(0.5), vec2<f32>(0.0), vec2<f32>(1.0));
    return textureSampleGrad(block_atlas, terrain_sampler, (center + offset) * pixel_size, i32(layer), du, dv);
}
fn material(in: SceneOut, display_encoded: bool, terrain: bool) -> vec4<f32> {
    // Derivatives precede per-fragment texture-presence branches.
    let du = dpdx(in.uv); let dv = dpdy(in.uv);
    let overlay_du = dpdx(in.overlay_uv); let overlay_dv = dpdy(in.overlay_uv);
    let tiled = terrain && in.overlay_tint.z < 0.0;
    var texel = vec4<f32>(1.0);
    if tiled || all(in.uv >= vec2<f32>(0.0)) {
        if terrain {
            var coord = in.uv; var gradient_x = du; var gradient_y = dv;
            if tiled {
                coord = in.overlay_uv + fract(in.uv) * in.overlay_tint.xy;
                gradient_x *= in.overlay_tint.xy; gradient_y *= in.overlay_tint.xy;
            }
            texel = terrain_sample(coord, in.layer, gradient_x, gradient_y);
        }
        else { texel = textureSampleLevel(block_atlas, block_sampler, in.uv, i32(in.layer), 0.0); }
    }
    if !display_encoded { texel = vec4<f32>(srgb_to_linear(texel.rgb), texel.a); }
    var color = in.color * texel.rgb;
    if !tiled && all(in.overlay_uv >= vec2<f32>(0.0)) {
        var overlay: vec4<f32>;
        if terrain { overlay = terrain_sample(in.overlay_uv, in.layer, overlay_du, overlay_dv); }
        else { overlay = textureSampleLevel(block_atlas, block_sampler, in.overlay_uv, i32(in.layer), 0.0); }
        if !display_encoded { overlay = vec4<f32>(srgb_to_linear(overlay.rgb), overlay.a); }
        color = mix(color, overlay.rgb * in.color * in.overlay_tint, overlay.a);
    }
    var rgba = vec4<f32>(color, texel.a * in.alpha);
    if terrain && in.flat_color.w < 1.0 {
        var fog_color = camera.fog.rgb;
        if camera.fancy.x > 0.5 { fog_color = srgb_to_linear(fog_color); }
        rgba = mix(vec4<f32>(fog_color, camera.star.w * rgba.a), rgba, in.flat_color.w);
    }
    return rgba;
}
fn linear_fog(distance: f32, start: f32, end: f32) -> f32 {
    if distance <= start { return 0.0; }
    if distance >= end { return 1.0; }
    return (distance - start) / (end - start);
}
fn sky_disc_fog(radius_fraction: f32) -> f32 {
    // sky.vsh computes distances from original ±16-height vertices, even for
    // the bottom disc subsequently translated +12 by renderDarkDisc.
    let spherical = mix(16.0, sqrt(512.0 * 512.0 + 16.0 * 16.0), radius_fraction);
    let cylindrical = mix(16.0, 512.0, radius_fraction);
    return max(linear_fog(spherical, 0.0, camera.viewport.z),
        linear_fog(cylindrical, camera.viewport.z, camera.viewport.z)) * camera.star.w;
}
fn fogged(in: SceneOut, color: vec4<f32>) -> vec4<f32> {
    let fog = max(linear_fog(in.fog_distance.x, camera.fog.w, camera.sky.w),
        linear_fog(in.fog_distance.y, camera.star.y, camera.star.z)) * camera.star.w;
    if camera.fancy.x > 0.5 {
        // Vanilla fog ramp, a distance floor and a light exponential aerial
        // perspective; the colour is the atmosphere sky along the same ray so
        // terrain dissolves into the pixel the sky pass drew behind it.
        let aerial = (1.0 - exp(-in.fog_distance.x * camera.atmosphere.y)) * camera.star.w;
        let distance_fog = max(max(fog, smoothstep(camera.sky.w * 0.24, camera.sky.w, in.fog_distance.x) * 0.72), aerial);
        let ray = normalize(in.relative + vec3<f32>(0.0, 0.00001, 0.0));
        let sky = select(srgb_to_linear(camera.fog.rgb), atmosphere_sky(ray), camera.viewport.w == 1.0);
        return vec4<f32>(mix(color.rgb, sky, distance_fog), color.a);
    }
    return target_color(vec4<f32>(mix(color.rgb, camera.fog.rgb, fog), color.a));
}
@fragment fn opaque_fragment(in: SceneOut) -> @location(0) vec4<f32> {
    let color = material(in, camera.fancy.x < 0.5 && camera.color_space.x > 0.5, true);
    return fogged(in, fancy_lighting(in, color));
}
@fragment fn cutout_fragment(in: SceneOut) -> @location(0) vec4<f32> {
    let color = material(in, camera.fancy.x < 0.5 && camera.color_space.x > 0.5, true);
    if color.a < 0.5 { discard; } return fogged(in, fancy_lighting(in, color));
}
fn linear_depth(depth: f32) -> f32 {
    return 0.05 * 2048.0 / (2048.0 - depth * (2048.0 - 0.05));
}
fn interleaved_noise(position: vec2<f32>) -> f32 {
    return fract(52.9829189 * fract(dot(position, vec2<f32>(0.06711056, 0.00583715))) + camera.fancy.y * 3.7);
}
// Screen-space reflection against the opaque depth/colour copy: geometric step
// growth (about 90 blocks reach), bisection refine, edge and distance fade.
// Returns rgb + hit weight; a miss (weight 0) is filled by the caller from the sky.
fn trace_reflection(origin: vec3<f32>, direction: vec3<f32>, noise: f32) -> vec4<f32> {
    let size = vec2<f32>(textureDimensions(opaque_depth));
    var travelled = 0.35 + noise * 0.25;
    var step = 0.55;
    var previous = 0.0;
    for (var i = 0; i < 22; i++) {
        let point = origin + direction * travelled;
        let clip = camera.view_projection * vec4<f32>(point, 1.0);
        if clip.w <= 0.0 { break; }
        let ndc = clip.xyz / clip.w;
        let uv = ndc.xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5);
        if any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0)) || ndc.z >= 1.0 { break; }
        let scene = textureLoad(opaque_depth, vec2<i32>(uv * size), 0);
        let gap = linear_depth(ndc.z) - linear_depth(scene);
        if gap > 0.0 && scene < 1.0 {
            if gap < 0.45 + travelled * 0.12 {
                // Bisect between the last in-front sample and this one.
                var low = previous; var high = travelled;
                for (var j = 0; j < 4; j++) {
                    let mid = (low + high) * 0.5;
                    let c = camera.view_projection * vec4<f32>(origin + direction * mid, 1.0);
                    let n = c.xyz / c.w;
                    let m_uv = n.xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5);
                    let s = textureLoad(opaque_depth, vec2<i32>(m_uv * size), 0);
                    if linear_depth(n.z) > linear_depth(s) { high = mid; } else { low = mid; }
                }
                let c = camera.view_projection * vec4<f32>(origin + direction * high, 1.0);
                let n = c.xyz / c.w;
                let hit_uv = n.xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5);
                let edge = min(min(hit_uv.x, 1.0 - hit_uv.x), min(hit_uv.y, 1.0 - hit_uv.y));
                let fade = smoothstep(0.0, 0.1, edge) * (1.0 - smoothstep(30.0, 85.0, high));
                return vec4<f32>(textureSampleLevel(scene_copy, scene_sampler, hit_uv, 0.0).rgb, fade);
            }
            break;
        }
        previous = travelled;
        travelled += step;
        step *= 1.2;
    }
    return vec4<f32>(0.0);
}
@fragment fn translucent_fragment(in: SceneOut) -> @location(0) vec4<f32> {
    var color = material(in, camera.fancy.x < 0.5 && camera.color_space.x > 0.5, true);
    if color.a < 0.01 { discard; }
    if camera.fancy.x > 0.5 && in.block_class == 4u {
        let world = in.relative;
        let t = camera.fancy.y;
        let wave = vec2<f32>(
            cos(world.x * 1.4 + world.z * 0.6 + camera.world_origin.x + t * 1.5),
            sin(world.z * 1.7 - world.x * 0.5 + camera.world_origin.y + t * 1.1)) * 0.085;
        let n = normalize(normalize(in.normal) + vec3<f32>(wave.x, 0.0, wave.y));
        let view = normalize(-in.relative);
        let fresnel = 0.02 + 0.98 * pow(1.0 - max(dot(n, view), 0.0), 5.0);
        let reflected = reflect(-view, n);
        let sky = atmosphere_sky(vec3<f32>(reflected.x, max(reflected.y, 0.01), reflected.z));
        let size = vec2<f32>(textureDimensions(opaque_depth));
        let pixel = clamp(vec2<i32>(in.position.xy), vec2<i32>(0), vec2<i32>(size) - vec2<i32>(1));
        let depth = textureLoad(opaque_depth, pixel, 0);
        let background = camera.inverse_view_projection * vec4<f32>(
            in.position.xy / camera.viewport.xy * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), depth, 1.0);
        let thickness = clamp(length(background.xyz / background.w - in.relative), 0.0, 24.0);
        let half_vector = normalize(view + camera.sun_direction.xyz);
        let specular = pow(max(dot(n, half_vector), 0.0), 180.0) * camera.sun_direction.w * 2.5;
        let glint = vec3<f32>(1.0, 0.9, 0.7) * dominant_light_tint() * specular;
        if camera.atmosphere.z > 0.5 {
            // High: refract the copied opaque scene (offset kept only where the
            // sample lies behind the surface), absorb it with Beer-Lambert along
            // the water column, and add a screen-space reflection over the sky.
            let uv = in.position.xy / size;
            let strength = 0.04 * clamp(thickness * 0.4, 0.0, 1.0) / (1.0 + length(world) * 0.03);
            let offset_uv = clamp(uv + n.xz * strength, vec2<f32>(0.001), vec2<f32>(0.999));
            let behind = textureLoad(opaque_depth, vec2<i32>(offset_uv * size), 0);
            let source_uv = select(uv, offset_uv, behind > in.position.z);
            let bed = textureSampleLevel(scene_copy, scene_sampler, source_uv, 0.0).rgb;
            let lit = clamp(0.3 + camera.sun_direction.w * camera.sun_direction.y, 0.0, 1.0);
            let absorption = exp(-vec3<f32>(0.45, 0.12, 0.06) * thickness);
            // Mostly the vanilla water colour (texture x biome tint): recognisably Minecraft.
            let body = mix(vec3<f32>(0.03, 0.2, 0.3), color.rgb, 0.7) * lit;
            let refracted = bed * absorption + body * (vec3<f32>(1.0) - absorption * absorption);
            var reflection = sky;
            if reflected.y > 0.0 {
                let hit = trace_reflection(world, reflected, interleaved_noise(in.position.xy));
                reflection = mix(sky, hit.rgb, hit.a);
            }
            let rgb = mix(refracted, reflection, fresnel * 0.9) + glint;
            return fogged(in, vec4<f32>(rgb, 1.0));
        }
        let tint = mix(vec3<f32>(0.035, 0.24, 0.27), vec3<f32>(0.008, 0.055, 0.11), 1.0 - exp(-thickness * 0.14));
        color = vec4<f32>(mix(mix(color.rgb, tint, 0.68), sky, fresnel * 0.85) + glint,
            clamp(0.38 + thickness * 0.025 + fresnel * 0.45, 0.38, 0.92));
    }
    return fogged(in, fancy_lighting(in, color));
}
@vertex fn overlay_vertex(@location(0) position: vec3<f32>, @location(1) color: vec3<f32>,
    @location(2) uv: vec2<f32>, @location(3) overlay_uv: vec2<f32>, @location(4) overlay_tint: vec3<f32>,
    @location(5) alpha: f32, @location(6) layer: u32) -> SceneOut {
    var out: SceneOut;
    // UI extraction supplies physical pixels, not clip-space positions. The
    // overlay is painter-ordered and has no depth attachment.
    out.position = vec4<f32>(position.x * 2.0 / camera.viewport.x - 1.0,
        1.0 - position.y * 2.0 / camera.viewport.y, 0.0, 1.0);
    out.color = color; out.uv = uv;
    out.overlay_uv = overlay_uv; out.overlay_tint = overlay_tint; out.alpha = alpha; out.layer = layer;
    out.fog_distance = vec2<f32>(0.0); return out;
}
@fragment fn overlay_fragment(in: SceneOut) -> @location(0) vec4<f32> { return material(in, true, false); }

struct BlitOut { @builtin(position) position: vec4<f32>, @location(0) ndc: vec2<f32> };
@vertex fn blit_vertex(@builtin(vertex_index) index: u32) -> BlitOut {
    let positions = array<vec2<f32>, 3>(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
    var out: BlitOut; out.position = vec4<f32>(positions[index], 0.0, 1.0); out.ndc = positions[index]; return out;
}
@group(0) @binding(1) var frame_texture: texture_2d<f32>;
@fragment fn blit_fragment(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    return textureLoad(frame_texture, vec2<i32>(position.xy), 0);
}
@fragment fn blit_srgb_fragment(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let color = textureLoad(frame_texture, vec2<i32>(position.xy), 0);
    return vec4<f32>(srgb_to_linear(color.rgb), color.a);
}
fn inverse_celestial(direction: vec3<f32>, degrees: f32) -> vec3<f32> {
    // SkyRenderer's Y(-90) * X(environment angle), inverted for ray lookup.
    let y_rotated = vec3<f32>(direction.z, direction.y, -direction.x);
    let angle = degrees * 0.017453292519943295;
    return vec3<f32>(y_rotated.x, y_rotated.y * cos(angle) + y_rotated.z * sin(angle),
        -y_rotated.y * sin(angle) + y_rotated.z * cos(angle));
}
fn celestial_sample(ray: vec3<f32>, angle: f32, height: f32, radius: f32, uv: vec4<f32>, layer: f32, reverse: bool) -> vec4<f32> {
    if layer < 0.0 { return vec4<f32>(0.0); }
    let local = inverse_celestial(ray, angle);
    if local.y * height <= 0.0 { return vec4<f32>(0.0); }
    let point = local.xz * (height / local.y);
    if any(abs(point) > vec2<f32>(radius)) { return vec4<f32>(0.0); }
    var coord = point / (radius * 2.0) + vec2<f32>(0.5);
    if reverse { coord = vec2<f32>(1.0) - coord; }
    return textureSampleLevel(block_atlas, block_sampler, mix(uv.xy, uv.zw, coord), i32(layer), 0.0);
}
@fragment fn sky_fragment(in: BlitOut) -> @location(0) vec4<f32> {
    let p = camera.inverse_view_projection * vec4<f32>(in.ndc, 1.0, 1.0);
    let ray = normalize(p.xyz / p.w);
    var color = camera.sky.rgb;
    if camera.fancy.x > 0.5 && camera.viewport.w == 1.0 {
        return vec4<f32>(atmosphere_sky(ray), 1.0);
    }
    if camera.viewport.w == 0.0 { return target_color(vec4<f32>(camera.fog.rgb, 1.0)); }
    if camera.viewport.w == 2.0 {
        if camera.layers.z >= 0.0 {
            let absolute = abs(ray);
            var coord = vec2<f32>(ray.x, select(ray.y, -ray.y, ray.z < 0.0)) / max(absolute.z, 0.0001);
            if absolute.y >= max(absolute.x, absolute.z) {
                coord = vec2<f32>(ray.x, select(ray.z, -ray.z, ray.y > 0.0)) / absolute.y;
            } else if absolute.x >= absolute.z {
                coord = vec2<f32>(select(-ray.y, ray.y, ray.x > 0.0), ray.z) / absolute.x;
            }
            let repeated = fract((coord * 0.5 + vec2<f32>(0.5)) * 16.0);
            color = textureSampleLevel(block_atlas, block_sampler,
                mix(camera.end_uv.xy, camera.end_uv.zw, repeated), i32(camera.layers.z), 0.0).rgb * (40.0 / 255.0);
        }
        return target_color(vec4<f32>(color, 1.0));
    }
    // Match the octagonal fan and interpolation of its original fog distances.
    color = camera.fog.rgb;
    if ray.y > 0.0 {
        let disc = abs(ray.xz * (16.0 / ray.y));
        let fraction = (max(disc.x, disc.y) + 0.41421356237 * min(disc.x, disc.y)) / 512.0;
        if fraction <= 1.0 {
            color = mix(camera.sky.rgb, camera.fog.rgb, sky_disc_fog(fraction));
        }
    }
    return target_color(vec4<f32>(color, 1.0));
}
@fragment fn celestial_fragment(in: BlitOut) -> @location(0) vec4<f32> {
    if camera.viewport.w == 2.0 {
        if camera.end_flash.x <= 0.0 || camera.end_flash.w < 0.0 { return vec4<f32>(0.0); }
        let p = camera.inverse_view_projection * vec4<f32>(in.ndc, 1.0, 1.0);
        let ray = normalize(p.xyz / p.w);
        let beta = (180.0 - camera.end_flash.z) * 0.017453292519943295;
        let gamma = (-90.0 - camera.end_flash.y) * 0.017453292519943295;
        let rotated_y = vec3<f32>(cos(beta) * ray.x - sin(beta) * ray.z, ray.y,
            sin(beta) * ray.x + cos(beta) * ray.z);
        let local = vec3<f32>(rotated_y.x, cos(gamma) * rotated_y.y + sin(gamma) * rotated_y.z,
            -sin(gamma) * rotated_y.y + cos(gamma) * rotated_y.z);
        if local.y <= 0.0 { return vec4<f32>(0.0); }
        let point = local.xz * (100.0 / local.y);
        if any(abs(point) > vec2<f32>(60.0)) { return vec4<f32>(0.0); }
        let coord = point / 120.0 + vec2<f32>(0.5);
        let flash = textureSampleLevel(block_atlas, block_sampler,
            mix(camera.end_flash_uv.xy, camera.end_flash_uv.zw, coord), i32(camera.end_flash.w), 0.0);
        return vec4<f32>(flash.rgb * flash.a * camera.end_flash.x, 0.0);
    }
    if camera.viewport.w != 1.0 { return vec4<f32>(0.0); }
    let p = camera.inverse_view_projection * vec4<f32>(in.ndc, 1.0, 1.0);
    let ray = normalize(p.xyz / p.w);
    let sun = celestial_sample(ray, camera.celestial.x, 100.0, 30.0, camera.sun_uv, camera.layers.x, false);
    let moon = celestial_sample(ray, camera.celestial.y, 100.0, 20.0, camera.moon_uv, camera.layers.y, true);
    // Vanilla celestial blend is additive; rain attenuates both visible sources.
    let radiance = (hdr_emission(sun).rgb * sun.a + hdr_emission(moon).rgb * moon.a) * camera.celestial.z;
    return vec4<f32>(radiance * select(1.0, 3.2, camera.fancy.x > 0.5), 0.0);
}
@fragment fn dark_disc_fragment(in: BlitOut) -> @location(0) vec4<f32> {
    if camera.viewport.w != 1.0 || camera.layers.w <= 0.0 { return vec4<f32>(0.0); }
    let p = camera.inverse_view_projection * vec4<f32>(in.ndc, 1.0, 1.0);
    let ray = normalize(p.xyz / p.w);
    if ray.y >= 0.0 { return vec4<f32>(0.0); }
    let disc = abs(ray.xz * (-4.0 / ray.y));
    let fraction = (max(disc.x, disc.y) + 0.41421356237 * min(disc.x, disc.y)) / 512.0;
    if fraction <= 1.0 {
        return target_color(vec4<f32>(hdr_emission(vec4<f32>(camera.fog.rgb, 1.0)).rgb * sky_disc_fog(fraction), 1.0));
    }
    return vec4<f32>(0.0);
}
struct StarOut { @builtin(position) position: vec4<f32> };
@vertex fn star_vertex(@location(0) position: vec3<f32>) -> StarOut {
    let a = camera.star.x * 0.017453292519943295;
    let p = vec3<f32>(position.x, position.y * cos(a) - position.z * sin(a), position.y * sin(a) + position.z * cos(a));
    let rotated = vec3<f32>(-p.z, p.y, p.x);
    var out: StarOut; out.position = camera.view_projection * vec4<f32>(rotated, 1.0); return out;
}
@fragment fn star_fragment() -> @location(0) vec4<f32> {
    return vec4<f32>(vec3<f32>(camera.celestial.w * camera.celestial.z), 1.0);
}

// Specializations are pinned Source RenderPipeline shader defines, not chunk layers.
override source_kind: u32 = 0u;
override source_cutoff: f32 = 0.0;
override source_cardinal: bool = false;
override source_per_face: bool = false;
override source_overlay: bool = false;
override source_lightmap: bool = false;
override source_wrap: bool = false;
override source_fog: bool = false;
override source_dissolve: bool = false;
override source_view_scale: f32 = 1.0;
override source_portal_layers: u32 = 0u;

fn source_light(light: vec2<f32>) -> f32 {
    return min((max(light.x, 0.0) + max(light.y, 0.0)) * 0.6 + 0.4, 1.0);
}
@vertex fn source_vertex(@location(0) position: vec3<f32>, @location(1) color: vec3<f32>,
    @location(2) uv: vec2<f32>, @location(3) overlay_uv: vec2<f32>, @location(4) overlay_tint: vec3<f32>,
    @location(5) alpha: f32, @location(6) layer: u32, @location(7) origin: vec4<f32>,
    @location(8) light: vec2<f32>, @location(9) normal: vec3<f32>) -> SceneOut {
    var out: SceneOut;
    let relative = position + origin.xyz;
    out.relative = relative; out.normal = normal; out.block_class = 0u;
    out.position = camera.view_projection * vec4<f32>(relative * source_view_scale, 1.0);
    out.fog_distance = vec2<f32>(length(relative), max(length(relative.xz), abs(relative.y)));
    out.color = color; out.back_color = color; out.alpha = alpha;
    out.uv = uv; out.overlay_uv = overlay_uv; out.overlay_tint = overlay_tint; out.layer = layer;
    out.light_color = vec4<f32>(1.0);
    out.entity_overlay = vec4<f32>(1.0);
    if source_cardinal {
        let dots = vec2<f32>(dot(camera.light_direction_0.xyz, normal), dot(camera.light_direction_1.xyz, normal));
        out.color *= source_light(dots);
        if source_per_face { out.back_color *= source_light(-dots); }
        else { out.back_color = out.color; }
    }
    if source_lightmap {
        out.light_color = textureSampleLevel(lightmap_texture, lightmap_sampler, (light.yx + vec2<f32>(0.5)) / 16.0, 0.0);
        // Source entity.vsh keeps this separate until AFTER the fragment overlay.
        if source_kind != 0u {
            out.color *= out.light_color.rgb;
            out.alpha *= out.light_color.a;
        }
    }
    if source_overlay { out.entity_overlay = textureLoad(entity_overlay_texture, vec2<i32>(overlay_uv), 0); }
    out.flat_color = vec4<f32>(out.color, out.alpha);
    if source_kind == 17u {
        let shrink = source_view_scale * (1.0 - 1.0 / 256.0);
        let start = camera.view_projection * vec4<f32>(relative * shrink, 1.0);
        let end = camera.view_projection * vec4<f32>((relative + normal) * shrink, 1.0);
        let a = start.xyz / start.w;
        let b = end.xyz / end.w;
        let direction = normalize((b.xy - a.xy) * camera.viewport.xy);
        var offset = vec2<f32>(-direction.y, direction.x) * uv.x / camera.viewport.xy;
        if offset.x < 0.0 { offset = -offset; }
        out.position = vec4<f32>((a + vec3<f32>(offset * overlay_uv.x, 0.0)) * start.w, start.w);
    }
    if source_kind == 18u {
        // Vulkan/WebGPU has no variable point size: equivalent square raster quad.
        out.position = vec4<f32>(out.position.xy + overlay_uv * uv.x / camera.viewport.xy * out.position.w,
            out.position.zw);
    }
    out.tex_projection = out.position * 0.5;
    out.tex_projection = vec4<f32>(out.tex_projection.xy + vec2<f32>(out.position.w * 0.5), out.position.zw);
    return out;
}
fn source_total_fog(in: SceneOut) -> f32 {
    return max(linear_fog(in.fog_distance.x, camera.fog.w, camera.sky.w),
        linear_fog(in.fog_distance.y, camera.star.y, camera.star.z));
}
fn source_portal_uv(projected: vec4<f32>, layer: f32) -> vec2<f32> {
    let angle = ((layer * layer * 4321.0 + layer * 9.0) * 2.0) * 0.017453292519943295;
    let scale = (4.5 - layer / 4.0) * 2.0;
    let rotate = mat4x4<f32>(
        vec4<f32>(scale * cos(angle), -scale * sin(angle), 0.0, 0.0),
        vec4<f32>(scale * sin(angle), scale * cos(angle), 0.0, 0.0),
        vec4<f32>(0.0, 0.0, 1.0, 0.0), vec4<f32>(0.0, 0.0, 0.0, 1.0));
    let translate = mat4x4<f32>(
        vec4<f32>(1.0, 0.0, 0.0, 17.0 / layer),
        vec4<f32>(0.0, 1.0, 0.0, (2.0 + layer / 1.5) * (camera.material_globals.z * 1.5)),
        vec4<f32>(0.0, 0.0, 1.0, 0.0), vec4<f32>(0.0, 0.0, 0.0, 1.0));
    let scale_translate = mat4x4<f32>(
        vec4<f32>(0.5, 0.0, 0.0, 0.25), vec4<f32>(0.0, 0.5, 0.0, 0.25),
        vec4<f32>(0.0, 0.0, 1.0, 0.0), vec4<f32>(0.0, 0.0, 0.0, 1.0));
    let coord = projected * (rotate * translate * scale_translate);
    return coord.xy / coord.w;
}
const SOURCE_PORTAL_COLORS = array<vec3<f32>, 16>(
    vec3<f32>(0.022087,0.098399,0.110818), vec3<f32>(0.011892,0.095924,0.089485),
    vec3<f32>(0.027636,0.101689,0.100326), vec3<f32>(0.046564,0.109883,0.114838),
    vec3<f32>(0.064901,0.117696,0.097189), vec3<f32>(0.063761,0.086895,0.123646),
    vec3<f32>(0.084817,0.111994,0.166380), vec3<f32>(0.097489,0.154120,0.091064),
    vec3<f32>(0.106152,0.131144,0.195191), vec3<f32>(0.097721,0.110188,0.187229),
    vec3<f32>(0.133516,0.138278,0.148582), vec3<f32>(0.070006,0.243332,0.235792),
    vec3<f32>(0.196766,0.142899,0.214696), vec3<f32>(0.047281,0.315338,0.321970),
    vec3<f32>(0.204675,0.390010,0.302066), vec3<f32>(0.080955,0.314821,0.661491));
fn source_portal_sample(coord: vec2<f32>, rect: vec4<f32>, layer: f32) -> vec3<f32> {
    return textureSampleLevel(block_atlas, block_sampler, mix(rect.xy, rect.zw, fract(coord)), i32(layer), 0.0).rgb;
}
@fragment fn source_fragment(in: SceneOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    // Compute derivatives before texture/alpha branches, as for implicit GLSL LOD.
    let du = dpdx(in.uv); let dv = dpdy(in.uv);
    if source_kind == 12u {
        var rgb = source_portal_sample(in.tex_projection.xy / in.tex_projection.w,
            camera.portal_uv_0, camera.material_globals.x) * SOURCE_PORTAL_COLORS[0];
        for (var i = 0u; i < source_portal_layers; i++) {
            rgb += source_portal_sample(source_portal_uv(in.tex_projection, f32(i + 1u)),
                camera.portal_uv_1, camera.material_globals.y) * SOURCE_PORTAL_COLORS[i];
        }
        return fogged(in, vec4<f32>(rgb, 1.0));
    }
    var face = vec4<f32>(in.color, in.alpha);
    if source_per_face && !front { face = vec4<f32>(in.back_color, in.alpha); }
    var texel = vec4<f32>(1.0);
    let textured = source_kind == 0u || source_kind == 1u || source_kind == 2u || source_kind == 5u
        || source_kind == 6u || source_kind == 8u || source_kind == 9u || source_kind == 10u
        || source_kind == 11u || source_kind == 15u || source_kind == 19u;
    if textured {
        var coord = in.uv;
        if source_wrap { coord = in.overlay_uv + fract(coord) * in.overlay_tint.xy; }
        if source_kind == 10u {
            // Source clamps a standalone SimpleTexture, not the entire packed page.
            let inset = vec2<f32>(0.5) / vec2<f32>(textureDimensions(block_atlas, 0));
            coord = clamp(in.overlay_uv + coord * in.overlay_tint.xy,
                in.overlay_uv + inset, in.overlay_uv + in.overlay_tint.xy - inset);
        }
        if source_kind == 1u || source_kind == 2u {
            texel = textureSampleGrad(block_atlas, block_sampler, coord, i32(in.layer), du, dv);
        } else { texel = textureSampleLevel(block_atlas, block_sampler, coord, i32(in.layer), 0.0); }
    }
    if source_kind == 0u || source_kind == 1u {
        if texel.a < source_cutoff { discard; }
    }
    if source_dissolve {
        let mask = textureSampleLevel(block_atlas, block_sampler, in.overlay_tint.xy, i32(in.overlay_tint.z), 0.0);
        if face.a < mask.a { discard; }
        face.a = 1.0;
    }
    if source_kind == 6u { texel = texel.rrrr; }
    var color = texel * face;
    if source_kind == 0u {
        if source_overlay { color = vec4<f32>(mix(in.entity_overlay.rgb, color.rgb, in.entity_overlay.a), color.a); }
        if source_lightmap { color *= in.light_color; }
    }
    if source_kind == 2u { if color.a < source_cutoff { discard; } }
    if source_kind == 5u || source_kind == 6u || source_kind == 7u || source_kind == 19u {
        if color.a < 0.1 { discard; }
    }
    if source_kind == 3u || source_kind == 15u { if color.a == 0.0 { discard; } }
    if source_kind == 11u {
        if texel.a == 0.0 { discard; }
        return target_color(vec4<f32>(in.color, in.alpha));
    }
    if source_kind == 8u {
        if texel.a < 0.1 { discard; }
        let emission = hdr_emission(vec4<f32>(texel.rgb * in.color, texel.a));
        return target_color(vec4<f32>(emission.rgb * (1.0 - source_total_fog(in)) * in.alpha, emission.a));
    }
    if source_kind == 4u { return target_color(hdr_emission(color) * (1.0 - source_total_fog(in))); }
    if source_kind == 13u { color = in.flat_color; }
    if source_kind == 9u {
        let distance = -camera.projection_fog.x / (-2.0 * in.position.z + 1.0 - camera.projection_fog.y);
        var beam = in; beam.fog_distance = vec2<f32>(distance); return fogged(beam, color);
    }
    if camera.fancy.x > 0.5 {
        color = fancy_lighting(in, vec4<f32>(srgb_to_linear(color.rgb), color.a));
    }
    if source_fog { return fogged(in, color); }
    return target_color(color);
}

struct ShadowOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) layer: u32,
    @location(2) alpha: f32,
    @location(3) tile_origin: vec2<f32>,
    @location(4) tile: vec3<f32>,
};
@vertex fn shadow_vertex(@location(0) position: vec3<f32>, @location(2) uv: vec2<f32>,
    @location(3) tile_origin: vec2<f32>, @location(4) tile: vec3<f32>,
    @location(5) alpha: f32, @location(6) layer: u32, @location(7) origin: vec4<f32>,
    @location(9) normal: vec3<f32>) -> ShadowOut {
    var out: ShadowOut;
    out.position = camera.shadow_projection * vec4<f32>(waving(position + origin.xyz, normal), 1.0);
    out.uv = uv; out.layer = layer; out.alpha = alpha;
    out.tile_origin = tile_origin; out.tile = tile;
    return out;
}
@vertex fn shadow_terrain_vertex(@location(0) words: vec4<u32>, @location(1) rgba: vec4<f32>,
    @location(2) lanes: vec4<u32>, @location(3) uv_bits: vec2<u32>,
    @location(7) origin: vec4<f32>) -> ShadowOut {
    let t = decode_terrain(words, rgba, lanes, uv_bits);
    var out: ShadowOut;
    out.position = camera.shadow_projection
        * vec4<f32>(waving_kind(t.position + origin.xyz, t.class_kind, t.weight), 1.0);
    out.uv = t.uv; out.layer = t.layer; out.alpha = t.alpha;
    out.tile_origin = t.tile_origin; out.tile = t.tile;
    return out;
}
@fragment fn shadow_fragment(in: ShadowOut) {
    var alpha = in.alpha;
    if in.tile.z < 0.0 || all(in.uv >= vec2<f32>(0.0)) {
        var coord = in.uv;
        if in.tile.z < 0.0 { coord = in.tile_origin + fract(coord) * in.tile.xy; }
        alpha *= textureSampleLevel(block_atlas, block_sampler, coord, i32(in.layer), 0.0).a;
    }
    if alpha < 0.5 { discard; }
}
