# Toybox Syscall Gaps

Last reviewed: 2026-02-13

This document tracks missing Linux syscalls needed to bring up Toybox utilities on PeetOS in a staged way.

For shell-specific bring-up priorities, see `docs/toybox_shell_syscalls.md`.

## Scope

- Focus first on core file/text/process utilities (`ls`, `cat`, `cp`, `mv`, `rm`, `mkdir`, `ln`, `touch`, `find`, `tar`).
- Defer network and system-management tools until core userspace is stable.
- Use `kernel/src/syscall.rs` as current implemented surface.

## Current Kernel Syscalls (Baseline)

Currently implemented in `kernel/src/syscall.rs`:

- `read`, `write`, `writev`, `open`, `openat`, `close`, `lseek`
- `stat`, `lstat`, `fstat`, `newfstatat`, `getdents64`
- `mkdirat`, `unlinkat`, `renameat`, `linkat`, `symlinkat`, `readlinkat`, `utimensat`
- legacy compatibility aliases: `rename` (82), `link` (86)
- `mmap`, `mprotect`, `munmap`, `brk`
- `dup`, `dup2`, `dup3`, `fcntl`, `pipe`, `pipe2`
- `ioctl` (console tty baseline)
- `rt_sigaction`, `rt_sigprocmask`, `rt_sigreturn`, `kill`
- `futex`, `shmget`, `shmat`, `shmdt`, `shmctl`
- `clone` (fork-compatible subset), `fork`, `vfork` (fork-compatible path), `wait4`, `copy_file_range`, `execve`, `exit`, `exit_group`, `sched_yield`
- `getpid`, `getppid`, `gettid`, `set_tid_address`
- `uname`
- `arch_prctl`, `umask`

## Current Staged Applets

The `musl-ext2-exec` smoke image now stages a shell-core set from one Toybox
multicall binary:

- `sh`, `echo`, `cat`, `ls`, `mkdir`, `rm`, `pwd`, `cp`, `mv`, `ln`, `touch`

`cp`/`mv`/`ln`/`touch` are staged for syscall-gap probing via
`scripts/run_toybox_next_smoke.sh`; metadata ownership/mode and node creation
paths are still pending.

## Latest Smoke Observation (2026-02-13)

From `scripts/run_toybox_next_smoke.sh` on `KERNEL_FEATURES=musl-ext2-cmd-smoke`
after adding `symlinkat`/`copy_file_range`:

- Observed syscall stub hits:
  - none

No additional unknown-syscall logs were emitted in that run (legacy syscall numbers now map to named handlers/stubs).

## Priority Backlog

## P0: Core Utility Bring-Up

Implement these first to unblock common Toybox utilities:

1. Path/metadata access:
- `access` (21) - done (baseline permission checks)
- `faccessat` (269) - done (baseline permission checks)
- `faccessat2` (439) - done (baseline flags accepted: `AT_EACCESS`, `AT_SYMLINK_NOFOLLOW`)

2. Links/path ops:
- `linkat` (265) - done (regular-file hard links; baseline implementation)
- `symlinkat` (266) - done (baseline symlink inode creation + readlink target storage)
- `readlinkat` (267) - done (baseline `/proc/self/exe` + non-symlink `-EINVAL`)

3. Metadata/time ownership:
- `fchmodat` (268) - stubbed (`-ENOSYS`)
- `fchownat` (260) - stubbed (`-ENOSYS`)
- `utimensat` (280) - done (baseline atime/mtime update with `UTIME_NOW`/`UTIME_OMIT`)
- `mknodat` (259) - stubbed (`-ENOSYS`)
- `utimes` (235) / `futimesat` (261) - compatibility aliases still return `-ENOSYS` (wrapper not wired yet)

4. CWD + identity:
- `chdir` (80) - done
- `fchdir` (81) - done
- `getcwd` (79) - done (kernel DFS path reconstruction, depth-limited)
- `getuid` (102), `geteuid` (107), `getgid` (104), `getegid` (108) - done

5. Time + wait:
- `clock_gettime` (228) - done (PIT-backed baseline)
- `nanosleep` (35) - done (PIT/scheduler-backed baseline)
- `gettimeofday` (96) - done (PIT-backed baseline)
- `wait4` (61) - done (baseline child reaping; `pid=-1`/`pid>0`, `WNOHANG`; `rusage` not yet supported)

