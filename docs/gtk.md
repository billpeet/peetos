# GTK 3

GTK 3.24 runs on PeetOS through its Wayland backend. Windows are drawn with cairo and pango (`docs/rendering.md`) into `wl_shm` buffers and composited by `peetwl` (`docs/wayland.md`), so no GL driver is needed.

**Where It Lives**
- `wayland/fetch-wayland.sh` pins the sources (SHA-256): libepoxy 1.5.10, ATK 2.38.0, gdk-pixbuf 2.42.12, GTK 3.24.43, hicolor-icon-theme 0.18, and the Khronos EGL headers from EGL-Registry at a fixed commit (it has no releases).
- `scripts/build_gtk.sh` builds them into the staged sysroot (`wayland/out/stage`). `make -C wayland` runs it after `scripts/build_cairo_pango.sh`.
- `user/gtk_smoke.c`, `scripts/gtk_smoke_driver.py`, `scripts/run_toybox_gtk_smoke.sh` and `tests/musl_ext2_gtk_smoke.sh` are the QEMU smoke (`make run-gtk-smoke`).

**Build Prerequisites**
On top of the rendering stack's prerequisites, GTK's build runs GLib's resource and schema compilers on the build machine: `glib-compile-resources` and `glib-compile-schemas` (Ubuntu: `libglib2.0-dev-bin`). Their output formats are stable, so the host version doesn't have to match the target GLib.

**How It Is Built**
- **libwayland-egl** is one C file in libwayland's tree. The script compiles it directly into `libwayland-egl.so.1` and writes its `wayland-egl.pc`. GTK's Wayland backend links it even when no GL is used.
- **EGL headers.** libepoxy's EGL dispatch and GTK's Wayland GL code include `<EGL/egl.h>`. The Khronos headers are installed with `EGL_NO_X11` predefined in `eglplatform.h` (PeetOS has no X11), along with a headers-only `egl.pc`. There is no libEGL: libepoxy loads one at run time only if an application asks for GL.
- **libepoxy** is built with EGL and without GLX/X11. **ATK** has no introspection.
- **gdk-pixbuf** has its PNG and GIF loaders built in (`builtin_loaders=all`, JPEG/TIFF off). There is then no `loaders.cache` to generate at build time and no loader modules at run time. GIO MIME sniffing is off (no shared-mime-info).
- **GTK** is Wayland-only (no X11 or Broadway backends), with the file print backend only, built-in input methods, and no colord, introspection, demos or tests.
- **Tools that run during the build:**
  - The meson gnome module and GTK find `glib-compile-resources`/`glib-compile-schemas` (host), GLib's Python generators (`glib-mkenums`, `glib-genmarshal`, staged copies, same version as the target library), and `wayland-scanner` (host build) through a build-machine pkg-config directory.
  - GTK's resource bundle converts its icons with `gdk-pixbuf-pixdata`. The script runs the staged PeetOS build of it on the build machine through musl's dynamic loader, which also works as a command: `libc.so --library-path <stage>/usr/lib:<musl> gdk-pixbuf-pixdata ...`. It is the same version, and no host gdk-pixbuf is needed.
- **Cross file.** The meson cross file adds the stage's `usr/include` as a system include directory (`-isystem`), as a sysroot-configured compiler would, so configure checks like GTK's `has_header('linux/input.h')` see the staged headers.
- **uapi headers.** `<linux/input.h>` is vendored from the Linux uapi headers (under the syscall-note exception) next to `input-event-codes.h` in `wayland/peetos/uapi`. The `<linux/types.h>` shim gained `__BITS_PER_LONG` and `__kernel_[u]long_t` for it.
- **Data.** After installing, the GSettings schemas are compiled with the host `glib-compile-schemas`. GTK aborts if a schema it uses (file chooser, color chooser) is missing. The image builder stages `usr/share/glib-2.0/schemas` and the hicolor icon theme index with the libraries.

**Kernel work GTK needed**
- **`getresuid(2)`/`getresgid(2)`** (118/120), which GLib uses to decide whether it is running setuid. All three ids are the process's one id: there is no separate saved/effective id model.

**Testing**
`make run-gtk-smoke` (part of `make test`) boots the `compositor-first musl-ext2-gtk-smoke` profile. `peetwl --smoke` runs `bin/gtk_smoke`: an undecorated GTK window holding a pango markup label, a cairo drawing area, a `GtkEntry`, a progress bar, a check button, a combo box and a button. `scripts/gtk_smoke_driver.py`:
- checks GTK chose the Wayland backend and the window mapped under `peetwl`;
- clicks the entry with a real pointer click and checks the screendump for `peetwl`'s focused title bar around a drawn GTK window;
- types `hello` with real PS/2 key presses; GTK must report each change, ending with `hello`;
- clicks the button, which must report the click with the entry text and exit;
- requires `peetwl` to see its only client exit cleanly.

The runner also fails on unhandled syscalls, on `Gtk`/`Gdk`/`GLib` `CRITICAL` or `ERROR` messages, and if nothing was presented through DRM page flips. Set `GTK_SHOTS=<dir>` to keep the screendumps.

Limitations / Simplifications
- GTK 3 only. GTK 4 effectively requires EGL/GL (Mesa), which is not ported yet.
- **Decorations.** GTK 3 only does server-side decorations through the KDE decoration protocol, which `peetwl` doesn't offer (it has `xdg-decoration`). A decorated GTK window would draw its own client-side title bar inside `peetwl`'s frame, so the smoke's window is undecorated. Client-side decorations, resizing from GTK's edges, and shadows are untested.
- **No SVG loader** (librsvg, which is written in Rust, is not ported). Adwaita theme assets that are SVG-only log `Could not load a pixbuf from .../*.svg` and fall back. Symbolic icons come from GTK's built-in PNGs. No icon theme beyond the hicolor index is installed.
- **D-Bus.** A session bus is available (`docs/dbus.md`), but accessibility (at-spi), portals and the settings portal are not ported, so GTK uses its built-in defaults. `GtkApplication` uniqueness works when a session bus is running.
- No input-method framework beyond GTK's built-in input methods, no primary selection, and no drag and drop between applications.
- The file chooser and printing are built but untested. Only the file print backend exists.
- gdk-pixbuf has no JPEG or TIFF loaders.
- Only regular DejaVu faces are installed, so bold and italic text is synthesised.
