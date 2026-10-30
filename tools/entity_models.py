#!/usr/bin/env python3
"""Author-side tooling for Solaris entity models (assets/solaris/models/entity/*.json).

  entity_models.py fmt     [model.json ...]   canonical compact formatting (run after every edit)
  entity_models.py check   [model.json ...]   schema/UV/texture validation (all models if none given)
  entity_models.py preview  model.json [-o out.png] [--texture path/below/textures.png]

`preview` renders front / 3-4 / side / back views with the real vanilla texture plus a UV
map panel: each cube face rectangle is outlined, opaque texels no cube samples are tinted red.
Use it to confirm every textured region of the vanilla layout is used and lands where it should.

Vanilla textures: $SOLARIS_TEXTURES, default dist/resources/assets/minecraft/textures.
Format reference: src/entities/solaris_models.rs (module docs).
"""
import glob, json, math, os, sys
import numpy as np
from PIL import Image, ImageDraw

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
MODELS = os.path.join(ROOT, "assets/solaris/models/entity")
TEXTURES = os.environ.get("SOLARIS_TEXTURES") or os.path.join(ROOT, "dist/resources/assets/minecraft/textures")
FACES = ["up", "down", "right", "front", "left", "back"]


def face_rects(c):
    u, v = c.get("uv", [0, 0])
    w, h, d = c["size"]
    r = {
        "up": [u + d, v, u + d + w, v + d],
        "down": [u + d + w, v, u + d + 2 * w, v + d],
        "right": [u, v + d, u + d, v + d + h],
        "front": [u + d, v + d, u + d + w, v + d + h],
        "left": [u + d + w, v + d, u + 2 * d + w, v + d + h],
        "back": [u + 2 * d + w, v + d, u + 2 * d + 2 * w, v + d + h],
    }
    r.update({k: list(map(float, x)) for k, x in c.get("faces", {}).items()})
    return r


def cube_quads(c, tw, th):
    """Mirror of cube_quads() in solaris_models.rs. Returns [(corners[4][5], normal)] in pixels/uv-px."""
    ox, oy, oz = c["origin"]
    w, h, d = c["size"]
    i = c.get("inflate", 0.0)
    x0, y0, z0, x1, y1, z1 = ox - i, oy - i, oz - i, ox + w + i, oy + h + i, oz + d + i
    rects = face_rects(c)
    spec = {
        "up": ((0, -1, 0), [(x1, y0, z1, 1, 0), (x0, y0, z1, 0, 0), (x0, y0, z0, 0, 1), (x1, y0, z0, 1, 1)]),
        "down": ((0, 1, 0), [(x1, y1, z0, 1, 1), (x0, y1, z0, 0, 1), (x0, y1, z1, 0, 0), (x1, y1, z1, 1, 0)]),
        "right": ((-1, 0, 0), [(x0, y0, z0, 1, 0), (x0, y0, z1, 0, 0), (x0, y1, z1, 0, 1), (x0, y1, z0, 1, 1)]),
        "front": ((0, 0, -1), [(x1, y0, z0, 1, 0), (x0, y0, z0, 0, 0), (x0, y1, z0, 0, 1), (x1, y1, z0, 1, 1)]),
        "left": ((1, 0, 0), [(x1, y0, z1, 1, 0), (x1, y0, z0, 0, 0), (x1, y1, z0, 0, 1), (x1, y1, z1, 1, 1)]),
        "back": ((0, 0, 1), [(x0, y0, z1, 1, 0), (x1, y0, z1, 0, 0), (x1, y1, z1, 0, 1), (x0, y1, z1, 1, 1)]),
    }
    cx = (x0 + x1) / 2
    out = []
    for f in FACES:
        n, corners = spec[f]
        ru0, rv0, ru1, rv1 = rects[f]
        q = []
        for x, y, z, su, sv in corners:
            if c.get("mirror"):
                x = 2 * cx - x
            q.append((x, y, z, ru1 if su else ru0, rv1 if sv else rv0))
        if c.get("mirror"):
            q.reverse()
            n = (-n[0], n[1], n[2])
        out.append((q, n))
    return out


def walk_bones(bones, parent_mat=None, path=()):
    for b in bones:
        yield b, path
        yield from walk_bones(b.get("bones", []), None, path + (b["name"],))


