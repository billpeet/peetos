#!/usr/bin/env python3
"""Drive the desktop session smoke: peet-session (D-Bus session bus + labwc)
with the sfwbar panel and the fuzzel launcher, through QEMU.

1. Waits for the session's autostart hook to see a compositor and a
   session bus, and for the panel to appear on the bus (sfwbar owns the
   tray's StatusNotifierWatcher name).
2. Checks with `screendump` that the panel is a light bar along the bottom
   edge of the black desktop, with an empty taskbar.
3. Clicks the panel's terminal launcher with the PS/2 mouse: a decorated
   foot window must appear, with a button for it in the taskbar.
4. Types `desktopok` + Enter; the shell inside foot runs it and reports its
   pid.
5. Clicks the panel's application menu button, its "System Tools" category
   and then "Thunar File Manager": Thunar must start (its name appears on
   the session bus) and get a second taskbar button.
6. Presses Alt+F2: the fuzzel launcher must appear in the middle of the
   screen and take typed text. `foot` + Enter starts a second terminal,
   which gets a third taskbar button and the keyboard focus (`desktopok`
   now reports another shell).
7. Clicks the first taskbar button to give the first terminal the keyboard
   focus back, and types `desktopbye` + Enter there: the shell stops labwc,
   and the session must end with its bus.

Usage: desktop_smoke_driver.py --log <serial log> --monitor <unix socket>
                               [--shots <dir for PNG copies of screendumps>]
Exits 0 and prints "[desktop-driver] pass" on success.
"""
import argparse
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from wlcomp_smoke_driver import Driver, Fail  # noqa: E402
from foot_smoke_driver import keys  # noqa: E402
from labwc_smoke_driver import find_title_bar, is_light, pointer_to  # noqa: E402

# changed_box() and locate_pointer() below compare whole screendumps.

DESKTOP = 0x000000
FOOT_BG = 0x242424

# The panel's buttons, left to right (user/desktop/sfwbar/sfwbar.config):
# application menu, terminal, file manager, launcher. Each is 28 px wide;
# the taskbar follows them and ends before the clock.
MENU_X = 14
TERMINAL_X = 42
TASKBAR_X0 = 118
CLOCK_WIDTH = 120


def px(img, x, y):
    return img.at(x, y) & 0xFFFFFF


