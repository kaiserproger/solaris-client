# Native launcher redesign

## Scope

This change redesigns the **launcher**, not the game's renderer or gameplay UI.
It retains the Rust/eframe 0.33.3 native OpenGL shell and existing content
bootstrap, download verification and client-process launch paths. It introduces
no additional dependencies, Microsoft authentication, web engine or image service.
The client remains an offline/test-identity preview, not full vanilla parity.

## Interface

- Home has a selected server, a compact offline-profile row, game version and a
  bottom Play/status area. Profile and server editing lives in dialogs.
- Settings has General, Appearance, Minecraft and Advanced sections; file repair
  is under Advanced. Logs remains a separate page.
- The default window size is 820 x 530 logical pixels, with a 640 x 460 minimum.
  Normal window dimensions are saved; the OS owns window decorations.
- Appearance contains 40 built-in palettes, searchable previews, category filters,
  favorites and a validated custom-theme editor with JSON import/export. Export
  creates a new file and refuses to overwrite an existing one.
- An optional Home banner uses an original Solaris illustration or a local PNG.
  Reduced motion disables decorative transitions and indeterminate spinners;
  progress and status still update.
- Local skin previews accept 64 x 64 / 64 x 32 PNGs and integer HD multiples up to
  1024 pixels wide. The installed Steve texture is used when available and no
  custom skin is selected; otherwise an explicitly labeled original Solaris
  avatar is shown. This preview does **not** change the player's in-game skin.

## Ownership and safety boundaries

| Module | Responsibility |
| --- | --- |
| `launcher/src/ui.rs` | Shell composition, dialogs, install/process phases and publication of saved selections |
| `launcher/src/config.rs` | Backward-compatible defaults, normalization, settings I/O and checked game-setting updates |
| `launcher/src/theme.rs` | Semantic color roles, custom-theme validation and bounded JSON interchange |
| `launcher/src/theme_presets.rs` | Built-in palette catalogue and stable IDs |
| `launcher/src/media.rs` | Bounded local PNG decoding, head/hat composition, illustrations and texture caching |
| `launcher/src/motion.rs` | Finite demand-driven reveals and button paint with stable hitboxes |
| `launcher/src/ui_tests.rs` | Production-widget composition and simulated-input regressions |

Profile/server selection and editing, theme application/deletion and favorites
save a candidate configuration **before** publishing the corresponding active
state. A failed save keeps the active selection and the editable dialog draft.
Play stops if its prerequisite settings save fails rather than erasing the error
and starting work anyway.

Game-setting updates reread the current `client-settings.json` and preserve its
other keys. Missing files may be created; corrupt, non-object or unreadable files
are not replaced with display defaults. Launcher editing of game settings is
blocked while installation or the launched game is active, because the game owns
its own settings snapshot. Each launch reads the current persisted fullscreen
value instead of applying a stale launcher snapshot. This is not cross-process
coordination for independently started clients or multiple launcher instances.

Custom-theme ID changes migrate favorites. Structurally damaged custom-theme
entries are discarded individually without resetting otherwise valid Appearance
settings. Long custom-theme names are truncated after reserving action space.

Local images have a 16 MiB encoded-input limit, a 64 MiB RGBA-output limit and a
4096-pixel per-side limit. Upload also checks the native texture-size limit.
Malformed, truncated, animated and oversized PNGs are rejected. Cache identity
includes the selected file revision, texture manager and device texture limit.
Original/installed Mojang resources are not added to Git or redistributed.

## Verification performed

The implementation was reviewed against upstream baseline
`2edbb5904c960c3e3501e4e265c208f4089ad494` on Windows using Rust 1.95.0.

- Initial launcher suite: **75 passed**.
- Audited launcher suite: **90 passed, 0 failed**.
- Strict package Clippy: passed with warnings denied.
- Rust formatting for the eight changed launcher source files and
  `git diff --check`: passed.
- Independent final read-only source review: no actionable security or logic
  errors found in the changed GUI scope. This is source review, not native QA.
- A separate debug launcher executable was built; its `--help` ran successfully.
  A real Windows launcher window opened and Home was captured. Automated native
  clicks were not confirmed, so a complete native interaction pass is **not**
  claimed.

The audit added executable failing-before/passing-after regressions for failed
profile/theme/favorite/selection/deletion saves, prerequisite-save handling,
corrupt game settings, competing launcher writes, stale fullscreen, damaged
custom-theme entries, favorite-ID migration, long-name action visibility and
native texture-limit rejection. Additional minimum-window coverage passed without
requiring production changes.

The package tests also cover legacy-config migration, all 40 theme choices and
persistence, contrast, custom-theme validation/import/export, PNG bounds/cache
lifecycle, finite motion, layout at default/minimum logical sizes and simulated
DPI, worker failure and late cancellation preventing autoplay. Existing manifest,
checksum and local-HTTP download tests remain included. Simulated frames and DPI
are not evidence of OS-level input, scaling, IME or presentation correctness.

### Reproduce the automated gates

From the repository root, with the pinned toolchain:

```sh
CARGO_INCREMENTAL=0 nice -n 19 cargo test --locked -p solaris-launcher -j 2
CARGO_INCREMENTAL=0 nice -n 19 cargo clippy --locked -p solaris-launcher --all-targets --no-deps -j 2 -- -D warnings
rustfmt --check --edition 2024 --config skip_children=true launcher/src/config.rs launcher/src/main.rs launcher/src/ui.rs launcher/src/ui_tests.rs launcher/src/media.rs launcher/src/motion.rs launcher/src/theme.rs launcher/src/theme_presets.rs
git diff --check
```

These are launcher-scoped checks, not a full-workspace gameplay or release gate.

## Remaining acceptance and known limitations

- Complete a real native mouse/keyboard pass through profiles, servers, themes,
  custom-theme import/export, background settings, logs and error recovery.
- Check actual Windows DPI/window controls/focus behavior and Linux native
  presentation. Headless logical-size checks do not close these gates.
- Repeat first-run installation, cancellation/retry and a real game launch with
  authorized resources and server access. No fabricated installed markers or fake
  game process were used to claim this coverage.
- Local PNG metadata reads and decoding remain synchronous on the UI thread.
  Limits bound input/output sizes, not I/O latency on a slow or network path.
- The unchanged blocking HTTP backend cannot immediately interrupt an already
  blocked send/read. Cancel may wait for a network timeout; late cancellation
  prevents autoplay, but is not an instant-abort guarantee.
- Full-workspace tests, new gameplay/server compatibility, hardware-audible output
  and release packaging are not established by this launcher audit. Existing
  compatibility-matrix stages and the separate P0 work cursor are unchanged.

These boundaries make the change suitable for a **draft review PR**, not a claim
of complete native/release acceptance.
