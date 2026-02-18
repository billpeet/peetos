# User Programs (Embedded)

User images embedded by the kernel:

- `user/hello.elf`: minimal static ELF64 baseline.
- `user/hello_dyn.elf`: PIE `ET_DYN` main image with `PT_INTERP` set to `/lib/ld-peetos.so`.
- `user/ld-peetos.so`: minimal interpreter stub that reads `AT_ENTRY` from `auxv` and jumps to the program entry point.
- `user/sample_ext2.img`: sample ext2 image (built from `user/ext2_root/`) mounted at `/mnt/ext2` with ext2-backed read/write/create/mkdir/truncate behavior that mutates inode/data metadata in the mounted image.

Related Stage-9 userspace bootstrap artifacts live outside this directory:

- `musl/musl-src`: upstream musl source (fetched via `make musl-fetch`).
- `musl/sysroot`: headers/libraries for PeetOS-oriented user-space cross-compilation (built via `make musl`).
- `user/musl_echo.elf`: feature-gated musl static runtime smoke binary (built on demand by `KERNEL_FEATURES=musl-smoke`).
- `user/musl_execve_ext2.elf`: feature-gated musl launcher that calls `execve("/mnt/ext2/bin/toybox", {"sh", ...})` for interactive TTY shell bring-up (`KERNEL_FEATURES=musl-ext2-exec`).
- `user/musl_execve_ext2_smoke.elf`: feature-gated musl launcher that runs scripted shell smoke commands from `/mnt/ext2/bin/next_smoke.sh` for syscall-gap capture (`KERNEL_FEATURES=musl-ext2-cmd-smoke`).
- `user/musl_execve_ext2_ping_smoke.elf`: feature-gated musl launcher that runs scripted ping smoke from `/mnt/ext2/bin/ping_smoke.sh` (`KERNEL_FEATURES=musl-ext2-ping-smoke`).
- `user/musl_execve_ext2_quickjs_smoke.elf`: feature-gated musl launcher that runs scripted QuickJS smoke from `/mnt/ext2/bin/quickjs_smoke.sh` (`KERNEL_FEATURES=musl-ext2-quickjs-smoke`).
- `user/sample_ext2_musl.img`: feature-gated ext2 image that includes `/bin/toybox` (shell/core + networking applets including `ping`) and optionally `/bin/qjs` + staged scripts for QuickJS smoke (`KERNEL_FEATURES=musl-ext2-exec` / `musl-ext2-cmd-smoke` / `musl-ext2-ping-smoke` / `musl-ext2-quickjs-smoke`).
- `user/qjs`: local staged QuickJS interpreter copy produced by `make quickjs` and inserted into ext2 as `/bin/qjs` for smoke runs.
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
