# libwayland Port

This document covers the userspace libwayland port: how it is built, which
kernel interfaces it relies on, and how it is tested. Roadmap context lives in
`docs/gui_wayland_plan.md` (Phase 5).

**Where It Lives**
- `wayland/Makefile`, `wayland/fetch-wayland.sh`: fetch and build glue
- `wayland/peetos/config.h`: replacement for libwayland's meson-generated `config.h`
- `scripts/build_wayland.sh`: builds the host scanner, libffi, and libwayland
- `user/wayland_smoke.c`: libwayland server + client end-to-end smoke binary
- `user/musl_execve_ext2_wayland_smoke.c`: init launcher for the smoke profile
- `scripts/run_toybox_wayland_smoke.sh`, `tests/musl_ext2_wayland_smoke.sh`: QEMU runner and harness entry

**Versions**
- libwayland `1.23.1` (core library and `wayland.xml` protocol)
- libffi `3.4.6` (used by libwayland to dispatch requests/events to listener functions)
- wayland-protocols `1.38` (currently only `stable/xdg-shell` is built)
- libxkbcommon `1.7.0` (keymap compiler and keyboard state machine)
- libdrm `2.4.123` (core `xf86drm`/`xf86drmMode` only, built by `scripts/build_libdrm.sh`; see `docs/drm.md`)
- Release archives are pinned by SHA-256 in `wayland/fetch-wayland.sh`.

**Build**
```bash
make -C wayland          # fetches sources on first use, then builds
make -C wayland info     # show what was installed
```
Outputs land in `wayland/out/`:
- `bin/wayland-scanner`: host tool (runs on the build machine)
- `include/`: `wayland-{util,version,client,client-core,server,server-core}.h`, generated `wayland-{client,server}-protocol.h`, `ffi.h`
- `lib/libwayland-server.a`, `lib/libwayland-client.a`, `lib/libffi.a`: static, musl, x86_64
- `lib/libwayland-server.so.0`, `lib/libwayland-client.so.0`, `lib/libffi.so.8`, `lib/libxkbcommon.so.0` (plus unversioned `.so` symlinks for the linker): shared builds of the same PIC objects, linked against musl's `libc.so`. They need `make -C musl shared` first, which `make -C wayland` runs if needed. Like upstream, the objects are compiled with `-fvisibility=hidden`, so only `WL_EXPORT`/`XKB_EXPORT` symbols are exported. The ext2 builder stages the soname files in `/usr/lib`.
- `lib/libxkbcommon.a` plus `include/xkbcommon/*.h` (built by `scripts/build_xkbcommon.sh`, which `make -C wayland` runs after `build_wayland.sh`)
- `share/X11/xkb/`: the verified xkeyboard-config subset staged into the ext2 image at `/usr/share/X11/xkb`
- `lib/libwlprotocols.a` plus `include/xdg-shell-{client,server}-protocol.h`: generated code for the wayland-protocols listed in `PROTOCOL_XMLS` in `scripts/build_wayland.sh` (add an XML path there to build another protocol)
- `share/wayland/wayland.xml`, `share/wayland-protocols/*.xml`

The build needs no meson, ninja, or autotools:
- libwayland's core is eight C files compiled directly by `scripts/build_wayland.sh` with `musl-gcc`.
- libffi's release tarball ships a generated `configure`, run as a cross build (`--host=x86_64-linux-musl`) because musl binaries cannot run on the build host.
- `wayland-scanner` is compiled from the same libwayland source with the host compiler and host expat (`libexpat1-dev`), so generated code always matches the library version.

Linking a client or compositor statically:
```bash
musl-gcc -static app.c -Iwayland/out/include \
  wayland/out/lib/libwayland-client.a wayland/out/lib/libffi.a -lm
```
`scripts/build_musl_echo.sh` accepts the extra arguments after `--`, which is how the Makefile builds `user/wayland_smoke.elf` and the upstream tests.

