# Thunar (file manager)

Thunar 4.20, the Xfce file manager, runs on PeetOS as a GTK 3 Wayland client under `peetwl`. It is the first full GTK application in the image. It lists folders with MIME-typed icons, navigates with the keyboard, and creates folders through its own dialogs on the ext2 root.

**Where It Lives**
- `wayland/fetch-wayland.sh` pins the sources (SHA-256): libxfce4util, xfconf, libxfce4ui, exo and Thunar 4.20.0, adwaita-icon-theme-legacy 46.2 and shared-mime-info 2.4.
- `scripts/build_thunar.sh` builds them into the staged sysroot (`wayland/out/stage`). `make -C wayland` runs it after `scripts/build_gtk.sh`.
- `wayland/peetos/patches/thunar-4.20.0-wayland-only.patch` guards Thunar's one unconditional `<gdk/gdkx.h>` include.
- `scripts/build_ext2_musl_execve.sh` stages `bin/thunar`, `/usr/lib/thunarx-3`, the MIME database, the icon theme, `/etc/gtk-3.0/settings.ini` and Thunar's `/etc/xdg` defaults when the stage has Thunar.
- `user/musl_execve_ext2_thunar_smoke.c`, `scripts/thunar_smoke_driver.py`, `scripts/run_toybox_thunar_smoke.sh` and `tests/musl_ext2_thunar_smoke.sh` are the QEMU smoke (`make run-thunar-smoke`).

**Build Prerequisites**
On top of GTK's prerequisites (`docs/gtk.md`), the build machine needs:
- `msgfmt` (gettext). Xfce merges its `.desktop` files with it even with NLS off.
- `gdbus-codegen` (`libglib2.0-dev-bin`).
- `update-mime-database` (`shared-mime-info`).
- `patch`.

**How It Is Built**
- **Autotools.** The Xfce 4.20 release tarballs are autotools dists, and their meson files are incomplete, so every package uses `configure --host=x86_64-linux-musl` against the stage (`-isystem` stage headers, stage pkg-config). Patches named `wayland/peetos/patches/<package>-*.patch` are applied to a fresh copy of the source.
- **Wayland only.** libxfce4ui is built `--disable-x11 --enable-wayland` without libSM, startup notification or glade. Thunar's configure insists on libX11. Passing `--x-includes`/`--x-libraries` makes `AC_PATH_X` accept them without probing. The libX11 link test then fails, `HAVE_LIBX11` stays undefined, and no X11 code is built. gudev, libnotify, libexif, pcre2, the wallpaper plugin and introspection are off.
- **xfconf** is built with its daemon. `xfconfd` is started by the D-Bus session bus on first use (`docs/dbus.md`); without a bus Thunar falls back to built-in defaults.
- **gdbus-codegen version skew.** The build machine's `gdbus-codegen` is newer than the target GLib (2.82):
  - It runs through a wrapper that passes `--glib-max-allowed 2.82`. `--glib-min-required` is not passed, because it changes the generated function signatures.
  - The generated code tests `GLIB_VERSION_MAX_ALLOWED >= GLIB_VERSION_2_84`. GLib 2.82 doesn't define `GLIB_VERSION_2_84`, so the test evaluates to 0 and picks the newer API. A force-included header defines the 2.84 to 2.88 version macros so the guards work.
- **Data.**
  - The MIME database is compiled from shared-mime-info's `freedesktop.org.xml` with the host `update-mime-database`. Its output format is stable.
  - The icon theme is AdwaitaLegacy, because it ships PNGs and PeetOS has no SVG loader (`docs/gtk.md`).
  - `/etc/gtk-3.0/settings.ini` selects that icon theme and DejaVu Sans.

**Kernel work Thunar needed**
- **Block-cache coherence (data loss fix).**
  - The ext2 backend writes blocks through the kernel's write-back block cache but reads them with the uncached `block::read`.
  - When ext2's own cache evicted a dirty metadata block and later reloaded it, the read went to the device and returned the old contents.
  - As a result, after a few MiB of reads (Thunar loading icons), inodes created at run time reverted to mode 0 and their numbers were handed out again.
  - Raw reads and writes, and cached I/O of a different size, now write back and drop overlapping dirty lines (`docs/drivers.md`). The fix has block-layer unit tests, and the smoke checks that new files survive several MiB of reads.
