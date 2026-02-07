# PeetOS (Rust Port)

This repository is the Rust reimplementation of the PeetOS C kernel located in `../peetos`. The goal is to achieve feature parity with the existing C system, then iterate on modernization and safety.

## Architecture Snapshot

- Target: `x86_64` bare metal, higher-half kernel.
- Boot flow: 2-stage BIOS bootloader -> 64-bit long mode -> kernel entry.
- Kernel entry: `kernel/arch/x86_64/entry.asm` jumps to Rust `kmain()`.
- Linker script: `linker/kernel.ld` (reused from the C kernel).

Key references in the C project:

- Feature scope: `../peetos/docs/ROADMAP.md`
- Kernel subsystems: `../peetos/kernel/src`
- Bootloader: `../peetos/boot`

## Repository Layout

- `boot/` Reused stage1 and stage2 bootloader assembly.
- `kernel/` Rust kernel binary crate.
- `crates/` Placeholder subsystem crates for drivers, filesystems, networking, syscalls, and user ABI.
- `linker/` Linker script for the kernel.
- `docs/PORTING_PLAN.md` High-level implementation roadmap.

## Build Prerequisites

- `nasm`
- `ld` and `objcopy` (binutils)
- `cargo` + `rustc`
- `qemu-system-x86_64`

Optional but commonly required:

- `rust-src` component if you need to rebuild `core` for a target.

## Build

The build is driven by the Makefile, which assembles the bootloader, compiles the kernel, and creates a bootable disk image.

```bash
make image
```

Build a debug kernel:

```bash
make image PROFILE=debug
```

Run in QEMU with serial output:

```bash
make run
```

## Testing

Initial smoke test is a QEMU boot to the kernel `kmain()` loop. As features land, re-run the C project shell tests for parity:

```bash
../peetos/tests/run_tests.sh
../peetos/tests/run_shell_cmds.sh -clean "echo hello" "ls /bin"
```

## Notes

- The kernel is compiled with `no_std` and `panic=abort`.
- The bootloader and linker script are reused from the C codebase to keep early bring-up stable.
- The Rust kernel currently contains a minimal `kmain()` and panic handler; subsystems will be layered in phase-by-phase per `docs/PORTING_PLAN.md`.
