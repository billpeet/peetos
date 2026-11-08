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
ALPHA = 0xD05050
ALPHA_ALT = 0x2FAFAF
BETA = 0x4080D0


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
    return {"x": x, "y": y, "w": w, "h": h, "center": (x + w // 2, y + h // 2),
            "probe": (x + w // 4, y + 3 * h // 4),
            "bar": (cx - 30, cy), "grab": (cx - 60, cy), "close": (cx, cy)}


def expect_pixel(img, point, color, what):
    got = img.at(*point)
    if got != color:
        raise Fail("%s at %s: expected 0x%06x, got 0x%06x" % (what, point, color, got))
    print("[wlcomp-driver] %s ok (0x%06x at %s)" % (what, color, point))


def run(d):
    d.wait_for(r"\[wl-demo:Alpha\] first frame presented", timeout=90)
    d.wait_for(r"\[wl-demo:Beta\] first frame presented", timeout=30)
    alpha = parse_mapped(d, "Alpha")
    beta = parse_mapped(d, "Beta")

    img = d.screendump()
    expect_pixel(img, alpha["probe"], ALPHA, "Alpha content")
    expect_pixel(img, beta["probe"], BETA, "Beta content")
    expect_pixel(img, alpha["bar"], COLOR_TITLE, "Alpha title bar (unfocused)")
    expect_pixel(img, beta["bar"], COLOR_TITLE_FOCUSED, "Beta title bar (focused)")

    # Pointer focus, click-to-focus and button delivery.
    mark = d.mark()
    d.move_to(*alpha["center"])
    d.wait_for(r"\[wl-demo:Alpha\] pointer enter", after=mark)
    mark = d.mark()
    d.click()
    d.wait_for(r"\[wl-demo:Alpha\] button 272 pressed", after=mark)
    d.wait_for(r"\[peetwl\] focus 'Alpha'", after=mark)
    d.wait_for(r"\[wl-demo:Alpha\] color now 0x2fafaf", after=mark)
    print("[wlcomp-driver] click routed to Alpha")

    # Keyboard goes to the focused client only.
    mark = d.mark()
    d.monitor("sendkey a")
    d.wait_for(r"\[wl-demo:Alpha\] key 30 pressed", after=mark)
    time.sleep(0.5)
    if "[wl-demo:Beta] key 30 pressed" in d.log()[mark:]:
        raise Fail("unfocused client Beta received the key")
    print("[wlcomp-driver] key routed to focused Alpha only")

    img = d.screendump()
    expect_pixel(img, alpha["probe"], ALPHA_ALT, "Alpha content after click")
    expect_pixel(img, alpha["bar"], COLOR_TITLE_FOCUSED, "Alpha title bar (focused)")
    expect_pixel(img, beta["bar"], COLOR_TITLE, "Beta title bar (unfocused)")

    # Drag Alpha by its title bar.
    d.move_to(*alpha["grab"])
    mark = d.mark()
    d.monitor("mouse_button 1")
    d.wait_for(r"\[peetwl\] move start 'Alpha'", after=mark)
    start = d.cursor()
    before_move = d.mark()
    d.monitor("mouse_move 60 200")
    d.wait_for(r"\[peetwl\] cursor \d+,\d+", after=before_move, timeout=5)
    time.sleep(0.5)
    d.monitor("mouse_button 0")
    m = d.wait_for(r"\[peetwl\] move end 'Alpha' content=(-?\d+),(-?\d+)", after=mark)
    nx, ny = int(m.group(1)), int(m.group(2))
    end = d.cursor()
    if (nx - alpha["x"], ny - alpha["y"]) != (end[0] - start[0], end[1] - start[1]) or ny <= alpha["y"]:
        raise Fail("window moved to %d,%d but cursor moved %s -> %s" % (nx, ny, start, end))
    img = d.screendump()
    moved_probe = (nx + alpha["w"] // 4, ny + 3 * alpha["h"] // 4)
    expect_pixel(img, moved_probe, ALPHA_ALT, "Alpha content after move")

    # Close Beta with its close box.
    mark = d.mark()
    d.move_to(*beta["close"])
    d.click()
    d.wait_for(r"\[peetwl\] close requested 'Beta'", after=mark)
    d.wait_for(r"\[wl-demo:Beta\] close requested", after=mark)
    d.wait_for(r"\[peetwl\] unmapped 'Beta'", after=mark)
    d.wait_for(r"\[peetwl\] focus 'Alpha'", after=mark)
    img = d.screendump()
    if img.at(*beta["probe"]) == BETA:
        raise Fail("Beta still visible after close")
    print("[wlcomp-driver] Beta closed via close box")

    # 'q' quits Alpha; the compositor exits once every client is gone.
    mark = d.mark()
    d.monitor("sendkey q")
    d.wait_for(r"\[wl-demo:Alpha\] key 16 pressed", after=mark)
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
