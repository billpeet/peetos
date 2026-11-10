# Desktop Session

PeetOS boots into a small desktop: `peet-session` starts a D-Bus session bus and the labwc compositor together, and labwc brings up a panel (sfwbar) with an application menu, launchers, a taskbar and a clock. Alt+F2 opens an application launcher (fuzzel). The applications are the foot terminal and the Thunar file manager.

```sh
make run-desktop        # interactive, in a QEMU window
make run-desktop-smoke  # automated (part of `make test`)
```

**Where It Lives**
- `user/desktop/` holds the session files, which `scripts/build_ext2_musl_execve.sh` stages into the image:
  - `peet-session` → `/usr/bin/peet-session`
  - `labwc/{rc.xml,menu.xml,autostart}` → `/etc/xdg/labwc/`
  - `sfwbar/sfwbar.config` → `/usr/share/sfwbar/sfwbar.config`
  - `fuzzel/fuzzel.ini` → `/etc/xdg/fuzzel/fuzzel.ini`
- `wayland/fetch-wayland.sh` pins the sources (SHA-256): json-c 0.18, gtk-layer-shell 0.9.2, sfwbar 1.0_beta16.1 and fuzzel 1.11.1.
- `scripts/build_desktop.sh` builds them into the staged sysroot (`wayland/out/stage`). `make -C wayland` runs it last, after `scripts/build_foot.sh`.
- `scripts/build_ext2_musl_execve.sh` also stages `sfwbar` and `fuzzel` in `/usr/bin`, sfwbar's library, modules and widget files, `/usr/bin/labwc`, GLib's `gio-launch-desktop`, and the desktop entries of foot and Thunar in `/usr/share/applications`.
- `user/musl_execve_ext2_desktop.c` (profile `musl-ext2-desktop`) and `scripts/run_toybox_desktop.sh` are the interactive session.
- `user/musl_execve_ext2_desktop_smoke.c`, `scripts/desktop_smoke_driver.py`, `scripts/run_toybox_desktop_smoke.sh` and `tests/musl_ext2_desktop_smoke.sh` are the QEMU smoke.

**The Session**

`peet-session` is a shell script with a `#!/bin/sh` line. It:
- sets the session environment: `HOME` (default `/root`), `XDG_RUNTIME_DIR`, `XDG_CONFIG_DIRS`, `XDG_DATA_DIRS`, `XDG_SESSION_TYPE=wayland`, the wlroots settings from `docs/labwc.md` (`LIBSEAT_BACKEND=noop`, `WLR_RENDERER=pixman`, `WLR_DRM_NO_ATOMIC=1`, `WLR_BACKENDS=drm,libinput`), and `GDK_BACKEND=wayland`;
- creates `/etc/machine-id` once (`dbus-uuidgen --ensure`);
- runs `dbus-run-session -- labwc`. `dbus-run-session` starts `dbus-daemon`, exports `DBUS_SESSION_BUS_ADDRESS` to labwc and stops the daemon when labwc exits. Arguments to `peet-session` go to labwc.

labwc then reads `/etc/xdg/labwc`:
- `autostart` starts `sfwbar` (its output goes to `/tmp/sfwbar.log`) and then runs `~/.config/peet-session/autostart` if the user has one.
- `rc.xml` adds key bindings to labwc's defaults and turns pointer acceleration off (QEMU's PS/2 mouse already has the host's):

  | Keys | Action |
  | --- | --- |
  | Alt+F2, Alt+F3, Super+Space | launcher (fuzzel) |
  | Super+Enter, Ctrl+Alt+T | terminal (foot) |
  | Super+E | file manager (Thunar) |
  | Alt+Tab, Alt+F4, ... | labwc's defaults |

- `menu.xml` is the menu on a desktop click (Terminal, File Manager, Run..., Reconfigure, Log Out) and the window menu.

To end the session, choose "Log Out" in the desktop menu. In `make run-desktop` the init process then exits.

**The Panel (sfwbar)**

sfwbar is a GTK 3 panel on a wlr-layer-shell surface (through gtk-layer-shell). `user/desktop/sfwbar/sfwbar.config` puts a bar along the bottom edge with:
- an application menu, built by sfwbar's `appmenu` module from `/usr/share/applications` and grouped by category;
- launcher buttons for the terminal, the file manager and the launcher;
- a taskbar with a button per window (wlr-foreign-toplevel-management). A click focuses the window, and a right click opens a menu to close, minimize or maximize it;
- a tray. sfwbar owns `org.kde.StatusNotifierWatcher` on the session bus, so it shows up there even with nothing in the tray;
- a clock with the wall-clock time (UTC, from the RTC).

It is built without the audio, network, Bluetooth, MPD and notification-center modules.

sfwbar looks for its configuration in `~/.config/sfwbar` and then in `XDG_DATA_DIRS`, not in `/etc/xdg`. The session's file therefore replaces the stock `/usr/share/sfwbar/sfwbar.config`.

**The Launcher (fuzzel)**

