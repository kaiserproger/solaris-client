"""Live proof of monitor audio: own server + two real windowed clients on private Xvfb
displays inside a private PipeWire graph (run through tools/screen_audio_graph.sh).

Owner shares a nested app (a looping tone played by `pw-play` into the monitor's private
null sink) on a `solaris:monitor`; the viewer must hear (decode) it ONLY while the owner
granted permission AND the viewer looks at the screen. Nothing is played to a device.

Usage: tools/screen_audio_graph.sh python3 tools/screen_audio_live.py SERVER_BIN CLIENT_BIN EVIDENCE_DIR
"""
import ctypes, json, math, os, pathlib, shutil, subprocess, sys, tempfile, time, wave

sys.path.insert(0, str(pathlib.Path(__file__).parent))
from mcp_client import Client

server_bin, client_bin, evidence = sys.argv[1], sys.argv[2], pathlib.Path(sys.argv[3])
evidence.mkdir(parents=True, exist_ok=True)
work = pathlib.Path(tempfile.mkdtemp(prefix="solaris-sa-live-"))
PORT = int(os.environ.get("SA_PORT", "25893"))
DISPLAYS = {"Owner": ":71", "Viewer": ":72"}
procs = []
log = open(evidence / "driver.log", "w")


def say(*a):
    line = " ".join(str(x) for x in a)
    print(line, flush=True)
    log.write(line + "\n"); log.flush()


def start(cmd, name, **kw):
    out = open(evidence / f"{name}.log", "w")
    p = subprocess.Popen(cmd, stdout=out, stderr=subprocess.STDOUT, **kw)
    procs.append(p)
    return p


def cleanup():
    for p in reversed(procs):
        if p.poll() is None:
            p.terminate()
    for p in reversed(procs):
        try:
            p.wait(5)
        except subprocess.TimeoutExpired:
            p.kill()


# ---------------------------------------------------------------- fixtures
tone = work / "tone.wav"
with wave.open(str(tone), "wb") as w:
    w.setnchannels(2); w.setsampwidth(2); w.setframerate(48000)
    frames = b"".join(
        int(0.4 * 32767 * math.sin(2 * math.pi * 440 * i / 48000)).to_bytes(2, "little", signed=True) * 2
        for i in range(48000 * 2))
    w.writeframes(frames)

plugins = work / "plugins"
shutil.copytree("/home/kaiserroman/wt/monitor-plugins/computer", plugins / "computer")
(work / "server.toml").write_text(f"""[server]
name = "screen-audio-proof"
view_distance = 4
simulation_distance = 4
[network]
bind_address = "127.0.0.1"
port = {PORT}
[auth]
online_mode = false
[plugins]
directory = "{plugins}"
[data]
world_dir = "{work / 'world'}"
vanilla_data_dir = "/home/kaiserroman/solaris/data/vanilla"
seed = 0
""")

# ---------------------------------------------------------------- server
say("server", server_bin)
server = start([server_bin, "--config", str(work / "server.toml")], "server", cwd="/home/kaiserroman/solaris",
               stdin=subprocess.PIPE, text=True)


def console(line):
    say("console>", line)
    server.stdin.write(line + "\n"); server.stdin.flush()


def wait_log(path, needle, timeout):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        if needle in pathlib.Path(path).read_text(errors="replace"):
            return
        if server.poll() is not None:
            raise SystemExit(f"server exited {server.returncode}")
        time.sleep(0.25)
    raise SystemExit(f"timeout waiting for {needle!r} in {path}")


wait_log(evidence / "server.log", "isten", 600)

