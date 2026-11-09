# IPC and Synchronization

This document describes the current baseline IPC/synchronization implementation wired into the kernel.

**Where It Lives**
- `kernel/src/sync.rs`
- `kernel/src/ipc.rs`
- `kernel/src/syscall/` (`signals.rs`, `fd.rs` for pipes, `misc.rs` for SysV shm, `sockets.rs` for `AF_UNIX`)
- `kernel/src/main.rs`
- `kernel/src/task.rs`

**Synchronization Primitives**
- `SpinLock<T>`: atomic compare/exchange lock with guard-based unlock.
- `Mutex<T>`: thin wrapper over `SpinLock<T>` for a stable higher-level API surface.
- `Semaphore`: atomic count with:
  - `try_down()` non-blocking acquire
  - `down()` looping acquire with scheduler yield
  - `up()` release

**IPC State**
- Global in-kernel IPC state protected by `SpinLock`:
  - Fixed-size pipe table (`IPC_MAX_PIPES`)
  - Fixed-size waiter table (`IPC_MAX_WAITERS`)
- Waiters are keyed by:
  - pipe-readable condition
  - pipe-writable condition
  - futex user address

**Pipes**
- `pipe_open()` allocates an anonymous pipe with one read end and one write end.
- `pipe_read()`:
  - returns available bytes from ring buffer
  - returns EOF when no writers remain
  - blocks when empty and writers still exist
- `pipe_write()`:
  - writes into ring buffer
  - returns `EPIPE` when no readers remain
  - blocks when buffer is full
- `pipe_close()` decrements read/write endpoint counts and wakes blocked peers.

**Futex**
- Every futex call takes a `space`: the caller's PML4 for `FUTEX_PRIVATE_FLAG` operations, `FUTEX_SPACE_GLOBAL` (0) otherwise. Waiters only match within their space, so private futexes of different processes at the same address are independent.
- `futex_wait(task, space, addr, deadline)` registers a waiter keyed by space and user address and blocks, optionally until a PIT-tick deadline (`ETIMEDOUT`). A signal interrupts the wait (`EINTR` when timed, `ERESTARTSYS` otherwise).
- `futex_wake(space, addr, count)` wakes up to `count` waiters for that address.
- `futex_requeue(space, addr, addr2, nr_wake, nr_requeue)` wakes some waiters and moves others to `addr2` in the same space.
- `remove_task_waiters(task)` removes stale waiters during task teardown.

**Shared Memory (System V Baseline)**
- Global in-kernel shared-memory table protected by `SpinLock`:
  - Fixed-size segment table (`SHM_MAX_SEGMENTS`, currently 16)
  - Per-segment physical-page list (up to `SHM_MAX_SIZE`, currently 1 MiB)
- Segment lifecycle:
  - `shm_get(key, size, flags)` creates/looks up a segment (`IPC_CREAT`/`IPC_EXCL` semantics)
  - `shm_ctl(shmid, IPC_RMID)` marks a segment for deletion
  - Segment pages are reclaimed when `IPC_RMID` is set and refcount reaches zero
- Per-task attachment tracking:
  - Fixed-size per-task attachment table (`MAX_TASK_SHM_ATTACH`, currently 8)
  - `shm_attach(task, shmid, addr, flags)` maps the segment into the task address space
  - `shm_detach(task, addr)` unmaps and decrements refcount
  - `cleanup_task(task)` detaches all remaining mappings during task exit

**Shared Buffer FDs (`memfd_create`-style baseline)**
- `memfd_create(name, flags)` is now available for compositor/client buffer sharing.
- Each memfd is a tmpfs inode in an internal namespace (`/temp/.memfd/...`), returned as a VFS fd, but its contents live in a page-backed shmem object (`kernel/src/shmem.rs`) rather than in tmpfs, so objects can grow to 256 MiB. Pages are allocated zero-filled on first touch.
- `read`/`write`/`pread64`/`pwrite64`/`lseek`/`fstat`/`ftruncate`/`fallocate` and `MAP_SHARED` faults on a memfd all go through the shmem object (`vfs_*` wrappers in `kernel/src/syscall/io.rs`). Shared mappings map the object's pages directly, each holding a PMM share reference, so pages stay valid after the fd is closed (the libwayland server closes pool fds right after `mmap`).
- `memfd_create` unlinks the tmpfs file immediately, as Linux memfds have no name. The inode, and with it the shmem object, then lives exactly as long as VFS open references hold it: one per fd (taken in `fd_alloc` and `fd_object_retain`) and one per mapped inode (`vm_sync_inode_pins`). The last release frees the inode and `vfs_inode_freed` destroys the shmem object.
- Supported flags:
  - `MFD_CLOEXEC` (maps to `FD_CLOEXEC` on returned descriptor)
  - `MFD_ALLOW_SEALING` (enables `F_ADD_SEALS`; without it the object starts with `F_SEAL_SEAL`)