fuzzel lists the desktop entries in `/usr/share/applications`. Type to filter, Enter starts the selected application, Escape closes it. It renders with fcft and pixman like foot, with cairo for the rounded border, libpng icons and its bundled nanosvg. `fuzzel-1.11.1-no-docs.patch` drops its man pages, which need scdoc on the build machine.

**Build-machine tools**: cmake (for json-c), in addition to what the GTK and wlroots builds need.

**Kernel work the desktop needed**
- **`#!` scripts.** `execve` of a file that starts with `#!interpreter [argument]` runs the interpreter with the optional argument, the script path and then the caller's arguments, as on Linux (`docs/syscall.md`). The session launcher is such a script.
- **A real-time clock.** `CLOCK_REALTIME` and `gettimeofday` are now the CMOS RTC's time at boot plus the uptime (`docs/syscall.md`). The panel clock used to show 1970.
- **Passed file descriptors stay with their message.** The kernel kept `SCM_RIGHTS` descriptors in a queue apart from the byte stream and gave `recvmsg` one batch per call. When labwc read two of a client's sends in one `recvmsg`, the second request's descriptor was missing. labwc then disconnected the client ("file descriptor expected"), which is how Thunar first died under labwc. Descriptors are now tied to stream positions, as on Linux (`docs/ipc.md`). One message may carry 28 descriptors instead of 8, which is libwayland's batch size.

**Other fixes it needed**
- **The icon theme had no index.** `scripts/build_thunar.sh` staged AdwaitaLegacy without its `index.theme`, so GTK did not treat it as a theme and found none of its icons by name (the panel's launcher buttons were empty).
- **`gio-launch-desktop`** was not in the image. GLib starts desktop entries through it (`g_app_info_launch`, which the application menu uses).

**Testing**

`make run-desktop-smoke` (part of `make test`) boots the `compositor-first musl-ext2-desktop-smoke` profile. `bin/desktop_smoke.sh` runs `/usr/bin/peet-session` the way `make run-desktop` does, with an autostart hook (`bin/desktop_watch.sh`) that reports to the console what is on the session bus. `scripts/desktop_smoke_driver.py` then drives QEMU with PS/2 key presses, mouse moves and clicks and checks screendumps:
- the autostart hook must run with `WAYLAND_DISPLAY`, `DBUS_SESSION_BUS_ADDRESS` and `LABWC_PID` set, and the panel must appear on the session bus;
- the panel must be a light bar along the bottom edge of the black desktop, with an empty taskbar;
- a click on the panel's terminal button must open a decorated foot window and add a taskbar button. Typing `desktopok` must reach the shell in foot;
- a click on the application menu button, its "System Tools" category and "Thunar File Manager" must start Thunar (its name appears on the session bus) and add a second taskbar button;
- Alt+F2 must show the launcher in the middle of the screen. Typing `foot` and Enter must start a second terminal, which gets a third taskbar button and the keyboard focus;
- a click on the first taskbar button must give the first terminal the keyboard focus back. Typing `desktopbye` there stops labwc;
- the session must exit, and nothing may answer on the session bus's address any more.

The scripts typed into foot report the pid of the shell that ran them, which is how the driver knows which terminal had the focus. The driver checks where the pointer ended up after each move (it nudges the pointer and compares two screendumps) and corrects it, because relative PS/2 motion can get lost while the guest is busy.

The runner fails on unhandled syscalls, user-process page faults, Wayland clients that labwc disconnects, and wlroots/labwc `[ERROR]` lines other than the missing EDID and libinput's lateness warnings. Set `DESKTOP_SHOTS=<dir>` to keep the screendumps.

The proc smoke (`make run-proc-smoke`) covers `#!` scripts, the real-time clock with absolute timerfd and futex timeouts, and descriptor passing across several messages. Host unit tests cover the `#!` parser, the RTC decoding and the descriptor queue.

Limitations / Simplifications
- **No login.** The session runs as root, and init starts it directly. There is no display manager, no screen locker and no session saving.
- **No wallpaper.** The desktop is black. labwc does not draw a background, and no background client (swaybg) is ported.
- **Two applications.** Only foot and Thunar have desktop entries, so the application menu has one category.
- **Menus take no keyboard input.** The panel never has the keyboard focus, so its menus are mouse-only, and Escape does not close them.
- **No notifications.** sfwbar's notification center is not built, because its widget is drawn from SVG and there is no SVG loader. For the same reason sfwbar's own SVG icons are not staged, and icons come from the AdwaitaLegacy PNG theme.
- **The tray is empty.** No staged application registers a status-notifier item.
- **The clock is UTC.** There is no time zone data.
- **toybox sh quirk.** A script run without arguments sees its own name as `$1`. `peet-session` works around it.
- **`#!` interpreters must be ELF programs**: an interpreter that is itself a script is `ENOEXEC` (Linux allows four levels).
- **Slow under emulation.** Everything is software-rendered (`docs/labwc.md`), and GTK applications take several seconds to start under QEMU TCG.
