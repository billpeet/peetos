# wlroots and labwc

labwc 0.8, a stacking Wayland compositor (Openbox-style), runs on PeetOS on top of wlroots 0.18. It drives the display through the wlroots DRM backend on `/dev/dri/card0` and takes input from libinput on the evdev devices. Clients such as foot get server-side window decorations, a root menu, keyboard focus and pointer interaction. `peetwl` remains the minimal test and fallback compositor.

**Where It Lives**
- `wayland/fetch-wayland.sh` pins the sources (SHA-256): libevdev 1.13.3 (GitLab tag archive), mtdev 1.1.7, libudev-zero 1.0.3, libinput 1.27.1, seatd 0.9.1, hwdata 0.390, libdisplay-info 0.2.0, wlroots 0.18.2, libxml2 2.13.5 and labwc 0.8.4.
- `scripts/build_wlroots.sh` builds them into the staged sysroot (`wayland/out/stage`). `make -C wayland` runs it after `scripts/build_thunar.sh`.
- `scripts/build_ext2_musl_execve.sh` stages `bin/labwc` and libinput's device quirks (`/usr/share/libinput`). The libraries come with the other shared libraries.
- `user/musl_execve_ext2_labwc_smoke.c`, `scripts/labwc_smoke_driver.py`, `scripts/run_toybox_labwc_smoke.sh` and `tests/musl_ext2_labwc_smoke.sh` are the QEMU smoke (`make run-labwc-smoke`).

**How It Is Built**
- **Input stack.**
  - libevdev and libinput are built with meson. libinput has no libwacom, debug GUI, tests or docs, and its tools are built but not staged.
  - mtdev uses autotools.
  - libudev-zero is a drop-in libudev that reads `/sys` and `/dev` directly, without a udev daemon. The kernel provides the static `/sys` it reads (`kernel/src/sysfs.rs`, see `docs/vfs.md`).
