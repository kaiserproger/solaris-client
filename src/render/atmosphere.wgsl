// Analytic single-scattering atmosphere shared by the sky pass, terrain fog
// (so distant terrain fades into the exact sky pixel behind it) and water
// reflection fallback. Appended to shader.wgsl; uses its `camera` uniform and
// `srgb_to_linear`.
//
// Model: Rayleigh + Mie scattering with a Kasten-Young air mass for the view
// and light paths, evaluated for the sun and for the moon (moon = same law,
// different power/direction). Sun light is attenuated by Rayleigh/Mie
// extinction along its own path, so low suns turn the sky and horizon orange
// and red, and the anti-solar horizon pink/blue, with no authored gradients.
// The vanilla camera sky/fog colours (biome, weather, dimension) stay the
// second term of a blend so the result remains recognisably Minecraft.

const TAU_RAYLEIGH: vec3<f32> = vec3<f32>(0.036, 0.097, 0.241); // zenith optical depth R,G,B
const TAU_MIE: f32 = 0.010;
const SUN_POWER: f32 = 30.0;
const MOON_POWER: f32 = 0.22;
const AIRMASS_CAP: f32 = 3.0;
// Chappuis-band ozone absorption (zenith optical depth R,G,B): keeps the
// twilight zenith blue-violet instead of letting a red-only transmittance
// paint the whole sky one orange.
const TAU_OZONE: vec3<f32> = vec3<f32>(0.020, 0.052, 0.002);
// Sunlight scattered into upward rays enters higher up, so it crossed less
// air than light scattered near the ground along the horizon.
const HIGH_RAY_PATH: f32 = 0.35;

fn rayleigh_phase(mu: f32) -> f32 { return 0.0596831 * (1.0 + mu * mu); }
fn mie_phase(mu: f32) -> f32 {
    let g = 0.76; let g2 = g * g;
    return (1.0 - g2) / (12.5663706 * pow(max(1.0 + g2 - 2.0 * g * mu, 0.002), 1.5));
}
// Kasten & Young (1989) relative air mass for a cosine of the zenith angle.
fn airmass(cos_zenith: f32) -> f32 {
    let c = clamp(cos_zenith, 0.0, 1.0);
    let zenith = degrees(acos(c));
    return 1.0 / (c + 0.50572 * pow(96.07995 - zenith, -1.6364));
}
fn sun_direction_true() -> vec3<f32> {
    return vec3<f32>(camera.color_space.z, camera.color_space.w, 0.0);
}
// Fraction of a light's top-of-atmosphere colour that survives the path to the
// ground: orange-red for a low sun.
fn light_transmittance(light_y: f32) -> vec3<f32> {
    return exp(-(TAU_RAYLEIGH + vec3<f32>(TAU_MIE)) * airmass(light_y) * 0.9);
}
fn sky_light_transmittance(light_y: f32, ray_y: f32) -> vec3<f32> {
    let am = airmass(light_y);
    let path = mix(1.0, HIGH_RAY_PATH, sqrt(clamp(ray_y, 0.0, 1.0)));
    return exp(-((TAU_RAYLEIGH + vec3<f32>(TAU_MIE)) * path + TAU_OZONE) * am * 0.9);
}
fn scatter_from(ray: vec3<f32>, light: vec3<f32>, power: f32) -> vec3<f32> {
    let mu = dot(ray, light);
    let tau = TAU_RAYLEIGH + vec3<f32>(TAU_MIE);
    let view_mass = min(airmass(ray.y), AIRMASS_CAP);
    // Light below the horizon fades out through twilight instead of snapping off.
    let twilight = smoothstep(-0.26, 0.03, light.y);
    let phase = (TAU_RAYLEIGH * rayleigh_phase(mu) + vec3<f32>(TAU_MIE * mie_phase(mu))) / tau;
    let path = vec3<f32>(1.0) - exp(-tau * view_mass);
    return power * twilight * sky_light_transmittance(light.y, ray.y) * phase * path;
}
fn vanilla_sky(ray: vec3<f32>) -> vec3<f32> {
    let horizon = pow(1.0 - clamp(abs(ray.y), 0.0, 1.0), 3.0);
    return mix(srgb_to_linear(camera.sky.rgb), srgb_to_linear(camera.fog.rgb), horizon)
        + vec3<f32>(0.003, 0.006, 0.014) * camera.color_space.y;
}
// Linear HDR sky radiance along a camera-relative ray. Also the fog colour.
fn atmosphere_sky(ray: vec3<f32>) -> vec3<f32> {
    let sun = sun_direction_true();
    // The sun's single-scatter term replaces the vanilla gradient by day and
    // fades back to it through twilight, so the vanilla night/dusk sky (and
    // the fog colour baked into it) is never dimmed by a model that has no sun.
    let day = smoothstep(-0.26, 0.03, sun.y);
    // The physical term dominates more as the sun gets low, where the vanilla
    // gradient (blue at every hour) would wash the sunset out.
    let weight = min(camera.atmosphere.x * mix(1.2, 1.0, smoothstep(0.0, 0.25, sun.y)), 1.0) * day;
    let sky = mix(vanilla_sky(ray), scatter_from(ray, sun, SUN_POWER), weight);
    let moon = scatter_from(ray, camera.moon_direction.xyz, MOON_POWER) * camera.moon_direction.w
        * vec3<f32>(0.62, 0.78, 1.0) * camera.atmosphere.x;
    return sky + moon;
}
// Colour of the dominant light for surface shading (sun, or cool moonlight).
fn dominant_light_tint() -> vec3<f32> {
    let sun = sun_direction_true();
    if dot(camera.sun_direction.xyz, sun) > 0.99 {
        let t = light_transmittance(sun.y);
        return pow(t / max(t.r, 1.0e-4), vec3<f32>(0.6));
    }
    return vec3<f32>(0.62, 0.78, 1.0);
}
