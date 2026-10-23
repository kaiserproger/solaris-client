"""Live creative search-tab gate: real XTest keyboard/pointer input plus MCP queries.

Usage: creative_search_live.py RUNTIME_DIR DISPLAY EVIDENCE_DIR [--before]

Creative-mode client on an own Xvfb display (no WM). Opens the creative inventory,
clicks the Search tab, types `stone` with real key events, checks the filtered
palette over MCP (every shown item contains `stone`), exercises Backspace and
Ctrl+A, drives the same field through `minecraft_set_screen_input`, captures the X
root window after each step, and writes `result.json`. `--before` only records the
screenshots and the observed state (the unfixed client is expected to fail checks).
"""
import glob
import json
import pathlib
import subprocess
import sys
import time

sys.path.insert(0, str(pathlib.Path(__file__).parent))
from mcp_client import Client
from x11_input import Display

runtime, display, evidence = pathlib.Path(sys.argv[1]), sys.argv[2], pathlib.Path(sys.argv[3])
before = "--before" in sys.argv
evidence.mkdir(parents=True, exist_ok=True)
client = Client(pathlib.Path(sorted(glob.glob(str(runtime / "*.json")))[0]), evidence)
client.initialize()
x = Display(display)
x.window()
x.focus()
x.move(x.width // 2, x.height // 2)
time.sleep(1.0)  # let the native Focused event arrive before automation actions
CREATIVE = "net.minecraft.client.gui.screens.inventory.CreativeModeInventoryScreen"


def tool(name, **args):
    return client.tool(name, args)


def nodes():
    return tool("minecraft_observe")["ui_nodes"]


def items():
    return [n for n in nodes() if n["id"].startswith("creative/item/")]


def shot(name):
    path = evidence / f"{name}.png"
    subprocess.run(["import", "-display", display, "-window", "root", str(path)], check=True)
    return str(path)


def wait_for(predicate, what, timeout=10.0):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        value = predicate()
        if value:
            return value
        time.sleep(0.05)
    raise SystemExit(f"timeout waiting for {what}")


def search_node():
    return next((n for n in nodes() if n["id"] == "creative/search"), None)


checks = []


def check(name, ok, detail=""):
    checks.append({"check": name, "ok": bool(ok), "detail": detail})
    print("PASS" if ok else "FAIL", name, detail)


tool("minecraft_send_chat", message="/gamemode creative")
wait_for(lambda: tool("minecraft_observe")["screen"]["class"] == "none", "game screen")
time.sleep(0.5)
tool("minecraft_open_inventory")
wait_for(lambda: tool("minecraft_observe")["screen"]["class"] == CREATIVE, "creative screen")
shot("01-creative-open")

tab = next(
    (n for n in nodes() if n["id"] == "creative/tab/minecraft:search" and n["role"] == "button"), None
)
check("search tab reachable on screen", tab is not None and tab.get("visible", True), json.dumps(tab))
scale = None
if tab:
    bx, by, bw, bh = tab["bounds"]
    # ui_nodes bounds are in physical pixels; click the tab centre with a real pointer.
    x.click(bx + bw / 2, by + bh / 2)
    time.sleep(0.3)
shot("02-search-tab")
field = wait_for(search_node, "search field node") if tab else None
check("search field exists after clicking tab", field is not None)
check("search field focused after clicking tab", field and field["focused"], json.dumps(field))

unfiltered = [n["label"] for n in items()]
before_count = len(unfiltered)
x.type_text("stone")
time.sleep(0.5)
shot("03-typed-stone")
typed = items()
labels = [n["label"] for n in typed]
check("typing 'stone' changes the palette", typed and labels != unfiltered, f"{unfiltered[:3]} -> {labels[:3]}")
check(
    "every shown item matches 'stone'",
    typed and all("stone" in l.lower() or "stone" in n["id"] for l, n in zip(labels, typed)),
    json.dumps(labels[:12]),
)

x.key("BackSpace")
time.sleep(0.3)
after_backspace = items()
check("Backspace widens the filter ('ston')", len(after_backspace) >= len(typed), f"{len(typed)} -> {len(after_backspace)}")
shot("04-backspace")

x.key("a", "Control_L")
x.type_text("dirt")
time.sleep(0.4)
dirt = [n["label"] for n in items()]
check("Ctrl+A then typing replaces the query", dirt and all("dirt" in l.lower() for l in dirt), json.dumps(dirt[:8]))
shot("05-ctrl-a-replace")

# MCP-driven typing through the same field.
mcp = tool(
    "minecraft_set_screen_input",
    expected_screen_class=CREATIVE,
    expected_title="",
    input_label="Search Items",
    value="diamond",
)
time.sleep(0.4)
diamond = [n["label"] for n in items()]
check("MCP set_screen_input filters live", diamond and all("diamond" in l.lower() for l in diamond), json.dumps(diamond[:6]))
shot("06-mcp-diamond")

x.key("BackSpace")
x.key("BackSpace")
x.key("BackSpace")
x.key("BackSpace")
x.key("BackSpace")
x.key("BackSpace")
x.key("BackSpace")
time.sleep(0.4)
check("backspacing to empty restores the full palette", [n["label"] for n in items()] == unfiltered, "")

result = {"before": before, "checks": checks, "pass": all(c["ok"] for c in checks)}
(evidence / "result.json").write_text(json.dumps(result, indent=1))
print("OVERALL", "PASS" if result["pass"] else "FAIL")
sys.exit(0 if result["pass"] else 1)
