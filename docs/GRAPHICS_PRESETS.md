# Graphics presets and shader design passes

Client setting `graphics_preset` (Options > Video > Graphics, cycles Low → Medium → High; persisted
client-locally; default `low`, the vanilla look). It replaces the old boolean `fancy_shaders`; a
persisted `fancy_shaders: false` migrates once to `low`, `true` to `medium`.

| Pass / feature | Low | Medium | High |
| --- | --- | --- | --- |
| Vanilla display-encoded pipeline (no HDR post, no shadows) | yes | – | – |
| HDR scene, bloom, ACES + sRGB | – | yes | yes |
| Atmosphere sky + fog (one model, see below) | – | yes | yes |
| Directional shadows | – | 1 map, 2048², r = 80 | 3 cascades, 2048² each, r = 18 / 52 / 144 |
| God rays (half-res radial, depth-occluded) | – | 12 taps | 24 taps |
| Water | vanilla | sky reflection + depth tint | + refraction of the opaque scene, Beer–Lambert absorption, screen-space reflection with sky fallback |
| TAA | – | – | 8-sample Halton jitter, Catmull-Rom history, variance clipping |

Low is the default because the client must look like vanilla Minecraft; Medium and High are opt-in
shader looks. Low is also the GT 1030 target: it runs none of the optional HDR design passes and
keeps world colours display-encoded instead of applying bloom or ACES.
Presets switch at runtime (`Renderer::set_graphics_preset` rebuilds cascade array, scene copy, post
targets and TAA history); a resize reallocates the same set.

The HUD darkness vignette uses the runtime-loaded `misc/vignette` texture and multiplicative
blending below the HUD. Its target follows received camera-block light and dimension ambient
light; a 1% per-tick easing rate was fitted to paired Java/native night/day captures. Low stars
use brightness squared, fitted to their observed contribution over the night sky. No game
implementation or model data was copied for either change. Sky colour and fog use the client
render-distance option capped by the server-announced chunk radius.
The vignette and inverting crosshair have dedicated UI index partitions; neither changes the
normal alpha blend of the three ordinary overlay layers.

## Atmosphere (`src/render/atmosphere.wgsl`)

Analytic single scattering: Rayleigh + Mie with Kasten–Young air mass along the view and the light
path, for the sun and (additively, cooler/weaker) the moon. Low suns lose blue through their own path
extinction, so sunsets/sunrises redden along the sun-side horizon and the anti-solar side stays
cooler without authored gradients. The vanilla camera sky/fog colours (biome, weather, dimension)
remain the second term of the blend, so the result stays recognisably Minecraft; rain removes the
physical term; underwater/special fog (`fog_start < 0`) keeps vanilla fog colours.
`atmosphere_sky(ray)` is used for the sky pass, terrain fog (distant terrain dissolves into the exact
pixel the sky drew behind it), and water's reflection fallback.
Water tint uses the received sky-and-block lightmap, like the underlying terrain. Sky reflections
and glints use the sky-only lightmap component, so block lights can illuminate water without
inventing a bright daytime sky reflection at night or under a roof. Medium retains more of the
animated vanilla water texture, with subtler surface waves. High includes the texture's surface
opacity in its refraction transmission, so shallow water still has a visible surface.

Sunlight scattered into upward rays enters higher in the atmosphere and crossed less air than light
scattered along the horizon (`HIGH_RAY_PATH`), and Chappuis-band ozone absorption (`TAU_OZONE`) is
applied along the full sun path, so at sunset the zenith stays blue-violet while the sun-side horizon
goes orange (`twilight_sky_is_orange_at_the_sun_and_blue_overhead`).

## Shadows (`src/render/shadow.rs`)

Cascades are sphere-fitted around the camera (rotation-stable) and texel-snapped in world space, so
shadow edges do not swim as the camera moves (`cascade_centre_is_texel_snapped_in_world_space`).
Each cascade has its own caster list/indirect range and its own depth pass (timing slots
`directional_shadow`, `directional_shadow_cascade1/2`); lookups pick the nearest covering cascade with
a blend band and normal-offset/texel-scaled bias.

## TAA caveats

No motion vectors: moving entities and water rely on the neighbourhood clamp (faster history decay
on large reprojected motion). Hi-Z and frustum culling use the unjittered matrix (jitter < 0.5 px).
High water draws opaque (alpha 1) over a copy of the opaque scene, so glass behind water is not
visible through it.

## Timing slots

`timing::PASSES`: `directional_shadow` (12), `bloom_horizontal` (13), `bloom_vertical` (14),
`aces_tonemap` (15, also the Low copy), `god_rays` (21), `taa_resolve` (22),
`directional_shadow_cascade1` (23), `directional_shadow_cascade2` (24).

## Tests

`src/render/design_tests.rs` (real GPU): sunset warmth, Low sky untouched, fog ends on the sky pixel,
god rays vs occluder, shadows in every cascade count, High water, jitter, TAA convergence/reprojection,
runtime preset switching, plus unit tests in `graphics.rs`/`shadow.rs`/`post.rs`.
`src/render/shader_tests.rs` covers multiplicative vignette output without changing normal UI
alpha blending, and the observed Low star contribution. `environment/vignette.rs` covers easing
and dimension ambient light; session regressions cover server-capped render distance and sampling
open sky above stored light sections.
Capture tooling for the before/after receipts: `.analysis/shaderdesign/capture_poses.py`.

## Receipts (RTX 4050 laptop, shared machine, plains render distance 12, uncapped fps)

`.analysis/shaderdesign/run-matrix.sh` captures the same six poses (horizon-noon, sunset, lake-shore,
underwater, forest-a, forest-b) for the pre-work Hi-Z client with `fancy_shaders` on and for the three
presets; `summarize.py` writes the per-pass GPU-ms table (`report/gpu-ms.md`), `sheets.py` the
side-by-side screenshots (`report/*.png`). Median total GPU ms over poses (timestamp-query sum):
before 3.3-5.7, Low 1.8-3.9, Medium 3.2-6.3, High 5.0-7.6. High adds TAA ~0.57 ms, cascades 1+2 ~1.9 ms
and god rays ~0.09 ms while its near cascade is cheaper than the old single map. The GT 1030 was not
available; Low is the preset for it.
