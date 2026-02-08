# Syscall Entry

This document describes the current syscall entry implementation. It is a minimal baseline meant to unblock later user-mode work in Phase 4.

**Where It Lives**
- `crates/arch_x86_64/src/syscall.rs`
- `crates/arch_x86_64/src/msr.rs`
- `kernel/src/syscall.rs`
- `kernel/src/main.rs`

**MSR Setup**
`arch_x86_64::syscall::init()` configures the syscall MSRs:
- `IA32_EFER`: sets `EFER_SCE` to enable `SYSCALL`/`SYSRET`.
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

**Syscall Handler**
`kernel/src/syscall.rs` implements a placeholder handler that logs the syscall number and returns `-ENOSYS`.

Limitations / Simplifications
- No syscall table or dispatch; all syscalls are rejected.
- No argument validation or errno translation beyond `-ENOSYS`.

**Integration**
`kernel/src/main.rs` calls `arch_syscall::init()` during early boot.

**Status Notes**
- This is a minimal stub for future user-mode support.
- No per-task kernel stack switching is implemented yet.
