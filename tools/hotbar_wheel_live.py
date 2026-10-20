"""Live hotbar-wheel gate: real XTest button 4/5 press+release notches against a
running client (Xvfb + local server); every notch must move the server-confirmed
slot exactly once, including pairs delivered back to back.

Usage: hotbar_wheel_live.py RUNTIME_DIR/INSTANCE.json DISPLAY EVIDENCE_DIR
"""
import json, pathlib, subprocess, sys, time

sys.path.insert(0, str(pathlib.Path(__file__).parent))
from mcp_client import Client

INJECTOR = pathlib.Path(__file__).parent / "hotbar_wheel_xtest.py"
descriptor, display, evidence = pathlib.Path(sys.argv[1]), sys.argv[2], pathlib.Path(sys.argv[3])
client = Client(descriptor, evidence)
client.initialize()


def slot():
    return client.tool("minecraft_observe")["player"]["selected_hotbar_slot"]


def settle(expected, timeout=5.0, quiet=0.4):
    """Wait for `expected`, then require it to stay put (no late extra step)."""
    deadline = time.monotonic() + timeout
    value = slot()
    while value != expected and time.monotonic() < deadline:
        time.sleep(0.02)
        value = slot()
    if value != expected:
        return value
    end = time.monotonic() + quiet
    while time.monotonic() < end:
        value = slot()
        if value != expected:
            return value
        time.sleep(0.02)
    return value


def inject(direction, count, burst):
    args = [sys.executable, str(INJECTOR), display, direction, str(count)]
    if burst:
        args.append("burst")
    return subprocess.run(args, capture_output=True, text=True, check=True).stdout.strip().splitlines()[-1]


client.tool("minecraft_set_hotbar_slot", {"slot": 8})
assert settle(8) == 8, "could not seed slot 8"

# (direction, notches, burst, expected slot after); down = +1 slot, up = -1, wrapping 0..8
steps = [("down", 1, False), ("up", 1, False), ("up", 1, False), ("up", 1, False),
         ("down", 1, False), ("down", 1, False), ("down", 1, False),
         ("down", 2, True), ("up", 2, True), ("down", 3, True), ("up", 3, True),
         ("up", 2, True), ("down", 2, True)]
current, results = 8, []
for direction, count, burst in steps:
    expected = (current + (count if direction == "down" else -count)) % 9
    before = slot()
    injected = inject(direction, count, burst)
    observed = settle(expected)
    results.append({"before": before, "wheel": direction, "notches": count, "burst": burst,
                    "observed": observed, "expected": expected, "ok": before == current and observed == expected,
                    "injector": injected})
    print("PASS" if results[-1]["ok"] else "FAIL",
          f"{before} --{count} notch{'es' if count > 1 else ''} {direction}{' burst' if burst else ''}--> {observed} (expect {expected})")
    current = observed
ok = all(r["ok"] for r in results)
(evidence / "result.json").write_text(json.dumps({"pass": ok, "results": results}, indent=1))
print("OVERALL", "PASS" if ok else "FAIL")
sys.exit(0 if ok else 1)
