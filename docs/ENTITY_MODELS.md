# Solaris entity models

Entity and block-entity geometry is **authored by Solaris** and shipped inside the client binary
(`assets/solaris/models/entity/<id>.json`, embedded by `build.rs`, baked by
`src/entities/solaris_models.rs`). Entity metadata (protocol ids, dimensions, metadata accessors,
texture/variant selection, head/body turn limits) is the Solaris-owned data table
`assets/solaris/entities/entity-facts.json`, embedded via `include_str!` in
`src/entities/resources.rs`. Nothing is read from `dist/entities` any more.

Models are UV-compatible with the vanilla entity textures, which the player supplies at runtime.
Authors derive boxes from the unwrapped regions of the vanilla PNG; no Mojang numbers, code or
mesh dumps are copied. Sleeping living entities use received pose and sleeping-position metadata
with the facing of the bed block in the received world. Passenger links bend the legs using the
ordinary rig animation.

## File format (schema 1)

```json
{
  "schema": 1,
  "id": "cow",
  "texture": {"path": "entity/cow/cow_temperate.png", "size": [64, 64]},
  "roles": {"head": ["head"], "right_leg": ["right_hind_leg", "left_front_leg"], "left_leg": [...]},
  "bones": [
    {"name": "head", "pivot": [0, 4, -8], "rotation": [0, 0, 0], "scale": [1, 1, 1],
     "cubes": [{"origin": [-4, -4, -6], "size": [8, 8, 6], "uv": [0, 0], "mirror": false,
                "inflate": 0, "faces": {"front": [u0, v0, u1, v1]}}],
     "bones": []}
  ]
}
```

* Lengths are pixels (1/16 block), +Y down, feet at `y = 24`, front is -Z, model -X is the
  entity's right. `pivot` is relative to the parent bone; cube `origin` relative to its bone;
  rotations are degrees (Z, then Y, then X).
* `texture.path` is relative to `assets/minecraft/textures/` and names the model's default vanilla
  texture; `size` must equal the PNG size. Runtime texture choice for variants (colour, profession,
  baby, tamed...) stays with the family/variant code and only needs the same layout.
* Cubes use the classic box unwrap (`uv` + size `w x h x d`); `mirror` mirrors the cube about its
  centre keeping texel assignment; `inflate` grows the box; `faces` replaces a face's rectangle
  (swapped corners flip) for flat planes.
* `roles` lists the bones the generic animation drives: `head` (look pitch/yaw), `right_leg` /
  `left_leg` (opposite swing phase; quadrupeds list diagonal pairs), `right_arm` / `left_arm`,
  `wing` (flap), `crossed_arms` (villager-family arms bone: held-item anchor of the source
  `CrossedArmsItemLayer`; not animated) and `nose` (witch nose: the potion anchor, tilts forward
  while the main hand holds an item). Bone names must be unique; code also special-cases `body` (sneak) and the
  humanoid names for armor stands and first-person arms.

## Rig keys

The renderer asks for rigs by key (`minecraft:cow#renderer_main`, `minecraft:cow_baby#main`,
`minecraft:oak_chest_boat#renderer/model`, `minecraft:zombie#layer3/modelSet/head`...).
`solaris_models::model_id` maps a key to a model id (file stem): qualifiers are dropped, wood
variants collapse (`boat`, `chest_boat`, `raft`, `chest_raft`, every `*_minecart` -> `minecart`),
armor-layer slots map to `armor_<slot>` / `armor_baby_<slot>`.

### Sign text belongs to the block-entity pass

In 26.3 the resource-pack block models supply standing, wall and hanging sign
bodies through terrain. The block-entity renderer supplies only front/back text,
using the pinned sign text transformations. It must not request the removed
`entity/signs` textures or a second body rig; those dependencies prevent any text
from being emitted even while the terrain board is visible.

Sign mesh regressions deliberately provide no legacy sign texture or body rig.
They receive both NBT text payload forms, assert text material indices and outward
winding on both faces, and reject duplicate body indices. Ordinary non-glowing
and glowing text are covered; windowed pixels remain a separate acceptance gate.

