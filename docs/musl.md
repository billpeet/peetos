# musl Bootstrap

This document tracks the Rust-port Stage 9 musl bootstrap setup.

## Scope

- Fetch pinned musl source into `musl/musl-src`.
- Configure musl with PeetOS-oriented defaults.
- Build/install into `musl/sysroot` for downstream user-space package builds.

## Commands

```bash
make musl-fetch
make musl-configure
make musl
make musl-info
```

Runtime smoke path (boots musl static `echo` program as PID 1):

```bash
make image PROFILE=debug KERNEL_FEATURES=musl-smoke
timeout 8s qemu-system-x86_64 -drive format=raw,file=build/peetos.img -serial stdio -display none -no-reboot -no-shutdown
```

`execve` ext2 smoke path (boots musl launcher, then `execve("/mnt/ext2/bin/echo", ...)`):

```bash
make musl
make toybox
make image PROFILE=release KERNEL_FEATURES=musl-ext2-exec
timeout 8s qemu-system-x86_64 -drive format=raw,file=build/peetos.img -serial stdio -display none -no-reboot -no-shutdown
```

Common overrides:

```bash
make musl TARGET=x86_64-linux-musl
make musl CROSS_PREFIX=x86_64-peetos-
make musl CC=clang AR=llvm-ar RANLIB=llvm-ranlib
```

## Output

- Headers: `musl/sysroot/include`
- Libraries/CRT objects: `musl/sysroot/lib`
- Ext2 exec smoke payload: `/mnt/ext2/bin/echo` built from `toybox/out/echo`

## Limitations / Simplifications

- The bootstrap currently targets `x86_64-linux-musl` rather than a dedicated upstream `x86_64-peetos` musl target.
- This stage sets up source/build plumbing and a narrow exec smoke path only; full dynamic-linker/runtime verification in-kernel is still pending.
- The default repository tests intentionally avoid networked musl builds and instead validate script behavior/pathing.
- `KERNEL_FEATURES=musl-ext2-exec` currently requires `PROFILE=release` in this tree because debug kernels can exceed stage2 low-memory kernel staging limits.