- Seals (`fcntl(F_ADD_SEALS / F_GET_SEALS)`, implemented in `kernel/src/syscall/fileext.rs`):
  - `F_SEAL_SHRINK` / `F_SEAL_GROW` are enforced on `ftruncate`, `fallocate`, and writes that would extend the file
  - `F_SEAL_WRITE` / `F_SEAL_FUTURE_WRITE` are enforced on `write` and on new writable `MAP_SHARED` mappings
  - `F_SEAL_SEAL` makes the seal set immutable (`-EPERM` on further adds)
  - `F_GET_SEALS` / `F_ADD_SEALS` on non-memfd descriptors return `-EINVAL`
- Descriptor lifecycle follows existing fd-table semantics (`dup*`, `fork`, `exec` close-on-exec handling, close).
- `mmap` now supports baseline writable shared mappings (`MAP_SHARED` + `PROT_WRITE`) on these fds with shared page-cache backing, so peer mappings observe writes predictably across tasks.
- Shared-buffer objects are now capped per task (`16` unique memfd inodes) as a resource-hardening guard.
- Task VMA coverage is now capped (`65536` pages, currently 256 MiB virtual coverage) to bound shared-buffer/event-loop mapping pressure.

**Event Loop FDs (`epoll` baseline)**
- `epoll_create1(flags)` supports baseline instance creation with `EPOLL_CLOEXEC`.
- `epoll_ctl(epfd, op, fd, event)` supports `EPOLL_CTL_ADD`/`MOD`/`DEL` with duplicate/add/remove validation.
- `epoll_wait(epfd, events, maxevents, timeout)` supports timeout waiting, level-triggered readiness delivery, and returns `-EINTR` when interrupted by pending unblocked signals.
- `EPOLLET` (edge-triggered: a readiness bit is reported when it becomes set) and `EPOLLONESHOT` (silent after one report until `EPOLL_CTL_MOD`) are supported; `EPOLLRDHUP`, `EPOLLWAKEUP` and `EPOLLEXCLUSIVE` are accepted. dbus-daemon uses `EPOLLET` for watches it has disabled.
- Per-task epoll watch ownership is capped (`64` watches across all instances), and task teardown now drops owner watches defensively.
- `fork` gives the child a copy of each of the parent's watches, so an inherited epoll fd keeps working after the parent exits (dbus-daemon `--fork` sets up its epoll set before forking). A watch and its inherited copies refer to the same fd on the same object and are reported once.

**Event FDs (`eventfd`, `timerfd`, `signalfd`)**
- Object state lives in `kernel/src/evfd.rs` (refcounted tables: 32 eventfds, 32 timerfds, 16 signalfds); syscalls and read/write/poll hooks are in `kernel/src/syscall/eventfds.rs`. Each is its own fd kind (`USER_FD_KIND_EVENTFD`/`_TIMERFD`/`_SIGNALFD`) and works with `poll`/`select`/`epoll`, `dup*`, `fork`, `fcntl(F_SETFL, O_NONBLOCK)`, and close-on-exec.
- `eventfd`/`eventfd2(initval, flags)`: 64-bit counter. `read` returns and clears it (or decrements by one with `EFD_SEMAPHORE`), `write` adds to it. Readable while non-zero; writable while below `0xffff_ffff_ffff_fffe`. Blocks unless `EFD_NONBLOCK`/`O_NONBLOCK`.
- `timerfd_create(clock, flags)` accepts `CLOCK_REALTIME`, `CLOCK_MONOTONIC`, and `CLOCK_BOOTTIME` (all backed by the same boot-relative PIT clock that `clock_gettime` uses). `timerfd_settime` supports relative and `TFD_TIMER_ABSTIME` deadlines, intervals, disarm, and returns the old setting; `timerfd_gettime` reports remaining time. `read` returns the number of expirations since the last read, counting missed periods.
- `signalfd`/`signalfd4(fd, mask, 8, flags)`: creates a signalfd or updates an existing one's mask. Reading dequeues pending signals in the mask from the calling task and returns one 128-byte `signalfd_siginfo` per signal (only `ssi_signo` is filled). `SIGKILL`/`SIGSTOP` are never reported. As on Linux, callers block the signals first so they stay pending instead of being delivered.

