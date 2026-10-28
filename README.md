# Solaris native client

A Rust/wgpu client for Minecraft Java Edition **26.3**, with a small launcher that sets everything up for you. This is an **offline/test-identity preview**: there is no Microsoft login yet and it is not full vanilla parity. It is built to play on Solaris servers.

## Install and play

1. Download the archive for your system from the project's Releases page:
   - Linux x86_64: `solaris-client-<version>-linux-x86_64.tar.gz`
   - Windows x86_64: `solaris-client-<version>-windows-x86_64.zip`
2. Unpack it anywhere (keep `solaris-launcher` and `solaris-client` in the same folder).
3. Run the launcher: `./solaris-launcher` (Linux) or double-click `solaris-launcher.exe` (Windows).
   Linux desktop entry (optional): `./install-desktop.sh` adds "Solaris Launcher" to your application menu; `./install-desktop.sh --uninstall` removes it.
4. Use **Profiles** to choose or edit an **offline username** and **Servers** to choose a server (the **Solaris test** server is pre-filled), then press **Play**.
5. **First run only:** the launcher downloads about **480 MB** and unpacks about **1 GB** into your data folder (see below), with a progress bar and a log. Later launches go straight into the game without touching the network. No Java is needed to play on a server.

The download is not bundled with the launcher: no Mojang files are ever shipped in this project's releases or repository. On first run your machine fetches them itself:

| What | From | Size |
|---|---|---|
| Minecraft 26.3 client jar (textures, models, fonts, lang) | Mojang (`piston-meta.mojang.com`, SHA-1 verified) | ~40 MB |
| Game assets (sounds, languages) | Mojang (`resources.download.minecraft.net`, SHA-1 verified) | ~440 MB |
| Solaris data tables (block/item facts, collision shapes) | Solaris static server (SHA-256 **pinned inside the launcher**) | ~1 MB |

Downloads are parallel, resumable (an interrupted run continues from the partial files) and every file is hash-checked. If anything is damaged use **Settings → Advanced → Verify / repair game files**.

### Launcher appearance

The native launcher opens at 820×530 logical pixels, resizes down to 640×460, and remembers its normal window size. Home keeps server/profile editing in compact dialogs rather than permanent forms.

**Settings → Appearance → Themes** contains 40 coordinated palettes with real interface miniatures, search, Dark/Light/Colorful filters and favorites. Click a preview or its name to apply it immediately; the choice is saved in `launcher.json`. Graphite is the default warm-accent dark theme. Custom themes support validated colors, editing, reset, deletion and JSON import/export (export never overwrites an existing file).

**Appearance → Background & motion** enables an optional landscape banner, accepts a local PNG, and offers reduced motion. The bundled default is an original Solaris illustration, not a Minecraft screenshot. Images and settings stay local; no image service or sign-in is required.

**Profiles → Local skin PNG** previews a 64×64/64×32 skin (or an integer HD multiple) with its hat layer. Without a selected file, the launcher uses the installed Steve texture when present, otherwise a clearly labeled original Solaris avatar. This is a launcher preview only: it does not change the skin used by an offline server. See [launcher verification and remaining acceptance gates](docs/LAUNCHER_REDESIGN.md).

### Connect to the Solaris test server

The launcher ships with a server entry `Solaris test` = `135.106.208.217:25565` (Minecraft Java 26.3, offline mode). Select it, choose a username (3–16 Latin letters, digits or `_`, as vanilla offline names) and press PLAY. Use **+ Add server** to add others (`host` or `host:port`).

Without the launcher: `solaris-client --server 135.106.208.217:25565 --username YourName`.

### Singleplayer

**Singleplayer** on the title screen lists your worlds (`worlds/<name>` in the data folder) with last-played time and size; **Create New World** takes a name, an optional seed (blank = random, text is hashed like vanilla), game mode (survival/creative) and difficulty. **Play Selected World** starts the bundled Solaris server (`solaris-v2`, next to the client or in `servers/`) on a free loopback port, joins it as the title screen's username and makes that player the operator. **Delete** asks for confirmation, **Open Folder** opens `worlds/`. In game, the pause menu's **Save & Quit to Title** stops the server gracefully so the world is saved.