Or dynamically, which is how `peetwl` and `wl_demo` are built (see "Dynamic Linking" in `docs/elf.md`):
```bash
scripts/build_musl_dyn.sh --src app.c --out app.elf -- -Iwayland/out/include \
  -Lwayland/out/lib wayland/out/lib/libwlprotocols.a -lwayland-client -lxkbcommon -lm
```
`libwlprotocols.a` stays static, like upstream wayland-protocols: each program compiles in the protocol interfaces it uses.

**Kernel Interfaces Used**
libwayland 1.23 needs these kernel features, all implemented:
- `AF_UNIX` stream sockets with pathname `bind`/`listen`/`accept4`/`connect`, `sendmsg`/`recvmsg` with `SCM_RIGHTS` and `MSG_CMSG_CLOEXEC`/`MSG_DONTWAIT`
- Listening `AF_UNIX` sockets report `POLLIN` only while connections are queued (previously they always reported `POLLHUP`, which made libwayland block in `accept`)
- `getsockopt(SOL_SOCKET, SO_PEERCRED)`: `wl_client_create` refuses clients without credentials
- `flock(LOCK_EX | LOCK_NB)`: `wl_display_add_socket` locks `$XDG_RUNTIME_DIR/wayland-N.lock`
- `epoll_create1`/`epoll_ctl`/`epoll_wait`: `wl_event_loop`
- `eventfd2`: `wl_display_terminate`
- `timerfd_create`/`timerfd_settime`: event-loop timers (`TFD_TIMER_ABSTIME` on `CLOCK_MONOTONIC`)
- `signalfd4` plus a correct `rt_sigprocmask`: event-loop signal sources
- `memfd_create(MFD_CLOEXEC | MFD_ALLOW_SEALING)`, `fcntl(F_ADD_SEALS / F_GET_SEALS)`: shm pools (client side) and the server's SIGBUS-avoidance check
- `fallocate` (via `posix_fallocate`): shm pool growth
- `mmap(MAP_SHARED)` on memfd fds, and `mremap(MREMAP_MAYMOVE)`: `wl_shm_pool` resize on the server
- `F_DUPFD_CLOEXEC`, `SOCK_CLOEXEC`, `EPOLL_CLOEXEC`, `EFD_CLOEXEC`, `TFD_CLOEXEC`, `SFD_CLOEXEC`

Kernel-side details for each are in `docs/ipc.md`, `docs/vfs.md`, `docs/memory.md`, and `docs/syscall.md`.

**Smoke Test**
```bash
make run-wayland-smoke   # or: tests/musl_ext2_wayland_smoke.sh
```
`user/wayland_smoke.c` runs twice inside one boot:
- Server (a minimal compositor on libwayland-server): listens on `/tmp/wayland-0`, exposes `wl_compositor` v4 and `wl_shm`, checks the connecting client's `SO_PEERCRED` pid, verifies committed pixels through `wl_shm_buffer`, answers frame callbacks, runs a periodic `timerfd` timer and a 20 s watchdog, and handles `SIGUSR1` through a `signalfd` source.
- Client (execs the same binary with `--client`, a separate process): connects through `WAYLAND_DISPLAY`/`XDG_RUNTIME_DIR`, binds the globals, creates a sealed memfd pool (`F_SEAL_SHRINK`, and checks that shrinking fails with `EPERM`), presents frame 1, grows the pool with `posix_fallocate` + `wl_shm_pool_resize` (the server then `mremap`s), presents frame 2 from the new region, signals the server, and disconnects.
- The runner requires every phase marker plus `[wl-smoke] pass`, and fails on any `[wl-smoke] FAIL`.