# ---------------------------------------------------------------- X11 helpers
x11 = ctypes.CDLL("libX11.so.6"); xtst = ctypes.CDLL("libXtst.so.6")
x11.XOpenDisplay.restype = ctypes.c_void_p
x11.XOpenDisplay.argtypes = [ctypes.c_char_p]
x11.XKeysymToKeycode.argtypes = [ctypes.c_void_p, ctypes.c_ulong]
x11.XStringToKeysym.restype = ctypes.c_ulong
x11.XStringToKeysym.argtypes = [ctypes.c_char_p]
xtst.XTestFakeKeyEvent.argtypes = [ctypes.c_void_p, ctypes.c_uint, ctypes.c_int, ctypes.c_ulong]
xtst.XTestFakeButtonEvent.argtypes = [ctypes.c_void_p, ctypes.c_uint, ctypes.c_int, ctypes.c_ulong]
x11.XSync.argtypes = [ctypes.c_void_p, ctypes.c_int]
x11.XDefaultRootWindow.restype = ctypes.c_ulong
x11.XDefaultRootWindow.argtypes = [ctypes.c_void_p]
x11.XQueryTree.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.POINTER(ctypes.c_ulong), ctypes.POINTER(ctypes.c_ulong),
                           ctypes.POINTER(ctypes.POINTER(ctypes.c_ulong)), ctypes.POINTER(ctypes.c_uint)]
x11.XGetGeometry.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.POINTER(ctypes.c_ulong), ctypes.POINTER(ctypes.c_int),
                             ctypes.POINTER(ctypes.c_int), ctypes.POINTER(ctypes.c_uint), ctypes.POINTER(ctypes.c_uint),
                             ctypes.POINTER(ctypes.c_uint), ctypes.POINTER(ctypes.c_uint)]
x11.XWarpPointer.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_ulong, ctypes.c_int, ctypes.c_int,
                             ctypes.c_uint, ctypes.c_uint, ctypes.c_int, ctypes.c_int]
x11.XSetInputFocus.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_ulong]