The first Play derives the server's content cache from the Mojang `server.jar` with a Java runtime (a few minutes, shown on a progress screen). A server player never downloads either: on that first singleplayer Play the client asks `solaris-launcher --singleplayer-runtime` (next to the client) to fetch the server jar (~60 MB) and a Temurin JDK 25 (~135 MB), once. The server binds an OS-assigned loopback port and the client reads the real port from its `is listening` line. If stopping the server fails or has to be killed, the error screen shows the end of its log ("world may not be fully saved"). Each world's server log is `worlds/<name>/logs/server.log`; if the server dies, the error screen shows the end of that log. Mode and difficulty are applied once with ordinary operator commands on the first join; later changes are yours via `/gamemode` and `/difficulty`. The bundled plugin pack is loaded without `solaris-auth` (offline, local operator).

### Server dialogs

The client renders vanilla 26.1.2 server-driven dialogs (`show_dialog`/`clear_dialog`): `notice`, `confirmation` and `multi_action` with plain-message bodies and `text`, `number_range`, `boolean` and `single_option` inputs. Buttons answer with `custom_click_action` (`dynamic/custom` and `custom` actions carry the typed input values; `run_command` actions send the templated command). `after_action`, `can_close_with_escape` and Escape-as-cancel follow vanilla. Not rendered: `server_links`/`dialog_list`, item bodies, button tooltips and vanilla's "custom screen" warning button. The server side is documented in the Solaris repo (`docs/DIALOGS.md`).

### Far terrain

On Solaris servers with `[lod]` enabled the client draws a coarse heightfield of the overworld beyond the render distance, fading into the sky. `far_terrain` in `client-settings.json` sets its reach in blocks (2048 by default, 1024 with the Low graphics preset, 0 turns it off; the server caps it). It is built from the chunks you received, the server's stored chunks and, for never-generated land, the server's terrain generator. Only if the server operator enables `share_terrain_keys` does the server send its generation program and this machine generate never-generated land itself, on the GPU (or a CPU thread without compute support). Tiles are cached as heights and colours under `far-terrain/`. `SOLARIS_CLIENT_FAR_TERRAIN=<blocks>` overrides the setting; `SOLARIS_CLIENT_FAR_GPU=0` forces CPU generation.

## System requirements

- 64-bit Linux (glibc 2.35 or newer, e.g. Ubuntu 22.04+) or Windows 10/11 x86_64.
- A GPU and driver with **Vulkan 1.1+** (Intel, AMD or NVIDIA; software Vulkan via Mesa `lavapipe` also works but slowly). The launcher window itself needs OpenGL 3.3.
- 4 GB RAM, ~1.5 GB free disk, working audio output (ALSA/PulseAudio/PipeWire on Linux).
- Internet access on first run.

## Where things live

The data folder is `$XDG_DATA_HOME/solaris-client` (default `~/.local/share/solaris-client`) on Linux and `%APPDATA%\solaris-client` on Windows; **Settings → General → Open data folder** shows it. Override with `solaris-launcher --data-dir DIR`.

```
facts/ data/vanilla/                  Solaris data tables (entity metadata and models ship inside the client)
resources/assets/                     textures/models/fonts unpacked from your client.jar
assets/asset-index.json, objects/     Mojang asset index and sound/language objects
mojang/versions/26.3/               official version json and client.jar
mojang/versions/26.3/server.jar     singleplayer only: fetched on its first Play (with `jdk/`)
jdk/                                  singleplayer only: managed Temurin JDK that runs the content import
servers/                              singleplayer: derived server content cache, plugin copy, optional solaris-v2
worlds/<name>/                        singleplayer worlds (world data, server.toml, ops.json, logs/server.log)
far-terrain/<server>/overworld/       far terrain tiles (heights and colours) per server
launcher.json  client-settings.json   your profiles/servers, and in-game Options
logs/launcher.log                     everything the launcher and the client printed
```

Render distance and fullscreen in the launcher's Settings use the same `client-settings.json` as the in-game Options screen. Launcher editing is disabled while installation or the launched game is active; use in-game Options while playing. Each launch reads the current persisted fullscreen value, and launcher edits refuse to overwrite corrupt or unreadable game settings with defaults. This does not coordinate independently started clients or multiple launcher instances.

## Troubleshooting

