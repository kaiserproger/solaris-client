"""Real X11 keyboard/pointer injection through XTest for an own Xvfb display (no WM).

Used by live gates that must prove genuine key/char/pointer events reach the
native window, e.g. `creative_search_live.py`.
"""
import ctypes
import ctypes.util

x11 = ctypes.CDLL("libX11.so.6")
xtst = ctypes.CDLL("libXtst.so.6")
x11.XOpenDisplay.restype = ctypes.c_void_p
x11.XOpenDisplay.argtypes = [ctypes.c_char_p]
x11.XDefaultRootWindow.restype = ctypes.c_ulong
x11.XDefaultRootWindow.argtypes = [ctypes.c_void_p]
x11.XQueryTree.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.POINTER(ctypes.c_ulong),
                           ctypes.POINTER(ctypes.c_ulong), ctypes.POINTER(ctypes.POINTER(ctypes.c_ulong)),
                           ctypes.POINTER(ctypes.c_uint)]
x11.XGetGeometry.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.POINTER(ctypes.c_ulong),
                             ctypes.POINTER(ctypes.c_int), ctypes.POINTER(ctypes.c_int),
                             ctypes.POINTER(ctypes.c_uint), ctypes.POINTER(ctypes.c_uint),
                             ctypes.POINTER(ctypes.c_uint), ctypes.POINTER(ctypes.c_uint)]
x11.XTranslateCoordinates.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_ulong, ctypes.c_int, ctypes.c_int,
                                      ctypes.POINTER(ctypes.c_int), ctypes.POINTER(ctypes.c_int),
                                      ctypes.POINTER(ctypes.c_ulong)]
x11.XWarpPointer.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_ulong, ctypes.c_int, ctypes.c_int,
                             ctypes.c_uint, ctypes.c_uint, ctypes.c_int, ctypes.c_int]
x11.XSetInputFocus.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_ulong]
x11.XSync.argtypes = [ctypes.c_void_p, ctypes.c_int]
x11.XStringToKeysym.restype = ctypes.c_ulong
x11.XStringToKeysym.argtypes = [ctypes.c_char_p]
x11.XKeysymToKeycode.restype = ctypes.c_ubyte
x11.XKeysymToKeycode.argtypes = [ctypes.c_void_p, ctypes.c_ulong]
xtst.XTestFakeKeyEvent.argtypes = [ctypes.c_void_p, ctypes.c_uint, ctypes.c_int, ctypes.c_ulong]
xtst.XTestFakeButtonEvent.argtypes = [ctypes.c_void_p, ctypes.c_uint, ctypes.c_int, ctypes.c_ulong]

SPECIAL = {" ": "space", "-": "minus", "_": "underscore", ":": "colon", "/": "slash", ".": "period"}


class Display:
    def __init__(self, display):
        self.dpy = x11.XOpenDisplay(display.encode())
        if not self.dpy:
            raise RuntimeError(f"cannot open display {display}")
        self.root = x11.XDefaultRootWindow(self.dpy)

    def window(self, want=800):
        root = ctypes.c_ulong()
        parent = ctypes.c_ulong()
        kids = ctypes.POINTER(ctypes.c_ulong)()
        count = ctypes.c_uint()
        x11.XQueryTree(self.dpy, self.root, ctypes.byref(root), ctypes.byref(parent), ctypes.byref(kids),
                       ctypes.byref(count))
        best = None
        for i in range(count.value):
            wid = kids[i]
            geometry = [ctypes.c_ulong(), ctypes.c_int(), ctypes.c_int(), ctypes.c_uint(), ctypes.c_uint(),
                        ctypes.c_uint(), ctypes.c_uint()]
            x11.XGetGeometry(self.dpy, wid, *[ctypes.byref(g) for g in geometry])
            width, height = geometry[3].value, geometry[4].value
            if width >= want and (best is None or width * height > best[1] * best[2]):
                best = (int(wid), width, height)
        if best is None:
            raise RuntimeError("no native window")
        self.win, self.width, self.height = best
        return best

    def origin(self):
        x, y, child = ctypes.c_int(), ctypes.c_int(), ctypes.c_ulong()
        x11.XTranslateCoordinates(self.dpy, self.win, self.root, 0, 0, ctypes.byref(x), ctypes.byref(y),
                                  ctypes.byref(child))
        return x.value, y.value

    def focus(self):
        x11.XSetInputFocus(self.dpy, self.win, 2, 0)
        x11.XSync(self.dpy, 0)

    def move(self, x, y):
        ox, oy = self.origin()
        x11.XWarpPointer(self.dpy, 0, self.root, 0, 0, 0, 0, ox + int(x), oy + int(y))
        x11.XSync(self.dpy, 0)

    def click(self, x, y, button=1):
        self.move(x, y)
        xtst.XTestFakeButtonEvent(self.dpy, button, 1, 0)
        x11.XSync(self.dpy, 0)
        xtst.XTestFakeButtonEvent(self.dpy, button, 0, 0)
        x11.XSync(self.dpy, 0)

    def _code(self, keysym):
        code = x11.XKeysymToKeycode(self.dpy, x11.XStringToKeysym(keysym.encode()))
        if not code:
            raise RuntimeError(f"no keycode for {keysym}")
        return code

    def key(self, keysym, *modifiers):
        for m in modifiers:
            xtst.XTestFakeKeyEvent(self.dpy, self._code(m), 1, 0)
        code = self._code(keysym)
        xtst.XTestFakeKeyEvent(self.dpy, code, 1, 0)
        x11.XSync(self.dpy, 0)
        xtst.XTestFakeKeyEvent(self.dpy, code, 0, 0)
        for m in reversed(modifiers):
            xtst.XTestFakeKeyEvent(self.dpy, self._code(m), 0, 0)
        x11.XSync(self.dpy, 0)

    def type_text(self, text):
        for ch in text:
            self.key(SPECIAL.get(ch, ch))
