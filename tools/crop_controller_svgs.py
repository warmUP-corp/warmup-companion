#!/usr/bin/env python3
"""Crop Figma-exported controller-icon SVGs down to what's actually visible.

Each icon ships the WHOLE controller atlas inside one `<g transform="translate(tx,ty)">`,
framed to a 32x32 viewBox. 200+ paths/rects/circles sit off-canvas yet are still in the
file (and compiled into the binary via include_str!). This keeps only elements whose
bounding box intersects the visible window, plus any <mask> a kept element references.

Bounding boxes over-estimate (control points included), so culling only ever errs toward
KEEPING an element -- never silently drops something visible.
"""
import re
import sys
import math
from pathlib import Path

ICON_DIR = Path(__file__).resolve().parent.parent / "controller-icons"
MARGIN = 4.0  # source units of slack around the visible window (covers strokes/outside-masks)

NUM = re.compile(r"[-+]?(?:\d*\.\d+|\d+\.?)(?:[eE][-+]?\d+)?")
# path command letter followed by its argument blob
PATH_TOKEN = re.compile(r"([MmLlHhVvCcSsQqTtAaZz])([^MmLlHhVvCcSsQqTtAaZz]*)")
# expected coord count per command (for splitting repeated arg sets)
CMD_ARGC = {"m": 2, "l": 2, "h": 1, "v": 1, "c": 6, "s": 4, "q": 4, "t": 2, "a": 7, "z": 0}


def floats(s):
    return [float(x) for x in NUM.findall(s)]


def path_bbox(d):
    """Walk a path, tracking the current point, return (minx,miny,maxx,maxy) over all
    on-path + control points. Over-estimates; that's the safe direction for culling."""
    xs, ys = [], []
    cx = cy = 0.0
    start_x = start_y = 0.0
    for letter, argblob in PATH_TOKEN.findall(d):
        rel = letter.islower()
        cmd = letter.lower()
        nums = floats(argblob)
        argc = CMD_ARGC[cmd]
        if cmd == "z":
            cx, cy = start_x, start_y
            continue
        if argc == 0:
            continue
        # consume argument sets in groups of argc
        i = 0
        first = True
        while i + argc <= len(nums):
            grp = nums[i:i + argc]
            i += argc
            if cmd == "h":
                x = (cx + grp[0]) if rel else grp[0]
                cx = x
                xs.append(x); ys.append(cy)
            elif cmd == "v":
                y = (cy + grp[0]) if rel else grp[0]
                cy = y
                xs.append(cx); ys.append(y)
            elif cmd == "a":
                # rx ry rot largeArc sweep x y -- only endpoint is a real coord
                ex, ey = grp[5], grp[6]
                x = (cx + ex) if rel else ex
                y = (cy + ey) if rel else ey
                cx, cy = x, y
                xs.append(x); ys.append(y)
            else:
                # pairs of (x,y); record every pair (incl control points)
                pts = []
                for j in range(0, argc, 2):
                    px, py = grp[j], grp[j + 1]
                    x = (cx + px) if rel else px
                    y = (cy + py) if rel else py
                    pts.append((x, y))
                    xs.append(x); ys.append(y)
                # current point becomes the last pair; M's first implicit-lineto rule
                # doesn't matter for bbox
                cx, cy = pts[-1]
                if cmd == "m" and first:
                    start_x, start_y = cx, cy
            first = False
    if not xs:
        return None
    return (min(xs), min(ys), max(xs), max(ys))


GEOM_ATTRS = ("x", "y", "width", "height", "cx", "cy", "r", "rx", "ry", "stroke-width")


def r2(x):
    s = f"{x:.2f}".rstrip("0").rstrip(".")
    return s if s not in ("", "-0") else "0"


def shrink_token(tok):
    """Trim coordinate precision to 2 decimals inside `d=` and geometry attrs only.
    Never touches fill/stroke (hex colors), id, or mask refs."""
    def round_d(m):
        return 'd="' + NUM.sub(lambda n: r2(float(n.group(0))), m.group(1)) + '"'
    tok = re.sub(r'd="([^"]*)"', round_d, tok)
    for a in GEOM_ATTRS:
        tok = re.sub(r'(\b%s=")([^"]*)(")' % re.escape(a),
                     lambda m: m.group(1) + r2(float(m.group(2))) + m.group(3)
                     if NUM.fullmatch(m.group(2).strip()) else m.group(0),
                     tok)
    return tok


