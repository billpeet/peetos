# Process Model: Copy-on-Write Fork and Thread Groups

This document covers how PeetOS creates processes and threads: copy-on-write
`fork`, `clone(CLONE_THREAD)` thread groups (enough for musl pthreads), and
how threads and processes exit.

**Where It Lives**
- `kernel/src/memory/pmm.rs`: per-frame share counts (`page_share`, `page_is_shared`, refcount-aware `free_page`)
- `kernel/src/memory/vmm.rs`: `PTE_COW`, `clone_user_space`, `resolve_cow_in`, `cow_adjust_flags`
- `kernel/src/task.rs`: `tgid`, `group_leader`, `leader_of`, thread flags
- `kernel/src/syscall.rs`: `clone`/`fork` entry (`validate_clone_args`, `create_fork_child`, `create_thread_child`), exit paths, `wait4`, `kill`/`tkill`/`tgkill`, COW fault handling
- `kernel/src/signal.rs`: process-directed delivery (`send_to_process`), `must_abort_wait`
- `kernel/src/shm.rs`: shm inheritance across `fork` (`fork_attachments`)
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
- Only thread group leaders are visible to `wait4`. A leader that exited
  before its threads (e.g. the main thread called `pthread_exit`) is reaped
  only after the whole group is gone. Exited non-leader threads are freed
  automatically (`reap_exited_threads`) from fork, exit and `wait4` paths.
- `execve` in a multi-threaded process first terminates the other threads.

Limitations / Simplifications
- Single core only. Fork, COW resolution and thread bookkeeping rely on
  syscalls and fault handlers running with interrupts masked; there is no TLB
  shootdown.
- Partial sharing is rejected: `CLONE_VM`, `CLONE_FILES`, `CLONE_FS` or
  `CLONE_SIGHAND` without `CLONE_THREAD` return `-EINVAL` (except the existing
  `CLONE_VM | CLONE_VFORK` compatibility path, which behaves as a COW fork:
  the parent is not suspended).
- `execve` from a non-leader thread while other threads are alive returns
  `-EINVAL`; the kernel does not move the process id to the exec'ing thread.
- Threads blocked in socket calls (accept/recv/connect loops) do not notice
  `exit_group` until those calls return on their own; until then the process
  is not fully exited and cannot be reaped.
- Signals are only delivered on syscall return, so a CPU-bound thread sees a
  `kill()` late (process-wide exits still terminate it immediately).
- Pending signals are per thread only; there is no separate shared
  (process-wide) pending set, and real-time/cancellation signals (32 and up,
  e.g. musl's `pthread_cancel`) are not supported.
- `futex` has no timeouts, requeue or robust lists, so timed waits
  (`pthread_cond_timedwait`, `pthread_mutex_timedlock`) and robust mutexes
  do not work.
- `/proc` lists every thread as if it were a process.
- CR0.WP is not set: kernel code that wrote to a user virtual address directly
  (instead of via `copy_user_bytes`) would bypass COW. All current paths use
  the physical map with an explicit COW break.
- The kernel writes to read-only, non-COW user pages if asked to (as before
  this work); shared read-only frames are not protected from such writes.
- Frame share counts are 16-bit; a frame that would overflow is copied
  instead of shared.