6. Device/control:
- `ioctl` (16) - done (baseline console tty subset: `TIOCGWINSZ`, `TCGETS`, `TCSETS*`, `TIOCGPGRP`, `TIOCSPGRP`)

## P1: Process/Shell Quality

Needed once we move beyond single-utility exec and toward richer shell flows:

- `fork` (57) - done (baseline)
- `vfork` (58) and `clone` (56) - done (baseline fork-compatible subset)
- `clone3` compatibility path (pending)
- `setpgid` (109), `setsid` (112), `getsid` (124) - done (baseline process-group/session tracking)
- `waitid` (247)
- `renameat2` (316) for optional advanced `mv` paths (`--swap` style)
- `copy_file_range` (326) - done (baseline buffered VFS-to-VFS copy path)

## P2: Defer Until Subsystems Exist

These depend on kernel subsystems not yet complete or intentionally out of scope for early bring-up.

1. Networking tools (`wget`, `ping`, `ifconfig`, `netcat`, etc.):
- `socket` (41), `connect` (42), `accept` (43), `sendto` (44), `recvfrom` (45)
- `bind` (49), `listen` (50), `setsockopt` (54), `getsockopt` (55), `poll` (7)

2. Mount/admin tools (`mount`, `umount`, `switch_root`, namespace tooling):
- `mount` (165), `umount2` (166), `pivot_root` (155), `setns` (308), `unshare` (272)

3. Scheduler/ioprio/module/sysadmin paths:
- `sched_setattr` (314), `sched_getattr` (315), `sched_setaffinity` (203), `sched_getaffinity` (204)
- `ioprio_set` (251), `ioprio_get` (252)
- module syscalls (`init_module`, `finit_module`, `delete_module`)

## Utility Mapping (Quick Reference)

- `cp`/`mv`/`install`: `faccessat`, `utimensat`, `fchownat`, `fchmodat`
- `ln`: `linkat`, `symlinkat`
- `rm`/`find`: `faccessat`, `fchdir`
- `touch`: `utimensat`, `access`
- `tar`: `linkat`, `symlinkat`, `mknodat`, `fchownat`, `fchmodat`, `ftruncate`
- `ps`/`time` and process wrappers: `wait4`, session/process-group syscalls

## Execution Plan

1. Implement remaining P0 metadata/device syscall stubs and kernel-side minimal semantics.
2. Expand/validate the feature-gated Toybox smoke image beyond the current shell-core applets (`echo`, `cat`, `ls`, `mkdir`, `rm`, `pwd`, `cp`, `mv`, `ln`, `touch`) as syscall gaps close.
3. Validate each staged applet under `execve` from ext2 and record missing syscalls from serial logs.
4. Move to P1 only after P0 core utility set is stable.

## Limitations / Simplifications

- This is a staged gap list, not full Linux parity.
- Some Toybox applets live under `toys/pending/`; those may need extra work unrelated to syscalls.
- Syscall implementation alone is not sufficient for deferred areas (networking, mount namespaces, full tty/pty, multi-process job control).
- Newly added P0 handlers are baseline-only:
  - `access`/`faccessat` checks use current VFS metadata and simple uid/gid permission selection.
  - `linkat` currently supports regular-file links only; tmpfs path is a pragmatic clone-style fallback.
  - `symlinkat` creates baseline symlink inodes but general symlink-follow path resolution is not implemented yet.
  - `readlinkat` supports baseline VFS symlink targets plus synthetic `/proc/self/exe`.
  - `utimensat` currently updates inode timestamps in VFS/ext2 metadata without full permission/ownership policy parity.
  - `copy_file_range` is currently implemented as a buffered read/write copy for VFS fds (not a zero-copy filesystem-optimized path).
  - time syscalls are PIT tick-derived monotonic values (no RTC or true wall clock).
  - `getcwd` path lookup is bounded and depends on current VFS traversal behavior.
  - `fork` currently clones full userspace mappings (no copy-on-write optimization yet).
  - `clone`/`vfork` currently map to a narrow fork-compatible subset (no full Linux thread/shared-VM semantics).
  - `wait4` currently supports `WNOHANG`; `rusage` output is unimplemented.
  - `ioctl` currently supports a minimal console tty subset with static/default behavior.
  - process-group/session syscalls are baseline and still need hardening for full job-control parity.