- **`newfstatat` with `AT_SYMLINK_NOFOLLOW`** used to fail with `-EINVAL`, so `ls -l` printed `?` for every field and GLib couldn't query files. It now accepts the flags Linux allows (`docs/syscall.md`).
- **`execve` string limits.** Each `argv`/`envp` string was capped at 256 bytes. GLib's spawn of a long command line failed with `E2BIG`. Strings now share one 28 KiB buffer.
- The smoke runs Thunar on a session bus (`docs/dbus.md`), which needed the D-Bus kernel work listed there.
- **inotify** syscalls are registered and return `-ENOSYS`. GLib then reports directory monitoring as unsupported without tripping the unknown-syscall log.
- `peetwl` gained `--watchdog-ms N` (Thunar's first start, with icon and MIME loading, takes longer than the default 90 s smoke watchdog allows under TCG) and logs title changes, which the driver uses to follow navigation.

**Testing**
`make run-thunar-smoke` (part of `make test`) boots the `compositor-first musl-ext2-thunar-smoke` profile.
- `bin/thunar_smoke.sh` creates `/tmp/thunar-demo` (folders, a text file, a shell script and PNGs) and starts a session bus (`dbus-daemon --session --fork`) whose address the clients inherit.
- It checks that freshly created files survive reading the whole icon theme (the cache regression).
- It then runs `peetwl --smoke` with Thunar on the demo folder, plus a watcher script.

`scripts/thunar_smoke_driver.py` then:
- requires the window title `thunar-demo - Thunar`, `peetwl`'s focused title bar, and a window full of drawn icons and text;
- presses Home and Enter, so Thunar opens the first item (`Documents`; folders sort first) and the title changes to `Documents - Thunar`;
- presses Ctrl+Shift+N, types `made-by-thunar` into the New Folder dialog and presses Enter. The watcher must find `/tmp/thunar-demo/Documents/made-by-thunar` on disk;
- presses Ctrl+Q and requires every client to exit cleanly.

The watcher also checks that Thunar owns `org.xfce.FileManager` on the bus. After the session the payload stops xfconfd and requires Thunar's `last-view` setting in `~/.config/xfce4/xfconf/xfce-perchannel-xml/thunar.xml`. The runner fails if Thunar logs `Failed to initialize Xfconf`.

The runner also fails on unhandled syscalls, on `Gtk`/`Gdk`/`GLib` `CRITICAL` or `ERROR` messages, and if nothing was presented through DRM. Set `THUNAR_SHOTS=<dir>` to keep the screendumps, and `THUNAR_SMOKE_MEM` to change the guest RAM (default 768M).

Limitations / Simplifications
- **D-Bus services.** Settings (xfconf) and single-instance handling work on the session bus, but:
  - there is no thumbnailer (tumbler), so Thunar logs `ServiceUnknown` warnings for `org.freedesktop.thumbnails.Thumbnailer1` and retries;
  - there is no trash (gvfs) and no volume management (udisks);
  - without a session bus (no `DBUS_SESSION_BUS_ADDRESS`) Thunar falls back to built-in settings and logs xfconf warnings.
- **No inotify.** Folders don't refresh on changes made outside Thunar. Thunar's own operations refresh their views directly.
- **Decorations.**
  - GTK 3 draws its client-side title bar and shadow inside `peetwl`'s frame, so the window has two title bars and a black margin.
  - `peetwl` implements neither the KDE server-decoration protocol nor `xdg_surface.set_window_geometry` cropping.
- **Root warnings.** Everything runs as root, so Thunar shows its "you are using the root account" bar. `access::can-delete` reports false because there is no permission model behind it.
- No SVG icons (librsvg is not ported) and no custom actions or plugins beyond the built-in ones. Only the C locale is staged, without translations.
- The smoke covers listing, keyboard navigation and folder creation only. Drag and drop, copy and move progress dialogs, renaming, the properties dialog, and opening files with applications are untested.
