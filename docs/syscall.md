# Syscall Entry

This document describes the current syscall entry implementation and dispatcher behavior.

**Where It Lives**
- `crates/arch_x86_64/src/syscall.rs`
- `crates/arch_x86_64/src/msr.rs`
- `kernel/src/syscall/` (see "Module Layout" below)
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
1. Uses `swapgs` and switches to the current task's kernel stack (tracked via `TSS.rsp0`).
2. Saves user RSP, RCX (return RIP), and R11 (return RFLAGS) plus syscall arguments.
3. Calls `syscall_handler` with a pointer to the `SyscallFrame` on the kernel stack.
4. Restores registers and returns via `swapgs` + `sysretq`.

Limitations / Simplifications
- No SMEP/SMAP toggling yet.
- User-pointer validation is baseline only (range + mapping checks); no hardened fault-recovery path yet.

**INT 0x80 Path**
For initial user-mode bring-up, a user-accessible `int 0x80` gate is installed in the IDT. The handler uses the `InterruptFrame` to extract syscall arguments and write the return value to `RAX`.

Limitations / Simplifications
- The `int 0x80` path shares the same Linux-numbered dispatcher as the SYSCALL path and supports the current baseline subset:
  - `SYS_OPEN`, `SYS_OPENAT`, `SYS_MKDIRAT`, `SYS_CLOSE`, `SYS_READ`, `SYS_WRITE`, `SYS_LSEEK`
  - `SYS_WRITEV`
  - `SYS_IOCTL` (console/pty tty baseline: `TIOCGWINSZ`, `TCGETS`, `TCSETS*`, `TIOCGPGRP`, `TIOCSPGRP`; framebuffer baseline: `FBIOGETINFO` on `/dev/fb0`)
  - `SYS_DUP`, `SYS_DUP2`, `SYS_DUP3`, `SYS_FCNTL` (baseline descriptor controls, `FD_CLOEXEC`, and `F_SETFL` `O_NONBLOCK` toggles)
  - `SYS_MMAP`, `SYS_MPROTECT`, `SYS_MUNMAP` (baseline userspace VM mapping path)
  - `SYS_ACCESS`, `SYS_FACCESSAT`, `SYS_FACCESSAT2` (baseline permission checks)
  - `SYS_RT_SIGACTION`, `SYS_RT_SIGPROCMASK`, `SYS_RT_SIGRETURN`, `SYS_KILL` (baseline signal path)
  - `SYS_SHMGET`, `SYS_SHMAT`, `SYS_SHMDT`, `SYS_SHMCTL` (baseline System V shared memory)
  - `SYS_UMASK`
  - `SYS_STAT`, `SYS_LSTAT`, `SYS_FSTAT`, `SYS_NEWFSTATAT`, `SYS_GETDENTS64`, `SYS_UNLINKAT`, `SYS_RENAMEAT`
  - `SYS_CHDIR`, `SYS_FCHDIR`, `SYS_GETCWD`
  - `SYS_GETUID`, `SYS_GETEUID`, `SYS_GETGID`, `SYS_GETEGID`
  - `SYS_UNAME`
  - `SYS_GETTIMEOFDAY`, `SYS_CLOCK_GETTIME`, `SYS_NANOSLEEP`
  - `SYS_PIPE`, `SYS_PIPE2`, `SYS_FUTEX`
  - `SYS_EXECVE` (image replacement with caller-provided `argv`/`envp`)
  - `SYS_ARCH_PRCTL` (baseline `ARCH_SET_FS`/`ARCH_GET_FS`)
  - `SYS_CLONE` (baseline fork-compatible subset)
  - `SYS_FORK` (trap-frame-based baseline child image clone)
  - `SYS_VFORK` (baseline fork-compatible path)
  - `SYS_WAIT4` (baseline zombie reaping path)
  - `SYS_EXIT`, `SYS_EXIT_GROUP`
  - `SYS_YIELD`
  - `SYS_GETPID`, `SYS_GETPPID`, `SYS_GETTID`
  - `SYS_SETPGID`, `SYS_SETSID`, `SYS_GETSID`
  - `SYS_SET_TID_ADDRESS` (stores per-task clear-tid pointer)
  - `SYS_BRK`
  - `SYS_MEMFD_CREATE` (tmpfs-backed shared-buffer fd create baseline)
  - `SYS_EPOLL_CREATE1`, `SYS_EPOLL_CTL`, `SYS_EPOLL_WAIT` (baseline level-triggered epoll)