def bone_matrices(model):
    """World matrices (pixels) for every bone name, same composition as the renderer."""
    mats = {}

    def rot(rx, ry, rz):
        rx, ry, rz = map(math.radians, (rx, ry, rz))
        cx, sx, cy, sy, cz, sz = math.cos(rx), math.sin(rx), math.cos(ry), math.sin(ry), math.cos(rz), math.sin(rz)
        Rx = np.array([[1, 0, 0], [0, cx, -sx], [0, sx, cx]])
        Ry = np.array([[cy, 0, sy], [0, 1, 0], [-sy, 0, cy]])
        Rz = np.array([[cz, -sz, 0], [sz, cz, 0], [0, 0, 1]])
        return Rz @ Ry @ Rx

    def rec(b, parent):
        m = np.eye(4)
        R = rot(*b.get("rotation", [0, 0, 0])) @ np.diag(b.get("scale", [1, 1, 1]))
        m[:3, :3] = R
        m[:3, 3] = b.get("pivot", [0, 0, 0])
        w = parent @ m
        yield b, w
        for ch in b.get("bones", []):
            yield from rec(ch, w)

    for b in model["bones"]:
        yield from rec(b, np.eye(4))


def load(path):
    with open(path) as f:
        return json.load(f)


def tex_image(model, override=None):
    p = os.path.join(TEXTURES, override or model["texture"]["path"])
    return Image.open(p).convert("RGBA")


def check(path):
    errs = []
    try:
        m = load(path)
    except Exception as e:  # noqa
        return [f"json: {e}"]
    allowed = {"schema", "id", "texture", "bones", "roles"}
    for k in m:
        if k not in allowed:
            errs.append(f"unknown top-level key {k}")
    if m.get("schema") != 1:
        errs.append("schema must be 1")
    stem = os.path.splitext(os.path.basename(path))[0]
    if m.get("id") != stem:
        errs.append(f"id {m.get('id')!r} != file name {stem!r}")
    tw, th = m["texture"]["size"]
    tp = os.path.join(TEXTURES, m["texture"]["path"])
    if not os.path.isfile(tp):
        errs.append(f"texture missing: {tp}")
    else:
        img = Image.open(tp)
        if list(img.size) != [tw, th]:
            errs.append(f"texture size declared {tw}x{th} but png is {img.size[0]}x{img.size[1]}")
    names = []
    cubes = 0
    for b, _ in walk_bones(m["bones"]):
        names.append(b["name"])
        for k in b:
            if k not in {"name", "pivot", "rotation", "scale", "cubes", "bones"}:
                errs.append(f"bone {b['name']}: unknown key {k}")
        for c in b.get("cubes", []):
            cubes += 1
            for k in c:
                if k not in {"origin", "size", "uv", "mirror", "inflate", "faces"}:
                    errs.append(f"bone {b['name']}: cube unknown key {k}")
            for f in c.get("faces", {}):
                if f not in FACES:
                    errs.append(f"bone {b['name']}: unknown face {f!r} (use up/down/front/back/left/right)")
            for f, (a, bb, cc, dd) in face_rects(c).items():
                lo_u, hi_u, lo_v, hi_v = min(a, cc), max(a, cc), min(bb, dd), max(bb, dd)
                if lo_u < -1e-4 or lo_v < -1e-4 or hi_u > tw + 1e-4 or hi_v > th + 1e-4:
                    errs.append(f"bone {b['name']}: face {f} rect {[a, bb, cc, dd]} outside {tw}x{th}")
    if len(names) != len(set(names)):
        dup = sorted({n for n in names if names.count(n) > 1})
        errs.append(f"duplicate bone names (animation looks bones up by name): {dup}")
    for role, lst in m.get("roles", {}).items():
        if role not in {"head", "right_arm", "left_arm", "right_leg", "left_leg", "wing", "crossed_arms", "nose"}:
            errs.append(f"unknown role {role}")
        for n in lst:
            if n not in names:
                errs.append(f"role {role}: bone {n!r} not found")
    if cubes == 0:
        errs.append("model has no cubes")
    errs += symmetry_errors(m)
    return errs


def _world_boxes(model):
    """{bone: (xmin,xmax,ymin,ymax,zmin,zmax)} of un-rotated chains (None if any ancestor rotates)."""
    out = {}

    def rec(b, off, ok):
        pv = b.get("pivot", [0, 0, 0])
        o = [off[i] + pv[i] for i in range(3)]
        ok = ok and not any(b.get("rotation", [0, 0, 0])) and b.get("scale", [1, 1, 1]) == [1, 1, 1]
        box = None
        for c in b.get("cubes", []):
            lo = [o[i] + c["origin"][i] for i in range(3)]
            hi = [lo[i] + c["size"][i] for i in range(3)]
            box = lo + hi if box is None else [min(box[i], lo[i]) for i in range(3)] + [max(box[3 + i], hi[i]) for i in range(3)]
        if box and ok:
            out[b["name"]] = (box[0], box[3], box[1], box[4], box[2], box[5])
        for ch in b.get("bones", []):
            rec(ch, o, ok)

    for b in model["bones"]:
        rec(b, [0, 0, 0], True)
    return out