**Signals (Baseline + User Handlers)**
- Per-task signal state:
  - pending mask (`sig_pending`)
  - blocked mask (`signal_blocked`)
  - per-signal action table (`handler`, `flags`, `mask`, shared restorer)
- Signal control syscalls:
  - `kill(pid, sig)` for directed signal delivery
  - `rt_sigaction` for action read/write (`SIG_DFL`, `SIG_IGN`, or user handler)
  - `rt_sigprocmask` for block/unblock/set mask operations
  - `rt_sigreturn` for restoring pre-handler user context
- Delivery model:
  - pending signals are consumed at syscall-return boundaries
  - default actions currently implemented as terminate/ignore policy
  - user handlers are entered by writing a kernel signal frame on user stack and jumping to the configured handler with `RDI=signum`
  - user return is mediated through `SA_RESTORER`; `rt_sigreturn` restores saved registers and signal mask
  - the signal frame also carries the interrupted x87/SSE state (`FXSAVE` image); handlers start with the default FPU state and `rt_sigreturn` restores the saved image, with reserved MXCSR bits masked off (see `docs/tasking.md`)
  - `SIGPIPE` is generated on broken-pipe write paths (`EPIPE`)

**Unix-Domain Socket Ancillary FD Passing (Baseline)**
- `AF_UNIX` stream sockets support descriptor transfer through
  `sendmsg`/`recvmsg` ancillary control data using `SOL_SOCKET` + `SCM_RIGHTS`.
- Pathname binds now reserve a VFS namespace entry and report conflict if the
  pathname is already present.
- Receive path installs transferred descriptors into the receiving task fd table
  as new fd numbers, preserving descriptor kind/inode/offset semantics.
- Supported transferable descriptor kinds:
  - VFS regular descriptors
  - pipe descriptors
  - socket descriptors
  - PTY master/slave descriptors
- Transfer lifetime:
  - sender-side references are retained at send time
  - queued rights are released on truncation/drop/close paths
  - transferred resources remain valid if sender closes after successful send
  - closing a bound socket drops its binding (later `connect`s get `ECONNREFUSED`) but, as on Linux, leaves the pathname node until someone `unlink`s it. A binding is only reachable while its path still names the node `bind` created, so `unlink` + re-`bind` of a stale path works (libwayland relies on both).
- Baseline truncation behavior:
  - if receive control buffer is missing/too short, pending rights are dropped
    and `MSG_CTRUNC` is reported
  - unsupported ancillary types return `-EOPNOTSUPP`

**Syscall Integration**
- Added syscall numbers and handlers:
  - `rt_sigaction` (`13`)
  - `rt_sigprocmask` (`14`)
  - `rt_sigreturn` (`15`)
  - `pipe` (`22`)
  - `shmget` (`29`)
  - `shmat` (`30`)
  - `shmctl` (`31`, `IPC_RMID` only)
  - `kill` (`62`)
  - `shmdt` (`67`)
  - `futex` (`202`)
  - `pipe2` (`293`)
- FD wiring:
  - new task fd kind for pipes (`USER_FD_KIND_PIPE`)
  - `read`/`write` route pipe fds to IPC backend
  - `lseek` on pipes returns `-ESPIPE`
  - `close` tears down pipe endpoints
- Exit behavior:
  - `clear_child_tid` now does best-effort zero + `futex_wake(FUTEX_SPACE_GLOBAL, addr, 1)` (a shared wake, as on Linux)
  - task exit removes pending IPC waiters for the exiting task, detaches shared-memory mappings, drops owner epoll watches, and tears down user mappings/address-space backing

