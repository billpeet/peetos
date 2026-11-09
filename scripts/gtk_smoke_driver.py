#!/usr/bin/env python3
"""Drive the GTK 3 smoke: peetwl + user/gtk_smoke.c, through QEMU.

1. Waits for the GTK window to map under peetwl and draw its first frame.
2. Clicks the GtkEntry (a real pointer click), checks with `screendump` that
   peetwl's focused title bar frames a GTK window with drawn widgets, and
   types "hello" with real PS/2 key presses; the entry must report the text.
3. Clicks the "Click me" GtkButton; the app must report the click (with the
   entry text) and exit, and peetwl must see its only client exit cleanly.

Usage: gtk_smoke_driver.py --log <serial log> --monitor <unix socket>
                           [--shots <dir for PNG copies of screendumps>]
Exits 0 and prints "[gtk-driver] pass" on success.
"""
import argparse
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from wlcomp_smoke_driver import COLOR_TITLE_FOCUSED, Driver, Fail  # noqa: E402
from foot_smoke_driver import keys, mapped  # noqa: E402

TITLE = "GTK on PeetOS"


def widget(d, name):
    m = d.wait_for(r"\[gtk-smoke\] widget %s at (-?\d+),(-?\d+) (\d+)x(\d+)" % name, timeout=60)
    return tuple(int(v) for v in m.groups())


def centre(win, w):
    x, y, ww, wh = w
    return win["x"] + x + ww // 2, win["y"] + y + wh // 2


def drawn_pixels(img, win):
    """Pixels in the window content that differ from GTK's light background:
    text, widget borders, the drawing area. A blank or failed client has few."""
    n = 0
    for py in range(win["y"], win["y"] + min(win["h"], 360), 2):
        for px in range(win["x"], win["x"] + min(win["w"], 500), 2):
            v = img.at(px, py)
            r, g, b = (v >> 16) & 0xFF, (v >> 8) & 0xFF, v & 0xFF
            if min(r, g, b) < 0xB0:
                n += 1
    return n


def run(d):
    d.wait_for(r"\[peetwl\] output drm \d+x\d+", timeout=120)
    m = d.wait_for(r"\[gtk-smoke\] start \(GTK ([\d.]+), display (\w+)\)", timeout=180)
    print("[gtk-driver] GTK %s, display %s" % m.groups())
    if m.group(2) != "wayland":
        raise Fail("GTK picked the %s backend, expected wayland" % m.group(2))
    win = mapped(d, TITLE)
    print("[gtk-driver] window at %d,%d %dx%d" % (win["x"], win["y"], win["w"], win["h"]))
    d.wait_for(r"\[gtk-smoke\] first frame drawn", timeout=60)
    entry = widget(d, "entry")
    button = widget(d, "button")

    # Click into the entry (the entry in GTK; peetwl focuses a new window when
    # it maps, and a click would focus it otherwise).
    d.move_to(*centre(win, entry))
    d.click()
    d.wait_for(r"\[peetwl\] focus '%s'" % TITLE, timeout=20)
    time.sleep(1.0)
    img = d.screendump()
    tx, ty = win["title"]
    if (img.at(tx, ty) & 0xFFFFFF) != COLOR_TITLE_FOCUSED:
        raise Fail("no focused peetwl title bar at %d,%d (got 0x%06x)" % (tx, ty, img.at(tx, ty) & 0xFFFFFF))
    n = drawn_pixels(img, win)
    print("[gtk-driver] %d drawn pixels in the window" % n)
    if n < 2000:
        raise Fail("the GTK window looks blank (%d drawn pixels)" % n)
    print("[gtk-driver] GTK window on screen ok")

    mark = d.mark()
    keys(d, ["h", "e", "l", "l", "o"])
    d.wait_for(r"\[gtk-smoke\] entry text 'hello'", timeout=30, after=mark)
    print("[gtk-driver] typed text reached the GtkEntry ok")
    time.sleep(0.5)
    d.screendump()

    mark = d.mark()
    d.move_to(*centre(win, button))
    d.click()
    d.wait_for(r"\[gtk-smoke\] button clicked with entry 'hello'", timeout=30, after=mark)
    print("[gtk-driver] pointer click reached the GtkButton ok")
    time.sleep(1.0)
    d.screendump()
    d.wait_for(r"\[gtk-smoke\] exiting", timeout=30, after=mark)
    d.wait_for(r"\[peetwl\] all clients exited \(failed=0\)", timeout=60)
    print("[gtk-driver] pass")


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
        print("[gtk-driver] FAIL %s" % e)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
