// Authored implementation of the verified entity-outline post chain.
@group(0) @binding(0) var input_texture: texture_2d<f32>;
@group(0) @binding(1) var linear_sampler: sampler;

@vertex
fn fullscreen(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let x = f32((index << 1u) & 2u);
    let y = f32(index & 2u);
    return vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
}

fn pixel(coordinate: vec2<i32>) -> vec4<f32> {
    let limit = vec2<i32>(textureDimensions(input_texture)) - vec2<i32>(1);
    return textureLoad(input_texture, clamp(coordinate, vec2<i32>(0), limit), 0);
}

@fragment
fn silhouette(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec2<i32>(position.xy);
    let center = pixel(p);
    let left = pixel(p + vec2<i32>(-1, 0));
    let right = pixel(p + vec2<i32>(1, 0));
    let up = pixel(p + vec2<i32>(0, -1));
    let down = pixel(p + vec2<i32>(0, 1));
    let edge = clamp(abs(center.a - left.a) + abs(center.a - right.a)
        + abs(center.a - up.a) + abs(center.a - down.a), 0.0, 1.0);
    let color = (center.rgb * center.a + left.rgb * left.a + right.rgb * right.a
        + up.rgb * up.a + down.rgb * down.a) * 0.2;
    return vec4<f32>(color, edge);
}

fn blur(position: vec2<f32>, direction: vec2<f32>) -> vec4<f32> {
    let dimensions = vec2<f32>(textureDimensions(input_texture));
    let uv = position / dimensions;
    let step = direction / dimensions;
    let value = textureSampleLevel(input_texture, linear_sampler, uv - step * 1.5, 0.0)
        + textureSampleLevel(input_texture, linear_sampler, uv + step * 0.5, 0.0)
        + textureSampleLevel(input_texture, linear_sampler, uv + step * 2.0, 0.0) * 0.5;
    // Source averages RGB only; the RGBA8 destination clamps the accumulated alpha.
    return vec4<f32>(value.rgb / 2.5, value.a);
}

@fragment
fn horizontal(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    return blur(position.xy, vec2<f32>(1.0, 0.0));
}

@fragment
fn vertical(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    // Source GL texture Y increases upward; native framebuffer texture Y goes down.
    return blur(position.xy, vec2<f32>(0.0, -1.0));
}

@fragment
fn blit(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    return pixel(vec2<i32>(position.xy));
}
