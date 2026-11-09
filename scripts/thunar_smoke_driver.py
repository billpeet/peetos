#!/usr/bin/env python3
"""Drive the Thunar smoke: peetwl + Thunar + a watcher shell, through QEMU.

1. Waits for Thunar's window on /tmp/thunar-demo to map under peetwl.
2. Checks with `screendump` that peetwl's focused title bar frames a window
   with drawn content (icons, labels, toolbar).
3. With real PS/2 key presses: selects the first item (the Documents folder,
   folders sort first), opens it with Enter (peetwl must see the window title
   change to Documents), then creates a folder with Ctrl+Shift+N, typing
   "made-by-thunar" into Thunar's New Folder dialog. The watcher shell must
   find the folder on disk at /tmp/thunar-demo/Documents/made-by-thunar.
4. Quits Thunar with Ctrl+Q; peetwl must see every client exit cleanly.

Usage: thunar_smoke_driver.py --log <serial log> --monitor <unix socket>
                              [--shots <dir for PNG copies of screendumps>]
Exits 0 and prints "[thunar-driver] pass" on success.
"""
import argparse
import os
import re
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from wlcomp_smoke_driver import COLOR_TITLE_FOCUSED, Driver, Fail  # noqa: E402
from foot_smoke_driver import keys, mapped  # noqa: E402

TITLE = "thunar-demo - Thunar"


def drawn_pixels(img, win):
    """Pixels in the window content that differ clearly from GTK's light
    background: icons, text, toolbar. A blank or failed client has few."""
    n = 0
    for py in range(win["y"], win["y"] + min(win["h"], 400), 2):
        for px in range(win["x"], win["x"] + min(win["w"], 600), 2):
            v = img.at(px, py)
            r, g, b = (v >> 16) & 0xFF, (v >> 8) & 0xFF, v & 0xFF
            if min(r, g, b) < 0xB0:
                n += 1
    return n


def run(d):
    d.wait_for(r"\[peetwl\] output drm \d+x\d+", timeout=120)
    m = d.wait_for(r"\[peetwl\] mapped '([^']* - Thunar)'", timeout=180)
    title = m.group(1)
    win = mapped(d, title)
    print("[thunar-driver] '%s' at %d,%d %dx%d" % (title, win["x"], win["y"], win["w"], win["h"]))
    d.wait_for(r"\[peetwl\] focus '%s'" % re.escape(title), timeout=30)
    # Let Thunar load the folder and its icons.
    time.sleep(4)
    img = d.screendump()
    if title != TITLE:
        raise Fail("Thunar opened '%s', expected '%s'" % (title, TITLE))
    # Sample the title bar right of its text (peetwl reports the text start;
    # a long title like this one puts glyph pixels under that point).
    tx, ty = win["x"] + win["w"] - 60, win["title"][1]
    if (img.at(tx, ty) & 0xFFFFFF) != COLOR_TITLE_FOCUSED:
        raise Fail("no focused peetwl title bar at %d,%d (got 0x%06x)" % (tx, ty, img.at(tx, ty) & 0xFFFFFF))
    n = drawn_pixels(img, win)
    print("[thunar-driver] %d drawn pixels in the window" % n)
    if n < 3000:
        raise Fail("the Thunar window looks blank (%d drawn pixels)" % n)
    print("[thunar-driver] Thunar window on screen ok")

    # Select the first item (Documents) and open it.
    mark = d.mark()
    keys(d, ["home"], gap=0.5)
    keys(d, ["ret"], gap=0.5)
    d.wait_for(r"\[peetwl\] title '[^']*' -> 'Documents - Thunar'", timeout=30, after=mark)
    print("[thunar-driver] keyboard navigation into Documents ok")
    time.sleep(2)

    # New folder through Thunar's dialog.
    mark = d.mark()
    keys(d, ["ctrl-shift-n"], gap=1.5)
    name = "made-by-thunar"
    keys(d, ["ctrl-a"] + ["minus" if c == "-" else c for c in name] + ["ret"], gap=0.15)
    m = d.wait_for(r"\[thunar-watch\] (found|wrong place) (\S+)", timeout=30, after=mark)
    if m.group(1) != "found":
        raise Fail("Thunar created the folder in the wrong place: %s" % m.group(2))
    print("[thunar-driver] New Folder dialog created %s ok" % m.group(2))
    time.sleep(1.5)
    d.screendump()

    mark = d.mark()
    keys(d, ["ctrl-q"], gap=0.5)
    d.wait_for(r"\[peetwl\] all clients exited \(failed=0\)", timeout=60, after=mark)
    print("[thunar-driver] pass")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--log", required=True)
    ap.add_argument("--monitor", required=True)
    ap.add_argument("--shots")
    args = ap.parse_args()
    d = Driver(args.log, args.monitor, args.shots)
    try:
        run(d)
    except Fail as e:
        print("[thunar-driver] FAIL %s" % e)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