class Display:
    def __init__(self, name):
        self.dpy = x11.XOpenDisplay(name.encode())
        assert self.dpy, f"cannot open {name}"

    def window(self):
        root = x11.XDefaultRootWindow(self.dpy)
        r, p = ctypes.c_ulong(), ctypes.c_ulong()
        kids = ctypes.POINTER(ctypes.c_ulong)(); n = ctypes.c_uint()
        x11.XQueryTree(self.dpy, root, ctypes.byref(r), ctypes.byref(p), ctypes.byref(kids), ctypes.byref(n))
        best = None
        for i in range(n.value):
            geo = [ctypes.c_ulong(), ctypes.c_int(), ctypes.c_int(), ctypes.c_uint(), ctypes.c_uint(), ctypes.c_uint(), ctypes.c_uint()]
            x11.XGetGeometry(self.dpy, kids[i], *[ctypes.byref(g) for g in geo])
            area = geo[3].value * geo[4].value
            if geo[3].value >= 800 and (best is None or area > best[1]):
                best = (kids[i], area)
        return best and best[0]

    def focus(self):
        win = self.window()
        assert win, "no client window"
        x11.XSetInputFocus(self.dpy, win, 2, 0)
        x11.XSync(self.dpy, 0)

    def key(self, name):
        code = x11.XKeysymToKeycode(self.dpy, x11.XStringToKeysym(name.encode()))
        xtst.XTestFakeKeyEvent(self.dpy, code, 1, 0); xtst.XTestFakeKeyEvent(self.dpy, code, 0, 0)
        x11.XSync(self.dpy, 0)

    def centre(self):
        win = self.window()
        geo = [ctypes.c_ulong(), ctypes.c_int(), ctypes.c_int(), ctypes.c_uint(), ctypes.c_uint(), ctypes.c_uint(), ctypes.c_uint()]
        x11.XGetGeometry(self.dpy, win, *[ctypes.byref(g) for g in geo])
        x11.XWarpPointer(self.dpy, 0, win, 0, 0, 0, 0, geo[3].value // 2, geo[4].value // 2)
        x11.XSync(self.dpy, 0)

    def click(self, button):
        self.centre()
        xtst.XTestFakeButtonEvent(self.dpy, button, 1, 0); xtst.XTestFakeButtonEvent(self.dpy, button, 0, 0)
        x11.XSync(self.dpy, 0)


# ---------------------------------------------------------------- clients
def launch(name):
    disp = DISPLAYS[name]
    start(["Xvfb", disp, "-screen", "0", "1280x720x24", "-nolisten", "tcp"], f"xvfb-{name}")
    sock = pathlib.Path("/tmp/.X11-unix") / f"X{disp[1:]}"
    end = time.monotonic() + 20
    while not sock.exists() and time.monotonic() < end:
        time.sleep(0.1)
    settings = work / f"{name}-settings.json"
    apps = [["sh", "-c", f"(while :; do pw-play {tone}; done) & exec ptyxis -s"]] if name == "Owner" else []
    settings.write_text(json.dumps({"desktop_computer": True, "monitor_apps": apps, "fullscreen": False,
                                    "render_distance": 3, "max_fps": 30, "volume": 1.0, "vsync": False,
                                    "fancy_shaders": False, "gamma": 1.0}))
    runtime = work / f"runtime-{name}"
    env = {k: v for k, v in os.environ.items() if k not in ("WAYLAND_DISPLAY", "DISPLAY")}
    env.update(DISPLAY=disp, SOLARIS_CLIENT_SETTINGS=str(settings),
               HOME=os.environ["HOME"], WINIT_UNIX_BACKEND="x11")
    start([client_bin, "play", "--windowed", "--profile", "offline-test", "--server", f"127.0.0.1:{PORT}",
           "--username", name, "--automation", "--mcp-http", "127.0.0.1:0",
           "--runtime-dir", str(runtime), "--artifact-dir", str(evidence / f"artifacts-{name}"),
           "--data-dir", "/home/kaiserroman/.local/share/solaris-client", "--width", "1280", "--height", "720"],
          f"client-{name}", env=env)
    end = time.monotonic() + 120
    while time.monotonic() < end:
        found = sorted(runtime.glob("*.json")) if runtime.exists() else []
        if found:
            return Client(found[0], evidence / f"mcp-{name}")
        time.sleep(0.25)
    raise SystemExit(f"{name} client never published its MCP descriptor")


def find(obj, key):
    if isinstance(obj, dict):
        if key in obj:
            return obj[key]
        for v in obj.values():
            r = find(v, key)
            if r is not None:
                return r
    elif isinstance(obj, list):
        for v in obj:
            r = find(v, key)
            if r is not None:
                return r
    return None


class Player:
    def __init__(self, name):
        self.name = name
        self.mcp = launch(name)
        self.mcp.initialize()
        names = [t["name"] for t in self.mcp.request("tools/list")["result"]["tools"]]
        (evidence / f"tools-{name}.json").write_text(json.dumps(names))
        self.x = Display(DISPLAYS[name])
        self.x.focus()

    def tool(self, name, **args):
        return self.mcp.tool(name, args)

    def observe(self):
        return self.mcp.tool("minecraft_observe", {})

    def facts(self):
        return find(self.observe(), "screen_audio")

    def ticks(self, n):
        self.tool("minecraft_wait_ticks", ticks=n)


def main():
    result = {"steps": []}

    def check(label, ok, **detail):
        result["steps"].append({"label": label, "ok": bool(ok), **detail})
        say("PASS" if ok else "FAIL", label, json.dumps(detail))
        if not ok:
            raise AssertionError(label)

    owner, viewer = Player("Owner"), Player("Viewer")
    for p in (owner, viewer):
        p.tool("minecraft_connect", server_addr=f"127.0.0.1:{PORT}")
        p.tool("minecraft_wait_for_play", timeout_seconds=120)
        p.x.focus()
    say("both in play")
    for p in (owner, viewer):
        console(f"op {p.name}")
        console(f"gamemode creative {p.name}")
    me = owner.observe()["player"]
    px, py, pz = math.floor(me["x"]), math.floor(me["y"]), math.floor(me["z"])
    say("spawn", px, py, pz)
    # Flat stone platform, clear air above, one monitor facing north at z0+8, players south of it.
    y0 = py
    console(f"fill {px-10} {y0-1} {pz-4} {px+10} {y0-1} {pz+14} minecraft:stone")
    console(f"fill {px-10} {y0} {pz-4} {px+10} {y0+6} {pz+14} minecraft:air")
    mx, mz = px, pz + 8
    console(f"setblock {mx} {y0} {mz} minecraft:black_glazed_terracotta[facing=north]")
    console(f"tp Owner {mx + 0.5} {y0} {mz - 3 + 0.5}")
    console(f"tp Viewer {mx + 0.5} {y0} {mz - 6 + 0.5}")
    owner.tool("minecraft_wait_for_loaded_block", x=mx, y=y0, z=mz)
    owner.tool("minecraft_wait_for_block_state", x=mx, y=y0, z=mz, block_id="minecraft:black_glazed_terracotta")
    viewer.tool("minecraft_wait_for_block_state", x=mx, y=y0, z=mz, block_id="minecraft:black_glazed_terracotta")
    owner.tool("minecraft_look", yaw_deg=0, pitch_deg=20)
    viewer.tool("minecraft_look", yaw_deg=0, pitch_deg=10)
    owner.ticks(10); viewer.ticks(10)
    check("monitor placed, both players in front of it",
          owner.observe()["player"]["z"] < mz and viewer.observe()["player"]["z"] < mz)

    # Owner opens the picker with a real right-click and picks the first app.
    owner.x.focus(); owner.x.click(3)
    owner.ticks(10)
    owner.x.key("Return")
    say("owner picked the monitor app")
    for _ in range(80):
        f = owner.facts()
        if f and f["capturing"]:
            break
        owner.ticks(20)
    f = owner.facts()
    check("owner captures the nested app's private sink", f and f["capturing"], facts=f)
    for _ in range(40):
        f = owner.facts()
        if f and f["heard"] and f["heard"][0]["decoded_frames"] > 20:
            break
        owner.ticks(10)
    f = owner.facts()
    check("owner hears its own app positioned at the monitor (private sink)",
          f["heard"] and f["heard"][0]["peak"] > 0.05, facts=f)

    # Viewer looks at the monitor and asks; nothing is heard before permission.
    viewer.x.focus(); viewer.x.click(3)
    viewer.ticks(60)
    f = viewer.facts()
    check("looking viewer hears nothing before the owner allows", not f["heard"], facts=f)

    # Owner answers the dialog: Deny is the default, Down selects Allow.
    owner.x.focus(); owner.x.key("Down"); owner.x.key("Return")
    for _ in range(60):
        f = viewer.facts()
        if f["heard"] and f["heard"][0]["decoded_frames"] > 10:
            break
        viewer.ticks(10)
    f = viewer.facts()
    check("granted and looking: viewer decodes non-silent app audio",
          f["heard"] and f["heard"][0]["decoded_frames"] > 10 and f["heard"][0]["peak"] > 0.05, facts=f)

    # Looking away stops delivery within a tick; looking back resumes it.
    viewer.tool("minecraft_look", yaw_deg=180, pitch_deg=10)
    viewer.ticks(20)
    away1 = viewer.facts()["heard"][0]["decoded_frames"]
    viewer.ticks(100)
    away2 = viewer.facts()["heard"][0]["decoded_frames"]
    check("looking away: no further decoded audio", away2 == away1, before=away1, after=away2)
    viewer.tool("minecraft_look", yaw_deg=0, pitch_deg=10)
    viewer.ticks(60)
    back = viewer.facts()["heard"][0]["decoded_frames"]
    check("looking back: audio resumes", back > away2 + 10, before=away2, after=back)

    # Owner revokes (F8): the viewer hears nothing more even while looking.
    viewer.ticks(5)
    owner.x.focus(); owner.x.key("F8")
    viewer.ticks(20)
    r1 = viewer.facts()["heard"][0]["decoded_frames"]
    viewer.ticks(100)
    r2 = viewer.facts()["heard"][0]["decoded_frames"]
    check("revoked: no further decoded audio while looking", r2 == r1, before=r1, after=r2)
    return result


try:
    r = main()
    (evidence / "result.json").write_text(json.dumps(r, indent=1))
finally:
    cleanup()