Limitations / Simplifications
- Single-core assumptions still apply; synchronization is designed for current non-SMP kernel flow.
- IPC capacities are static (`IPC_MAX_PIPES`, `IPC_MAX_WAITERS`, fixed pipe buffer size).
- Pipe semantics are baseline only: `O_NONBLOCK` plus `poll`/`select`/`epoll` integration are now supported, but there is still no `splice`/`tee`.
- Futex support covers `WAIT`/`WAKE` (with timeouts), `WAIT_BITSET`/`WAKE_BITSET` (bitsets not used for filtering) and `REQUEUE`/`CMP_REQUEUE`; there is no `WAKE_OP`, priority inheritance or robust-list handling. See `docs/process_model.md`.
- Shared memory is a baseline System V subset only: no permission checks, no `IPC_STAT`/`IPC_SET`, no `SHM_REMAP`/`SHM_EXEC`. Attachments are inherited by `fork`; `execve` does not detach them yet.
- `memfd_create` is baseline-only: at most 64 shmem objects system-wide, 256 MiB per object, no hugepage variants, and per-task shared-buffer object caps (`16`) are enforced. Shrinking a memfd drops the object's pages beyond the new size, but pages already mapped stay mapped (Linux would raise `SIGBUS` on access beyond EOF); faults beyond the current size fail.
- memfd seals are tracked per inode in a fixed 64-entry table. `F_SEAL_WRITE` does not check for existing writable mappings (Linux returns `-EBUSY`) and does not revoke them; it only blocks new ones and `write`. If the table is full, new memfds behave as if created without `MFD_ALLOW_SEALING`.
- Event fds are baseline-only: object tables are fixed-size, timer resolution is the 10 ms PIT tick, `TFD_TIMER_CANCEL_ON_SET` is accepted but ignored (there is no settable realtime clock), blocking reads busy-yield instead of sleeping on a wait queue, `signalfd_siginfo` carries only the signal number, and eventfd/timerfd/signalfd descriptors cannot be passed over `SCM_RIGHTS` yet.
- Signal handling: delivery happens at syscall-return boundaries and requires `SA_RESTORER`. Handlers get a Linux `rt_sigframe` (`siginfo_t`, `ucontext_t`), signals 1..=64 are supported, and blocking waits are interrupted with `SA_RESTART`-aware restart. There are no alternate signal stacks or queued real-time payloads. See `docs/process_model.md`.
- Unix ancillary coverage is baseline-only:
  - `SCM_RIGHTS` is implemented
  - `SCM_CREDENTIALS`: a sent `struct ucred` is validated like Linux (only root may claim another pid/uid/gid); with `SO_PASSCRED` set (inherited by accepted sockets) `recvmsg` returns the peer's credentials as an `SCM_CREDENTIALS` message, ahead of any `SCM_RIGHTS`. They are the peer's credentials at connection time, not the per-message sender's, which differs only when another process writes to the same socket (after `fork`).
  - `SO_PEERSEC`, `SO_PEERGROUPS` and `SO_PEERPIDFD` fail with `ENOPROTOOPT` (no LSM, group lists or pidfds); other unknown socket options still read back as zero
  - `SO_PEERCRED` reports the peer's pid/uid/gid captured when the connection formed (`socketpair`, or `connect` against a listener, which records the listening process's credentials at `listen`)
  - ancillary/data atomicity is simplified for stream sockets
  - Unix abstract-namespace addresses are not implemented
- `epoll` coverage is baseline-only:
  - `epoll_create1`/`epoll_ctl`/`epoll_wait`/`epoll_pwait` with level-triggered, `EPOLLET` and `EPOLLONESHOT` watches
  - `EPOLLET` is approximated by state changes: new data arriving while an fd stays readable does not produce a new event (Linux reports one)
  - `EPOLLRDHUP` is never reported on its own (only as `EPOLLHUP` on a full close)
  - watches are per owner process: after `fork` each side has its own copy, so `EPOLL_CTL_MOD`/`DEL` in one process does not affect the other's (on Linux the watch is shared); a copy that does not fit the instance (128 watches) is dropped with a log line
  - per-task watch ownership is capped (`64`) for hardening
- Socket tables are static: 256 sockets system-wide (`MAX_SOCKETS`, about 2.3 KiB each including the 2 KiB stream buffer) and 128 `AF_UNIX` pathname bindings. A bound socket's filesystem node is a regular file (`stat` does not report `S_IFSOCK`).
- No message-queue IPC primitives are implemented yet.
