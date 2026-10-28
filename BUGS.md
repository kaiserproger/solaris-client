# Open validation issues

## CLIENT-LINT-001 — inherited strict Clippy debt (open, quarantined)

Baseline `65a7f76` and the client-input changes both report **428 library
diagnostics** under `cargo clippy --release -p solaris-client --all-targets --
-D warnings`. The changed tree also reports 452 library-test diagnostics; the
baseline run stopped after the library, so no baseline library-test total is
claimed. Commands ran under the required `mem-run.sh -m 6G` envelope.

Coordinator Bump263Server approved retaining existing debt provided touched
paths introduce no new diagnostics. The five touched Rust paths have the same
diagnostic message multiset as the baseline (13 total, zero additions):

| Baseline source location | Exact diagnostic |
|---|---|
| `src/render.rs:226` | field `instance` is never read |
| `src/render.rs:157` | needlessly taken reference of left operand |
| `src/render.rs:931` | this function has too many arguments (11/7) |
| `src/session/observation.rs:333` | this `if` statement can be collapsed |
| `src/session/observation.rs:737` | this `if` statement can be collapsed |
| `src/session/observation.rs:962` | you seem to want to iterate on a map's values |
| `src/session/observation.rs:1097` | float equality check without `.abs()` |
| `src/session/observation.rs:1102` | float equality check without `.abs()` |
| `src/lib.rs:668` | this `if` statement can be collapsed |
| `src/lib.rs:1219` | unneeded `return` statement |
| `src/lib.rs:971` | this `if` statement can be collapsed |
| `src/lib.rs:1226` | this `if` statement can be collapsed |
| `src/lib.rs:1246` | this `if` statement can be collapsed |

Other existing crate-wide failures include `unused_imports`, `dead_code`,
`private_interfaces`, and Clippy style/correctness diagnostics outside these
paths; they are not suppressed or declared fixed by the pacing patch. Exact
local receipts are `.analysis/clippy-base.log`, `.analysis/clippy.log`, and
`.analysis/clippy-comparison.json`. To reproduce the baseline without changing
the working tree, archive `65a7f76` into an ignored directory and run the same
strict command there with the same toolchain and build-support environment.

This quarantine is not a claim that strict Clippy is green or that the client
input/pacing behavior has passed graphical acceptance.

## CLIENT-TEST-001 — default graphics-preset cycle assertion (open, quarantined)

`ui::state::client_settings_tests::cycles_wrap_and_clamp` fails at
`src/ui/state.rs:4093`: `next_setting_value("graphics_preset", None)` returns
`"high"`, while the assertion expects `"medium"`. The implementation first
resolves the default preset (`Medium`) and then advances the cycle, so the
assertion disagrees with the existing implementation. Neither behavior nor
assertion is changed by the pointer/pacing slice.

The complete `src/ui/state.rs` and `src/render/graphics.rs` files are byte-identical
to `65a7f76` (SHA-256 `b865dddfe033d8c85090cbf8aad4989b62d0612a3d3a67b07fed61e30118ccbb`
and `1cb49f45f5b22a097c0ff9ea1ed7cc18f9eb233710afbf4246e9fb873ca4bd55`,
respectively). This records provenance, not a baseline test execution.
No assertion is weakened or skipped. Reproduce with
`cargo test -p solaris-client --lib -- --test-threads=1` under `mem-run.sh -m 6G`.

Far-program tests additionally require the local, non-committed
`tests/fixtures/far-program-seed81.zlib` produced by the server's
`lod_program_exports_without_the_seed` test with `SOLARIS_LOD_PROGRAM_FIXTURE`.
The local fixture used here has SHA-256
`60027474361e68ca68c4f55c95ac6e5a0b9d019e1d4eb8220c38bac048fb263f`.
Missing this prerequisite fails three tests; it is not addressed by dropping
their coverage. The default parallel GPU-test run also terminated with SIGSEGV;
the serial run completed and exposed the actual individual failures.
With the real fixture installed and the tooltip length regression corrected, the
complete serial run finished with **597 passed, 1 failed, 1 ignored**; only the
preset-cycle assertion above remains red. Exact receipt:
`.analysis/tests-final.log`. The focused window-policy test command passes all
three tests; FPS averaging, FIFO mode selection and F3 formatting also pass in
the complete serial run.