**Keyboard Keymaps (libxkbcommon)**
- `scripts/build_xkbcommon.sh` compiles libxkbcommon's C sources directly (bison generates the keymap parser), using `wayland/peetos/xkbcommon-config.h` in place of meson's `config.h`. The default XKB root is `/usr/share/X11/xkb`, with rules `evdev`, model `pc105`, and layout `us`.
- Keymap data comes from the build host's installed xkeyboard-config (`XKB_DATA_SRC`, default `/usr/share/X11/xkb`; on Ubuntu the `xkb-data` package). `scripts/xkb_data_subset.py` copies only the include closure of what the `evdev` rules select for `XKB_LAYOUTS` (default `us,gb,de,de:nodeadkeys,fr,us:dvorak`) and `XKB_OPTIONS` (default `ctrl:nocaps,caps:escape,compose:ralt`): about 50 files instead of about 290, which keeps every directory within the ext2 64-entry directory limit.
- The same sources are also built for the host and `wayland/peetos/xkb-check.c` compiles every staged layout and option set against the subset alone, so a missing data file fails the build rather than a QEMU run.
- `peetwl` compiles the keymap at startup (`--xkb-layout/--xkb-variant/--xkb-options`, otherwise the `XKB_DEFAULT_*` environment, otherwise `evdev`/`pc105`/`us`), sends it to clients as `WL_KEYBOARD_KEYMAP_FORMAT_XKB_V1` in a memfd sealed against writes and resizing, and tracks depressed/latched/locked modifiers and the effective group with `xkb_state`. If compilation fails it falls back to `NO_KEYMAP` raw keycodes.
- `wl-demo` maps the keymap `MAP_PRIVATE`, applies `wl_keyboard.modifiers` with `xkb_state_update_mask`, and logs each key's keysym name and UTF-8 text.

**peetwl Compositor**
`user/peetwl.c` is the PeetOS Wayland compositor, built on libwayland-server:
- Output: DRM/KMS by default (`docs/drm.md`). It composites into a back buffer, copies the frame into one of two dumb buffers and page-flips to it, then sends frame callbacks when the flip-complete event arrives (60 Hz pacing). With `--fbdev`, or when `/dev/dri/card0` is unusable, it maps `/dev/fb0` instead and copies the damaged rectangle at most every 16 ms, with frame callbacks after each repaint.
- Globals: `wl_compositor` v4, `wl_shm` (XRGB8888/ARGB8888), `wl_seat` v5 (pointer + keyboard), `wl_output` v3, `xdg_wm_base` v6.
- xdg-shell: toplevels get an initial configure, must `ack_configure` before attaching a buffer, and receive `activated` state changes on focus. `xdg_toplevel.move` starts an interactive move. Popups are created and immediately dismissed (`popup_done`); positioners are accepted and ignored.
- Window management: new toplevels are cascaded and focused. Server-side decorations (title bar with the toplevel title, focus-coloured border, close box) are drawn by the compositor. Clicking raises and focuses a window, dragging the title bar moves it, and the close box sends `xdg_toplevel.close`.
- Input: reads `/dev/input/event0` (keyboard) and `/dev/input/event1` (relative PS/2 or absolute tablet pointer), draws a software cursor, does pointer hit-testing for `wl_pointer` enter/leave/motion/button/frame, and delivers keys with an XKB keymap and xkb_state-tracked modifiers (see Keyboard Keymaps).
- Clients: `--client "<cmd> [args]"` spawns clients after the socket is up; `--smoke` adds cursor/focus logging, exits when every spawned client has exited, and enforces a watchdog. The compositor learns about exited children through a `SIGCHLD` signalfd source.

`user/wl_demo.c` (`wl-demo <title> <0xRRGGBB> [--size WxH] [--animate]`) is a small xdg-shell client: double-buffered shm pool, activated-state accent bar, left click toggles its colour, every key press is logged, `q` or the close box exits, and `--animate` redraws continuously paced by frame callbacks.

Running it:
```bash
make run-wayland-demo   # interactive: three wl-demo windows in a QEMU window (usb-tablet pointer)
make run-wlcomp-smoke   # automated: tests/musl_ext2_wlcomp_smoke.sh
```
The automated smoke boots the `compositor-first musl-ext2-wlcomp-smoke` profile with two clients and drives it from `scripts/wlcomp_smoke_driver.py` through the QEMU monitor, injecting real PS/2 input (`mouse_move`, `mouse_button`, `sendkey`) and checking the screen with `screendump`:
1. both windows show their colours, and the focused/unfocused title bar colours are right
2. moving onto Alpha and clicking gives it pointer and keyboard focus, delivers the button, and the client's colour change reaches the screen
3. a key press reaches only the focused client, and with the compositor running the German layout (`--xkb-layout de`) the client's xkbcommon translation gives `a`, Shift+a gives `A`, the `y` key gives `z`, Shift+2 gives `"`, and Caps Lock locks and unlocks uppercase
4. dragging Alpha's title bar moves the window by exactly the cursor delta
5. Beta's close box makes the client exit and unmaps it
6. `q` exits Alpha, and the compositor exits cleanly after both children are reaped