- `int 0x80` is retained as a compatibility path for current userspace bring-up and tooling.

**Syscall Handler**
`kernel/src/syscall/mod.rs` implements a table-driven dispatcher shared by both the SYSCALL and int 0x80 paths. Arguments are normalized into a small `SyscallArgs` struct, syscall entries carry mode metadata (any task vs user task), unknown syscall logs are rate-limited, and return values use Linux-style negative errno.

Current implemented handlers:
- `open/openat/close/read/write/writev/lseek` via baseline VFS descriptors (tmpfs + virtual `devfs`/`procfs`)
- `dup/dup2/dup3/fcntl` baseline descriptor operations (`F_GETFD`, `F_SETFD`, `F_GETFL`, `F_SETFL`, `F_DUPFD`, `F_DUPFD_CLOEXEC`) with `FD_CLOEXEC` and runtime `O_NONBLOCK` updates on sockets/pipes/PTYs
- `memfd_create(name, flags)` baseline tmpfs-backed anonymous-buffer fd creation (`MFD_CLOEXEC` supported; sealing flags currently rejected) with per-task shared-buffer object caps
- `mmap/mprotect/munmap` for baseline userspace mapping management (anonymous/private plus file-backed private/shared mappings from VFS fds, fixed/fixed-noreplace handling, per-task VMA metadata, lazy page-fault mapping, transactional VMA split/update handling, page-level protection updates, and per-task mapped-page caps)
- `msync` baseline shared-file mapping writeback path (range walk + page-cache dirty writeback + backing ext2 sync/flush)
- `kill`, `rt_sigaction`, `rt_sigprocmask`, `rt_sigreturn` for baseline signal control and user-handler return
- `pipe/pipe2` via in-kernel anonymous pipe state and per-task fd tables (`pipe2` supports `O_CLOEXEC` and `O_NONBLOCK`)
- `epoll_create1`/`epoll_ctl`/`epoll_wait` baseline level-triggered event-loop support (`ADD`/`MOD`/`DEL`, timeout, `-EINTR` on pending unblocked signals while waiting, per-owner watch caps, and owner-watch teardown cleanup on task exit)
- `epoll_pwait` null-sigmask compatibility alias for musl/libc `epoll_wait` call paths
- socket baseline:
  - `socket`/`bind`/`listen`/`accept`/`accept4`/`connect`/`shutdown`
  - `sendto`/`recvfrom`
  - `getsockname`/`getpeername`
  - `setsockopt`/`getsockopt` (stub/default behavior for common options)
  - `socketpair` baseline for `AF_UNIX` stream pairs
  - `sendmsg`/`recvmsg` baseline payload path for `AF_INET` and `AF_UNIX`
  - `SCM_RIGHTS` ancillary fd passing over `AF_UNIX` stream sockets