def symmetry_errors(model):
    """left_*/right_* pairs must mirror about x=0 (catches mirrored cubes left at the right limb's origin)."""
    errs = []
    boxes = _world_boxes(model)
    for name, bx in boxes.items():
        if not name.startswith("left_"):
            continue
        other = boxes.get("right_" + name[5:])
        if not other:
            continue
        cl, cr = (bx[0] + bx[1]) / 2, (other[0] + other[1]) / 2
        if abs(cl + cr) > 0.6:
            errs.append(f"{name}/right_{name[5:]} not mirror-symmetric about x=0 (centres {cl:g} vs {cr:g}) - mirror flips texture only, move the origin")
    return errs


def coverage(model, img):
    """(mapped mask, opaque mask) over texture pixels."""
    tw, th = model["texture"]["size"]
    mapped = np.zeros((th, tw), bool)
    for b, _ in walk_bones(model["bones"]):
        for c in b.get("cubes", []):
            for f, (a, bb, cc, dd) in face_rects(c).items():
                u0, u1 = sorted((a, cc))
                v0, v1 = sorted((bb, dd))
                mapped[int(math.floor(v0 + 1e-4)):int(math.ceil(v1 - 1e-4)), int(math.floor(u0 + 1e-4)):int(math.ceil(u1 - 1e-4))] = True
    a = np.array(img)[:, :, 3] > 8
    return mapped, a


def render(model, img, yaw, pitch, scale, size, extra_mat=None):
    tw, th = model["texture"]["size"]
    tex = np.array(img.resize((tw, th), Image.NEAREST)) if img.size != (tw, th) else np.array(img)
    quads = []
    ry, rx = math.radians(yaw), math.radians(pitch)
    Ry = np.array([[math.cos(ry), 0, math.sin(ry)], [0, 1, 0], [-math.sin(ry), 0, math.cos(ry)]])
    Rx = np.array([[1, 0, 0], [0, math.cos(rx), -math.sin(rx)], [0, math.sin(rx), math.cos(rx)]])
    V = Rx @ Ry
    for b, W in bone_matrices(model):
        for c in b.get("cubes", []):
            for q, n in cube_quads(c, tw, th):
                pts = np.array([[p[0], p[1], p[2], 1.0] for p in q]) @ W.T
                pts = pts[:, :3] @ V.T
                quads.append((pts, [(p[3], p[4]) for p in q]))
    if not quads:
        return Image.new("RGBA", size, (40, 40, 48, 255))
    allp = np.concatenate([q[0] for q in quads])
    return quads, allp


def raster(quads, tex, tw, th, scale, size, center):
    W, H = size
    col = np.zeros((H, W, 4), np.uint8)
    col[:, :] = (40, 40, 48, 255)
    zbuf = np.full((H, W), 1e9)
    for pts, uvs in quads:
        sx = pts[:, 0] * scale + center[0]
        sy = pts[:, 1] * scale + center[1]
        for tri in ((0, 1, 2), (0, 2, 3)):
            a, b, c = tri
            x = np.array([sx[a], sx[b], sx[c]])
            y = np.array([sy[a], sy[b], sy[c]])
            area = (x[1] - x[0]) * (y[2] - y[0]) - (x[2] - x[0]) * (y[1] - y[0])
            if abs(area) < 1e-6:
                continue
            xmin, xmax = int(max(math.floor(x.min()), 0)), int(min(math.ceil(x.max()), W - 1))
            ymin, ymax = int(max(math.floor(y.min()), 0)), int(min(math.ceil(y.max()), H - 1))
            if xmin > xmax or ymin > ymax:
                continue
            gx, gy = np.meshgrid(np.arange(xmin, xmax + 1) + 0.5, np.arange(ymin, ymax + 1) + 0.5)
            w0 = ((x[1] - gx) * (y[2] - gy) - (x[2] - gx) * (y[1] - gy)) / area
            w1 = ((x[2] - gx) * (y[0] - gy) - (x[0] - gx) * (y[2] - gy)) / area
            w2 = 1 - w0 - w1
            inside = (w0 >= -1e-6) & (w1 >= -1e-6) & (w2 >= -1e-6)
            if not inside.any():
                continue
            z = w0 * pts[a, 2] + w1 * pts[b, 2] + w2 * pts[c, 2]
            u = w0 * uvs[a][0] + w1 * uvs[b][0] + w2 * uvs[c][0]
            v = w0 * uvs[a][1] + w1 * uvs[b][1] + w2 * uvs[c][1]
            tx = np.clip(np.floor(u).astype(int), 0, tw - 1)
            ty = np.clip(np.floor(v).astype(int), 0, th - 1)
            texel = tex[ty, tx]
            # shade by facing for depth cue
            ok = inside & (texel[:, :, 3] > 100)
            sub_z = zbuf[ymin:ymax + 1, xmin:xmax + 1]
            ok &= z < sub_z
            sub_z[ok] = z[ok]
            sub_c = col[ymin:ymax + 1, xmin:xmax + 1]
            sub_c[ok] = texel[ok]
    return Image.fromarray(col, "RGBA")


