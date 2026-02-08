# Syscall Entry

This document describes the current syscall entry implementation. It is a minimal baseline meant to unblock later user-mode work in Phase 4.

**Where It Lives**
- `crates/arch_x86_64/src/syscall.rs`
- `crates/arch_x86_64/src/msr.rs`
- `kernel/src/syscall.rs`
- `kernel/src/main.rs`

**MSR Setup**
`arch_x86_64::syscall::init()` configures the syscall MSRs:
- `IA32_EFER`: sets `EFER_SCE` to enable `SYSCALL`/`SYSRET`, and `EFER_NXE` so page-table NX bits are honored.
- `IA32_STAR`: uses the GDT layout where user data is at 0x18 and user code at 0x20. The base is set to `GDT_KERNEL_DS` (0x10) so SYSRET computes:
  - SS = 0x10 + 8 = 0x18
  - CS = 0x10 + 16 = 0x20
- `IA32_LSTAR`: points to `syscall_entry`.
- `IA32_FMASK`: masks IF/TF/DF on entry.

MSR read/write helpers are in `crates/arch_x86_64/src/msr.rs`.

Limitations / Simplifications
- Assumes the GDT selector layout defined in `gdt.rs`; no runtime validation is performed.
- No CPU feature checks (e.g., for `SYSCALL` support) are done yet.

**Syscall Entry Stub**
`syscall_entry` is defined via `global_asm!` in `crates/arch_x86_64/src/syscall.rs`. It:
1. Saves RCX (return RIP) and R11 (return RFLAGS).
2. Saves the syscall number (RAX) and the standard SysV argument registers.
3. Calls `syscall_handler` with a pointer to the `SyscallFrame`.
4. Restores registers and returns via `sysretq`.

The current stub does not switch stacks. It assumes execution on the current stack and is suitable only for early bring-up. There is no SWAPGS or TSS.RSP0 integration yet.

Limitations / Simplifications
- No stack switching, GS base swap, or per-task kernel stack support.
- No SMEP/SMAP toggling or user pointer validation.

**INT 0x80 Path**
For initial user-mode bring-up, a user-accessible `int 0x80` gate is installed in the IDT. The handler uses the `InterruptFrame` to extract syscall arguments and write the return value to `RAX`.

Limitations / Simplifications
- The `int 0x80` path currently shares the same Linux-numbered dispatcher as the SYSCALL path and supports a baseline subset:
  - `SYS_OPEN`, `SYS_OPENAT`, `SYS_CLOSE`, `SYS_READ`, `SYS_WRITE`, `SYS_LSEEK`
  - `SYS_EXIT`, `SYS_EXIT_GROUP`
  - `SYS_YIELD`
  - `SYS_GETPID`, `SYS_GETPPID`, `SYS_GETTID`
  - `SYS_SET_TID_ADDRESS` (stores per-task clear-tid pointer for future thread-exit/futex wiring)
  - `SYS_BRK`
- The int 0x80 path is temporary and will be replaced by a full SYSCALL/SYSRET flow with stack switching.

**Syscall Handler**
`kernel/src/syscall.rs` now implements a table-driven dispatcher shared by both the SYSCALL and int 0x80 paths. Arguments are normalized into a small `SyscallArgs` struct, syscall entries carry mode metadata (any task vs user task), unknown syscall logs are rate-limited, and return values use Linux-style negative errno.

Current implemented handlers:
- `open/openat/close/read/write/lseek` via baseline VFS descriptors (tmpfs + virtual `devfs`/`procfs`)
- `exit(status)` and `exit_group(status)`
- `sched_yield()`
- `getpid()`, `getppid()`, `gettid()`
- `set_tid_address(ptr)` (clear-tid pointer tracking; best-effort zeroing on exit)
- `brk(addr)` with per-task program-break metadata and page mapping/unmapping

Limitations / Simplifications
- Most Linux syscall numbers are still unimplemented and return `-ENOSYS`.
- VFS integration is baseline-only: `openat` currently supports `AT_FDCWD` with absolute paths; no full mount namespace/path-root semantics.
- `devfs`/`procfs` are minimal virtual backends (`/dev/null`, `/dev/zero`, `/dev/console`, `/proc/{uptime,meminfo,mounts,version}`) and are not yet full Linux-compatible implementations.
- Console I/O is simplified: stdio descriptors are pre-seeded for user tasks, console writes go to serial, and console reads currently return EOF.
- `brk` only manages simple per-task heap growth/shrink and does not yet integrate with full userspace allocator semantics (no `mmap` fallback, no overcommit policy, no guard pages).
- `set_tid_address` does not yet perform futex wakeups for robust thread teardown semantics.

**Integration**
`kernel/src/main.rs` calls `arch_syscall::init()` (MSR setup) and `syscall::init()` (dispatcher state init) during early boot.

**Status Notes**
- This is a minimal stub for future user-mode support.
- No per-task kernel stack switching is implemented yet.