def attr(tag, name):
    m = re.search(r'\b%s="([^"]*)"' % re.escape(name), tag)
    return m.group(1) if m else None


def fnum(tag, name, default=0.0):
    v = attr(tag, name)
    return float(v) if v is not None else default


def elem_bbox(tag, kind):
    if kind == "rect":
        x = fnum(tag, "x"); y = fnum(tag, "y")
        w = fnum(tag, "width"); h = fnum(tag, "height")
        return (x, y, x + w, y + h)
    if kind == "circle":
        cx = fnum(tag, "cx"); cy = fnum(tag, "cy"); r = fnum(tag, "r")
        return (cx - r, cy - r, cx + r, cy + r)
    if kind == "ellipse":
        cx = fnum(tag, "cx"); cy = fnum(tag, "cy")
        rx = fnum(tag, "rx"); ry = fnum(tag, "ry")
        return (cx - rx, cy - ry, cx + rx, cy + ry)
    if kind == "path":
        d = attr(tag, "d")
        return path_bbox(d) if d else None
    return None


def intersects(bb, win):
    if bb is None:
        return True  # unknown -> keep (safe)
    return not (bb[2] < win[0] or bb[0] > win[1] or bb[3] < win[2] or bb[1] > win[3])


# match a <mask ...>...</mask> block OR a self-closing primitive
TOKEN = re.compile(
    r'<mask\b[^>]*>.*?</mask>|<(?:path|rect|circle|ellipse|line|polygon|polyline)\b[^>]*/>',
    re.DOTALL,
)


def crop(text):
    gm = re.search(r'(<g\b[^>]*transform="translate\(\s*([-0-9.]+)\s*,\s*([-0-9.]+)\s*\)"[^>]*>)(.*)(</g>)',
                   text, re.DOTALL)
    if not gm:
        return text, (0, 0)
    g_open, tx, ty, inner, g_close = gm.group(1), float(gm.group(2)), float(gm.group(3)), gm.group(4), gm.group(5)
    vb = attr(text, "viewBox") or "0 0 32 32"
    _, _, vw, vh = [float(x) for x in vb.split()]
    win = (-tx - MARGIN, -tx + vw + MARGIN, -ty - MARGIN, -ty + vh + MARGIN)

    tokens = list(TOKEN.finditer(inner))
    kept = []
    kept_mask_refs = set()
    total = 0
    for m in tokens:
        tok = m.group(0)
        total += 1
        if tok.startswith("<mask"):
            continue  # decide masks in a second pass (keep only if referenced)
        kind = re.match(r"<(\w+)", tok).group(1)
        bb = elem_bbox(tok, kind)
        if intersects(bb, win):
            kept.append(m)
            ref = attr(tok, "mask")
            if ref:
                rid = re.search(r"url\(#([^)]+)\)", ref)
                if rid:
                    kept_mask_refs.add(rid.group(1))
    # second pass: keep masks that a kept element references
    final = []
    for m in tokens:
        tok = m.group(0)
        if tok.startswith("<mask"):
            mid = attr(tok, "id")
            if mid in kept_mask_refs:
                final.append(m)
        elif m in kept:
            final.append(m)
    final.sort(key=lambda m: m.start())
    new_inner = "\n".join(shrink_token(t.group(0)) for t in final)
    new_text = text[:gm.start()] + g_open + "\n" + new_inner + "\n" + g_close + text[gm.end():]
    return new_text, (len(final), total)


def main():
    files = sorted(ICON_DIR.glob("*.svg"))
    grand_before = grand_after = 0
    for f in files:
        text = f.read_text(encoding="utf-8")
        before = len(text.encode("utf-8"))
        new, (kept, total) = crop(text)
        after = len(new.encode("utf-8"))
        grand_before += before
        grand_after += after
        if "--write" in sys.argv:
            f.write_text(new, encoding="utf-8")
        print(f"{f.name:32s} {before:8d} -> {after:7d}  ({kept}/{total} elems)")
    print(f"{'TOTAL':32s} {grand_before:8d} -> {grand_after:7d}  "
          f"({100*(grand_before-grand_after)/grand_before:.1f}% smaller)")


if __name__ == "__main__":
    main()