- `futex` (`WAIT`/`WAKE` baseline ops only)
- `shmget/shmat/shmdt/shmctl(IPC_RMID)` via baseline in-kernel System V shared-memory segments
- `access/faccessat/faccessat2` baseline path existence + permission checks
- `umask(mask)` baseline process umask tracking (used by userspace runtime bring-up)
- `stat`/`lstat`/`fstat`/`newfstatat` baseline file metadata paths (legacy + modern entry points)
- `linkat` baseline hard-link creation for regular files (ext2-backed hard links; tmpfs baseline fallback)
- `symlinkat` baseline symlink inode creation with target payload storage
- `readlinkat` baseline `/proc/self/exe` handling plus non-symlink `-EINVAL` behavior
- `utimensat` baseline atime/mtime updates with `UTIME_NOW`/`UTIME_OMIT`
- `copy_file_range` baseline buffered VFS-to-VFS copy path
- `execve(path, argv, envp)` with in-kernel argument/environment copy, VFS-backed ELF load, and direct frame handoff to the new image entry
- `fork()` with baseline address-space clone + task metadata/fd table copy (no COW yet)
- `clone(flags, child_stack, ...)` with a narrow fork-compatible subset (`SIGCHLD`-exit signal, optional child stack, and optional `CLONE_VM|CLONE_VFORK` compatibility flags without full Linux shared-VM semantics)
- `vfork()` as a fork-compatible compatibility path (no parent suspension or shared-VM semantics yet)
- `wait4(pid, status, options, rusage)` with baseline zombie-child reaping (`pid=-1` and specific positive `pid`, `WNOHANG`; `rusage` currently unimplemented)
- `ioctl(fd, request, argp)` with baseline tty subset (`TIOCGWINSZ`, `TCGETS`, `TCSETS*`, `TIOCGPGRP`, `TIOCSPGRP`) plus framebuffer metadata query (`FBIOGETINFO`) on `/dev/fb0`
- `setpgid(pid, pgid)`, `setsid()`, `getsid(pid)` with baseline process-group/session tracking
- `arch_prctl(ARCH_SET_FS/ARCH_GET_FS)` with per-task FS-base tracking and scheduler restore on context switch
- `exit(status)` and `exit_group(status)` with task-teardown cleanup of open fds/epoll watches/shared mappings before zombie state
- `sched_yield()`
- `getpid()`, `getppid()`, `gettid()`
- `getuid()`, `geteuid()`, `getgid()`, `getegid()`
- `uname(buf)` baseline utsname fields (`sysname`, `nodename`, `release`, `version`, `machine`, `domainname`)
- `chdir()`, `fchdir()`, `getcwd()` (baseline cwd-inode path reconstruction)
- `gettimeofday()`, `clock_gettime()`, `nanosleep()` (PIT tick based)
- `set_tid_address(ptr)` (clear-tid pointer tracking; best-effort zeroing + wake on exit)
- `brk(addr)` with per-task program-break metadata and page mapping/unmapping

Framebuffer ioctl contract (`/dev/fb0`):
- `FBIOGETINFO` (`0x5001`) writes a `FramebufferDeviceInfo` payload to user memory:
  `width`, `height`, `pitch`, `bpp`, `format`, `map_size`.
- Current format value is fixed to `XRGB8888` (`format = 1`).
- `/dev/fb0` also supports direct `mmap` with these Phase 3 baseline rules:
  - `MAP_SHARED` only
  - `PROT_READ` allowed; `PROT_WRITE` requires the fd to be writable
  - `PROT_EXEC` is rejected
  - `offset` must be page-aligned and stay within the framebuffer mapping window
- Error returns:
  - `-EFAULT` for null/invalid output pointer
  - `-ENXIO` when no framebuffer was successfully probed at boot
  - `-ENOTTY` for unsupported ioctl requests

Input event device contract (`/dev/input/event0`, `/dev/input/event1`):
- `/dev/input/event0` is the keyboard event queue; `/dev/input/event1` is the
  pointer event queue.
- `read()` returns packed 24-byte records matching this baseline layout:
  `tv_sec: i64`, `tv_usec: i64`, `type: u16`, `code: u16`, `value: i32`.
- Timestamps come from monotonic PIT uptime and currently have millisecond source
  precision expressed as microseconds.
- Keyboard uses `EV_KEY` + `SYN_REPORT`; pointer uses `EV_REL` (`REL_X`, `REL_Y`)
  for PS/2 motion, `EV_ABS` (`ABS_X`, `ABS_Y`) for USB-tablet absolute reports,
  plus `EV_KEY` button transitions.
- Read/poll behavior:
  - `read` requires room for at least one full record; shorter buffers return `-EINVAL`
  - empty nonblocking reads return `-EAGAIN`
  - readiness is exposed through `poll`/`ppoll`/`select`/`epoll` as `POLLIN`
- Overflow policy:
  - if a whole event packet will not fit, that packet is dropped
  - the next successfully queued packet is prefixed with `EV_SYN`/`SYN_DROPPED`
    so userspace can resynchronize
- Writes are unsupported and currently return `-EINVAL`.