Chest and bell rigs use the same +Y-down authoring convention as living
entities. Their block-entity roots convert that space to world coordinates;
the bell's two cuboids follow the body and rim regions of its supplied texture.
Double-chest halves each occupy their own block and share the centered latch.
The base's sides start one texel below its own top rim row (faces `v = 34`), so
the lid's bottom rim is the single dark band at the seam; the latch hangs two
pixels above and two below that seam.

### Oracle-measured silhouettes

Box sizes and UV rectangles are authored by inspecting the texture nets supplied
by the local resource pack, not by importing a game model or renderer. Relative
positions are fitted to the Java oracle screenshots: the golem is 43 authored
model units tall, with its head on the shoulder line and its arms ending 3.5
units above the feet; the child's nose covers the lowest two face rows.
These are our reconstruction parameters, not extracted Mojang model constants.
Tests in `solaris_models/tests.rs` pin that authored reconstruction.

### Labels and shadows

`text_display` entities (server settlement labels) are drawn by the name-tag pass
(`render_nameplates.rs`): received text, line width, billboard, transformation,
background, opacity, style flags and brightness override; the backdrop rests on the
entity position and the text is lit by the received light. Living entities cast a
blob shadow (`shadows.rs`): the runtime-supplied `misc/shadow` disc on full-block
tops under the feet. Opacity 0.5, height fade 2 blocks, distance fade 16 blocks
and squared light falloff are chosen approximation parameters, not measured
vanilla formulae. The radius fit (`0.55 * width + 0.17`, golem 0.7) and extra baby
factor 0.5 approximate the observed discs; other mobs are not individually
measured yet.

Text-display style bits and packed brightness are interpreted from server-sent
metadata, using the existing display transform owner. Font scale 0.025 reuses the
authored nameplate convention; the 10-unit line step and bottom-aligned backdrop
are fitted to the visible settlement label. These values are not read from Java
bytecode. The NoAI head-glance gate uses the server's mob flags rather than
inventing a local look target for a stationary oracle fixture.

## Missing models

`Models::rig(key)` never fails. A key with no authored model renders a **magenta, entity-sized
placeholder box** (family rigs are sized from the entity's bounding box at load; other keys use
a fixed box). Placeholders are tinted in `presentation::emit_rig`.

## Tooling and tests

* `python3 tools/entity_models.py fmt|check|preview` - canonical formatting, validation
  (schema, UV bounds, texture existence/size, unique bones, roles, texel coverage) and a textured
  four-view + UV-map preview using the vanilla PNG (`SOLARIS_TEXTURES` overrides the texture root).
* `cargo test --lib solaris_models` - geometry winding/unwrap/mirror tests, key resolution, and a
  sanity test over every shipped model (UVs inside the declared texture, roles/bones valid, and -
  when vanilla textures are available under `SOLARIS_CLIENT_RESOURCE_ROOT` or `dist/resources` -
  the texture exists with the declared size).

## Coverage and known limits

* Authored models cover players (wide/slim), the humanoid and skeleton families, illagers/villagers,
  farm and wild animals with their baby layouts, aquatic mobs, Nether/End mobs, vehicles,
  projectiles, armor shells, armor stand and the block entities the renderer draws (chests,
  beds, banners, skulls, bell, book, conduit, decorated pot, shulker box, shield).
  `sheep_wool` / `sheep_wool_baby` are the dyed coat drawn by `src/entities/sheep.rs`
  (undercoat tinted body once sheared).
* Several creatures are simplified silhouettes (ender dragon, wither, warden, guardian,
  nautilus, sniffer, parrot...). Overlay-only PNG layers (outer slime/drowned layers, stray/bogged
  overlays, horse markings/saddles) are not modelled.
* Quality tier is visible in `python3 tools/entity_models.py check` coverage: classic nets
  (cow, wolf, cat, creeper, skeleton family, humanoids...) are near 100%; fitted models list the
  deliberately unmapped texels.
* Entity metadata is Solaris-owned data: `assets/solaris/entities/entity-facts.json` (embedded in
  the binary; protocol ids, dimensions, metadata accessors, texture/variant selection, head/body
  turn limits, armor shell keys, motion rules). Pure tables, no meshes/UV/animation keyframes;
  the launcher needs no JDK and nothing is derived from the client jar for entities.
