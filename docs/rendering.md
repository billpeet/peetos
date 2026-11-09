# Software Rendering Stack

PeetOS userspace has a CPU rendering stack: fontconfig finds fonts, freetype rasterizes them, harfbuzz shapes text, and pixman composites the result. Together with DRM/KMS (`docs/drm.md`), that is enough to draw anti-aliased text and images on screen. It is the base the roadmap's next steps build on (fcft and foot, then cairo/pango and toolkits; see `docs/desktop_roadmap.md`).

**Where It Lives**
- `wayland/fetch-wayland.sh` pins and fetches the sources (SHA-256): pixman 0.43.4, expat 2.6.3, freetype 2.13.3, fontconfig 2.15.0, harfbuzz 10.1.0, utf8proc 2.9.0 and DejaVu fonts 2.37.
- `scripts/build_render_stack.sh` builds them. `make -C wayland` runs it after libwayland, libxkbcommon and libdrm.
- `wayland/out/stage/` is the staged target sysroot (`/usr`, `/etc`) that meson projects build against.
- `user/text_smoke.c`, `scripts/run_toybox_text_smoke.sh` and `tests/musl_ext2_text_smoke.sh` are the QEMU smoke (`make run-text-smoke`).

**Build Prerequisites**
On top of the musl toolchain (with `make -C musl shared`) and the C++ cross toolchain (`make -C toolchain`, which `make -C wayland` builds first; see `docs/cxx.md`), the build machine needs meson, ninja and gperf. If they are not installed system-wide, a private virtualenv works, and the build script picks it up automatically from `~/.peetos-venv/bin`:

```bash
python3 -m venv ~/.peetos-venv
~/.peetos-venv/bin/pip install meson==1.5.2 ninja
# gperf (fontconfig needs it at build time)
curl -LO https://ftp.gnu.org/gnu/gperf/gperf-3.1.tar.gz && tar xf gperf-3.1.tar.gz
(cd gperf-3.1 && ./configure --prefix=$HOME/.peetos-venv && make && make install)
```

**How It Is Built**
- **Staged sysroot.** Everything installs with prefix `/usr`, sysconfdir `/etc` and localstatedir `/var` into `wayland/out/stage` (`DESTDIR`). Paths fontconfig bakes in (`/etc/fonts/fonts.conf`, `/usr/share/fonts`, `/var/cache/fontconfig`) are therefore correct on PeetOS.
- **Existing libraries.** The ones built earlier (libwayland, libffi, libxkbcommon, libdrm) are copied into the stage with generated pkg-config files, along with the full wayland-protocols tree. `wayland-protocols.pc` uses `${pc_sysrootdir}` like upstream's.
- **Cross file.** A generated meson cross file (`wayland/out/peetos-cross.ini`) uses `musl-gcc` for C and `toolchain/out/bin/x86_64-linux-musl-g++` for C++, and points pkg-config at the stage (`sys_root`, `pkg_config_libdir`). It also links against musl's shared `libc.so` (`-L musl/shared/lib`), so every library and tool is a dynamic musl object using `/lib/ld-musl-x86_64.so.1`.
- **Libraries.**
  - expat uses its release tarball's `configure`.
  - freetype, fontconfig and pixman use meson.
  - freetype is built without zlib, bzip2, brotli, libpng or harfbuzz.
  - harfbuzz is C++, compiled with the cross g++. It uses its built-in Unicode tables and the freetype font functions, without glib, ICU, cairo, introspection or the command-line utilities (they need glib). Like upstream's default build, it uses no exceptions, RTTI or C++ standard library, so `libharfbuzz.so.0` needs only libfreetype and libc. `libharfbuzz-subset.so.0` is built too.
  - utf8proc is one C file; the script compiles it straight into `libutf8proc.so.3` and writes its pkg-config file, rather than using upstream's CMake or Makefile.
  - The C++ runtime (`libstdc++.so.6`, `libgcc_s.so.1`) is copied into the stage from the toolchain.
  - fontconfig is built without NLS, docs or tests, and with tools (`fc-list`, `fc-match`, `fc-cache`, ...). Its cache isn't pre-built.
  - pixman uses its default x86_64 SIMD paths (SSE2/SSSE3), with no tests, demos, GTK or libpng.
- **Ext2 image.** The image builder copies every staged shared library into `/usr/lib` under its soname. It also adds `/etc/fonts` (with the `conf.d` symlinks resolved to plain files), `/usr/share/fontconfig`, `/usr/share/fonts`, an empty `/var/cache/fontconfig`, and `fc-list`/`fc-match`/`fc-cache` in `/bin`.

**foot**
foot 1.18.1 runs under `peetwl`. That is the roadmap's first milestone: a real terminal emulator with fonts and a shell.
- `scripts/build_foot.sh` (run by `make -C wayland` after the rendering stack) builds tllist 1.1.0, fcft 3.1.8 and foot with meson against the stage:
  - fcft with harfbuzz and utf8proc (grapheme and run shaping), without SVG glyphs;
  - foot with utf8proc grapheme clustering, without terminfo, docs, themes or utmp.
