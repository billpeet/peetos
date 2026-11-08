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
- `share/wayland/wayland.xml`

The build needs no meson, ninja, or autotools:
- libwayland's core is eight C files compiled directly by `scripts/build_wayland.sh` with `musl-gcc`.
- libffi's release tarball ships a generated `configure`, run as a cross build (`--host=x86_64-linux-musl`) because musl binaries cannot run on the build host.
- `wayland-scanner` is compiled from the same libwayland source with the host compiler and host expat (`libexpat1-dev`), so generated code always matches the library version.

Linking a client or compositor:
```bash
musl-gcc -static app.c -Iwayland/out/include \
  wayland/out/lib/libwayland-client.a wayland/out/lib/libffi.a -lm
```
`scripts/build_musl_echo.sh` accepts the extra arguments after `--`, which is how the Makefile builds `user/wayland_smoke.elf`.

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

Limitations / Simplifications
- Only the core library is ported: no `libwayland-cursor`, `libwayland-egl`, or `wayland-protocols` (so no `xdg-shell` yet). The smoke compositor implements `wl_compositor`/`wl_surface`/`wl_region` minimally and does not present to `/dev/fb0`.
- Libraries are static only; PeetOS's dynamic loader is bootstrap-grade, so shared libraries are out of scope for now.
- libffi is built with `--disable-exec-static-tramp` because the musl sysroot has no Linux UAPI headers. libwayland only uses `ffi_call`, not closures, so this does not matter for it.
- The smoke uses `/tmp` on the ext2 root as `XDG_RUNTIME_DIR`; there is no per-user runtime directory or permission model.
- `wayland-scanner` is built without libxml2, so it does not DTD-validate protocol XML (meson's `dtd_validation` option).
- Upstream libwayland's own test suite is not built or run yet.
