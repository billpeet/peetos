# User Programs (Embedded)

User images embedded by the kernel:

- `user/hello.elf`: minimal static ELF64 baseline.
- `user/hello_dyn.elf`: PIE `ET_DYN` main image with `PT_INTERP` set to `/lib/ld-peetos.so`.
- `user/ld-peetos.so`: minimal interpreter stub that reads `AT_ENTRY` from `auxv` and jumps to the program entry point.
- `user/sample_ext2.img`: sample ext2 image (built from `user/ext2_root/`) mounted at `/mnt/ext2` with ext2-backed read/write/create/mkdir/truncate behavior that mutates inode/data metadata in the mounted image. The builder intentionally emits a minimal feature set (`filetype`, `sparse_super`) so the fixture matches the kernel's current compatibility allowlist.

Related userspace bootstrap artifacts live outside this directory:

- `musl/musl-src`: upstream musl source (fetched via `make musl-fetch`).
- `musl/sysroot`: headers/libraries for PeetOS-oriented user-space cross-compilation (built via `make musl`).
- `user/musl_echo.elf`: feature-gated musl static runtime smoke binary (built on demand by `KERNEL_FEATURES=musl-smoke`).
- `user/musl_execve_ext2.elf`: feature-gated musl launcher that calls `execve("/mnt/ext2/bin/toybox", {"sh", ...})` for interactive TTY shell bring-up (`KERNEL_FEATURES=musl-ext2-exec`).
- `user/musl_execve_ext2_smoke.elf`: feature-gated musl launcher that runs scripted shell smoke commands from `/mnt/ext2/bin/next_smoke.sh` for syscall-gap capture (`KERNEL_FEATURES=musl-ext2-cmd-smoke`).
- `user/musl_execve_ext2_console_first_smoke.elf`: feature-gated musl launcher that runs a minimal console-first shell smoke from `/mnt/ext2/bin/console_first_smoke.sh` (`KERNEL_FEATURES=musl-ext2-console-first-smoke`).
- `user/musl_execve_ext2_ping_smoke.elf`: feature-gated musl launcher that runs scripted ping smoke from `/mnt/ext2/bin/ping_smoke.sh` (`KERNEL_FEATURES=musl-ext2-ping-smoke`).
- `user/musl_execve_ext2_gui_smoke.elf`: feature-gated musl launcher that runs scripted compositor-first GUI smoke from `/mnt/ext2/bin/gui_smoke.sh` (`KERNEL_FEATURES="compositor-first musl-ext2-gui-smoke"`).
- `user/musl_execve_ext2_peet_gui_smoke.elf`: feature-gated musl launcher that runs the initial Phase 4 compositor-core smoke from `/mnt/ext2/bin/peet_gui_smoke.sh` (`KERNEL_FEATURES="compositor-first musl-ext2-peet-gui-smoke"`).
- `user/musl_execve_ext2_peet_shell_demo.elf`: feature-gated musl launcher that runs the visual shell scaffold from `/mnt/ext2/bin/peet_shell_demo.sh` (`KERNEL_FEATURES="compositor-first musl-ext2-peet-shell-demo"`).
- `user/musl_execve_ext2_quickjs_smoke.elf`: feature-gated musl launcher that runs scripted QuickJS smoke from `/mnt/ext2/bin/quickjs_smoke.sh` (`KERNEL_FEATURES=musl-ext2-quickjs-smoke`).
- `user/musl_execve_ext2_micropython_smoke.elf`: feature-gated musl launcher that runs scripted MicroPython smoke from `/mnt/ext2/bin/micropython_smoke.sh` (`KERNEL_FEATURES=musl-ext2-micropython-smoke`).
- `user/musl_execve_ext2_dropbear_smoke.elf`: feature-gated musl launcher that runs scripted Dropbear smoke from `/mnt/ext2/bin/dropbear_smoke.sh` (`KERNEL_FEATURES=musl-ext2-dropbear-smoke`).
- `user/gui_compositor_smoke.elf`: userspace helper staged into the ext2 smoke image as `/bin/gui_compositor_smoke`; it validates compositor-first environment variables, opens `/dev/fb0` and `/dev/input/event{0,1}`, maps/writes the framebuffer, and confirms injected keyboard/pointer events arrive through the input-device path.
- `user/peet_gui_compositor_smoke.elf`: initial Phase 4 compositor-core helper staged as `/bin/peet_gui_compositor_smoke`; it renders two overlapping surfaces into the shared framebuffer, validates initial z-order plus raise-on-focus behavior, validates a damage-limited partial redraw/commit on one surface, tracks focus state, routes injected keyboard/pointer events through the compositor-first loop, and validates that a client detach redraw leaves the remaining surface stable.
- `user/peet_shell_demo.elf`: Phase 4 visual shell scaffold staged as `/bin/peet_shell_demo`; it renders a desktop shell layout directly on `/dev/fb0` with a top bar, launcher rail, and terminal/files/inspector panels composed from independent offscreen surface buffers with focus raise. The `FILES` panel is now driven by a separate forked userspace client over an `AF_UNIX socketpair` message path, while the demo still provides a software pointer cursor, theme cycling, and terminal-style command feedback for manual GUI iteration.
- `user/sample_ext2_musl.img`: feature-gated ext2 image that includes `/bin/toybox` (shell/core + networking applets including `ping`) and optionally staged runtime smoke scripts such as `/bin/console_first_smoke.sh`, `/bin/gui_smoke.sh`, or `/bin/peet_gui_smoke.sh`, plus `/bin/gui_compositor_smoke`, `/bin/peet_gui_compositor_smoke`, `/bin/qjs`, `/bin/micropython`, or `/bin/dropbear` as needed (`KERNEL_FEATURES=musl-ext2-exec` / `musl-ext2-cmd-smoke` / `musl-ext2-console-first-smoke` / `musl-ext2-ping-smoke` / `musl-ext2-gui-smoke` / `musl-ext2-peet-gui-smoke` / `musl-ext2-quickjs-smoke` / `musl-ext2-micropython-smoke` / `musl-ext2-dropbear-smoke`); the same compositor-first image is reused by `run-gui-smoke`, `run-gui-fallback-smoke`, and `run-peet-gui-smoke`. Like the smaller sample image, it is built with a minimal ext2 feature set to stay inside the runtime's current compatibility envelope.
- `user/qjs`: local staged QuickJS interpreter copy produced by `make quickjs` and inserted into ext2 as `/bin/qjs` for smoke runs.
- `user/micropython`: local staged MicroPython interpreter copy produced by `make micropython` and inserted into ext2 as `/bin/micropython` for smoke runs.
- `user/dropbear`, `user/dropbearkey`: local staged Dropbear daemon/keygen copies produced by `make dropbear` and inserted into ext2 as `/bin/dropbear` and `/bin/dropbearkey` for smoke runs.
- `user/toybox_echo.elf`: optional local copy of the staged Toybox `echo` binary (`scripts/build_toybox_echo.sh`).
- `user/toybox_sh.elf`: optional local copy of the staged Toybox `sh` binary (`scripts/build_toybox_sh.sh`).
- `user/toybox_core.elf`: optional local copy of the staged shell-core Toybox multicall binary (`scripts/build_toybox_core.sh`).

Regenerate the binary if needed:

```bash
make user
```

Regenerate only the ext2 sample image:

```bash
make ext2-image
```

Kernel user-ELF payloads are embedded via `include_bytes!`; ext2 runtime images
(`user/sample_ext2.img`, `user/sample_ext2_musl.img`) are built alongside the
kernel and attached/mounted as block-backed payloads for runtime smoke paths.
