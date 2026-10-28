# Playing Solaris with the native client

Target: Minecraft Java Edition 26.3-compatible Solaris server. Offline/test
identity only (`offline-test` profile); official Microsoft auth is NOT
implemented (see Checklist row `connect-auth` in
`.analysis/native-ui-20261008/RESULT.md`).

## 1. Start the server

Harness path (owns server/Xvfb/client lifecycle, picks a free port, writes a
receipt under `/home/kaiserroman/solaris/.analysis/validation/`). It needs the
content env from `docs/AGENT_TESTING.md` (`SOLARIS_CLIENT_DATA_DIR`,
`_RESOURCE_ROOT`, `_ASSET_ROOT`, `_ASSET_INDEX` plus the fact files); the
Title-screen leg is a separate profile that also needs a built server:

```sh
cd /home/kaiserroman/solaris
python3 -m tools.harness run survival --client native --windowed --timeout-seconds 120
SOLARIS_SERVER_BINARY=$PWD/target/debug/solaris-v2 \
  python3 -m tools.harness run title-connect --client native --windowed --timeout-seconds 120
```

Manual server (same binary the harness drives; v1 `mc-server` is retired, the
server is `solaris-v2` from `mc-edge`):

```sh
cd /home/kaiserroman/solaris
# build once; on the shared host wrap it: .analysis/tools/mem-run.sh -m 6G <cmd>
cargo build -p mc-edge --bin solaris-v2
# offline loopback profile: 127.0.0.1:25565, view/simulation distance 4,
# world .analysis/test-world-v12, content cache data/vanilla
target/debug/solaris-v2 --config playable.toml
```

`stop` on its stdin (or Ctrl+C) writes the final checkpoint. Edit
`playable.toml` (`port`, `world_dir`, `seed`) for another address or world.

## 2. Start the client

`dist/solaris-client` is a copied binary and goes stale: rebuild the current
source and refresh it before playing (needs the ALSA sysroot env below):

```sh
cd /home/kaiserroman/solaris-client
export PKG_CONFIG_PATH=$PWD/.analysis/build-support/usr/lib/x86_64-linux-gnu/pkgconfig
export LIBRARY_PATH=$PWD/.analysis/build-support/usr/lib/x86_64-linux-gnu
export LD_LIBRARY_PATH=$PWD/.analysis/build-support/usr/lib/x86_64-linux-gnu
CARGO_INCREMENTAL=0 /home/kaiserroman/solaris/.analysis/tools/mem-run.sh -m 6G nice -n 19 cargo build --release -j 2
cp target/release/solaris-client dist/solaris-client
```

The player-ready bundle reads resource, username and address defaults next to
the executable. Flags/environment still win.

- `./dist/solaris-client play` opens the Title screen even when `client.toml`
  supplies a server. Choose **Multiplayer** for the saved list, **Add Server**
  to persist an address, or **Direct Connect** to type an address and offline
  username.
- An explicit `--server` retains direct joining:

```sh
./dist/solaris-client play
./dist/solaris-client play --profile offline-test --server 127.0.0.1:25565 --username Steve
```

Saved servers live in `<exe-dir>/servers.json`; managed Options live in
`<exe-dir>/client-settings.json`. These are local JSON files, not server data
or bundled Mojang assets.

After joining, the client stays on a Loading screen until the spawn chunk,
its received light, and the spawn-area meshes have been uploaded and drawn.
Gameplay input is not applied while this screen is visible.

**Singleplayer** starts the bundled `solaris-v2` (next to the client or in
`<data>/servers/`) on a free loopback port for a world under `<data>/worlds/`;
the pause menu then reads **Save & Quit to Title** and stops the server
gracefully so the world checkpoints. See README.md for the full flow.

Disconnect in the pause menu returns to Title. A failed connection or kick
shows a separate Connection Lost screen with the reason and Back to Title.
Esc opens/closes Pause; resuming gameplay recaptures the mouse. F3 toggles
the debug overlay; block-break cracks render on the targeted block.

The local view has speed-scaled walking bob, a smoothed sprint FOV and smooth
sneak eye-height transitions. First-person arms and held-item models use the
activated vanilla resources; ordinary attack, use and mining drive their swing,
equip and bob animation. F5 cycles first-person, rear third-person and front
third-person. The third-person camera pulls inward against received collision
shapes rather than viewing through walls. Remote living models use interpolated
motion for walking limbs, body/head turns and hurt flashes; this does not claim
complete special-pose or entity-family parity.

Entity nameplates use received custom-name metadata and player profile names,
including scoreboard team formatting and visibility. Ordinary named entities
show their label when targeted or when `CustomNameVisible` is set; armor stands
follow vanilla's stricter rule and require `CustomNameVisible` even when targeted.
Labels billboard toward the camera within vanilla's 64-block range; discrete
living-entity labels use the shorter 32-block range and dim, depth-tested text.
Their background uses the text-background option, independently of chat-only
backgrounds. These rendering rules do not imply complete entity-family parity.

Build env: `cargo` in the client repo needs the vendored ALSA sysroot (the
three `export` lines in section 2), else `alsa-sys` fails to configure.

## 3. Controls