Limitations / Simplifications
- Most Linux syscall numbers are still unimplemented and return `-ENOSYS`.
- VFS integration is baseline-only: `openat` supports absolute paths plus relative lookups from `AT_FDCWD` or a directory fd, but there is no mount namespace/chroot/path-root isolation.
- `lstat` currently matches `stat` behavior; path resolution does not yet follow symlinks for final-component operations.
- `devfs`/`procfs` are minimal virtual backends (`/dev/null`, `/dev/zero`, `/dev/console`, `/dev/tty`, `/dev/ptmx`, `/dev/pts/*`, `/dev/fb0`, `/dev/input/event{0,1}`, `/proc/{uptime,meminfo,mounts,version}`) and are not yet full Linux-compatible implementations.
- Console I/O is still simplified: stdio descriptors are pre-seeded for user tasks, console writes go through the serial path (mirrored to framebuffer text when available), and console reads use a canonical line-buffered TTY path backed by keyboard input (`read` blocks until a line is committed with Enter).
- TTY behavior is still minimal: canonical mode only, no pty support, and `ioctl` support is currently a narrow console subset with mostly static responses/defaults.
- Input-event coverage is still baseline-only: fixed device numbering (`event0`
  keyboard, `event1` pointer), no `ioctl(EVIOC*)` capability queries yet, no key
  repeat policy, and no multi-seat/hotplug routing.
- `memfd_create` is currently a pragmatic baseline: objects are created as controlled tmpfs files under an internal namespace, `MFD_CLOEXEC` is honored, `MFD_ALLOW_SEALING` returns `-ENOSYS`, full Linux sealing/fcntl semantics are not implemented yet, and per-task shared-buffer object count is capped (`16`).
- `fcntl(F_SETFL)` currently supports runtime `O_NONBLOCK` toggles for sockets/pipes/PTYs and `O_APPEND` on VFS fds, but full Linux open-file-description sharing semantics remain simplified.
- Socket coverage is still partial: `AF_INET` remains the primary implemented family, and `AF_UNIX` now supports stream `socket()`/`socketpair()` plus pathname `bind`/`connect`/`listen`/`accept`. Unix sockets still do not support abstract namespace addresses.
- `epoll` is baseline-only: level-triggered behavior is implemented for existing pollable fd kinds, and `epoll_pwait` currently supports only the common null-sigmask compatibility path used by libc wrappers. Advanced Linux semantics (temporary signal-mask swap for `epoll_pwait*`, `EPOLLET`/`EPOLLONESHOT`, nested epoll graphs, and large watch scalability) are deferred; per-task watch ownership is capped (`64`) as a hardening guard.
- AF_UNIX pathname binds currently reserve a VFS path entry and reject conflicts at bind time; abstract namespace socket addresses are still deferred.
- Message-based socket compatibility is still partial: `sendmsg`/`recvmsg` support payload iovec handling and baseline `SCM_RIGHTS`, but `SCM_CREDENTIALS` and full Linux ancillary edge semantics are not yet complete.
- Process-group/session semantics are currently minimal: no full job-control signal semantics yet, and no controlling-tty foreground group state beyond basic group-id validation for `TIOCSPGRP`.
- `clone`/`vfork` support is intentionally narrow: no `CLONE_THREAD`/TLS/tid-pointer semantics, no thread-group model, and no true Linux `vfork` parent-blocking/shared-address-space behavior.
- VM syscalls are still simplified: `mmap` now supports `MAP_PRIVATE` (anonymous + file-backed), baseline file-backed `MAP_SHARED` mappings including writable shared mappings with software dirty tracking and `msync`/`sync`/`fsync` writeback hooks, and direct shared `/dev/fb0` framebuffer mappings for GUI bring-up. User-page allocation now does best-effort reclaim by unmapping reclaim-safe file-backed pages and evicting unmapped VM page-cache pages before retrying allocation. There is still no swap-backed reclaim for anonymous/private-writable pages, no copy-on-write fork optimization, and no advanced display-buffer ownership/fencing model; per-task mapped VMA pages are capped (`8192`, currently 32 MiB virtual coverage).
- User-mode page faults now resolve non-present `mmap` pages on demand; invalid faults are handled as fatal user faults (SIGSEGV-style exit path), with no `siginfo_t` fault payload/ucontext delivery yet.
- VM mappings are currently constrained to a dedicated high user range managed by the kernel mmap allocator (used to avoid touching legacy low-memory bootstrap mappings).
- `brk` still uses simple contiguous growth/shrink and does not yet integrate with full userspace allocator policies (no overcommit accounting, no guard-page policy).
- `futex` support is intentionally narrow: no timeout handling, no `*_PRIVATE`/bitset/requeue/PI variants, and no robust-list support yet.
- Shared memory support is intentionally narrow: fixed-size segment/attachment tables, `IPC_RMID` only for `shmctl`, no permission enforcement, and no inheritance semantics across `fork`/`exec` yet.
- `execve` is intentionally narrow: `FD_CLOEXEC` teardown is now wired, but there is still no shebang/script loading, bounded `argv`/`envp` vector sizes, and no full Linux signal-reset semantics.
- `arch_prctl` is intentionally narrow: only `ARCH_SET_FS` and `ARCH_GET_FS` are implemented; GS-base operations return `-EINVAL`.
- `access`/`faccessat` semantics are baseline-only (no ACLs/capabilities, simplified effective-vs-real ID behavior).
- Time-related syscalls currently derive from PIT uptime ticks; there is no realtime clock source or wall-clock synchronization.
- `getcwd` uses bounded in-kernel directory traversal and does not yet have dedicated VFS reverse-path metadata.
- Signal support remains simplified: delivery is syscall-boundary based and user handlers currently require `SA_RESTORER`; `sigaltstack`, queued RT signal payloads/`siginfo_t`, and full Linux ucontext semantics are not implemented yet.
- Several Toybox-relevant syscalls are still stubbed and return `-ENOSYS` pending VFS/metadata support (`fchmodat`, `fchownat`, `mknodat`, plus compatibility wrappers `utimes`/`futimesat`).

