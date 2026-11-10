# D-Bus

PeetOS runs a D-Bus session bus: the reference `dbus-daemon` 1.16, with libdbus clients (`dbus-send`), GLib's GDBus clients (`gdbus`, GTK applications) and bus-activated services such as Xfce's settings daemon `xfconfd`. Thunar uses the bus for its settings and its file-manager names (`docs/thunar.md`).

**Where It Lives**
- `wayland/fetch-wayland.sh` pins dbus 1.16.2 (SHA-256; the release is signed by the dbus maintainer, Simon McVittie).
- `scripts/build_dbus.sh` builds libdbus-1, `dbus-daemon` and the tools into the staged sysroot (`wayland/out/stage`). `make -C wayland` runs it after `scripts/build_gtk.sh`.
- `scripts/build_ext2_musl_execve.sh` stages `dbus-daemon`, `dbus-send`, `dbus-monitor`, `dbus-uuidgen`, `dbus-run-session`, GLib's `gdbus`, `/usr/share/dbus-1` (the session configuration and the `.service` activation files installed by xfconf and Thunar), `xfconfd` and `xfconf-query`. libdbus comes with the other shared libraries.
- `user/musl_execve_ext2_dbus_smoke.c`, `scripts/run_toybox_dbus_smoke.sh` and `tests/musl_ext2_dbus_smoke.sh` are the QEMU smoke (`make run-dbus-smoke`).

**How It Is Built**
- meson against the stage, reusing GTK's cross file. Only the session bus matters: systemd, SELinux, AppArmor, libaudit, X11 autolaunch, launchd, kqueue and inotify are off, as are the docs and tests. epoll is on.
- The session configuration is upstream's: `unix:tmpdir=/tmp` by default, EXTERNAL authentication, and the standard service directories (`/usr/share/dbus-1/services`).

**Starting a Session Bus**
```sh
dbus-uuidgen --ensure=/etc/machine-id
dbus-daemon --session --fork --address=unix:path=/tmp/dbus-session --print-pid=1
export DBUS_SESSION_BUS_ADDRESS=unix:path=/tmp/dbus-session
```
`dbus-run-session -- <command>` does the same for one command, and stops the bus when the command exits. The desktop session uses it: `peet-session` runs `dbus-run-session -- labwc` (`docs/desktop.md`). GLib and libdbus need `/etc/machine-id`; `dbus-uuidgen --ensure` creates it once.

**Kernel work D-Bus needed**
- **Credentials.** libdbus servers read the client's pid/uid/gid with `SO_PEERCRED` (already there). GDBus clients send their credentials as an `SCM_CREDENTIALS` message with the authentication nul byte, which `sendmsg` used to reject. It now validates them like Linux, and `SO_PASSCRED` makes `recvmsg` deliver the peer's credentials (`docs/ipc.md`).
- **Peer options.** `getsockopt` answered every unknown option with success and a zero value. For `SO_PEERPIDFD` that handed dbus-daemon "pidfd 0". `SO_PEERSEC`, `SO_PEERGROUPS` and `SO_PEERPIDFD` now fail with `ENOPROTOOPT`, as on Linux without the feature, and libdbus falls back. `pidfd_open` returns `-ENOSYS`.
- **User copies of untouched pages.** mmap regions are populated lazily, and the kernel reads user memory through the physical map, so a syscall argument in a page the process had not touched failed with `EFAULT`. The forked daemon's `open("/dev/null")`, with the path in libdbus's read-only data, hit this. User copies now fault such pages in (`docs/smp.md`).
- **epoll across fork.** `dbus-daemon --fork` creates its epoll set and registers the listening socket, then forks. Watches were per process and the child had none, so after the parent exited no connection was ever noticed. The child now inherits copies of the parent's watches.
- **`EPOLLET`/`EPOLLONESHOT`.** dbus-daemon registers each new connection with `EPOLLET` until it enables the watch, and `epoll_ctl` rejected the flag. Both flags are now supported (`docs/ipc.md`).
- `clock_getres` and `fdatasync` (xfconfd saves its channels with it).
- A `syscall-trace` kernel feature (`docs/diagnostics.md`) logs every syscall with its result. It is how the hangs above were found.

**Testing**
`make run-dbus-smoke` (part of `make test`) boots the `musl-ext2-dbus-smoke` profile, which runs `bin/dbus_smoke.sh` on the console. It checks:
- `dbus-uuidgen --ensure` writes a machine id;
- `dbus-daemon --session --fork` comes up on `/tmp/dbus-session`;
- `dbus-send` (libdbus, `SO_PEERCRED` authentication) lists the bus names;
- `gdbus call` (GDBus, `SCM_CREDENTIALS` authentication) reads the daemon's own pid and uid back from the bus;
- `xfconf-query` sets and reads a property, which makes the bus start `xfconfd` from its `.service` file, and `org.xfce.Xfconf` then appears on the bus;
- after `xfconfd` gets `SIGTERM`, the channel is on disk under `~/.config/xfce4/xfconf`, and a new query activates `xfconfd` again and reads the value back.

The runner fails on unhandled syscalls. Set `DBUS_SMOKE_EXTRA_FEATURES=syscall-trace` to trace the run. The proc smoke (`make run-proc-smoke`) covers `SCM_CREDENTIALS`/`SO_PASSCRED` between processes and syscall arguments in untouched pages. The Thunar smoke runs Thunar on a session bus.

Limitations / Simplifications
- Session bus only. There is no system bus, no `messagebus` user, no `dbus-daemon-launch-helper` setup, and no service supervisor: something has to start the bus and export `DBUS_SESSION_BUS_ADDRESS`. `peet-session` does that for the desktop session (`docs/desktop.md`); the other smokes do it themselves.
- Everything runs as root, so the bus's per-user policy is trivially satisfied. The credentials a server sees are the peer's at connection time, not per message (`docs/ipc.md`).
- No inotify: the daemon does not notice configuration or `.service` files changing while it runs.
- No abstract-namespace Unix sockets: buses must use path addresses (`unix:path=`, `unix:tmpdir=`).
- Activated services are only those staged: xfconfd and Thunar. There is no thumbnailer (tumbler), notification daemon, accessibility bus (at-spi), portal or gvfs.
- No X11 autolaunch. A client without `DBUS_SESSION_BUS_ADDRESS` fails to connect (GLib logs a warning, and applications such as Thunar fall back to defaults).
