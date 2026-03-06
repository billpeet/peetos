# IPC and Synchronization

This document describes the current baseline IPC/synchronization implementation wired into the kernel.

**Where It Lives**
- `kernel/src/sync.rs`
- `kernel/src/ipc.rs`
- `kernel/src/syscall.rs`
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
- `futex_wait(task, addr)` registers a waiter keyed by user address and blocks.
- `futex_wake(addr, count)` wakes up to `count` waiters for that address.
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
- Backing objects are created as controlled tmpfs regular files in an internal namespace (`/temp/.memfd/...`) and returned as VFS fds.
- Supported baseline flags:
  - `MFD_CLOEXEC` (maps to `FD_CLOEXEC` on returned descriptor)
- Unsupported baseline flags:
  - `MFD_ALLOW_SEALING` currently returns `-ENOSYS` (sealing semantics deferred)
- Descriptor lifecycle follows existing fd-table semantics (`dup*`, `fork`, `exec` close-on-exec handling, close).
- `mmap` now supports baseline writable shared mappings (`MAP_SHARED` + `PROT_WRITE`) on these fds with shared page-cache backing, so peer mappings observe writes predictably across tasks.
- Shared-buffer objects are now capped per task (`16` unique memfd inodes) as a resource-hardening guard.
- Task VMA coverage is now capped (`8192` pages, currently 32 MiB virtual coverage) to bound shared-buffer/event-loop mapping pressure.

**Event Loop FDs (`epoll` baseline)**
- `epoll_create1(flags)` supports baseline instance creation with `EPOLL_CLOEXEC`.
- `epoll_ctl(epfd, op, fd, event)` supports `EPOLL_CTL_ADD`/`MOD`/`DEL` with duplicate/add/remove validation.
- `epoll_wait(epfd, events, maxevents, timeout)` supports timeout waiting, level-triggered readiness delivery, and returns `-EINTR` when interrupted by pending unblocked signals.
- Per-task epoll watch ownership is capped (`64` watches across all instances), and task teardown now drops owner watches defensively.

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
  - bound pathname entries are released when the bound socket is closed
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
  - `clear_child_tid` now does best-effort zero + `futex_wake(addr, 1)`
  - task exit removes pending IPC waiters for the exiting task, detaches shared-memory mappings, drops owner epoll watches, and tears down user mappings/address-space backing

Limitations / Simplifications
- Single-core assumptions still apply; synchronization is designed for current non-SMP kernel flow.
- IPC capacities are static (`IPC_MAX_PIPES`, `IPC_MAX_WAITERS`, fixed pipe buffer size).
- Pipe semantics are baseline only: `O_NONBLOCK` plus `poll`/`select`/`epoll` integration are now supported, but there is still no `splice`/`tee`.
- Futex support is limited to `FUTEX_WAIT`/`FUTEX_WAKE` with no timeout, bitset, requeue, priority inheritance, or robust-list handling.
- Shared memory is a baseline System V subset only: no permission checks, no `IPC_STAT`/`IPC_SET`, no `SHM_REMAP`/`SHM_EXEC`, and no `fork`/`exec` inheritance semantics yet.
- `memfd_create` is baseline-only: tmpfs-backed internal-path objects, no sealing (`F_ADD_SEALS`/`F_GET_SEALS`) support yet, no hugepage variants, and per-task shared-buffer object caps (`16`) are enforced.
- Signal handling is baseline-grade only: delivery is limited to syscall-return boundaries and requires `SA_RESTORER`; no alternate signal stacks, queued realtime signal payloads, `siginfo_t`, or full ucontext semantics are implemented yet.
- Unix ancillary coverage is baseline-only:
  - `SCM_RIGHTS` is implemented, but `SCM_CREDENTIALS` is not
  - ancillary/data atomicity is simplified for stream sockets
  - Unix abstract-namespace addresses are not implemented
- `epoll` coverage is baseline-only:
  - level-triggered `epoll_create1`/`epoll_ctl`/`epoll_wait` is implemented
  - per-task watch ownership is capped (`64`) for hardening
  - advanced flags/variants (`EPOLLET`, `EPOLLONESHOT`, `epoll_pwait*`) are deferred
- No message-queue IPC primitives are implemented yet.
