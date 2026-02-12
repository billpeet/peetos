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
- `user/musl_execve_ext2.elf`: feature-gated musl launcher that calls `execve("/mnt/ext2/bin/echo", ...)` (`KERNEL_FEATURES=musl-ext2-exec`).
- `user/sample_ext2_musl.img`: feature-gated ext2 image that includes Toybox-backed `/bin/echo` for exec smoke testing (`KERNEL_FEATURES=musl-ext2-exec`).
- `user/toybox_echo.elf`: optional local copy of the staged Toybox `echo` binary (`scripts/build_toybox_echo.sh`).

Regenerate the binary if needed:

```bash
make user
```

Regenerate only the ext2 sample image:

```bash
make ext2-image
```

These binaries are embedded in the kernel via `include_bytes!`, and `make image`
rebuilds them automatically.
