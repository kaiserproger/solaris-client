"""Live chest-rig gate: give chest items, place one, open it, open inventory; the client
session must survive (no 'special model rig not activated').

Usage: chest_rig_live.py INSTANCE.json EVIDENCE_DIR DISPLAY (e.g. :148)
"""
import json, pathlib, sys, time

sys.path.insert(0, str(pathlib.Path(__file__).parent))
from mcp_client import Client

descriptor, evidence = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2])
evidence.mkdir(parents=True, exist_ok=True)
client = Client(descriptor, evidence)
client.initialize()
log = []


import hotbar_wheel_xtest as xt

dpy = xt.x11.XOpenDisplay(sys.argv[3].encode())
window = xt.pick(dpy)
xt.focus_and_point(dpy, window)


def step(name, fn):
    try:
        result = fn()
        log.append({"step": name, "ok": True, "result": str(result)[:300]})
        print("OK  ", name, str(result)[:160], flush=True)
        return result
    except Exception as error:  # noqa: BLE001
        log.append({"step": name, "ok": False, "error": str(error)[:400]})
        print("FAIL", name, str(error)[:300], flush=True)
        return None


def alive():
    try:
        return client.tool("minecraft_observe")
    except Exception as error:  # noqa: BLE001
        return {"dead": str(error)}


step("wait_for_play", lambda: client.tool("minecraft_wait_for_play", {"timeout_seconds": 60}))
obs = alive()
player = obs.get("player", {})
pos = [player["x"], player["y"], player["z"]] if "x" in player else (player.get("position") or player.get("server_position"))
print("position", pos, flush=True)
step("give", lambda: client.tool("minecraft_send_chat", {"message": "give @s minecraft:chest 4", "command": True}))
step("give_ender", lambda: client.tool("minecraft_send_chat", {"message": "give @s minecraft:ender_chest 1", "command": True}))
step("give_trapped", lambda: client.tool("minecraft_send_chat", {"message": "give @s minecraft:trapped_chest 1", "command": True}))
step("wait_chest", lambda: client.tool("minecraft_wait_for_inventory", {"item_id": "minecraft:chest", "count": 1, "timeout_seconds": 20}))
step("inv_open", lambda: client.tool("minecraft_open_inventory", {}))
time.sleep(1.0)
step("alive_after_inventory", alive)
step("inv_close", lambda: client.tool("minecraft_close_screen", {}))
x, y, z = int(pos[0] // 1), int(pos[1] // 1), int(pos[2] // 1)
for _ in range(60):
    if alive()["player"].get("on_ground"):
        break
    time.sleep(0.5)
x, y, z = (int(alive()["player"][k] // 1) for k in ("x", "y", "z"))
support = None
for dy in range(4, -12, -1):
    block = client.tool("minecraft_read_block", {"x": x + 2, "y": y + dy, "z": z})
    if not block["is_air"] and "water" not in block["block_id"]:
        support = y + dy
        break
print("support y", support, flush=True)
step("select_chest", lambda: client.tool("minecraft_select_hotbar_item", {"item_id": "minecraft:chest", "count": 1}))
step("place", lambda: client.tool("minecraft_use_item_on", {"x": x + 2, "y": support, "z": z, "face": "up"}))
time.sleep(1.0)
step("read_block", lambda: client.tool("minecraft_read_block", {"x": x + 2, "y": y, "z": z}))
step("open_chest", lambda: client.tool("minecraft_use_item_on", {"x": x + 2, "y": support + 1, "z": z, "face": "up"}))
time.sleep(2.0)
final = alive()
step("alive_with_container", lambda: final)
step("screenshot", lambda: client.tool("minecraft_screenshot", {"path": str(evidence / "screenshots" / "chest-open.png")}))
step("close", lambda: client.tool("minecraft_close_screen", {}))
ok = "dead" not in alive()
json.dump({"log": log, "session_alive_at_end": ok}, open(evidence / "chest-live-result.json", "w"), indent=2)
print("SESSION ALIVE" if ok else "SESSION DEAD")
