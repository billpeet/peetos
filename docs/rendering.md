# Software Rendering Stack

PeetOS userspace has a CPU rendering stack: fontconfig finds fonts, freetype rasterizes them, and pixman composites the result. Together with DRM/KMS (`docs/drm.md`), that is enough to draw anti-aliased text and images on screen. It is the base the roadmap's next steps build on (fcft and foot, then cairo/pango and toolkits; see `docs/desktop_roadmap.md`).

**Where It Lives**
- `wayland/fetch-wayland.sh` pins and fetches the sources (SHA-256): pixman 0.43.4, expat 2.6.3, freetype 2.13.3, fontconfig 2.15.0 and DejaVu fonts 2.37.
- `scripts/build_render_stack.sh` builds them. `make -C wayland` runs it after libwayland, libxkbcommon and libdrm.
- `wayland/out/stage/` is the staged target sysroot (`/usr`, `/etc`) that meson projects build against.
- `user/text_smoke.c`, `scripts/run_toybox_text_smoke.sh` and `tests/musl_ext2_text_smoke.sh` are the QEMU smoke (`make run-text-smoke`).

**Build Prerequisites**
On top of the musl toolchain (with `make -C musl shared`), the build machine needs meson, ninja and gperf. If they are not installed system-wide, a private virtualenv works, and the build script picks it up automatically from `~/.peetos-venv/bin`:

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
- **Cross file.** A generated meson cross file (`wayland/out/peetos-cross.ini`) uses `musl-gcc` and points pkg-config at the stage (`sys_root`, `pkg_config_libdir`). It also links against musl's shared `libc.so` (`-L musl/shared/lib`), so every library and tool is a dynamic musl object using `/lib/ld-musl-x86_64.so.1`.
- **Libraries.**
  - expat uses its release tarball's `configure`.
  - freetype, fontconfig and pixman use meson.
  - freetype is built without zlib, bzip2, brotli, libpng or harfbuzz.
  - fontconfig is built without NLS, docs or tests, and with tools (`fc-list`, `fc-match`, `fc-cache`, ...). Its cache isn't pre-built.
  - pixman uses its default x86_64 SIMD paths (SSE2/SSSE3), with no tests, demos, GTK or libpng.
- **Ext2 image.** The image builder copies every staged shared library into `/usr/lib` under its soname. It also adds `/etc/fonts` (with the `conf.d` symlinks resolved to plain files), `/usr/share/fontconfig`, `/usr/share/fonts`, an empty `/var/cache/fontconfig`, and `fc-list`/`fc-match`/`fc-cache` in `/bin`.

**Kernel work it needed**
- **8 MiB main-thread stack.** User stacks were a fixed 32 KiB, and fontconfig and freetype overflowed them. The stack is now a lazily populated private anonymous VM region, `[USER_STACK_TOP - 8 MiB, USER_STACK_TOP)`, registered at exec and at the initial `init` spawn. Only the top 32 KiB (argv, envp, auxv) is mapped eagerly.
- **`chmod(2)`** (legacy syscall 90, as `fchmodat(AT_FDCWD, ...)`). fontconfig uses it on its cache directory.
- **`fadvise64(2)`** hints are validated and ignored.

**Testing**
`make run-text-smoke` (part of `make test`) lists fonts with `fc-list` and `fc-match`, then runs `text_smoke`:
- fontconfig resolves `monospace` to DejaVu Sans Mono and `sans-serif` to DejaVu Sans.
- freetype opens the face and rasterizes an anti-aliased glyph.
- pixman composites glyph masks and a bilinear 3x upscale into a DRM dumb buffer.
- The frame is shown with `SETCRTC` and read back from `/dev/fb0`.

The runner saves a QEMU screendump of the frame to `build/text_smoke.png`.

Limitations / Simplifications
- No harfbuzz yet. It needs a C++ runtime, which musl does not provide, so there is no complex-script shaping, and fcft/foot will build without it at first. libpng, cairo and pango are not ported yet.
- Only two fonts (DejaVu Sans and DejaVu Sans Mono) are staged. There is no fontconfig cache built ahead of time: the first `FcInit` in each process scans the font directory, which is fast with two fonts but grows with more.
- The stage seeds pkg-config files by hand for the libraries built outside meson. Their versions and flags are hard-coded in `scripts/build_render_stack.sh`.
- `wayland-scanner` is a build-machine tool. Meson projects that look it up as a native dependency (foot, wlroots) will need a native file or `PKG_CONFIG_PATH` entry for it, which is not set up yet.
- The build tools (meson, ninja, gperf) are not part of the repo and must be installed on the build machine (see above).
- The lazily populated stack has no guard gap beyond the 8 MiB reserve, and its size is fixed (there is no `setrlimit(RLIMIT_STACK)`). Threads use musl's mmapped stacks.
