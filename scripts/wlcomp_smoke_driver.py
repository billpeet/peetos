#!/usr/bin/env python3
"""Drive the peetwl compositor smoke through the QEMU monitor.

Injects real PS/2 input (mouse_move / mouse_button / sendkey), follows the
serial log, and checks what is actually on screen with `screendump`.

Usage: wlcomp_smoke_driver.py --log <serial log> --monitor <unix socket>
                              [--shots <dir for PNG copies of screendumps>]
Exits 0 and prints "[wlcomp-driver] pass" on success.
"""
import argparse
import os
import re
import socket
import struct
import sys
import time
import zlib

COLOR_TITLE = 0x505860
COLOR_TITLE_FOCUSED = 0x3A6EA5
# Client colour and the colour it toggles to on a left click (bitwise NOT).
COLORS = {
    "Alpha": (0xD05050, 0x2FAFAF),
    "Beta": (0x4080D0, 0xBF7F2F),
}


class Fail(Exception):
    pass


class Driver:
    def __init__(self, log_path, monitor_path, shots):
        self.log_path = log_path
        self.monitor_path = monitor_path
        self.shots = shots
        self.shot_no = 0

    # -- log ---------------------------------------------------------------
    def log(self):
        try:
            with open(self.log_path, "r", errors="replace") as f:
                return f.read()
        except FileNotFoundError:
            return ""

    def wait_for(self, pattern, timeout=30.0, after=0):
        rx = re.compile(pattern)
        deadline = time.time() + timeout
        while time.time() < deadline:
            m = rx.search(self.log(), after)
            if m:
                return m
            time.sleep(0.1)
        raise Fail("timed out waiting for /%s/" % pattern)

    def mark(self):
        return len(self.log())

    def cursor(self):
        found = re.findall(r"\[peetwl\] cursor (\d+),(\d+)", self.log())
        if not found:
            raise Fail("no cursor position logged")
        x, y = found[-1]
        return int(x), int(y)

    # -- monitor -----------------------------------------------------------
    def monitor(self, cmd):
        s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        s.connect(self.monitor_path)
        s.sendall((cmd + "\n").encode())
        time.sleep(0.15)
        s.setblocking(False)
        try:
            s.recv(65536)
        except BlockingIOError:
            pass
        s.close()

    def move_to(self, x, y):
        for _ in range(10):
            cx, cy = self.cursor()
            dx, dy = x - cx, y - cy
            if abs(dx) <= 1 and abs(dy) <= 1:
                return
            before = self.mark()
            self.monitor("mouse_move %d %d" % (dx, dy))
            try:
                self.wait_for(r"\[peetwl\] cursor \d+,\d+", timeout=3.0, after=before)
            except Fail:
                pass
            time.sleep(0.2)
        raise Fail("could not move cursor to %d,%d (at %s)" % (x, y, self.cursor()))

    def click(self):
        self.monitor("mouse_button 1")
        time.sleep(0.25)
        self.monitor("mouse_button 0")
        time.sleep(0.25)

    # -- screen ------------------------------------------------------------
    def screendump(self):
        path = "/tmp/wlcomp-shot-%d-%d.ppm" % (os.getpid(), self.shot_no)
        self.shot_no += 1
        if os.path.exists(path):
            os.unlink(path)
        time.sleep(0.4)  # let the compositor repaint
        self.monitor("screendump %s" % path)
        deadline = time.time() + 10
        last = -1
        while time.time() < deadline:
            if os.path.exists(path):
                size = os.path.getsize(path)
                if size > 0 and size == last:
                    break
                last = size
            time.sleep(0.2)
        with open(path, "rb") as f:
            data = f.read()
        os.unlink(path)
        header, w, h, px = parse_ppm(data)
        if self.shots:
            os.makedirs(self.shots, exist_ok=True)
            write_png(os.path.join(self.shots, "shot%d.png" % (self.shot_no - 1)), w, h, px)
        return Image(w, h, px)


class Image:
    def __init__(self, w, h, px):
        self.w, self.h, self.px = w, h, px

    def at(self, x, y):
        i = (y * self.w + x) * 3
        return (self.px[i] << 16) | (self.px[i + 1] << 8) | self.px[i + 2]