- **Seat.** libseat (seatd's library) is used with its noop backend (`LIBSEAT_BACKEND=noop`), which opens devices directly as root. The seatd client backend is built only because libseat needs one real backend. There is no seatd daemon, logind or built-in seat, since those need VT/KD ioctls PeetOS lacks.
- **Display info.** hwdata's `pnp.ids` is staged, and a build-machine `hwdata.pc` points libdisplay-info and wlroots at it.
- **wlroots** is built with the DRM and libinput backends, session support, no renderer beyond the always-built pixman one, no GBM allocator (it uses DRM dumb buffers), and no Xwayland, libliftoff or color management.
- **labwc** has no Xwayland, SVG buttons, window icons, NLS or man pages.
  - libxml2 2.13's meson build writes a wrong `Cflags`, so the script fixes `libxml-2.0.pc`.
  - libudev-zero's monitor gets a minimal `<linux/netlink.h>` from a private include directory (`wayland/peetos/libudev-zero-include`). In the shared uapi shims it would make GLib build its netlink network monitor, which needs `<linux/rtnetlink.h>`.

**Running labwc**
```sh
export XDG_RUNTIME_DIR=/tmp/xdg-runtime LIBSEAT_BACKEND=noop
export WLR_RENDERER=pixman WLR_DRM_NO_ATOMIC=1 WLR_BACKENDS=drm,libinput
mkdir -p -m 0700 "$XDG_RUNTIME_DIR"
labwc -s foot
```
The DRM device is legacy-KMS only, so wlroots needs `WLR_DRM_NO_ATOMIC=1`. There is no GPU, so it needs the pixman renderer. labwc reads its configuration from `~/.config/labwc` and then `/etc/xdg/labwc`. The defaults give a root menu (Reconfigure, Exit) on a desktop click.

**Kernel work labwc needed**
- **Device nodes** in `/dev` report `S_IFCHR` with Linux device numbers (`st_rdev`), for example 226:0 for `card0` and 13:64+N for `eventN`. Before this they reported regular files (`docs/vfs.md`).
- **`/sys`**: a static tree with the DRM node under a simpledrm-like platform device, the evdev devices with their capability masks, and `/sys/dev/char`/`/sys/class` links, enough for libudev-zero and libdrm's `drmGetDeviceNameFromFd2` (`docs/vfs.md`).
- **Symlinks in the middle of paths** are now followed. libdrm opens `/sys/dev/char/226:0/uevent`, which goes through a symlink.
- **evdev**:
  - `EVIOC*` capability ioctls (version, id, name, phys, capability bits, absolute axis info, key/LED/switch state, grab, clock).
  - Separate devices for the PS/2 mouse (`event2`) and the USB tablet (`event3`). `event1` stays the legacy pointer aggregate that `peetwl` reads (`docs/drivers.md`).
- **ioctl requests are 32 bits.** musl passes them as `int`, so every `_IOR` request arrived sign-extended, and `EVIOCGID` failed.
- **netlink**: an `AF_NETLINK`/`NETLINK_KOBJECT_UEVENT` socket for udev monitors. It binds, and never has events since there is no hotplug.
- **Nested epoll.** libinput hands its own epoll fd to the compositor's event loop. An epoll fd is now pollable and can be added to another epoll set.
- **DRM**:
  - `DRM_MODE_OBJECT_ANY` property queries.
  - Hiding the (nonexistent) hardware cursor succeeds, because wlroots' legacy commit does that on every modeset.
- **POSIX shared memory**: `/dev/shm` exists, tmpfs files may grow to 16 MiB, and writes through a `MAP_SHARED` tmpfs mapping are visible to `read()` and private mappings. wlroots passes keymaps that way.
- **`waitid`**, and **`ENOEXEC`** (instead of `EINVAL`) from `execve` of a non-ELF file, so shells run scripts without a `#!` line.

**Testing**
`make run-labwc-smoke` (part of `make test`) boots the `compositor-first musl-ext2-labwc-smoke` profile. `bin/labwc_smoke.sh`:
- writes a labwc `rc.xml` with libinput's flat pointer acceleration at speed 0, so PS/2 motion is 1:1;
- starts `labwc -s "foot … labwc_foot.sh"`, where the shell inside foot reports to the console and stops labwc (SIGTERM to `$LABWC_PID`) when it exits.

`scripts/labwc_smoke_driver.py` then:
- waits for wlroots to set the mode on the DRM connector and for foot's shell to start;
- finds foot's window in a screendump: labwc's light title bar above foot's dark terminal, on the black desktop;
- types `labwcok` + Enter with real PS/2 key presses. That script must report from inside foot's shell, which proves the path libinput → labwc → foot → pty → shell;
- pushes the PS/2 pointer into the corner, moves it onto labwc's maximize button and clicks. Foot must then cover the screen, with its title bar at the top edge;
- types `exit` + Enter, and labwc must exit after its client.

The runner fails on unhandled syscalls, user-process page faults, and wlroots/labwc `[ERROR]` lines other than the missing EDID and libinput's lateness warnings. Set `LABWC_SHOTS=<dir>` to keep the screendumps.

Limitations / Simplifications
- **Software only.** The pixman renderer composites into DRM dumb buffers, and the kernel copies each frame to the VESA framebuffer. There is no GL (no Mesa), so GLES2-only clients and effects are out, and there is no hardware cursor (wlroots draws it).
- **Legacy KMS, one fixed mode, no EDID.** wlroots logs `Failed to parse EDID`, and outputs cannot change mode.
- **No seat management.** The noop seat means one session, root only, and no VT switching. Devices are opened directly, and nothing revokes them.
- **No hotplug.** `/sys` is static and the netlink uevent socket stays silent. Input devices are the fixed PS/2 keyboard and mouse and the USB tablet.
- **The USB tablet** (`event3`) is exposed to libinput, but the kernel's UHCI driver does not enumerate QEMU's `usb-tablet` in this configuration, so the smoke uses the PS/2 mouse.
- **No Xwayland.** Only Wayland clients run.
- **No D-Bus integration** in the smoke. labwc updates the D-Bus activation environment when `DBUS_SESSION_BUS_ADDRESS` is set (`docs/dbus.md`).
- **libinput timing.** Under TCG emulation libinput warns that "your system is too slow": events are processed tens of milliseconds late, and the 10 ms PIT tick limits timer precision.
- No X11 Compose tables are installed, so foot logs that dead keys will not work.