def preview(path, out, texture_override=None):
    m = load(path)
    img = tex_image(m, texture_override)
    tw, th = m["texture"]["size"]
    tex = np.array(img.resize((tw, th), Image.NEAREST)) if img.size != (tw, th) else np.array(img)
    views = [("front", 0, 0), ("3/4", 145, -20), ("side", 90, 0), ("back", 180, 0)]
    ext = 0
    prepared = []
    for name, yaw, pitch in views:
        r = render(m, img, yaw, pitch, 1, (10, 10))
        if isinstance(r, Image.Image):
            continue
        quads, allp = r
        prepared.append((name, quads, allp))
        ext = max(ext, np.abs(allp[:, 0]).max(), np.abs(allp[:, 1] - allp[:, 1].mean()).max())
    if not prepared:
        raise SystemExit("no geometry")
    cell = 320
    scale = (cell / 2 - 8) / max(ext, 1)
    tiles = []
    for name, quads, allp in prepared:
        cy = allp[:, 1].mean()
        im = raster(quads, tex, tw, th, scale, (cell, cell), (cell / 2, cell / 2 - cy * scale))
        d = ImageDraw.Draw(im)
        d.text((4, 4), name, fill=(255, 255, 0, 255))
        tiles.append(im)
    # UV panel
    mapped, opaque = coverage(m, img)
    ts = max(1, 320 // max(tw, th))
    panel = Image.new("RGBA", (tw * ts, th * ts), (40, 40, 48, 255))
    big = img.resize((tw * ts, th * ts), Image.NEAREST)
    panel.alpha_composite(big)
    parr = np.array(panel)
    miss = np.kron((opaque & ~mapped).astype(np.uint8), np.ones((ts, ts), np.uint8)).astype(bool)
    parr[miss] = (0.5 * parr[miss] + 0.5 * np.array([255, 0, 0, 255])).astype(np.uint8)
    panel = Image.fromarray(parr, "RGBA")
    d = ImageDraw.Draw(panel)
    palette = [(255, 255, 0), (0, 255, 255), (255, 0, 255), (0, 255, 0), (255, 128, 0), (128, 128, 255)]
    k = 0
    for b, _ in walk_bones(m["bones"]):
        for c in b.get("cubes", []):
            k += 1
            for f, (a, bb, cc, dd) in face_rects(c).items():
                d.rectangle([min(a, cc) * ts, min(bb, dd) * ts, max(a, cc) * ts - 1, max(bb, dd) * ts - 1], outline=palette[k % 6] + (255,))
    pct = 100.0 * (opaque & mapped).sum() / max(opaque.sum(), 1)
    W = cell * len(tiles) + 0
    canvas = Image.new("RGBA", (max(W, panel.width), cell + panel.height + 14), (20, 20, 24, 255))
    for i, t in enumerate(tiles):
        canvas.paste(t, (i * cell, 0))
    canvas.paste(panel, (0, cell + 14))
    ImageDraw.Draw(canvas).text((4, cell + 1), f"{m['id']}  texture {tw}x{th}  opaque texels covered by cubes: {pct:.1f}%  (red = opaque but unmapped)", fill=(255, 255, 255, 255))
    canvas.convert("RGB").save(out)
    print(f"wrote {out}  coverage {pct:.1f}%")


def sheet(files, out, cols=4, cell=240):
    """Contact sheet: front + 3/4 view of many models."""
    rows = (len(files) + cols - 1) // cols
    canvas = Image.new("RGBA", (cols * cell * 2 // 1, rows * cell), (20, 20, 24, 255))
    for i, path in enumerate(files):
        m = load(path)
        try:
            img = tex_image(m)
        except Exception:
            continue
        tw, th = m["texture"]["size"]
        tex = np.array(img.resize((tw, th), Image.NEAREST)) if img.size != (tw, th) else np.array(img)
        ext = 1
        prep = []
        for yaw, pitch in ((0, 0), (145, -20)):
            r = render(m, img, yaw, pitch, 1, (10, 10))
            if isinstance(r, Image.Image):
                continue
            quads, allp = r
            prep.append((quads, allp))
            ext = max(ext, np.abs(allp[:, 0]).max(), np.abs(allp[:, 1] - allp[:, 1].mean()).max())
        scale = (cell / 2 - 8) / ext
        for k, (quads, allp) in enumerate(prep):
            cy = allp[:, 1].mean()
            im = raster(quads, tex, tw, th, scale, (cell, cell), (cell / 2, cell / 2 - cy * scale))
            ImageDraw.Draw(im).text((4, 4), m["id"] if k == 0 else "", fill=(255, 255, 0, 255))
            canvas.paste(im, (((i % cols) * 2 + k) * cell, (i // cols) * cell))
    canvas.convert("RGB").save(out)


def _c(v):
    """Compact JSON for leaves (arrays/small objects on one line)."""
    return json.dumps(v, separators=(", ", ": "))


def _bone(b, ind):
    pad = "  " * ind
    lines = [f'{pad}{{']
    keys = [k for k in ("name", "pivot", "rotation", "scale") if k in b]
    head = ", ".join(f'"{k}": {_c(b[k])}' for k in keys)
    lines.append(f"{pad}  {head}" + ("," if b.get("cubes") or b.get("bones") else ""))
    if b.get("cubes"):
        lines.append(f'{pad}  "cubes": [')
        lines.append(",\n".join(f"{pad}    {_c(c)}" for c in b["cubes"]))
        lines.append(f'{pad}  ]' + ("," if b.get("bones") else ""))
    if b.get("bones"):
        lines.append(f'{pad}  "bones": [')
        lines.append(",\n".join(_bone(x, ind + 2) for x in b["bones"]))
        lines.append(f"{pad}  ]")
    lines.append(f"{pad}}}")
    return "\n".join(lines)


def fmt(path):
    m = load(path)
    out = ["{", f'  "schema": {m["schema"]},', f'  "id": {_c(m["id"])},', f'  "texture": {_c(m["texture"])},']
    if m.get("roles"):
        out.append('  "roles": {')
        out.append(",\n".join(f'    "{k}": {_c(v)}' for k, v in m["roles"].items()))
        out.append("  },")
    out.append('  "bones": [')
    out.append(",\n".join(_bone(b, 2) for b in m["bones"]))
    out.append("  ]")
    out.append("}")
    with open(path, "w") as f:
        f.write("\n".join(out) + "\n")


def main(argv):
    if argv and argv[0] == "sheet":
        out = argv[argv.index("-o") + 1]
        files = [a for a in argv[1:] if a.endswith(".json")]
        sheet(files, out)
        return 0
    if argv and argv[0] == "fmt":
        for f in argv[1:] or sorted(glob.glob(os.path.join(MODELS, "*.json"))):
            fmt(f)
        return 0
    if not argv or argv[0] not in ("check", "preview"):
        print(__doc__)
        return 2
    if argv[0] == "check":
        files = argv[1:] or sorted(glob.glob(os.path.join(MODELS, "*.json")))
        bad = 0
        for f in files:
            e = check(f)
            if e:
                bad += 1
                print(f"FAIL {os.path.basename(f)}")
                for x in e:
                    print("   ", x)
            else:
                m = load(f)
                try:
                    mapped, opaque = coverage(m, tex_image(m))
                    print(f"ok   {os.path.basename(f):40s} coverage {100.0 * (opaque & mapped).sum() / max(opaque.sum(), 1):5.1f}%")
                except Exception:
                    print(f"ok   {os.path.basename(f)}")
        return 1 if bad else 0
    out = None
    tex = None
    args = argv[1:]
    path = args[0]
    if "-o" in args:
        out = args[args.index("-o") + 1]
    if "--texture" in args:
        tex = args[args.index("--texture") + 1]
    preview(path, out or f"/tmp/preview_{os.path.splitext(os.path.basename(path))[0]}.png", tex)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