def parse_ppm(data):
    fields = []
    pos = 0
    while len(fields) < 4:
        while data[pos:pos + 1].isspace():
            pos += 1
        if data[pos:pos + 1] == b"#":
            pos = data.index(b"\n", pos) + 1
            continue
        start = pos
        while not data[pos:pos + 1].isspace():
            pos += 1
        fields.append(data[start:pos])
    pos += 1
    if fields[0] != b"P6" or int(fields[3]) != 255:
        raise Fail("unexpected screendump format %r" % fields[:4])
    w, h = int(fields[1]), int(fields[2])
    return fields, w, h, data[pos:pos + w * h * 3]


def write_png(path, w, h, rgb):
    raw = b"".join(b"\x00" + rgb[y * w * 3:(y + 1) * w * 3] for y in range(h))

    def chunk(tag, payload):
        return (struct.pack(">I", len(payload)) + tag + payload
                + struct.pack(">I", zlib.crc32(tag + payload) & 0xFFFFFFFF))

    with open(path, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n")
        f.write(chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)))
        f.write(chunk(b"IDAT", zlib.compress(raw, 6)))
        f.write(chunk(b"IEND", b""))


def parse_mapped(d, title):
    m = d.wait_for(r"\[peetwl\] mapped '%s' content=(\d+),(\d+),(\d+)x(\d+) "
                   r"title=(\d+),(\d+) close=(\d+),(\d+)" % title)
    x, y, w, h, tx, ty, cx, cy = map(int, m.groups())
    # "center" is where the pointer clicks; pixels are sampled at "probe" so
    # the software cursor never covers the sample.
    return {"name": title, "order": m.start(), "x": x, "y": y, "w": w, "h": h,
            "center": (x + w // 2, y + h // 2),
            "probe": (x + w // 4, y + 3 * h // 4),
            "bar": (cx - 30, cy), "grab": (cx - 60, cy), "close": (cx, cy),
            "color": COLORS[title][0], "alt": COLORS[title][1]}


def expect_pixel(img, point, color, what):
    got = img.at(*point)
    if got != color:
        raise Fail("%s at %s: expected 0x%06x, got 0x%06x" % (what, point, color, got))
    print("[wlcomp-driver] %s ok (0x%06x at %s)" % (what, color, point))


def run(d):
    d.wait_for(r"\[peetwl\] keymap layout=German", timeout=90)
    for name in COLORS:
        d.wait_for(r"\[wl-demo:%s\] xkb keymap loaded \(\d+ bytes, layout German\)" % name,
                   timeout=90)
        d.wait_for(r"\[wl-demo:%s\] first frame presented" % name, timeout=90)
    windows = sorted((parse_mapped(d, name) for name in COLORS), key=lambda w: w["order"])
    # Clients start in parallel, so map order varies (notably with -smp). The
    # last-mapped window starts focused; the other one is exercised for
    # click-to-focus, typing and moving, and the focused one gets closed.
    first, second = windows
    a, b = first["name"], second["name"]
    print("[wlcomp-driver] mapped %s then %s" % (a, b))

    img = d.screendump()
    expect_pixel(img, first["probe"], first["color"], "%s content" % a)
    expect_pixel(img, second["probe"], second["color"], "%s content" % b)
    expect_pixel(img, first["bar"], COLOR_TITLE, "%s title bar (unfocused)" % a)
    expect_pixel(img, second["bar"], COLOR_TITLE_FOCUSED, "%s title bar (focused)" % b)

    # Pointer focus, click-to-focus and button delivery.
    mark = d.mark()
    d.move_to(*first["center"])
    d.wait_for(r"\[wl-demo:%s\] pointer enter" % a, after=mark)
    mark = d.mark()
    d.click()
    d.wait_for(r"\[wl-demo:%s\] button 272 pressed" % a, after=mark)
    d.wait_for(r"\[peetwl\] focus '%s'" % a, after=mark)
    d.wait_for(r"\[wl-demo:%s\] color now 0x%06x" % (a, first["alt"]), after=mark)
    print("[wlcomp-driver] click routed to %s" % a)

    # Keyboard goes to the focused client only.
    mark = d.mark()
    d.monitor("sendkey a")
    d.wait_for(r"\[wl-demo:%s\] key 30 pressed sym=a text='a'" % a, after=mark)
    time.sleep(0.5)
    if "[wl-demo:%s] key 30 pressed" % b in d.log()[mark:]:
        raise Fail("unfocused client %s received the key" % b)
    print("[wlcomp-driver] key routed to focused %s only" % a)

    # xkbcommon: the compositor runs the German layout, so these checks prove
    # the staged keymap data (not just US defaults) drives translation.
    def typed(keys, pattern, what):
        mark = d.mark()
        d.monitor("sendkey %s" % keys)
        d.wait_for(r"\[wl-demo:%s\] key \d+ pressed %s" % (a, pattern), after=mark)
        print("[wlcomp-driver] %s" % what)

    typed("shift-a", r"sym=A text='A'", "Shift+a gives 'A'")
    typed("y", r"sym=z text='z'", "de layout: y key types 'z'")
    typed("shift-2", r"sym=quotedbl text='\"'", "de layout: Shift+2 types '\"'")
    typed("caps_lock", r"sym=Caps_Lock", "Caps Lock pressed")
    typed("b", r"sym=B text='B'", "Caps Lock locks uppercase")
    typed("caps_lock", r"sym=Caps_Lock", "Caps Lock released")
    typed("c", r"sym=c text='c'", "Caps Lock unlocked")

    img = d.screendump()
    expect_pixel(img, first["probe"], first["alt"], "%s content after click" % a)
    expect_pixel(img, first["bar"], COLOR_TITLE_FOCUSED, "%s title bar (focused)" % a)
    expect_pixel(img, second["bar"], COLOR_TITLE, "%s title bar (unfocused)" % b)

    # Drag the clicked window by its title bar.
    d.move_to(*first["grab"])
    mark = d.mark()
    d.monitor("mouse_button 1")
    d.wait_for(r"\[peetwl\] move start '%s'" % a, after=mark)
    start = d.cursor()
    before_move = d.mark()
    d.monitor("mouse_move 60 200")
    d.wait_for(r"\[peetwl\] cursor \d+,\d+", after=before_move, timeout=5)
    time.sleep(0.5)
    d.monitor("mouse_button 0")
    m = d.wait_for(r"\[peetwl\] move end '%s' content=(-?\d+),(-?\d+)" % a, after=mark)
    nx, ny = int(m.group(1)), int(m.group(2))
    end = d.cursor()
    if (nx - first["x"], ny - first["y"]) != (end[0] - start[0], end[1] - start[1]) or ny <= first["y"]:
        raise Fail("window moved to %d,%d but cursor moved %s -> %s" % (nx, ny, start, end))
    img = d.screendump()
    moved_probe = (nx + first["w"] // 4, ny + 3 * first["h"] // 4)
    expect_pixel(img, moved_probe, first["alt"], "%s content after move" % a)

    # Close the other window with its close box.
    mark = d.mark()
    d.move_to(*second["close"])
    d.click()
    d.wait_for(r"\[peetwl\] close requested '%s'" % b, after=mark)
    d.wait_for(r"\[wl-demo:%s\] close requested" % b, after=mark)
    d.wait_for(r"\[peetwl\] unmapped '%s'" % b, after=mark)
    d.wait_for(r"\[peetwl\] focus '%s'" % a, after=mark)
    img = d.screendump()
    if img.at(*second["probe"]) == second["color"]:
        raise Fail("%s still visible after close" % b)
    print("[wlcomp-driver] %s closed via close box" % b)

    # 'q' quits the remaining client; the compositor exits once all are gone.
    mark = d.mark()
    d.monitor("sendkey q")
    d.wait_for(r"\[wl-demo:%s\] key 16 pressed sym=q" % a, after=mark)
    d.wait_for(r"\[peetwl\] all clients exited \(failed=0\)", after=mark, timeout=30)
    d.wait_for(r"\[peetwl\] pass", after=mark, timeout=30)
    print("[wlcomp-driver] pass")


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
        print("[wlcomp-driver] FAIL %s" % e)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