**Module Layout**
`kernel/src/syscall/` is split by area. `mod.rs` holds the syscall numbers and constants, the shared Linux ABI structs, per-task state tables, the `SYSCALL_TABLE`, errno conversion, dispatch, and the SYSCALL/int 0x80 entry handlers. Handlers and their helpers live in submodules:
- `uaccess.rs`: copying data to and from user memory
- `vm.rs`: `mmap`/`munmap`/`mprotect`/`msync`/`brk`, user page faults, and the file-backed page cache
- `fd.rs`: fd table helpers, `close`/`dup*`/`fcntl`, pipes, `memfd_create`
- `file.rs`: path resolution, `open*`, metadata (`stat*`, `statfs`, `chmod`/`chown`, times), directory operations, `getdents64`, `truncate`/`sync`
- `io.rs`: `read`/`write`/`readv`/`writev`/`pread64`/`pwrite64`/`lseek`/`copy_file_range`
- `process.rs`: `fork`/`vfork`/`clone`, `execve`, `wait4`, `exit*`, ids and process groups, `arch_prctl`, `futex`
- `signals.rs`: `kill`, `rt_sig*`, signal-frame install/restore (including FPU state), pending-signal delivery
- `ioctl.rs`: console, pty and framebuffer `ioctl`
- `poll.rs`: `select`/`poll`/`ppoll` and epoll
- `sockets.rs`: BSD sockets, `sendmsg`/`recvmsg`, `SCM_RIGHTS`
- `misc.rs`: time, `uname`, `getrandom`, `sysinfo`, System V shm
- `tests.rs`: host unit tests

Submodules import everything from `mod.rs` with `use super::*`, and `mod.rs` glob-imports each submodule, so items are `pub(super)` and visible across the whole `syscall` module.

Limitations / Simplifications
- The split is by file only; the submodules still share the global state in `mod.rs` and call each other freely, so there is no enforced layering between areas.
- `mod.rs` is still large because the syscall table and constants for every area live there.

**Integration**
`kernel/src/main.rs` calls `arch_syscall::init()` (MSR setup) and `syscall::init()` (dispatcher state init) during early boot.

**Status Notes**
- This is a baseline user-mode syscall path with staged Linux compatibility, not full Linux behavior coverage.
- The syscall fast path now uses per-task kernel stacks (`TSS.rsp0`) and `swapgs`.
