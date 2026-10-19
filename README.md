# Solaris native client

A Rust/wgpu client for Minecraft Java Edition **26.1.2**, with a small launcher that sets everything up for you. This is an **offline/test-identity preview**: there is no Microsoft login yet and it is not full vanilla parity. It is built to play on Solaris servers.

## Install and play

1. Download the archive for your system from the project's Releases page:
   - Linux x86_64: `solaris-client-<version>-linux-x86_64.tar.gz`
   - Windows x86_64: `solaris-client-<version>-windows-x86_64.zip`
2. Unpack it anywhere (keep `solaris-launcher` and `solaris-client` in the same folder).
3. Run the launcher: `./solaris-launcher` (Linux) or double-click `solaris-launcher.exe` (Windows).
   Linux desktop entry (optional): `./install-desktop.sh` adds "Solaris Launcher" to your application menu; `./install-desktop.sh --uninstall` removes it.
4. Pick or type an **offline username**, pick a server (the **Solaris test** server is pre-filled) and press **PLAY**.
5. **First run only:** the launcher downloads about **670 MB** and unpacks about **1 GB** into your data folder (see below), with a progress bar and a log. Later launches go straight into the game without touching the network.

The download is not bundled with the launcher: no Mojang files are ever shipped in this project's releases or repository. On first run your machine fetches them itself:

| What | From | Size |
|---|---|---|
| Minecraft 26.1.2 client jar (textures, models, fonts, lang) and server jar | Mojang (`piston-meta.mojang.com`, SHA-1 verified) | ~95 MB |
| Game assets (sounds, languages) | Mojang (`resources.download.minecraft.net`, SHA-1 verified) | ~440 MB |
| Temurin JDK 25 (one-off, derives entity models from your own client jar) | Adoptium (`api.adoptium.net`, SHA-256 verified) | ~135 MB |
| Solaris data tables (block/item facts, collision shapes) | Solaris static server (SHA-256 **pinned inside the launcher**) | ~1 MB |

Downloads are parallel, resumable (an interrupted run continues from the partial files) and every file is hash-checked. If anything is damaged use **Settings → Verify / repair game files**.

### Connect to the Solaris test server

The launcher ships with a server entry `Solaris test` = `135.106.208.217:25565` (Minecraft Java 26.1.2, offline mode). Select it, choose a username (3–16 letters, digits or `_`) and press PLAY. Use **+ Add server** to add others (`host` or `host:port`).

Without the launcher: `solaris-client --server 135.106.208.217:25565 --username YourName`.

## System requirements

- 64-bit Linux (glibc 2.35 or newer, e.g. Ubuntu 22.04+) or Windows 10/11 x86_64.
- A GPU and driver with **Vulkan 1.1+** (Intel, AMD or NVIDIA; software Vulkan via Mesa `lavapipe` also works but slowly). The launcher window itself needs OpenGL 3.3.
- 4 GB RAM, ~1.5 GB free disk, working audio output (ALSA/PulseAudio/PipeWire on Linux).
- Internet access on first run.

## Where things live

The data folder is `$XDG_DATA_HOME/solaris-client` (default `~/.local/share/solaris-client`) on Linux and `%APPDATA%\solaris-client` on Windows; Settings → **Open** shows it. Override with `solaris-launcher --data-dir DIR`.

```
facts/ entities/ data/vanilla/        Solaris data tables + entity data derived on your machine
resources/assets/                     textures/models/fonts unpacked from your client.jar
assets/asset-index.json, objects/     Mojang asset index and sound/language objects
mojang/versions/26.1.2/               official version json, client.jar, server.jar
jdk/                                  managed Temurin JDK (also reserved for singleplayer)
servers/  worlds/                     reserved for singleplayer
launcher.json  client-settings.json   your profiles/servers, and in-game Options
logs/launcher.log                     everything the launcher and the client printed
```

Render distance and fullscreen in the launcher's Settings write the same `client-settings.json` the in-game Options screen uses, so they never disagree.

## Troubleshooting

- **No window / "Failed to create wgpu adapter" / black window.** The client needs Vulkan. Check `vulkaninfo --summary`; install your GPU's Vulkan driver (Linux: `mesa-vulkan-drivers`, or NVIDIA's proprietary driver). On Windows update the GPU driver. `WGPU_BACKEND=gl` selects the OpenGL fallback if Vulkan is broken. Set it in your shell, then start the launcher from that shell.
- **Launcher window does not open or is garbled.** It needs OpenGL 3.3 (`LIBGL_ALWAYS_SOFTWARE=1 ./solaris-launcher` forces Mesa software rendering). Check `glxinfo -B`.
- **Wayland vs X11 (Linux).** Both work. If a window is missing, scaled wrongly or has no input under Wayland, force X11 with `WAYLAND_DISPLAY= WINIT_UNIX_BACKEND=x11 ./solaris-launcher`; to force Wayland unset `DISPLAY`. The game inherits the launcher's environment.
- **Download fails / stalls.** Press PLAY again: it resumes. Behind a proxy set `HTTPS_PROXY`/`HTTP_PROXY` (also used for the Solaris data download over plain HTTP). **Verify / repair** re-checks every file. The full log is in the Logs tab and `logs/launcher.log`.
- **"refusing content bundle … does not match the hash pinned in this launcher".** The Solaris data on the server changed after this launcher was built. Download the newest launcher release. The launcher never installs a bundle it was not built to trust.
- **"Temurin JDK" step fails.** It needs `api.adoptium.net`. Or point `SOLARIS_JDK` at an existing JDK 25 (a folder with `bin/java`, `javac`, `javap`).
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
tools/publish_facts.sh <rev>      # builds target/publish/solaris-facts-26.1.2-r<rev>.tar.zst + solaris-content.json
```

Upload both files to `http://135.106.208.217:8080/static/`, then set `FACTS_NAME`/`FACTS_SHA256` in `launcher/src/pins.rs` to the printed values and release a new launcher. The bundle holds only pure data tables (block/item/interaction facts, collision shapes, registry and item-default reports); entity models, meshes, textures and sounds are never published: they are derived on each user's machine from their own downloaded client.

### Developer content setup (no launcher)

```sh
./target/release/solaris-client setup --from-dist ./dist --assets "$HOME/.local/share/PrismLauncher/assets"
./target/release/solaris-client
```

`setup` links an existing locally prepared bundle and Prism assets cache into the data folder. `SOLARIS_CLIENT_*` variables optionally override individual content paths (see [docs/AGENT_TESTING.md](docs/AGENT_TESTING.md)); `SOLARIS_CLIENT_DATA_DIR` is the **vanilla metadata directory** (`data/vanilla`), not the whole root. Missing startup files produce an error naming the path before a window opens. `SOLARIS_CLIENT_SETTINGS` overrides where Options are saved (default: next to the executable; the launcher points it at the data folder).

No arguments opens the Title screen; `--server HOST[:PORT]` joins directly (default port `25565`, default username `Player`). The `play` subcommand and automation flags remain supported (`play --help`).

## Legal

Solaris is an independent client. It contains no Mojang code or assets; everything from Mojang is downloaded by the user's own machine from Mojang's official endpoints, as any third-party launcher does. Minecraft is a trademark of Mojang AB / Microsoft; this project is not affiliated with them. Licensed MIT OR Apache-2.0.
