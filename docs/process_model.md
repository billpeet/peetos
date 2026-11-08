# Process Model: Fork, Threads and Signals

This document covers how PeetOS creates processes and threads (copy-on-write
`fork` and `clone(CLONE_THREAD)` thread groups, enough for musl pthreads),
how signals reach them and interrupt blocking syscalls, futex waits, and how
threads and processes exit.

**Where It Lives**
- `kernel/src/memory/pmm.rs`: per-frame share counts (`page_share`, `page_is_shared`, refcount-aware `free_page`)
- `kernel/src/memory/vmm.rs`: `PTE_COW`, `clone_user_space`, `resolve_cow_in`, `cow_adjust_flags`
- `kernel/src/task.rs`: `tgid`, `group_leader`, `leader_of`, thread flags
- `kernel/src/syscall.rs`: `clone`/`fork` entry (`validate_clone_args`, `create_fork_child`, `create_thread_child`), exit paths, `wait4`, `kill`/`tkill`/`tgkill`, COW fault handling
- `kernel/src/signal.rs`: process-directed delivery (`send_to_process`), `must_abort_wait`
- `kernel/src/shm.rs`: shm inheritance across `fork` (`fork_attachments`)
- `kernel/src/ipc.rs`: futex waits with deadlines and requeue; `kernel/src/sched.rs`: `block_until`
- Test: `user/proc_model_smoke.c`, run by `make run-proc-smoke` (`tests/musl_ext2_proc_smoke.sh`)

## Copy-on-Write Fork

`fork`, `vfork` and non-thread `clone` give the child a new address space that
shares the parent's private pages instead of copying them:

1. `vmm::clone_user_space` walks the parent's user half. For each present page
   it bumps the frame's share count (`pmm::page_share`) and maps the same
   frame in the child. Writable pages lose the hardware write bit in **both**
   address spaces and gain the software `PTE_COW` bit (bit 9). Read-only pages
   are shared as they are. The parent's TLB is flushed afterwards.
2. A user write to a `PTE_COW` page faults (present + write).
   `try_handle_page_fault` calls `vmm::resolve_cow_in`:
   - if the frame is still shared, the page is copied into a new frame mapped
     writable, and the old frame's share count drops;
   - if this mapping is the last owner, the existing frame is made writable
     again in place.
3. `pmm::free_page` drops one reference and only releases a frame once no
   other mapping holds it, so every existing unmap/teardown path (`munmap`,
   `brk` shrink, `execve`, exit) is share-aware without changes.

The kernel reaches user memory through the physical map, which ignores the
hardware write bit. `copy_user_bytes` therefore breaks COW itself before every
write to user memory (`read()` into a buffer, signal frames, `clear_child_tid`,
`CLONE_CHILD_SETTID`). `mprotect` keeps pages that are still shared
write-protected (they become COW instead of writable).

Some mappings must stay shared after `fork` and are left out of the COW clone
(the `skip` predicate in `create_fork_child`):
- file-backed `MAP_SHARED` mappings and the `/dev/fb0` framebuffer: the child
  refaults them from the inherited mapping table;
- System V shm attachments: `shm::fork_attachments` maps the same segment
  pages into the child and takes a reference on each segment.

The serial log reports each fork as
`[VMM] Fork: N pages shared copy-on-write, M copied`; `M` is non-zero only for
frames the PMM cannot share (unmanaged memory or share-count overflow).

## Thread Groups

`clone` with `CLONE_THREAD` creates a new task in the caller's thread group.
Threads must share everything: `CLONE_VM | CLONE_FS | CLONE_FILES |
CLONE_SIGHAND | CLONE_THREAD` are required together, a child stack is
required, and no exit signal may be set. `CLONE_SYSVSEM` and `CLONE_DETACHED`
are accepted. This is exactly what musl's `pthread_create` passes.

