#!/usr/bin/env python3
"""Drive the foot terminal smoke: peetwl + foot + wl-demo, through QEMU.

1. Waits for foot and the wl-demo "Clip" window to map.
2. Clicks Clip, which takes the clipboard ("clip-from-demo") on keyboard enter.
3. Clicks foot and checks with `screendump` that the terminal drew text under
   a focused peetwl title bar.
4. Types "hello" with real PS/2 key presses (sendkey) and waits for the shell
   inside foot to report the line on the serial console.
5. Presses Ctrl+Shift+V in foot: the shell must receive "clip-from-demo"
   (wl-demo -> peetwl -> foot over a pipe).
6. Drag-selects the first line ("COPYME-1234") in foot, presses Ctrl+Shift+C,
   then clicks Clip, which must log the pasted text (foot -> wl-demo).
7. Quits Clip with 'q', then presses Enter in foot so its shell exits.

Usage: foot_smoke_driver.py --log <serial log> --monitor <unix socket>
                            [--shots <dir for PNG copies of screendumps>]
Exits 0 and prints "[foot-driver] pass" on success.
"""
import argparse
import os
import re
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from wlcomp_smoke_driver import COLOR_TITLE_FOCUSED, Driver, Fail  # noqa: E402

FOOT_BG = (0x24, 0x24, 0x24)  # foot's default background (#242424)
TITLE_H = 22  # peetwl.c TITLE_H
BORDER = 2    # peetwl.c BORDER


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


def mapped(d, title):
    m = d.wait_for(r"\[peetwl\] mapped '%s' content=(\d+),(\d+),(\d+)x(\d+) title=(\d+),(\d+)"
                   % re.escape(title), timeout=180)
    x, y, w, h, tx, ty = (int(v) for v in m.groups())
    return {"name": title, "x": x, "y": y, "w": w, "h": h, "title": (tx, ty)}


def frame_contains(win, px, py):
    return (win["x"] - BORDER <= px < win["x"] + win["w"] + BORDER
            and win["y"] - TITLE_H - BORDER <= py < win["y"] + win["h"] + BORDER)


def clear_point(win, other):
    """A point inside win's content that other's frame cannot cover, so a
    click lands on win whichever window is on top."""
    for fy in (0.5, 0.25, 0.75, 0.1, 0.9):
        for fx in (0.5, 0.25, 0.75, 0.1, 0.9, 0.05, 0.95):
            px = win["x"] + int(win["w"] * fx)
            py = win["y"] + int(win["h"] * fy)
            if not frame_contains(other, px, py):
                return px, py
    raise Fail("no point of '%s' is clear of '%s'" % (win["name"], other["name"]))


def click_at(d, point):
    d.move_to(*point)
    d.click()


def keys(d, names, gap=0.15):
    for k in names:
        d.monitor("sendkey %s" % k)
        time.sleep(gap)


def run(d):
    d.wait_for(r"\[peetwl\] output drm \d+x\d+", timeout=120)
    foot = mapped(d, "foot")
    clip = mapped(d, "Clip")
    print("[foot-driver] foot at %d,%d %dx%d; Clip at %d,%d %dx%d"
          % (foot["x"], foot["y"], foot["w"], foot["h"], clip["x"], clip["y"], clip["w"], clip["h"]))
    d.wait_for(r"\[foot-child\] started", timeout=60)
    d.wait_for(r"\[foot-child\] prompt shown", timeout=30)

    # Clip takes the clipboard when it first gets keyboard focus. If it mapped
    # last it already has focus; otherwise this click gives it focus.
    click_at(d, clear_point(clip, foot))
    d.wait_for(r"\[wl-demo:Clip\] clipboard set 'clip-from-demo'", timeout=20)
    d.wait_for(r"\[peetwl\] selection set \(focus 'Clip'", timeout=20)
    print("[foot-driver] Clip owns the clipboard")

    # Focus foot (raises it over Clip).
    mark = d.mark()
    click_at(d, clear_point(foot, clip))
    d.wait_for(r"\[peetwl\] focus 'foot'", timeout=20, after=mark)
    time.sleep(1.5)
    img = d.screendump()
    # The frame must be peetwl's (focused foot title bar), not the text
    # console a crashed session would fall back to.
    tx, ty = foot["title"]
    if (img.at(tx, ty) & 0xFFFFFF) != COLOR_TITLE_FOCUSED:
        raise Fail("no focused peetwl title bar at %d,%d (got 0x%06x)" % (tx, ty, img.at(tx, ty) & 0xFFFFFF))
    n = text_pixels(img, foot["x"], foot["y"], min(foot["w"], 600), min(foot["h"], 200))
    print("[foot-driver] %d text pixels in the terminal" % n)
    if n < 500:
        raise Fail("foot did not draw text (only %d bright pixels)" % n)
    print("[foot-driver] terminal text on screen ok")

    mark = d.mark()
    keys(d, ["h", "e", "l", "l", "o", "ret"])
    d.wait_for(r"\[foot-child\] got hello", timeout=30, after=mark)
    print("[foot-driver] typed line reached the shell ok")

    # Clipboard, Clip -> foot.
    d.wait_for(r"\[foot-child\] paste prompt shown", timeout=30, after=mark)
    mark = d.mark()
    keys(d, ["ctrl-shift-v"])
    d.wait_for(r"\[peetwl\] selection transfer mime=", timeout=20, after=mark)
    time.sleep(0.5)
    keys(d, ["ret"])
    d.wait_for(r"\[foot-child\] pasted clip-from-demo", timeout=30, after=mark)
    print("[foot-driver] paste from Clip into foot ok")

    # Clipboard, foot -> Clip. Drag across the first line ("COPYME-1234",
    # about 6 px per cell) with the button held, then copy.
    row_y = foot["y"] + 6
    mark = d.mark()
    d.move_to(foot["x"] + 1, row_y)
    d.monitor("mouse_button 1")
    time.sleep(0.3)
    d.move_to(foot["x"] + 64, row_y)
    time.sleep(0.3)
    d.monitor("mouse_button 0")
    time.sleep(0.5)
    keys(d, ["ctrl-shift-c"])
    d.wait_for(r"\[peetwl\] selection set \(focus 'foot'", timeout=20, after=mark)
    d.wait_for(r"\[wl-demo:Clip\] clipboard ownership lost", timeout=20, after=mark)
    print("[foot-driver] foot owns the clipboard")
    mark = d.mark()
    click_at(d, clear_point(clip, foot))
    m = d.wait_for(r"\[wl-demo:Clip\] paste '([^']*)'", timeout=20, after=mark)
    if "COPYME" not in m.group(1):
        raise Fail("Clip pasted %r, expected the COPYME-1234 selection" % m.group(1))
    print("[foot-driver] paste from foot into Clip ok (%r)" % m.group(1))
    d.screendump()

    # Wind down: Clip has focus, 'q' quits it, and peetwl hands focus back to
    # foot when Clip unmaps; then Enter ends foot's shell.
    mark = d.mark()
    keys(d, ["q"])
    d.wait_for(r"\[wl-demo:Clip\] exiting", timeout=20, after=mark)
    d.wait_for(r"\[peetwl\] focus 'foot'", timeout=20, after=mark)
    time.sleep(0.5)
    keys(d, ["ret"])
    d.wait_for(r"\[foot-child\] exiting", timeout=30, after=mark)
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