def panel_top(img):
    """The panel: light rows along the bottom edge. Returns its top row."""
    y = img.h - 1
    while y > 0 and is_light(img.at(img.w // 2, y)):
        y -= 1
    return y + 1


def locate_pointer(d):
    """Where the pointer is now: nudge it a few pixels to the right and
    compare two screendumps. Only the pointer moved, so the changed box
    starts at the old arrow's tip and is one arrow plus the nudge wide."""
    nudge = 6
    for _ in range(5):
        before = d.screendump()
        d.monitor("mouse_move %d 0" % nudge)
        time.sleep(0.7)
        box = changed_box(before, d.screendump())
        if box is not None and nudge <= box[2] - box[0] <= 40 and box[3] - box[1] <= 32:
            return box[0] + nudge, box[1]
    raise Fail("could not locate the pointer on screen")


def move_pointer(d, x, y):
    """Put the pointer's tip on (x, y). PS/2 motion is relative and some of
    it can get lost while the guest is busy, so check where the pointer
    ended up and correct it. Locating it leaves it a few pixels off, so
    the last step is always a small correction."""
    pointer_to(d, x, y)
    for _ in range(6):
        cx, cy = locate_pointer(d)
        dx, dy = x - cx, y - cy
        small = abs(dx) <= 12 and abs(dy) <= 12
        if not small:
            print("[desktop-driver] pointer at %d,%d, correcting by %d,%d" % (cx, cy, dx, dy))
        while dx != 0 or dy != 0:
            sx, sy = max(-60, min(60, dx)), max(-60, min(60, dy))
            d.monitor("mouse_move %d %d" % (sx, sy))
            dx, dy = dx - sx, dy - sy
            time.sleep(0.15)
        time.sleep(0.6)
        if small:
            return
    raise Fail("could not move the pointer to %d,%d" % (x, y))


def park_pointer(d, img):
    """Move the pointer to the top-right corner of the desktop, away from
    the panel and the windows, so that it is not mistaken for content."""
    pointer_to(d, img.w - 24, 24)


def taskbar_buttons(img, top):
    """The taskbar's buttons as (x0, x1) spans of what is drawn in them: an
    icon and a label, darker than the panel and than the focused button's
    highlight. Neighbouring buttons are at least 16 px apart (their
    padding); the gaps inside one (icon to label, between words) are
    smaller."""
    spans = []
    start = last = None
    for x in range(TASKBAR_X0, img.w - CLOCK_WIDTH):
        ink = False
        for y in range(top + 4, img.h - 4, 2):
            v = img.at(x, y)
            if min((v >> 16) & 0xFF, (v >> 8) & 0xFF, v & 0xFF) < 0xA0:
                ink = True
                break
        if ink:
            if start is None:
                start = x
            last = x
        elif start is not None and x - last > 12:
            spans.append((start, last))
            start = last = None
    if start is not None:
        spans.append((start, last))
    return spans


def wait_buttons(d, top, count, what, tries=120):
    """Wait until the taskbar shows `count` buttons; returns their spans."""
    spans = []
    for _ in range(tries):
        spans = taskbar_buttons(d.screendump(), top)
        if len(spans) == count:
            return spans
        time.sleep(1.5)
    raise Fail("%s: the taskbar shows %d button(s), expected %d" % (what, len(spans), count))


def typed_in_foot(d, command, report, attempts=2):
    """Type `command` + Enter; the script reports the pid of the shell that
    ran it to the console. Returns that pid. A window that has only just
    appeared can miss the first keys, so the command is typed again once."""
    for attempt in range(attempts):
        mark = d.mark()
        keys(d, list(command) + ["ret"], gap=0.2)
        try:
            m = d.wait_for(r"\[desktop-child\] %s \(shell (\d+)\)" % report, timeout=20, after=mark)
            return int(m.group(1))
        except Fail:
            if attempt + 1 == attempts:
                raise
    raise Fail("unreachable")


def changed_box(a, b):
    """Bounding box (x0, y0, x1, y1) of the pixels that differ between two
    screendumps, or None."""
    stride = a.w * 3
    x0 = y0 = x1 = y1 = None
    for y in range(a.h):
        ra, rb = a.px[y * stride:(y + 1) * stride], b.px[y * stride:(y + 1) * stride]
        if ra == rb:
            continue
        xs = [i // 3 for i in range(stride) if ra[i] != rb[i]]
        x0 = min(xs) if x0 is None else min(x0, min(xs))
        x1 = max(xs) if x1 is None else max(x1, max(xs))
        y0 = y if y0 is None else y0
        y1 = y
    return None if x0 is None else (x0, y0, x1, y1)


def run(d):
    d.wait_for(r"\[desktop-child\] autostart ran with a compositor and a session bus", timeout=300)
    print("[desktop-driver] labwc's autostart ran with the session bus")
    d.wait_for(r"\[desktop-child\] panel is on the session bus", timeout=240)
    print("[desktop-driver] the panel is on the session bus")
    time.sleep(3)

    img = d.screendump()
    park_pointer(d, img)
    img = d.screendump()
    top = panel_top(img)
    height = img.h - top
    full = all(is_light(img.at(x, img.h - 3)) for x in range(130, img.w - 130, 16))
    desk = px(img, img.w // 2, top - 40)
    print("[desktop-driver] panel rows %d..%d, desktop 0x%06x" % (top, img.h - 1, desk))
    if not (20 <= height <= 48) or not full:
        raise Fail("no panel along the bottom edge (top row %d)" % top)
    if desk != DESKTOP:
        raise Fail("desktop above the panel is not black (0x%06x)" % desk)
    if taskbar_buttons(img, top):
        raise Fail("the taskbar is not empty before any window exists")
    row = top + height // 2
    print("[desktop-driver] panel along the bottom edge ok")

    # --- terminal launcher -------------------------------------------------
    move_pointer(d, TERMINAL_X, row)
    d.click()
    park_pointer(d, img)
    buttons = wait_buttons(d, top, 1, "terminal launcher")
    img = d.screendump()
    bar = find_title_bar(img)
    if bar is None or bar[0] > top - 60 or px(img, (bar[1] + bar[2]) // 2, bar[0] + 80) != FOOT_BG:
        raise Fail("clicking the terminal launcher did not open a foot window")
    print("[desktop-driver] terminal launcher opened foot (title bar at y=%d x=%d..%d)" % bar)
    print("[desktop-driver] taskbar shows the foot window (x=%d..%d)" % buttons[0])
    first_button = buttons[0][0] + 8
    time.sleep(2.0)
    first_shell = typed_in_foot(d, "desktopok", r"keyboard input reached the shell in foot")
    print("[desktop-driver] keyboard input reaches foot (shell pid %d)" % first_shell)

    # --- application menu --------------------------------------------------
    # The menu pops up above its button; its only category (both
    # applications are "System Tools") opens the submenu beside it, with
    # Foot and then Thunar File Manager.
    category = (MENU_X + 46, top - 17)
    submenu = (290, top - 20)
    before = px(img, *category)
    move_pointer(d, MENU_X, row)
    d.click()
    time.sleep(3.0)
    img = d.screendump()
    print("[desktop-driver] above the menu button: 0x%06x -> 0x%06x" % (before, px(img, *category)))
    if before != DESKTOP or px(img, *category) == DESKTOP:
        raise Fail("clicking the menu button did not pop up the application menu")
    before = px(img, *submenu)
    move_pointer(d, *category)
    time.sleep(2.0)
    img = d.screendump()
    if not is_light(img.at(*submenu)):
        # Hovering did not open it (the pointer arrived without moving
        # inside the item): a click does.
        d.click()
        time.sleep(3.0)
        img = d.screendump()
    print("[desktop-driver] beside the category: 0x%06x -> 0x%06x" % (before, px(img, *submenu)))
    if before != DESKTOP or not is_light(img.at(*submenu)):
        raise Fail("the menu's category did not open its submenu")
    mark = d.mark()
    move_pointer(d, 225, top + 8)
    d.click()
    d.wait_for(r"\[desktop-child\] Thunar is on the session bus", timeout=300, after=mark)
    print("[desktop-driver] the application menu started Thunar")
    park_pointer(d, img)
    buttons = wait_buttons(d, top, 2, "Thunar", tries=200)
    print("[desktop-driver] taskbar shows foot and Thunar (x=%d..%d and %d..%d)" % (buttons[0] + buttons[1]))
    img = d.screendump()
    if px(img, *category) != DESKTOP:
        raise Fail("the application menu stayed open after starting Thunar")
    time.sleep(5.0)  # let Thunar finish loading its icons

    # --- launcher ----------------------------------------------------------
    # fuzzel is anchored to the centre of the screen.
    keys(d, ["alt-f2"])
    time.sleep(4.0)
    before = d.screendump()
    keys(d, list("foo"), gap=0.3)
    time.sleep(2.5)
    after = d.screendump()
    typed = changed_box(before, after)
    print("[desktop-driver] typing into the launcher changed %s" % (typed,))
    centre_x, centre_y = after.w // 2, after.h // 2
    if typed is None or abs((typed[0] + typed[2]) // 2 - centre_x) > 250 or abs((typed[1] + typed[3]) // 2 - centre_y) > 200:
        raise Fail("Alt+F2 did not show the launcher in the middle of the screen")
    keys(d, ["t", "ret"], gap=0.3)
    buttons = wait_buttons(d, top, 3, "second terminal")
    print("[desktop-driver] the launcher started a second foot (third button at x=%d..%d)" % buttons[2])
    time.sleep(4.0)
    second_shell = typed_in_foot(d, "desktopok", r"keyboard input reached the shell in foot")
    if second_shell == first_shell:
        raise Fail("the new terminal did not get the keyboard focus")
    print("[desktop-driver] the new terminal has the keyboard focus (shell pid %d)" % second_shell)

    # --- taskbar switches windows ------------------------------------------
    move_pointer(d, first_button, row)
    d.click()
    time.sleep(2.5)
    d.screendump()
    mark = d.mark()
    bye_shell = typed_in_foot(d, "desktopbye", r"logging out from foot", attempts=1)
    if bye_shell != first_shell:
        raise Fail("clicking the first taskbar button did not focus the first terminal (shell pid %d)" % bye_shell)
    print("[desktop-driver] the taskbar gave the first terminal the keyboard focus")
    d.wait_for(r"\[musl-ext2-desktop-smoke\] session exited", timeout=90, after=mark)
    d.wait_for(r"\[musl-ext2-desktop-smoke\] the session bus stopped with the session", timeout=30, after=mark)
    print("[desktop-driver] the session ended with its bus")
    print("[desktop-driver] pass")


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
        print("[desktop-driver] FAIL %s" % exc)
        if args.shots:
            try:
                d.screendump()  # keep the screen at the failure
            except Exception:  # noqa: BLE001 - best effort
                pass
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