- **No window / "Failed to create wgpu adapter" / black window.** The client needs Vulkan. Check `vulkaninfo --summary`; install your GPU's Vulkan driver (Linux: `mesa-vulkan-drivers`, or NVIDIA's proprietary driver). On Windows update the GPU driver. `WGPU_BACKEND=gl` selects the OpenGL fallback if Vulkan is broken. Set it in your shell, then start the launcher from that shell.
- **Launcher window does not open or is garbled.** It needs OpenGL 3.3 (`LIBGL_ALWAYS_SOFTWARE=1 ./solaris-launcher` forces Mesa software rendering). Check `glxinfo -B`.
- **Wayland vs X11 (Linux).** Both work. If a window is missing, scaled wrongly or has no input under Wayland, force X11 with `WAYLAND_DISPLAY= WINIT_UNIX_BACKEND=x11 ./solaris-launcher`; to force Wayland unset `DISPLAY`. The game inherits the launcher's environment.
- **Download fails / stalls.** Press PLAY again: it resumes. Behind a proxy set `HTTPS_PROXY`/`HTTP_PROXY` (also used for the Solaris data download over plain HTTP). **Verify / repair** re-checks every file. The full log is in the Logs tab and `logs/launcher.log`.
- **"refusing content bundle … does not match the hash pinned in this launcher".** The Solaris data on the server changed after this launcher was built. Download the newest launcher release. The launcher never installs a bundle it was not built to trust.
- **Singleplayer's first Play fails while downloading the Java runtime.** Only singleplayer needs it (multiplayer never does): the client runs `solaris-launcher --singleplayer-runtime` from beside itself, which needs `api.adoptium.net`. Or point `SOLARIS_JDK` at an existing JDK 25 (a folder with `bin/java` and `javac`) and set `SOLARIS_LAUNCHER_BIN` if the launcher is not next to the client.
- **Windows SmartScreen warns about an unsigned app.** The preview is unsigned: "More info → Run anyway".
- **`Client exited with …`.** See the Logs tab; the client's output is prefixed `[client]`.
- **Start fresh.** Quit everything and delete the data folder.

## Command line

```
solaris-launcher                                  open the window
solaris-launcher --headless-bootstrap [--force] [--play]
                 [--data-dir DIR] [--manifest URL] [--server HOST[:PORT]] [--username NAME]
solaris-client [--server HOST[:PORT]] [--username NAME] [--width W --height H] [--fullscreen] [--data-dir DIR]
solaris-client --help
```

`--headless-bootstrap` does the whole first-run install without a window (CI, servers; `--play` then starts the game and waits). `--autoplay` / `--tab settings|logs` are window-mode helpers for testing.

## Building from source

```sh
cargo build --release          # builds solaris-client and solaris-launcher into target/release/
```

`rust-toolchain.toml` pins Rust 1.95.0. The client links ALSA on Linux (`libasound2-dev`); the launcher uses OpenGL/X11/Wayland via eframe. On the shared dev host the build is wrapped as described in [PLAYING.md](PLAYING.md).

CI (`.github/workflows/release.yml`) builds the Linux tar.gz and Windows zip on every `v*` tag and attaches them with `SHA256SUMS`; it asserts that no `.jar` or `assets/` directory is in the package.

### Publishing the data bundle

The launcher installs exactly one data bundle, pinned by name and SHA-256 in `launcher/src/pins.rs`. To publish a new revision:

```sh
tools/publish_facts.sh <rev>      # builds target/publish/solaris-facts-26.3-r<rev>.tar.zst + solaris-content.json
```

Upload both files to `http://135.106.208.217:8080/static/`, then set `FACTS_NAME`/`FACTS_SHA256` in `launcher/src/pins.rs` to the printed values and release a new launcher. The bundle holds only pure data tables (block/item/interaction facts, collision shapes, registry and item-default reports); textures and sounds are never published (they come from the user's own downloaded client), and entity models/metadata are Solaris-authored data inside the client binary.

### Developer content setup (no launcher)

```sh
./target/release/solaris-client setup --from-dist ./dist --assets "$HOME/.local/share/PrismLauncher/assets"
./target/release/solaris-client
```

`setup` links an existing locally prepared bundle and Prism assets cache into the data folder. `SOLARIS_CLIENT_*` variables optionally override individual content paths (see [docs/AGENT_TESTING.md](docs/AGENT_TESTING.md)); `SOLARIS_CLIENT_DATA_DIR` is the **vanilla metadata directory** (`data/vanilla`), not the whole root. Missing startup files produce an error naming the path before a window opens. `SOLARIS_CLIENT_SETTINGS` overrides where Options are saved (default: next to the executable; the launcher points it at the data folder).

No arguments opens the Title screen; `--server HOST[:PORT]` joins directly (default port `25565`, default username `Player`). The `play` subcommand and automation flags remain supported (`play --help`).

## Legal

Solaris is an independent client. It contains no Mojang code or assets; everything from Mojang is downloaded by the user's own machine from Mojang's official endpoints, as any third-party launcher does. Minecraft is a trademark of Mojang AB / Microsoft; this project is not affiliated with them. Licensed MIT OR Apache-2.0.
