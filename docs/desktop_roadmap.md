# Desktop Environment Roadmap (Living Document)

Last updated: 2026-11-08

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
  `SO_PEERCRED`.
- Display: one fixed VESA framebuffer (`/dev/fb0`), no mode setting, no GPU.
- Input: PS/2 keyboard and mouse, optional USB tablet, evdev-style
  `/dev/input/event{0,1}` (no `EVIOCG*` ioctls).
- Userspace: static musl binaries; libwayland 1.23.1, wayland-protocols 1.38
  (xdg-shell), libxkbcommon 1.7.0; the `peetwl` compositor with server-side
  decorations, focus, move/close, and XKB keymaps.

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
- [ ] **System information.** A usable `/proc` (and parts of `/sys`), a real-time
  clock, and the input ioctls libinput needs (`EVIOCGNAME`, `EVIOCGBIT`,
  `EVIOCGABS`, ...).

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

- [ ] Port wlroots (pixman renderer, DRM backend, libinput backend) plus
  libinput, libevdev and a minimal seat/udev substitute.
- [ ] Run **labwc** (Openbox-like stacking, closest to Windows behaviour) or a
  custom shell on wlroots. This brings layer-shell (panels, docks), popups,
  xdg-decoration, data-device (drag and drop; `peetwl` already has the
  clipboard), primary selection, viewporter,
  fractional scaling, cursor-shape, presentation-time and linux-dmabuf.
- [ ] Keep `peetwl` as the minimal test and fallback compositor.

### 5. Desktop shell and core applications

- [x] **foot** terminal: foot 1.18.1 with fcft 3.1.8 runs under `peetwl`
  (`make run-foot-demo`, `make run-foot-smoke`; see `docs/rendering.md`), with
  harfbuzz shaping and utf8proc grapheme clustering. It still lacks terminfo.
- [ ] Panel/taskbar and launcher (for example waybar or sfwbar, fuzzel or wofi).
- [x] GTK 3.24 (Wayland backend) with libepoxy, ATK and gdk-pixbuf
  (`make run-gtk-smoke`; see `docs/gtk.md`).
- [ ] GTK applications: a file manager (Thunar, PCManFM), a text editor
  (mousepad) and settings applications. They need D-Bus, librsvg for SVG
  icons, and an icon theme.
- [ ] GTK 4 (needs Mesa/EGL).

### 6. Session services

- [ ] D-Bus (kernel side still needs `SCM_CREDENTIALS` and abstract Unix
  sockets).
- [ ] A session/login manager instead of `init` launching a compositor directly.
- [ ] Notifications; audio (Intel HDA or virtio-snd driver plus PipeWire).
- [ ] Networking basics: DHCP and DNS.

## Near-Term Order

1. ~~Dynamic linking end to end (musl loader, a shared library, `dlopen`, TLS).~~ Done.
2. ~~Lift the platform limits, starting with the VM region-table stack copies.~~ Done.
3. ~~DRM/KMS minimum: mode setting, dumb buffers, page flips.~~ Done (legacy KMS over the boot framebuffer).

4. ~~fcft + foot (without harfbuzz at first)~~ Done.
5. ~~C++ runtime, then harfbuzz~~ Done (GCC + libstdc++; fcft/foot now shape text).
6. ~~libpng, cairo, pango (with GLib)~~ Done.
7. ~~gdk-pixbuf, libepoxy, GTK 3~~ Done. Next: wlroots + labwc, or D-Bus and librsvg for real GTK applications.

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
