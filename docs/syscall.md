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
1. Saves user RSP, RCX (return RIP), and R11 (return RFLAGS).
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
  - `SYS_OPEN`, `SYS_OPENAT`, `SYS_MKDIRAT`, `SYS_CLOSE`, `SYS_READ`, `SYS_WRITE`, `SYS_LSEEK`
  - `SYS_MMAP`, `SYS_MPROTECT`, `SYS_MUNMAP` (baseline userspace VM mapping path)
  - `SYS_RT_SIGACTION`, `SYS_RT_SIGPROCMASK`, `SYS_RT_SIGRETURN`, `SYS_KILL` (baseline signal path)
  - `SYS_SHMGET`, `SYS_SHMAT`, `SYS_SHMDT`, `SYS_SHMCTL` (baseline System V shared memory)
  - `SYS_FSTAT`, `SYS_NEWFSTATAT`, `SYS_GETDENTS64`, `SYS_UNLINKAT`, `SYS_RENAMEAT`
  - `SYS_PIPE`, `SYS_PIPE2`, `SYS_FUTEX`
  - `SYS_EXECVE` (image replacement with caller-provided `argv`/`envp`)
  - `SYS_ARCH_PRCTL` (baseline `ARCH_SET_FS`/`ARCH_GET_FS`)
  - `SYS_EXIT`, `SYS_EXIT_GROUP`
  - `SYS_YIELD`
  - `SYS_GETPID`, `SYS_GETPPID`, `SYS_GETTID`
  - `SYS_SET_TID_ADDRESS` (stores per-task clear-tid pointer)
  - `SYS_BRK`
- The int 0x80 path is temporary and will be replaced by a full SYSCALL/SYSRET flow with stack switching.

**Syscall Handler**
`kernel/src/syscall.rs` now implements a table-driven dispatcher shared by both the SYSCALL and int 0x80 paths. Arguments are normalized into a small `SyscallArgs` struct, syscall entries carry mode metadata (any task vs user task), unknown syscall logs are rate-limited, and return values use Linux-style negative errno.

Current implemented handlers:
- `open/openat/close/read/write/lseek` via baseline VFS descriptors (tmpfs + virtual `devfs`/`procfs`)
- `mmap/mprotect/munmap` for baseline userspace mapping management (anonymous private mappings plus private file-backed mappings from VFS fds, fixed/fixed-noreplace handling, and page-level protection changes)
- `kill`, `rt_sigaction`, `rt_sigprocmask`, `rt_sigreturn` for baseline signal control and user-handler return
- `pipe/pipe2` via in-kernel anonymous pipe state and per-task fd tables
- `futex` (`WAIT`/`WAKE` baseline ops only)
- `shmget/shmat/shmdt/shmctl(IPC_RMID)` via baseline in-kernel System V shared-memory segments
- `execve(path, argv, envp)` with in-kernel argument/environment copy, VFS-backed ELF load, and direct frame handoff to the new image entry
- `arch_prctl(ARCH_SET_FS/ARCH_GET_FS)` with per-task FS-base tracking and scheduler restore on context switch
- `exit(status)` and `exit_group(status)`
- `sched_yield()`
- `getpid()`, `getppid()`, `gettid()`
- `set_tid_address(ptr)` (clear-tid pointer tracking; best-effort zeroing + wake on exit)
- `brk(addr)` with per-task program-break metadata and page mapping/unmapping

Limitations / Simplifications
- Most Linux syscall numbers are still unimplemented and return `-ENOSYS`.
- VFS integration is baseline-only: `openat` supports absolute paths plus relative lookups from `AT_FDCWD` or a directory fd, but there is no mount namespace/chroot/path-root isolation.
- `devfs`/`procfs` are minimal virtual backends (`/dev/null`, `/dev/zero`, `/dev/console`, `/proc/{uptime,meminfo,mounts,version}`) and are not yet full Linux-compatible implementations.
- Console I/O is still simplified: stdio descriptors are pre-seeded for user tasks, console writes go to serial, and console reads use a canonical line-buffered TTY path backed by keyboard input (`read` blocks until a line is committed with Enter).
- TTY behavior is minimal: canonical mode only, no `termios` ioctls/flags, no signal-generating control characters, and no pty support.
- VM syscalls are still simplified: `mmap` currently supports `MAP_PRIVATE` mappings only (anonymous and file-backed), with no shared mappings, no copy-on-write optimization, no demand paging, and no VMA metadata layer.
- VM mappings are currently constrained to a dedicated high user range managed by the kernel mmap allocator (used to avoid touching legacy low-memory bootstrap mappings).
- `brk` still uses simple contiguous growth/shrink and does not yet integrate with full userspace allocator policies (no overcommit accounting, no guard-page policy).
- `futex` support is intentionally narrow: no timeout handling, no `*_PRIVATE`/bitset/requeue/PI variants, and no robust-list support yet.
- Shared memory support is intentionally narrow: fixed-size segment/attachment tables, `IPC_RMID` only for `shmctl`, no permission enforcement, and no inheritance semantics across `fork`/`exec` yet.
- `execve` is intentionally narrow: no `CLOEXEC` handling yet, no shebang/script loading, bounded `argv`/`envp` vector sizes, and no full Linux signal-reset semantics.
- `arch_prctl` is intentionally narrow: only `ARCH_SET_FS` and `ARCH_GET_FS` are implemented; GS-base operations return `-EINVAL`.
- Signal support remains simplified: delivery is syscall-boundary based and user handlers currently require `SA_RESTORER`; `sigaltstack`, queued RT signal payloads/`siginfo_t`, and full Linux ucontext semantics are not implemented yet.

**Integration**
`kernel/src/main.rs` calls `arch_syscall::init()` (MSR setup) and `syscall::init()` (dispatcher state init) during early boot.

**Status Notes**
- This is a minimal stub for future user-mode support.
- No per-task kernel stack switching is implemented yet.
