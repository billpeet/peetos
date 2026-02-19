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

Expected serial output includes `hello from-musl`.

`execve` ext2 shell path (boots musl launcher, then `execve("/mnt/ext2/bin/toybox", {"sh", ...})`):

```bash
make musl
make toybox
make image PROFILE=release KERNEL_FEATURES=musl-ext2-exec
qemu-system-x86_64 \
  -drive id=disk0,file=build/peetos.img,if=none,format=raw \
  -device ich9-ahci,id=ahci \
  -device ide-hd,drive=disk0,bus=ahci.0 \
  -drive id=disk1,file=user/sample_ext2_musl.img,if=none,format=raw \
  -device ide-hd,drive=disk1,bus=ahci.1 \
  -serial stdio -no-reboot -no-shutdown
```

Expected serial output includes `[musl-ext2-exec] launching interactive toybox sh`.
Interactive input is via the QEMU display window (PS/2 keyboard -> kernel TTY -> user stdin).

Scripted command smoke path (boots a shell-script launcher and captures syscall-gap logs):

```bash
scripts/run_toybox_next_smoke.sh
```

Scripted ping smoke path (boots a shell-script launcher and validates userspace ICMP ping through E1000):

```bash
scripts/run_toybox_ping_smoke.sh
# or:
make run-ping-smoke
```

Scripted QuickJS smoke path (boots a shell-script launcher and validates staged `/bin/qjs` on ext2):

```bash
scripts/run_toybox_quickjs_smoke.sh
# or:
make run-quickjs-smoke
```

Scripted MicroPython smoke path (boots a shell-script launcher and validates staged `/bin/micropython` on ext2):

```bash
scripts/run_toybox_micropython_smoke.sh
# or:
make run-micropython-smoke
```

Scripted Dropbear smoke path (boots a shell-script launcher and validates staged `/bin/dropbear` + `/bin/dropbearkey` on ext2):

```bash
scripts/run_toybox_dropbear_smoke.sh
# or:
make run-dropbear-smoke
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
- Ext2 exec smoke payload: `/mnt/ext2/bin/toybox` (multicall applets: `sh`, `echo`, `cat`, `ls`, `mkdir`, `rm`, `pwd`, `cp`, `mv`, `ln`, `touch`) used for `execve` bring-up, built via `scripts/build_toybox_core.sh`

## Limitations / Simplifications

- The bootstrap currently targets `x86_64-linux-musl` rather than a dedicated upstream `x86_64-peetos` musl target.
- This stage sets up source/build plumbing and a narrow shell bring-up path only; full dynamic-linker/runtime verification in-kernel is still pending.
- The shell-core utility set intentionally includes a small syscall-gap probe tier (`cp`, `mv`, `ln`, `touch`) while VFS metadata/node-creation gaps (`fchmodat`, `fchownat`, `mknodat`) are still being closed.
- The default repository tests intentionally avoid networked musl builds and instead validate script behavior/pathing.
- `KERNEL_FEATURES=musl-ext2-exec` is typically run with `PROFILE=release` for faster build/boot iteration on this feature set.
- ext2 shell runtime/smoke flows currently expect attaching `user/sample_ext2_musl.img` as a second block device in QEMU.
- Ping smoke currently validates the QEMU user-mode NAT gateway (`10.0.2.2`) and is not yet a general network interoperability test suite.
- QuickJS smoke currently validates staged script execution (`/mnt/ext2/tmp/test_hello.js`, `/mnt/ext2/tmp/test_math.js`) and not broader JS engine conformance.
- MicroPython smoke currently validates staged script execution (`/mnt/ext2/tmp/test_hello.py`, `/mnt/ext2/tmp/test_math.py`) and not broader module/runtime compatibility.
- Dropbear smoke currently validates basic tool bring-up (`dropbear -V`, `dropbearkey -h`), staged host-key usage, and daemonized startup (`dropbear ... -p 2222` with same-port bind-collision probe) only; full authenticated SSH session/interoperability testing remain pending.
