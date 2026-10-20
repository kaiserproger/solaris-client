"""Real X11 wheel injection through XTest (no WM, own Xvfb display).

Usage: hotbar_wheel_xtest.py DISPLAY list|up|down [count] [burst]
"""
import ctypes, ctypes.util, sys, json

x11 = ctypes.CDLL("libX11.so.6")
xtst = ctypes.CDLL("libXtst.so.6")

x11.XOpenDisplay.restype = ctypes.c_void_p
x11.XOpenDisplay.argtypes = [ctypes.c_char_p]
x11.XDefaultRootWindow.restype = ctypes.c_ulong
x11.XDefaultRootWindow.argtypes = [ctypes.c_void_p]
x11.XQueryTree.argtypes = [ctypes.c_void_p, ctypes.c_ulong,
                           ctypes.POINTER(ctypes.c_ulong), ctypes.POINTER(ctypes.c_ulong),
                           ctypes.POINTER(ctypes.POINTER(ctypes.c_ulong)), ctypes.POINTER(ctypes.c_uint)]
x11.XFetchName.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.POINTER(ctypes.c_char_p)]
x11.XGetGeometry.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.POINTER(ctypes.c_ulong),
                             ctypes.POINTER(ctypes.c_int), ctypes.POINTER(ctypes.c_int),
                             ctypes.POINTER(ctypes.c_uint), ctypes.POINTER(ctypes.c_uint),
                             ctypes.POINTER(ctypes.c_uint), ctypes.POINTER(ctypes.c_uint)]
x11.XTranslateCoordinates.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_ulong,
                                      ctypes.c_int, ctypes.c_int,
                                      ctypes.POINTER(ctypes.c_int), ctypes.POINTER(ctypes.c_int),
                                      ctypes.POINTER(ctypes.c_ulong)]
x11.XWarpPointer.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_ulong, ctypes.c_int,
                             ctypes.c_int, ctypes.c_uint, ctypes.c_uint, ctypes.c_int, ctypes.c_int]
x11.XSetInputFocus.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_ulong]
x11.XSync.argtypes = [ctypes.c_void_p, ctypes.c_int]
x11.XFlush.argtypes = [ctypes.c_void_p]
xtst.XTestFakeButtonEvent.argtypes = [ctypes.c_void_p, ctypes.c_uint, ctypes.c_int, ctypes.c_ulong]

RevertToParent = 2


def windows(dpy):
    root = x11.XDefaultRootWindow(dpy)
    r = ctypes.c_ulong()
    p = ctypes.c_ulong()
    kids = ctypes.POINTER(ctypes.c_ulong)()
    n = ctypes.c_uint()
    x11.XQueryTree(dpy, root, ctypes.byref(r), ctypes.byref(p), ctypes.byref(kids), ctypes.byref(n))
    out = []
    for i in range(n.value):
        w = kids[i]
        name = ctypes.c_char_p()
        x11.XFetchName(dpy, w, ctypes.byref(name))
        wroot = ctypes.c_ulong()
        x = ctypes.c_int(); y = ctypes.c_int()
        width = ctypes.c_uint(); height = ctypes.c_uint(); border = ctypes.c_uint(); depth = ctypes.c_uint()
        x11.XGetGeometry(dpy, w, ctypes.byref(wroot), ctypes.byref(x), ctypes.byref(y),
                         ctypes.byref(width), ctypes.byref(height), ctypes.byref(border), ctypes.byref(depth))
        out.append({"id": int(w), "name": (name.value or b"").decode(errors="replace"),
                    "w": width.value, "h": height.value, "mapped": width.value > 1 and height.value > 1})
    return out


def pick(dpy, want=800):
    cands = [w for w in windows(dpy) if w["mapped"] and w["w"] >= want]
    if not cands:
        raise SystemExit(f"no window >= {want}px: {windows(dpy)}")
    return max(cands, key=lambda w: w["w"] * w["h"])


def focus_and_point(dpy, win):
    root = x11.XDefaultRootWindow(dpy)
    ax = ctypes.c_int(); ay = ctypes.c_int(); child = ctypes.c_ulong()
    x11.XTranslateCoordinates(dpy, win["id"], root, 0, 0, ctypes.byref(ax), ctypes.byref(ay), ctypes.byref(child))
    x11.XSetInputFocus(dpy, win["id"], RevertToParent, 0)
    x11.XWarpPointer(dpy, 0, root, 0, 0, 0, 0,
                     ax.value + win["w"] // 2, ay.value + win["h"] // 2)
    x11.XSync(dpy, 0)
    return ax.value + win["w"] // 2, ay.value + win["h"] // 2


def wheel(dpy, button, count=1, burst=False):
    """One notch = button press + release. `burst` queues every pair before a
    single XSync so the pairs reach the client back to back."""
    for _ in range(count):
        xtst.XTestFakeButtonEvent(dpy, button, 1, 0)
        xtst.XTestFakeButtonEvent(dpy, button, 0, 0)
        if not burst:
            x11.XSync(dpy, 0)
    x11.XSync(dpy, 0)


if __name__ == "__main__":
    display = sys.argv[1]
    action = sys.argv[2]
    dpy = x11.XOpenDisplay(display.encode())
    if not dpy:
        raise SystemExit(f"cannot open display {display}")
    win = pick(dpy)
    print(json.dumps({"window": win, "pointer": focus_and_point(dpy, win)}))
    if action == "list":
        pass
    else:
        button = 4 if action == "up" else 5
        count = int(sys.argv[3]) if len(sys.argv) > 3 else 1
        burst = len(sys.argv) > 4 and sys.argv[4] == "burst"
        wheel(dpy, button, count, burst)
        print(json.dumps({"sent": action, "x_button": button, "count": count, "burst": burst}))
    x11.XSync(dpy, 0)
