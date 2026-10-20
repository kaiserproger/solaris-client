# Solaris patch to winit 0.30.12

Upstream crate: `winit` 0.30.12 (Apache-2.0), trimmed of examples/tests/docs and
dev-dependencies so it builds as a path dependency via `[patch.crates-io]`.

Only source change: `src/platform_impl/linux/x11/event_processor.rs`,
`xinput2_button_input`: core-protocol wheel buttons 4..=7 are reported as
`MouseWheel` for both press and release, so one notch moved two hotbar slots.
The release is now ignored (the press is the notch, as in GLFW). Still present
in upstream 0.30.13. Drop this vendor copy once upstream fixes it.
