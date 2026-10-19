# Solaris entity models

Entity and block-entity geometry is **authored by Solaris** and shipped inside the client binary
(`assets/solaris/models/entity/<id>.json`, embedded by `build.rs`, baked by
`src/entities/solaris_models.rs`). Entity metadata (protocol ids, dimensions, metadata accessors,
texture/variant selection, head/body turn limits) is the Solaris-owned data table
`assets/solaris/entities/entity-facts.json`, embedded via `include_str!` in
`src/entities/resources.rs`. Nothing is read from `dist/entities` any more.

Models are UV-compatible with the vanilla entity textures, which the player supplies at runtime.
Authors derive boxes from the unwrapped regions of the vanilla PNG; no Mojang numbers, code or
mesh dumps are copied.

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
  `wing` (flap). Bone names must be unique; code also special-cases `body` (sneak) and the
  humanoid names for armor stands and first-person arms.

## Rig keys

The renderer asks for rigs by key (`minecraft:cow#renderer_main`, `minecraft:cow_baby#main`,
`minecraft:oak_chest_boat#renderer/model`, `minecraft:zombie#layer3/modelSet/head`...).
`solaris_models::model_id` maps a key to a model id (file stem): qualifiers are dropped, wood
variants collapse (`boat`, `chest_boat`, `raft`, `chest_raft`, every `*_minecart` -> `minecart`),
signs map to `sign_<attachment>` / `hanging_sign_<attachment>`, armor-layer slots to
`armor_<slot>` / `armor_baby_<slot>`.

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

* 187 models ship: players (wide/slim), the humanoid and skeleton families, illagers/villagers,
  farm and wild animals with their baby layouts, aquatic mobs, Nether/End mobs, vehicles,
  projectiles, armor shells, armor stand and the block entities the renderer draws (chests,
  beds, signs, banners, skulls, bell, book, conduit, decorated pot, shulker box, shield).
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
