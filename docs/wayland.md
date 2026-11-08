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
- Capacity limits that a compositor hits: 32 fds per process (now 128; `Task` is now two pages), 32 sockets (now 96), 16 `AF_UNIX` bindings (now 48), 32 `flock` locks (now 128), and 64 entries per directory (now 128). `wl_display_add_socket_auto` alone can hold 33 sockets and 33 lock files.
- Closing a bound `AF_UNIX` socket unlinked its path. Linux leaves the node until `unlink`, and upstream's `absolute_socket_path` checks that.

Limitations / Simplifications
- The selected tests run in one QEMU boot, sequentially. A test that hangs is caught only by upstream's own `alarm()` timeouts or the runner's overall timeout (`WAYLAND_TESTS_TIMEOUT_SECS`, default 600 s).
- Upstream's debugger detection is compiled out instead of emulated, because PeetOS has no `ptrace`.
- The test binaries add about 10 MB to the shared ext2 image once built; `scripts/build_ext2_musl_execve.sh` stages them into every image built afterwards, as it does for other optional helpers.
- This section supersedes the earlier "Upstream libwayland's own test suite is not built or run yet" limitation above.
