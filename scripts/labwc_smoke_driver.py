#!/usr/bin/env python3
"""Drive the labwc smoke: labwc (wlroots) on DRM with foot, through QEMU.

1. Waits for wlroots to set the mode on the DRM connector and for foot's
   shell to start.
2. Checks with `screendump` that labwc draws foot in a decorated window
   (a light title bar above foot's dark terminal) on the black desktop.
3. Types `labwcok` + Enter with real PS/2 key presses; the shell inside foot
   runs it (libinput -> labwc -> foot -> pty -> shell).
4. Moves the PS/2 mouse pointer onto labwc's maximize button and clicks it;
   the foot window must then cover the desktop.
5. Types `exit` + Enter: the shell exits, the payload stops labwc, and labwc
   must exit cleanly.

Usage: labwc_smoke_driver.py --log <serial log> --monitor <unix socket>
                             [--shots <dir for PNG copies of screendumps>]
Exits 0 and prints "[labwc-driver] pass" on success.
"""
import argparse
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from wlcomp_smoke_driver import Driver, Fail  # noqa: E402
from foot_smoke_driver import keys  # noqa: E402

DESKTOP = 0x000000
FOOT_BG = 0x242424


def pointer_to(d, x, y):
    """Put the PS/2 mouse pointer at (x, y): push it into the top-left
    corner, then move by the exact offset. The payload's labwc config uses
    libinput's flat acceleration profile at speed 0, so motion is 1:1."""
    # Small steps: QEMU's PS/2 queue drops motion sent in one large burst.
    for _ in range(12):
        d.monitor("mouse_move -100 -100")
        time.sleep(0.1)
    time.sleep(0.5)
    while x > 0 or y > 0:
        dx, dy = min(x, 100), min(y, 100)
        d.monitor("mouse_move %d %d" % (dx, dy))
        x, y = x - dx, y - dy
        time.sleep(0.1)
    time.sleep(0.8)


def rgb(v):
    return (v >> 16) & 0xFF, (v >> 8) & 0xFF, v & 0xFF


def is_light(v):
    return min(rgb(v)) >= 0xC0


def find_title_bar(img):
    """The foot window's title bar: the first row from the top with a long
    run of light pixels. Returns (y, x0, x1) or None."""
    for y in range(0, img.h, 2):
        x0 = None
        run = 0
        for x in range(0, img.w):
            if is_light(img.at(x, y)):
                if x0 is None:
                    x0 = x
                run += 1
            elif run < 200:
                x0, run = None, 0
            else:
                break
        if run >= 200:
            return y, x0, x0 + run - 1
    return None


def run(d):
    d.wait_for(r"Modesetting with \d+x\d+", timeout=180)
    print("[labwc-driver] wlroots set the mode on the DRM connector")
    d.wait_for(r"\[labwc-child\] started in foot", timeout=120)
    time.sleep(3)

    img = d.screendump()
    bar = find_title_bar(img)
    if bar is None:
        raise Fail("no decorated window on screen")
    y, x0, x1 = bar
    mid = (x0 + x1) // 2
    body = img.at(mid, y + 80) & 0xFFFFFF
    corner = img.at(5, img.h - 5) & 0xFFFFFF
    print("[labwc-driver] title bar at y=%d x=%d..%d, body 0x%06x, desktop 0x%06x" % (y, x0, x1, body, corner))
    if body != FOOT_BG:
        raise Fail("no foot terminal under the title bar (0x%06x)" % body)
    if corner != DESKTOP:
        raise Fail("desktop corner is not black (0x%06x)" % corner)
    print("[labwc-driver] foot window decorated by labwc ok")

    mark = d.mark()
    keys(d, ["l", "a", "b", "w", "c", "o", "k", "ret"], gap=0.2)
    d.wait_for(r"\[labwc-child\] keyboard input reached the shell", timeout=30, after=mark)
    print("[labwc-driver] keyboard input through libinput ok")

    # labwc's title bar: menu button on the left; iconify, maximize and close
    # on the right, each about 26 px wide. Maximize is second from the right.
    max_x, max_y = x1 - 38, y + 12
    pointer_to(d, max_x, max_y)
    d.click()
    time.sleep(2.0)
    img = d.screendump()
    covered = img.at(5, img.h - 5) & 0xFFFFFF
    bar = find_title_bar(img)
    print("[labwc-driver] after the maximize click: corner 0x%06x, title bar %s" % (covered, bar))
    if covered != FOOT_BG or bar is None or bar[1] > 2 or bar[2] < img.w - 3:
        raise Fail("clicking maximize did not maximize foot")
    print("[labwc-driver] pointer click on the maximize button ok")

    mark = d.mark()
    keys(d, ["e", "x", "i", "t", "ret"], gap=0.2)
    d.wait_for(r"\[labwc-child\] shell exited", timeout=30, after=mark)
    d.wait_for(r"\[musl-ext2-labwc-smoke\] labwc exited", timeout=60, after=mark)
    print("[labwc-driver] labwc exited after its client")
    print("[labwc-driver] pass")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--log", required=True)
    ap.add_argument("--monitor", required=True)
    ap.add_argument("--shots")
    args = ap.parse_args()
    d = Driver(args.log, args.monitor, args.shots)
    try:
        run(d)
    except Fail as exc:
        print("[labwc-driver] FAIL %s" % exc)
        if args.shots:
            try:
                d.screendump()  # keep the screen at the failure
            except Exception:  # noqa: BLE001 - best effort
                pass
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