- foot looks `wayland-scanner` up as a native dependency. The script writes a build-machine `wayland-scanner.pc` pointing at the host scanner and passes it with `-Dbuild.pkg_config_path`.
- libwayland-cursor (with its built-in fallback cursors) is now built by `scripts/build_wayland.sh`. `<linux/input-event-codes.h>` is vendored in `wayland/peetos/uapi/linux/` (a Linux uapi header under the syscall-note exception), and the whole uapi shim directory is installed with libdrm.
- The ext2 image ships `/bin/foot` and `/etc/xdg/foot/foot.ini`.
- `make run-foot-smoke` (part of `make test`) boots the `compositor-first musl-ext2-foot-smoke` profile. It runs `peetwl --smoke --client "foot -e toybox sh foot_child.sh"`. `scripts/foot_smoke_driver.py` then:
  - waits for foot's window;
  - checks the screendump shows `peetwl`'s focused title bar and terminal text;
  - types `hello` with real PS/2 key presses;
  - waits for the shell inside foot to report the line on the serial console.

  The runner also requires DRM page flips. Screenshots go to `$FOOT_SHOTS` when set.
- `make run-foot-demo` is the interactive version. It boots the `compositor-first musl-ext2-foot-demo usb-tablet` profile in a QEMU window (1 GiB, 2 CPUs; override with `FOOT_DEMO_MEM` and `QEMU_SMP`). `peetwl` (not in smoke mode) runs one foot window holding `toybox sh -i`. Click the QEMU window to give it the keyboard, then type into foot; drag the title bar to move the window. `exit` or ^D closes foot and leaves an empty desktop. The profile is left out of `make run`'s feature set so `make run` is unchanged.

**Kernel work foot needed**
- **`epoll_pwait` and `ppoll` honour their signal mask.** It is installed for the wait and restored on syscall exit, or saved in the signal frame when a signal is delivered under it, like Linux's `TIF_RESTORE_SIGMASK`. foot's event loop depends on this (it unblocks SIGCHLD only while waiting). Before, `epoll_pwait` with a mask returned `ENOSYS` and `ppoll` ignored it.
- **pty output processing.** `OPOST | ONLCR` turns `\n` written to a pty slave into `\r\n`, so terminal output starts each line at column 0.
- **`prctl(2)`.** `PR_SET_NAME`/`PR_GET_NAME` work, and the death-signal, dumpable, subreaper and no-new-privs flags are accepted without effect.
- **`membarrier(2)`.** Only `MEMBARRIER_CMD_QUERY` is supported, reporting no commands, so musl uses its fallback.
- **Larger sparse limits.** memfd/shmem objects can be up to 1 GiB (sparse) and the per-process mapping budget is 4 GiB of address space. foot sizes its pool at 512 MiB and maps all of it, and the compositor maps it again.

**Kernel work the rendering stack needed**
- **8 MiB main-thread stack.** User stacks were a fixed 32 KiB, and fontconfig and freetype overflowed them. The stack is now a lazily populated private anonymous VM region, `[USER_STACK_TOP - 8 MiB, USER_STACK_TOP)`, registered at exec and at the initial `init` spawn. Only the top 32 KiB (argv, envp, auxv) is mapped eagerly.
- **`chmod(2)`** (legacy syscall 90, as `fchmodat(AT_FDCWD, ...)`). fontconfig uses it on its cache directory.
- **`fadvise64(2)`** hints are validated and ignored.

**Testing**
`make run-text-smoke` (part of `make test`) lists fonts with `fc-list` and `fc-match`, then runs `text_smoke`:
- fontconfig resolves `monospace` to DejaVu Sans Mono and `sans-serif` to DejaVu Sans.
- freetype opens the face and rasterizes an anti-aliased glyph.
- harfbuzz, on a font made from the DejaVu Sans freetype face:
  - applies GPOS kerning: `A` advances less before `V`;
  - composes `e` + U+0301 into the same single glyph as U+00E9;
  - picks Arabic joining forms: two seens shape to two glyphs different from the isolated form and from each other.
- A harfbuzz-shaped line (kerned "AVATAR", a combining accent, and Arabic drawn right to left) is drawn on screen.
- pixman composites glyph masks and a bilinear 3x upscale into a DRM dumb buffer.
- The frame is shown with `SETCRTC` and read back from `/dev/fb0`.

The runner saves a QEMU screendump of the frame to `build/text_smoke.png`.

Limitations / Simplifications
- foot runs without terminfo. Toybox's shell has no line editing or job control, and there is no clipboard transfer through `peetwl`. `fallocate(PUNCH_HOLE)` is not supported, so foot disables its scrolling-by-offset optimization and redraws instead.
- The interactive demo has a single foot window, which cannot be reopened after the shell exits (there is no launcher), and `make test` does not cover it; its init payload and scripts mirror the smoke's.
- harfbuzz has no glib, ICU or graphite2 integration and no `hb-view`/`hb-shape` utilities. The shaping smoke checks Latin kerning, mark composition and Arabic joining with DejaVu Sans only; other scripts depend on font coverage, and only two fonts are staged. libpng, cairo and pango are not ported yet.
- Building the stack now needs the C++ cross toolchain (about 4 minutes on top of the rest).
- Only two fonts (DejaVu Sans and DejaVu Sans Mono) are staged. There is no fontconfig cache built ahead of time: the first `FcInit` in each process scans the font directory, which is fast with two fonts but grows with more.
- The stage seeds pkg-config files by hand for the libraries built outside meson. Their versions and flags are hard-coded in `scripts/build_render_stack.sh`.
- `wayland-scanner` is a build-machine tool. Meson projects that look it up as a native dependency (foot, wlroots) will need a native file or `PKG_CONFIG_PATH` entry for it, which is not set up yet.
- The build tools (meson, ninja, gperf) are not part of the repo and must be installed on the build machine (see above).
- The lazily populated stack has no guard gap beyond the 8 MiB reserve, and its size is fixed (there is no `setrlimit(RLIMIT_STACK)`). Threads use musl's mmapped stacks.
