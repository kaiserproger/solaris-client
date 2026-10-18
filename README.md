# Solaris native client

Rust/wgpu client for Minecraft Java 26.1.2. This is an **offline/test-identity preview**, not Microsoft authentication or full vanilla parity. Start an offline server separately before joining.

## Quick start

```sh
cargo build --release
./target/release/solaris-client setup --from-dist ./dist --assets "$HOME/.local/share/PrismLauncher/assets"
./target/release/solaris-client
./target/release/solaris-client --server 127.0.0.1:25566 --username Player
./target/release/solaris-client --help
```

Setup links existing local content; it does not download or redistribute Mojang bytes. Keep `dist` and the Prism assets cache in place. On this workstation the build's ALSA/pkg-config prerequisites are described in [PLAYING.md](PLAYING.md).

No arguments opens Title; `--server HOST[:PORT]` joins directly (default port `25565`, default username `Player`). `--width W --height H --fullscreen` controls the initial window. The existing `play` subcommand and automation flags remain supported; `play --help` also prints usage.

Content defaults to `$XDG_DATA_HOME/solaris-client`, or `~/.local/share/solaris-client` when XDG_DATA_HOME is unset. Pass `--data-dir DIR` to both setup and launch to choose another root. The layout is `facts/`, `entities/`, `data/vanilla/`, `resources/assets/`, and `assets/{asset-index.json,objects/}`. Setup pins `dist/assets/asset-index.json` and links Prism's `objects/`; it does not guess a different asset index. Missing startup files produce an error naming the path before opening a window.

Existing `SOLARIS_CLIENT_*` content variables remain optional per-input overrides (including `SOLARIS_CLIENT_DATA_DIR`, which means the **vanilla metadata directory**, not the whole new root). Explicit environment paths win over data-root discovery; no environment exports are required after setup. Exe-adjacent `client.toml` still supplies connection/window defaults, but its saved address only prefills Title unless `--server` is supplied.
