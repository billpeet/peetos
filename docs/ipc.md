# IPC and Synchronization

This document describes the phase-6 baseline IPC/synchronization implementation currently wired into the kernel.

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
  - task exit removes pending IPC waiters for the exiting task and detaches shared-memory mappings

Limitations / Simplifications
- Single-core assumptions still apply; synchronization is designed for current non-SMP kernel flow.
- IPC capacities are static (`IPC_MAX_PIPES`, `IPC_MAX_WAITERS`, fixed pipe buffer size).
- Pipe semantics are baseline only: no `poll`/`epoll`, no `O_NONBLOCK`, and no `splice`/`tee`.
- Futex support is limited to `FUTEX_WAIT`/`FUTEX_WAKE` with no timeout, bitset, requeue, priority inheritance, or robust-list handling.
- Shared memory is a baseline System V subset only: no permission checks, no `IPC_STAT`/`IPC_SET`, no `SHM_REMAP`/`SHM_EXEC`, and no `fork`/`exec` inheritance semantics yet.
- Signal handling is baseline-grade only: delivery is limited to syscall-return boundaries and requires `SA_RESTORER`; no alternate signal stacks, queued realtime signal payloads, `siginfo_t`, or full ucontext semantics are implemented yet.
- No message-queue IPC primitives are implemented yet.
