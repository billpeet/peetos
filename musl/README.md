# musl Setup for PeetOS

This directory bootstraps musl source and a sysroot for PeetOS user-space cross-compilation work.

## Quick Start

From repo root:

```bash
make musl-fetch
make musl-configure
make musl
```

Useful variables:

```bash
make musl TARGET=x86_64-linux-musl
make musl TARGET=x86_64-linux-musl CROSS_PREFIX=x86_64-peetos-
make musl CC=clang AR=llvm-ar RANLIB=llvm-ranlib
```

## Layout

- `musl/fetch-musl.sh`: Downloads/extracts a pinned musl release into `musl/musl-src`.
- `musl/configure-musl.sh`: Runs musl `configure` with PeetOS-oriented defaults.
- `musl/Makefile`: Wraps fetch/configure/build/install/clean/info.
- `musl/musl-src` (generated): Upstream musl source tree.
- `musl/sysroot` (generated): Installed headers and libraries (`include/`, `lib/`).

## Cross-Compile Notes

- Current default target is `x86_64-linux-musl` because PeetOS currently tracks Linux syscall ABI numbering/behavior for userspace compatibility work.
- `CROSS_PREFIX=x86_64-peetos-` is supported for downstream packages that expect prefixed cross tools.
- Dynamic-linker/runtime compatibility is still in-progress in the kernel and loader paths; this setup is build-system scaffolding for userspace runtime bring-up.

## Limitations / Simplifications

- This setup does not patch upstream musl yet for a dedicated `x86_64-peetos` target.
- Shared-library runtime validation in PeetOS is not complete; static-focused bring-up is expected first.
- The repo test harness validates setup scripts and argument plumbing, not a full musl libc build (to avoid network/toolchain assumptions in CI).