Each task keeps its own `pid` (the thread id) and `tgid` (the process id: the
leader's `pid`). Process-wide state is stored only on the **thread group
leader**; `task::leader_of(task)` returns it, and all accesses to these fields
go through it:
- file descriptor table and cwd
- `brk`, mmap region table, shm attachments
- signal actions (handlers, flags, masks, restorer)
- uid/gid, pgid/sid/controlling tty, parent pid
- exec metadata (entry, auxv, program headers)

Per-thread state stays on each task: kernel stack and saved registers,
`signal_blocked` and pending signals, FS base (TLS), FPU/SSE state (copied
from the creating thread), `clear_child_tid`, and
scheduling state. Every thread holds a copy of the shared `pml4_phys`.

The tid flags work for threads and forks:
- `CLONE_SETTLS`: the new task's FS base is the `tls` argument.
- `CLONE_PARENT_SETTID`: the new tid is written to `ptid` in the parent.
- `CLONE_CHILD_SETTID`: the new tid is written to `ctid` in the child.
- `CLONE_CHILD_CLEARTID`: on thread exit the kernel zeroes `ctid` and wakes a
  futex waiter there (`pthread_join`).

Identity syscalls: `getpid()` returns the `tgid`, `gettid()` the task id, and
`getppid()` the leader's parent. A process created by any thread has the
process (not the thread) as its parent.

## Signals

- `kill(pid, sig)` is process-directed: the signal is queued on the leader if
  it is alive and not blocking it, otherwise on the first live thread that is
  not blocking it.
- `tkill(tid, sig)` and `tgkill(tgid, tid, sig)` target one thread (musl uses
  them for `raise()` and `pthread_kill()`).
- Terminal and job-control signals sent to a process group are delivered once
  per process, not once per thread.
- A default "terminate" action, or a fatal fault in any thread, ends the whole
  process.
- Signals 1..=64 are supported, including the real-time range musl uses
  internally (32 for timers, 33 for thread cancellation). Masks use the Linux
  `sigset_t` layout (signal N is bit N-1). Real-time signals are not queued:
  like classic signals, at most one instance of each is pending.
- Handlers get a Linux-layout `rt_sigframe`: `siginfo_t` (`si_signo`,
  `si_code` = `SI_USER` for `kill`, `SI_TKILL` for `tkill`/`tgkill`,
  `SI_KERNEL` otherwise, and `si_pid`), and a `ucontext_t` with the
  interrupted registers, signal mask and FXSAVE image. Changes a handler makes
  to the `ucontext_t` (registers, `uc_sigmask`, FPU state) take effect at
  `rt_sigreturn`; musl's `pthread_cancel` relies on this.
- `SA_SIGINFO`, `SA_RESTART`, `SA_NODEFER` and `SA_RESETHAND` are honored.
- Ignored signals that are not blocked are discarded when sent, as on Linux,
  so they never wake or interrupt anything.
- `setitimer(ITIMER_REAL)`/`alarm` arm a per-process timer stored on the
  thread-group leader (`itimer_real_deadline`/`itimer_real_interval`, in
  ticks). The timer tick sends `SIGALRM` to the process when it expires and
  reloads periodic timers. Timers survive `execve` (including a non-leader
  `execve` taking over the leader) and are cleared in `fork` children.

## Child Exit Notification

When the last thread of a process exits, its parent process is sent
`SIGCHLD` (`notify_parent_of_exit` in `kernel/src/syscall/process.rs`).
`clone` only accepts `SIGCHLD` as the exit signal for new processes, so no
other exit signal exists. With the default disposition the signal is
discarded at send; a parent that installs a handler, or blocks `SIGCHLD` and
reads it through a signalfd (as the `peetwl` compositor does), sees it. Before
this, `SIGCHLD` was never raised and parents could only poll with `wait4`.

## Interrupted Syscalls

A signal that will run a handler or terminate the process interrupts blocking
waits (pipes, tty/pty, input devices, sockets including TCP/UDP/ICMP, futex,
`wait4`, `nanosleep`, `poll`/`select`/`epoll_wait`). Blocked and ignored
signals do not. The wait returns the kernel-internal `ERESTARTSYS`, and on the
way back to user mode:
- if a handler runs and has `SA_RESTART`, or no handler runs, the syscall is
  restarted (RIP is rewound over the 2-byte `syscall`/`int 0x80` and RAX gets
  the syscall number back);
- if a handler without `SA_RESTART` runs, the syscall fails with `EINTR`.

Timed waits (`nanosleep`, timed futex waits, `poll`/`select`/`epoll_wait`)
fail with `EINTR` instead of restarting; `nanosleep` reports the time left.
Pipe writes that already moved data return the partial count.

## Futex

- `FUTEX_WAIT` takes an optional relative timeout and `FUTEX_WAIT_BITSET` an
  absolute one (CLOCK_MONOTONIC and CLOCK_REALTIME share the PIT tick clock).
  An expired wait fails with `ETIMEDOUT`; the kernel sleeps the task with a
  deadline rather than polling.
- `FUTEX_WAKE_BITSET` wakes like `FUTEX_WAKE`. Bitsets must be non-zero but do
  not filter wakeups.
- `FUTEX_REQUEUE` and `FUTEX_CMP_REQUEUE` wake some waiters and move others to
  a second futex word without waking them.
- `FUTEX_PRIVATE_FLAG` and `FUTEX_CLOCK_REALTIME` are accepted. Other
  operations (`WAKE_OP`, PI futexes) return `-ENOSYS`.

## Exit and Reaping

- `exit` (`SYS_exit`) ends only the calling thread. When it is the last live
  thread, the process ends with that status.
- `exit_group`, fatal signals and fatal faults end the process: the leader is
  marked `TASK_FLAG_GROUP_EXIT` with the status, then every other thread is
  told to exit:
  - a thread preempted in user mode is terminated on the spot;
  - a thread inside the kernel is flagged `TASK_FLAG_EXITING` and woken. It
    leaves on its next return towards user mode. Pipe, tty, pty, input,
    `wait4`, `poll` and `select` waits give up early for such threads (and for
    a pending `SIGKILL`) via `signal::must_abort_wait`.
- When the last thread exits, the process's shared resources are released
  once: mmap regions, fds, epoll watches, shm attachments and the address
  space.
- An exiting task never returns to user mode: after becoming a zombie it
  yields, and the scheduler never runs a zombie again.
- `wait4` reports a process killed by a signal (default "terminate" action,
  or a fatal fault as `SIGSEGV`) as `WIFSIGNALED` with `WTERMSIG`, not as an
  `exit(128 + sig)`.
- Only thread group leaders are visible to `wait4`. A leader that exited
  before its threads (e.g. the main thread called `pthread_exit`) is reaped
  only after the whole group is gone. Exited non-leader threads are freed
  automatically (`reap_exited_threads`) from fork, exit and `wait4` paths.
- `execve` in a multi-threaded process terminates the other threads once the
  new image has been read (earlier failures such as `ENOENT` leave the
  process untouched). If a non-leader thread calls `execve`, it first takes
  over the leader's identity, as on Linux: it gets the process id and all
  process-wide state (including the mmap table), and the old leader becomes
  an ordinary thread that is terminated with the other siblings.

Limitations / Simplifications
- Single core only. Fork, COW resolution and thread bookkeeping rely on
  syscalls and fault handlers running with interrupts masked; there is no TLB
  shootdown.
- Partial sharing is rejected: `CLONE_VM`, `CLONE_FILES`, `CLONE_FS` or
  `CLONE_SIGHAND` without `CLONE_THREAD` return `-EINVAL` (except the existing
  `CLONE_VM | CLONE_VFORK` compatibility path, which behaves as a COW fork:
  the parent is not suspended).
- Signals are only delivered on syscall return, so a CPU-bound thread sees a
  `kill()` late (process-wide exits still terminate it immediately). Faults
  are not delivered as `SIGSEGV`/`SIGBUS` to handlers; they end the process.
- Pending signals are per thread only; there is no separate shared
  (process-wide) pending set. Real-time signals are not queued and carry no
  `sigqueue` payload.
- `sigaltstack` is not implemented; `SA_ONSTACK` is accepted and ignored.
- After `rt_sigreturn` through the SYSCALL path, RCX and R11 hold the restored
  RIP and RFLAGS rather than their saved values (SYSRET returns through those
  registers). Signals are only delivered at syscall boundaries, where the
  `syscall` instruction has already clobbered them.
- Futex bitsets do not filter wakeups, and there is no `FUTEX_WAKE_OP`,
  priority inheritance or robust-list support (robust mutexes do not recover
  from an owner's death).
- Interrupted waits restart from scratch; there is no restart block, so a
  restarted wait gets its full timeout again (only untimed waits restart).
- `/proc` has no per-process entries (`/proc/<pid>`, `/proc/self/...` other
  than the `readlink("/proc/self/exe")` special case), so it cannot be used to
  list processes or threads.
- CR0.WP is not set: kernel code that wrote to a user virtual address directly
  (instead of via `copy_user_bytes`) would bypass COW. All current paths use
  the physical map with an explicit COW break.
- The kernel writes to read-only, non-COW user pages if asked to (as before
  this work); shared read-only frames are not protected from such writes.
- Frame share counts are 16-bit; a frame that would overflow is copied
  instead of shared.
