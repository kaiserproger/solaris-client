# Open validation issues

## CLIENT-LINT-001 — inherited strict Clippy debt (open, quarantined)

The 2026-10-28 protocol-777 cutover and the known-green full-suite baseline
`f5ac9a4` both report **428 library / 452 library-test errors** under:

```sh
cargo clippy --release -p solaris-client --all-targets --message-format=json -- -D warnings
```

Both commands used the same toolchain/build-support environment and the required
`mem-run.sh -m 12G` envelope. The complete multiset of **(lint code, primary source
path)** is identical: 973 primary spans, zero additions and zero removals.
New placement-test module ordering and sign-decoder needless-borrow diagnostics
were corrected rather than added to this quarantine.

Exact local receipts are `.analysis/native263/clippy-baseline.jsonl`,
`.analysis/native263/final-lint-correction.log` and
`.analysis/native263/clippy-final-comparison.json`. Existing failures include
`unused_imports`, `dead_code`, `private_interfaces` and Clippy diagnostics; none
are suppressed or declared fixed. Earlier `65a7f76` comparison receipts remain
historical evidence in `.analysis/clippy-base.log` and `.analysis/clippy.log`.
Strict Clippy is **not green**.

## CLIENT-TEST-001 — historical graphics-preset assertion (superseded)

The earlier `65a7f76` receipt recorded
`ui::state::client_settings_tests::cycles_wrap_and_clamp` failing because the
assertion expected `"medium"` after advancing the default Medium preset.
The current complete workspace run passes that same test and all ordinary
tests: **627 client-library + 6 CLI + 90 launcher + 1 path test**, zero failures.
The two ignored client tests are the existing
`motion::tests::bench_motion_cost` and the explicitly external-data
`protocol::packet_ids::tests::all_packet_ids_match_26_3_report`.
The latter was run separately against the explicit local 26.3 report: one
passing test, all 255 packet IDs checked. No assertion was weakened or skipped
to obtain the ordinary suite result. Receipt:
`.analysis/native263/final-acceptance.log`; final sign-decoder closure cleanup
additionally passes all five component regressions in
`.analysis/native263/final-lint-correction.log`.

## CLIENT-GPU-001 — default-driver parallel GPU-test SIGSEGV (open)

The earlier validation history already recorded a parallel GPU-suite SIGSEGV
before this cutover. The reported new SIGSEGV is retained verbatim in
`.analysis/native263/historical-gpu-segv.log`. This is classified as a
pre-existing/environment-dependent GPU-test failure, not a demonstrated new
protocol regression or a claimed driver fix. The exact driver/concurrency
cause remains unproven.

The complete current workspace suite passes with
`VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/radeon_icd.json` (receipt above).
The generic GPU implementation/dependencies were unchanged against `f5ac9a4`;
protocol-specific inline painting presentation is separately regression-tested.
This driver-qualified green result does not establish every backend or physical
display scanout. Windowed lifecycle evidence uses an isolated Xvfb/lavapipe
surface and is labeled accordingly.

## CLIENT-INV-001 — Shift-double-click bulk quick move (open)

The RC.1 inventory-input review found that Shift-click quick-moves the hovered
stack, but Shift-double-click does not bulk-move the other matching stacks from
the same container. This is distinct from s6-03's plain double-click collection
onto the cursor (`PICKUP_ALL`). The local 26.3
`AbstractContainerScreen.mouseReleased` bytecode iterates matching slots for
the Shift variant and emits `QUICK_MOVE`. The client pointer path still only
emits the individual Shift-clicks. No fix or live verification is claimed for
this inherited shortcut gap.

