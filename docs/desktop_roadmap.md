# Desktop Environment Roadmap (Living Document)

Last updated: 2026-11-10

This is the plan from today's Wayland stack (libwayland, wayland-protocols,
xkbcommon, the `peetwl` compositor) to a modern stacking desktop in the style of
Windows, GNOME or Plasma. It complements `docs/gui_wayland_plan.md` (the
phase-by-phase Wayland bring-up) and `docs/PORTING_PLAN.md` (overall kernel
status).

## Target

GNOME's Mutter and Plasma's KWin depend on systemd/logind, D-Bus, GLib or Qt,
PipeWire, polkit and GL-accelerated rendering, none of which PeetOS has yet. The
practical target is a **stacking desktop built from wlroots-family
components**: a real compositor (for example labwc), a panel/taskbar, a
launcher, a terminal and a file manager, all running Wayland-native apps. Every
layer of that stack is also a prerequisite for GNOME- or Plasma-class software
later.

## Where We Are

- Kernel: x86_64, SMP, CoW fork + threads, Linux-compatible syscall surface
  including epoll/eventfd/timerfd/signalfd, memfd + sealing, `SCM_RIGHTS`,
  `SO_PEERCRED`, `#!` scripts and a real-time clock.
- Display: a simpledrm-style DRM/KMS device over one fixed VESA framebuffer;
  no mode switching, no GPU.
- Input: PS/2 keyboard and mouse, optional USB tablet, evdev devices with the
  ioctls libinput needs, and a static `/sys`.
- Userspace: musl with shared libraries and a C++ runtime; the Wayland,
  rendering, GTK 3, D-Bus and wlroots stacks below.
- Desktop: `peet-session` starts a session bus and labwc with a panel
  (sfwbar) and a launcher (fuzzel); foot and Thunar are the applications
  (`make run-desktop`, `docs/desktop.md`).

## Phases (in dependency order)

### 1. Platform limits real applications hit first

- [x] **Shared libraries.** musl's dynamic loader (`/lib/ld-musl-x86_64.so.1`)
  running dynamically linked PIE programs with `.so` dependencies, `dlopen`, and
  TLS in shared objects. Toolkits are far too large to link statically into
  every program, and static linking multiplies memory use. *(Done:
  `make run-dynlink-smoke`, see "Dynamic Linking" in `docs/elf.md`. libwayland,
  libffi and libxkbcommon also build as `.so` files, and `peetwl`/`wl_demo`
  link against them.)*
- [x] **Process and memory limits.** Done:
  - Physical memory: up to 4 GiB, through a new kernel direct map (was 128 MiB).
  - VM region table: 256 slots (was 32).
  - Per-process mapping budget: 256 MiB (was 32 MiB).
  - memfd size: 256 MiB (was 64 MiB).
  - Tasks: 256 (was 64).
  - Sockets: 256, with 128 `AF_UNIX` bindings (was 96 and 48).
  - Directory size: the ext4 work removed the 64-entries-per-directory limit.

  `make run-dynlink-smoke` checks the new limits in QEMU with 512 MiB.

  Still static and worth raising when something hits them:
  - 128 fds per process
  - 32 fds per `poll` call (the array is on the stack)
  - 64 epoll watches per instance
  - 64 pipes, 64 shmem objects
  - 32 eventfds and 32 timerfds
- [ ] **System information.** A usable `/proc`. *(Done so far: a static `/sys`
  and the input ioctls libinput needs (`docs/labwc.md`), and a real-time
  clock from the CMOS RTC (`docs/syscall.md`). `/proc` is still minimal:
  there is no `/proc/self/fd`, for example, so `ttyname` fails.)*

### 2. A real display interface

- [x] **Minimal DRM/KMS.** `/dev/dri/card0` is a simpledrm-style device over
  the boot framebuffer. It supports legacy mode setting, dumb buffers, page
  flips with completion events, DIRTYFB and PRIME, and libdrm 2.4.123 is
  ported. `peetwl` presents through it with page flips. See `docs/drm.md`.
- [ ] A real display driver: QEMU `bochs-display` or `virtio-gpu`, for mode
  switching (any resolution), real vblank and a hardware cursor. Atomic KMS,
  which wlroots prefers, also needs adding.
- [ ] Later: `virtio-gpu` with virgl for host-accelerated GL.

### 3. Software rendering stack

- [x] pixman, freetype, fontconfig + fonts (DejaVu), and expat. They are
  built into a staged sysroot with meson, and text is rendered on screen
  through DRM (`docs/rendering.md`).
- [x] harfbuzz 10.1.0 and utf8proc 2.9.0: text shaping in fcft and grapheme
  clustering in foot (`docs/rendering.md`).
- [x] zlib, libpng, pcre2, GLib 2.82, fribidi, cairo 1.18 and pango 1.54
  (`make run-pango-smoke`; see `docs/rendering.md`).