| Input | Action |
|---|---|
| Left click (window) | capture mouse / attack (hold to mine); click UI buttons and slots |
| Right click | use / place block; split a stack in inventory/container UI |
| WASD / Space / Shift / Ctrl | move / jump / sneak / sprint |
| Mouse | look (sensitivity in Options) |
| 1–9 / wheel (game) | hotbar select |
| Wheel (GUI list) | scroll the hovered list |
| E | inventory open/close |
| Shift + click (inventory/container) | quick-move a stack |
| Hold + drag (inventory/container) | distribute a carried stack across slots |
| T | chat open · `/` opens command chat · Esc closes · Enter sends |
| Q | drop one item |
| F | swap offhand |
| Esc (game) | pause menu (Back to game / Advancements / Statistics / Options / Accessibility / Disconnect) |
| Esc (pause/options/chat/inventory) | back |
| F2 | screenshot to artifact dir |
| F3 | debug overlay (FPS/XYZ/facing/chunk/biome/entities/day/phase) |
| F5 | cycle first-person / third-person rear / third-person front |
| Window close | disconnect + quit cleanly |

Windowed agent playthrough (2026-10-16, `survival --client native
--windowed`, artifact `20261016T090849-survival-o9kznayi`): hearts/HUD →
F3 → inventory preview → chat → pause/Options with applied GUI scale →
physical XTest inventory movement at scale 2 (server-confirmed 20-item fixture) →
4096-cell scan in ten bounded pages → block crack → death → loaded respawn.
The Title leg (type address + name, Connect, Disconnect, reconnect without
retyping) passed in `20261016T090947-title-connect-hlujwkso`.
These are software-rendered Xvfb agent checks, not human play or hardware FPS evidence.

Motion polish (render-only; `src/motion.rs`, `src/session/block_motion.rs`):
frame-time springs/easing drive landing dip (scaled by fall), strafe lean (±1.2°),
first-person hand idle sway, turn inertia, use/place bump, eased equip and
eat/drink pose, and a quick-strike attack swing; remote mobs get landing squash,
idle breathing, a short hit lean, and an idle head glance at a player within six
blocks; a mid-tick position packet now continues from the pose on screen instead
of snapping; block-break fragments glide and shrink out; placed blocks pop in
(110 ms) and doors/trapdoors swing (200 ms) through the terrain moving-block
path (the cell meshes as air until the mesh catches up). Nothing here feeds
physics, raycasts or packets; `minecraft_observe.player.motion` exposes the
live spring state for evidence.

Options (Esc → Options, persisted to `<exe-dir>/client-settings.json`):
Render distance 2–16, FOV 30–110, mouse sensitivity 0.1–3.0, Master volume
0–1, damage tilt strength 0–1 (default 1), GUI scale (Auto by default; the largest
integer scale retaining at least 320×240 logical pixels).
Options also expose Fullscreen, VSync, Max FPS (30–260 or Unlimited), and
Shaders (Fancy/Off). These values persist with the other managed settings.

UI theme: all menus use the warm translucent pixel theme (stepped-corner
dark-amber panels over the live scene; the Title slowly pans its backdrop).
Options are Sodium-style: category tabs (Video / Controls / Audio / Interface)
on the left, label + slider or switch rows on the right, and a one-line
description of the hovered/focused row at the bottom. Sliders take mouse
drag, wheel (one stop per notch) and ←/→ (Shift ×5, Home/End); Enter on a
focused row cycles it. Every change applies instantly and persists as above.
Wheel over a switch or the row margin scrolls the Interface list. Menu
motion (hover glow/lift, press, panel fade+slide, tab slide, row stagger,
slider thumb and switch knob easing, hotbar selection slide) is driven by the
frame clock, runs 110–250 ms with ease-out curves, repaints the overlay only
while something moves, and drops the slides (fades only) with Interface →
Reduced motion.
The unfinished player-inventory recipe-book panel and floating Close button
are hidden; E/Esc closes the clean vanilla inventory frame. Typed recipe
actions and other container functionality remain separate from book pixels.
Received local-player HurtAnimation / DamageEvent restart a ten-tick vanilla camera tilt without
changing look/movement. MCP exposes `minecraft.player.hurt_tilt_deg`,
`hurt_direction`, `hurt_tick`, and `camera.hurt_tilt_deg` / `damage_tilt_strength`
(degrees, current client tick). Death screen: Respawn / Title screen (disconnect).
Containers (chest, furnace, crafting table, enchanting, anvil, …), recipe
book, and creative inventory work with mouse (click / shift-click /
right-click split / drag-distribute) plus hotbar number keys.

Received worn armor on players, zombie/skeleton families and piglins uses the
activated per-slot humanoid armor models and equipment textures, including baby
models/textures, leather dye (including black), and the existing enchantment
glint pass. Armor-stand/small-stand equipment remains supported. Armor trims are
not rendered: their generated palette atlas/decal pass is not activated.

Advancements (Esc → Advancements) groups received rows under displayed root
tabs (including story, nether, end, adventure and husbandry when sent by the
server). Server-selected tabs are honored. Mouse wheel scrolls the list;
MCP uses the same tab buttons `advancement/tab/<root-id>` and scroll target
`advancements/list` to reach rows `advancement/<advancement-id>`. Completed rows
use an ASCII `[Done]` marker so completion stays readable with the client font.

Statistics (Esc → Statistics) requests the server's counters and waits for the
received response. General shows custom statistics with vanilla distance, time
and damage formatting; Items and Mobs show named count lists. Tab changes and
scrolling do not request statistics again. Done or Escape returns to the pause
menu. Automation uses the same Statistics button and screen nodes as human
input; the observation's `statistics` field also exposes received raw counters
and their formatted values, separately from locally predicted gameplay.