Set `WLCOMP_SHOTS=<dir>` to keep PNG copies of the screendumps.

Limitations / Simplifications
- Keymap data is limited to the staged layouts and options (rebuild with different `XKB_LAYOUTS`/`XKB_OPTIONS` to change them), and it comes from the build host's xkeyboard-config rather than a pinned release. The keymap is chosen at startup only: there is no runtime layout switching, compose table (`XLOCALEDIR` data is not staged), or keyboard LED handling, and repeat is left to clients (`repeat_info` 25/s after 600 ms).
- The compositor always draws its own cursor and ignores `wl_pointer.set_cursor`. There is no touch support, and scroll/axis events are never sent.
- Committed buffers are copied into compositor memory on every commit (whole surface, ignoring `wl_surface.damage`), and only XRGB8888/ARGB8888 `wl_shm` buffers are supported. Alpha is ignored: windows are drawn opaque.
- xdg-shell subset: no popups (dismissed immediately), no interactive resize, maximise, fullscreen or minimise, configures always suggest 0x0 (client-chosen size), and window geometry is ignored. Placement is a fixed cascade.
- No subsurfaces (`wl_subcompositor`), no `wl_data_device_manager` (clipboard/drag and drop), no multi-output or output scaling.
- Repaint is timer-driven at up to ~60 Hz with a single damage rectangle; there is no vblank synchronisation.
- Only the core library is ported: no `libwayland-cursor` or `libwayland-egl`, and from wayland-protocols only `xdg-shell` is built.
- Both static and shared builds exist. `peetwl` and `wl_demo` are dynamic. `wayland_smoke` and the upstream test binaries stay static, so both link modes stay covered.
- The shared libffi is linked from the static archive (`--whole-archive`) instead of through libffi's libtool shared build, so it has no symbol version script.
- libffi is built with `--disable-exec-static-tramp` because the musl sysroot has no Linux UAPI headers. libwayland only uses `ffi_call`, not closures, so this does not matter for it.
- The smoke uses `/tmp` on the ext2 root as `XDG_RUNTIME_DIR`; there is no per-user runtime directory or permission model.
- `wayland-scanner` is built without libxml2, so it does not DTD-validate protocol XML (meson's `dtd_validation` option).

**Upstream Test Suite**
A subset of libwayland 1.23.1's own test suite (`wayland-src/tests`) runs inside PeetOS:
```bash
make run-wayland-tests   # or: tests/musl_ext2_wayland_tests.sh (part of `make test`)
scripts/build_wayland_tests.sh --list   # the selected tests
```
- `scripts/build_wayland_tests.sh` builds each selected upstream test as its own static musl binary, linked against upstream's test runner (`test-runner.c`, `test-helpers.c`, `test-compositor.c`) as meson does, plus the `tests.xml` protocol generated by our `wayland-scanner`. Outputs go to `wayland/out/tests/` (a `TESTS` list, the binaries, and `exec-fd-leak-checker`); the Makefile rebuilds them when `wayland/out/lib` or the script changes.
- Upstream sources are not patched. `wayland/peetos/tests-config.h` is force-included before every test source; it pulls in the normal `config.h` and then undefines `HAVE_SYS_PRCTL_H`. Upstream's `is_debugger_attached()` forks a child that `PTRACE_ATTACH`es to the runner and treats any failure as "a debugger is attached", which would silently turn off the fd-leak checks and the `SIGALRM` timeouts. PeetOS has no `ptrace`, so the tests take the portable branch, which keeps both checks on.
- The `musl-ext2-wayland-tests` profile stages the binaries in `/mnt/ext2/wltests` and runs each one with `TEST_BUILD_DIR=/mnt/ext2/wltests` and `XDG_RUNTIME_DIR=/tmp`. Upstream's runner forks every test case, and the runner script `scripts/run_toybox_wayland_tests.sh` requires a `[wl-tests] PASS <name>` marker for every test that `--list` reports.
- Leak checking is on: upstream 1.23 checks for fd leaks by counting `/dev/fd` entries (there is no malloc wrapping in this version), and must-fail cases such as `sanity-test`'s leak and timeout tests have to fail for the right reason.

Tests that run (21 binaries, 131 runner cases):
- Data structures: `array-test`, `list-test`, `map-test`, `fixed-test`, `interface-test`, `signal-test`, `newsignal-test` (compiles `wayland-server.c` in, as upstream does)
- Wire protocol: `connection-test` (marshalling, fd passing), `message-test`, `enum-validator-test`, `headers-test` (header compile checks), `protocol-logger-test`, `compositor-introspection-test`
- Client/server over real sockets in forked processes: `client-test`, `display-test`, `queue-test`, `proxy-test`, `resources-test`, `socket-test`, `event-loop-test` (epoll, timerfd, signalfd, eventfd), `sanity-test` (fd-leak detection across `exec`, `SIGALRM` timeouts, crash handling)

Tests that are skipped, and why:
- `os-wrappers-test`: it wraps `socket`, `fcntl`, `recvmsg`, `epoll_create1` and friends through `dlsym(RTLD_NEXT)` to inject failures, which needs dynamic linking. The binaries here are static musl.
- `cpp-compile-test`: a host C++ compile check of the generated headers, not a runtime test (no musl C++ toolchain).
- `scanner-test`: tests `wayland-scanner`, which runs on the build host, not in PeetOS.
- `exec-fd-leak-checker` is a helper, not a test. It is built and staged because `sanity-test` and `display-test` exec it.

Kernel gaps these tests found (all fixed, with host unit tests):
- `mkdir`/`rmdir` (legacy syscalls 83/84) were missing; musl uses them on x86_64.
- `/dev/fd` did not exist; it now lists the caller's open descriptors (`docs/vfs.md`).
- Unlinked files died immediately even while open (`fstat`/`write`/`lseek` returned `ENOENT`, so client logs in `mkstemp`+`unlink` files and `connection-test`'s fd passing broke). Open descriptors and mappings now keep orphaned inodes alive (`docs/vfs.md`).
- `fstat` returned `EBADF` for sockets and other non-file descriptors.
- `setitimer`/`getitimer`/`alarm` were missing, so upstream's per-test `alarm()` timeouts never fired.
- `wait4` reported processes killed by a signal as `exit(128 + sig)`, so `WIFSIGNALED` was never true (upstream checks that crashing clients die with `SIGABRT`).
- `mmap(MAP_SHARED | MAP_ANONYMOUS)` returned `EINVAL` (`display-test` shares memory with a forked child).
- Capacity limits that a compositor hits: 32 fds per process (now 128; `Task` is now two pages), 32 sockets (now 256), 16 `AF_UNIX` bindings (now 128), 32 `flock` locks (now 128), and 64 entries per directory (no per-directory cap since the ext4 work). `wl_display_add_socket_auto` alone can hold 33 sockets and 33 lock files.
- Closing a bound `AF_UNIX` socket unlinked its path. Linux leaves the node until `unlink`, and upstream's `absolute_socket_path` checks that.

Limitations / Simplifications
- The selected tests run in one QEMU boot, sequentially. A test that hangs is caught only by upstream's own `alarm()` timeouts or the runner's overall timeout (`WAYLAND_TESTS_TIMEOUT_SECS`, default 600 s).
- Upstream's debugger detection is compiled out instead of emulated, because PeetOS has no `ptrace`.
- The test binaries add about 10 MB to the shared ext2 image once built; `scripts/build_ext2_musl_execve.sh` stages them into every image built afterwards, as it does for other optional helpers.
- This section supersedes the earlier "Upstream libwayland's own test suite is not built or run yet" limitation above.
