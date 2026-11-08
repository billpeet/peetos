#!/usr/bin/env python3
"""Drive the foot terminal smoke: peetwl + foot + a shell, through QEMU.

Waits for foot's window to map, checks with `screendump` that the terminal
drew text, types "hello" with real PS/2 key presses (sendkey), and waits for
the shell inside foot to report the line on the serial console.

Usage: foot_smoke_driver.py --log <serial log> --monitor <unix socket>
                            [--shots <dir for PNG copies of screendumps>]
Exits 0 and prints "[foot-driver] pass" on success.
"""
import argparse
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from wlcomp_smoke_driver import COLOR_TITLE_FOCUSED, Driver, Fail  # noqa: E402

FOOT_BG = (0x24, 0x24, 0x24)  # foot's default background (#242424)


def text_pixels(img, x, y, w, h):
    """Pixels in the box that differ clearly from foot's background."""
    n = 0
    for py in range(y, y + h, 1):
        for px in range(x, x + w, 1):
            v = img.at(px, py)
            r, g, b = (v >> 16) & 0xFF, (v >> 8) & 0xFF, v & 0xFF
            if abs(r - FOOT_BG[0]) + abs(g - FOOT_BG[1]) + abs(b - FOOT_BG[2]) > 120:
                n += 1
    return n


def run(d):
    d.wait_for(r"\[peetwl\] output drm \d+x\d+", timeout=120)
    m = d.wait_for(r"\[peetwl\] mapped '([^']*)' content=(\d+),(\d+),(\d+)x(\d+) title=(\d+),(\d+)",
                   timeout=180)
    title = m.group(1)
    x, y, w, h, tx, ty = (int(v) for v in m.groups()[1:])
    print("[foot-driver] foot mapped as '%s' at %d,%d %dx%d" % (title, x, y, w, h))
    d.wait_for(r"\[foot-child\] started", timeout=60)
    d.wait_for(r"\[foot-child\] prompt shown", timeout=30)
    time.sleep(1.5)
    img = d.screendump()
    # The frame must be peetwl's (focused foot title bar), not the text
    # console a crashed session would fall back to.
    if (img.at(tx, ty) & 0xFFFFFF) != COLOR_TITLE_FOCUSED:
        raise Fail("no focused peetwl title bar at %d,%d (got 0x%06x)" % (tx, ty, img.at(tx, ty) & 0xFFFFFF))
    n = text_pixels(img, x, y, min(w, 600), min(h, 200))
    print("[foot-driver] %d text pixels in the terminal" % n)
    if n < 500:
        raise Fail("foot did not draw text (only %d bright pixels)" % n)
    print("[foot-driver] terminal text on screen ok")

    mark = d.mark()
    for key in ["h", "e", "l", "l", "o", "ret"]:
        d.monitor("sendkey %s" % key)
        time.sleep(0.15)
    d.wait_for(r"\[foot-child\] got hello", timeout=30, after=mark)
    print("[foot-driver] typed line reached the shell ok")
    time.sleep(1.0)
    d.screendump()

    d.wait_for(r"\[peetwl\] all clients exited \(failed=0\)", timeout=60)
    print("[foot-driver] pass")


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
        print("[foot-driver] FAIL %s" % e)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