- [x] A C++ runtime for musl: GCC 15.2 cross toolchain with libstdc++,
  libgcc_s and libatomic (`make -C toolchain`, `make run-cxx-smoke`; see
  `docs/cxx.md`), needed by Mesa, Qt and many applications.
- [ ] Mesa with `softpipe` (or `llvmpipe`, which needs LLVM) for EGL/GLES; GTK4
  and Qt prefer GL even though they have software fallbacks.

### 4. A real compositor stack

- [x] Port wlroots 0.18 (pixman renderer, DRM backend, libinput backend) plus
  libinput, libevdev, libudev-zero over a static `/sys`, and libseat's noop
  seat (`docs/labwc.md`).
- [x] Run **labwc** 0.8 (Openbox-like stacking) on wlroots
  (`make run-labwc-smoke`). This brings layer-shell (panels, docks), popups,
  xdg-decoration, data-device, primary selection, viewporter, fractional
  scaling, cursor-shape and presentation-time; linux-dmabuf needs a GPU.
- [x] Keep `peetwl` as the minimal test and fallback compositor (it still reads
  the legacy `event1` pointer aggregate).

### 5. Desktop shell and core applications

- [x] **foot** terminal: foot 1.18.1 with fcft 3.1.8 runs under `peetwl`
  (`make run-foot-demo`, `make run-foot-smoke`; see `docs/rendering.md`), with
  harfbuzz shaping and utf8proc grapheme clustering. It still lacks terminfo.
- [x] Panel/taskbar and launcher: sfwbar 1.0 (application menu, launchers,
  taskbar, tray, clock) and fuzzel 1.11 (`make run-desktop`,
  `make run-desktop-smoke`; see `docs/desktop.md`). There is no notification
  center and no wallpaper yet.
- [x] GTK 3.24 (Wayland backend) with libepoxy, ATK and gdk-pixbuf
  (`make run-gtk-smoke`; see `docs/gtk.md`).
- [x] File manager: Thunar 4.20 (`make run-thunar-smoke`; see
  `docs/thunar.md`), with xfconf settings on the session bus, but without
  inotify, a thumbnailer or SVG icons.
- [ ] More GTK applications: a text editor (mousepad) and settings
  applications. inotify and librsvg would make them (and Thunar) fully
  functional.
- [ ] GTK 4 (needs Mesa/EGL).

### 6. Session services

- [x] D-Bus session bus (dbus 1.16; `make run-dbus-smoke`, see `docs/dbus.md`).
  Abstract Unix sockets and a system bus are still missing.
- [x] A session launcher: `peet-session` starts the session bus and labwc
  together, and labwc's autostart starts the panel (`docs/desktop.md`).
- [ ] A login/display manager: `init` still starts the session directly, as
  root.
- [ ] Notifications; audio (Intel HDA or virtio-snd driver plus PipeWire).
- [ ] Networking basics: DHCP and DNS.

## Near-Term Order

1. ~~Dynamic linking end to end (musl loader, a shared library, `dlopen`, TLS).~~ Done.
2. ~~Lift the platform limits, starting with the VM region-table stack copies.~~ Done.
3. ~~DRM/KMS minimum: mode setting, dumb buffers, page flips.~~ Done (legacy KMS over the boot framebuffer).

4. ~~fcft + foot (without harfbuzz at first)~~ Done.
5. ~~C++ runtime, then harfbuzz~~ Done (GCC + libstdc++; fcft/foot now shape text).
6. ~~libpng, cairo, pango (with GLib)~~ Done.
7. ~~gdk-pixbuf, libepoxy, GTK 3~~ Done.
8. ~~A GTK application: Thunar~~ Done.
9. ~~D-Bus session bus~~ Done (Thunar settings through xfconf).
10. ~~wlroots + labwc~~ Done (`make run-labwc-smoke`).
11. ~~A session launcher that starts the bus and labwc, a panel and a launcher; GTK applications under labwc~~ Done (`make run-desktop`). Next: inotify (Thunar refresh), more applications with desktop entries (a text editor, settings), a wallpaper and notifications (both want an SVG loader), and a login manager.

First big milestone: **foot running under `peetwl`** with fonts and a usable
shell, before the much larger wlroots and GTK ports. *(Reached: foot renders
text with fcft and the shell inside it receives typed input.)*

## Risks and Dependencies

- Each toolkit port drags in a large dependency tree; shared-library support
  and the platform limits above decide whether that is tractable.
- Software rendering at desktop resolutions is CPU-heavy under QEMU TCG;
  `virtio-gpu`/virgl is the escape hatch.
- Build-host assumptions (xkeyboard-config data, bison, expat) should move to
  pinned, fetched inputs as the stack grows.

## Update Rules

- Tick items as they land and link the doc that describes them.
- Keep detailed status in the subsystem docs; this file tracks direction.
