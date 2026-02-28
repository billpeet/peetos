# Toybox Shell Syscall Focus

Last reviewed: 2026-02-28

This document narrows syscall tracking specifically for Toybox `sh` (`toysh`) so we can use the shell as a realistic runtime test harness.

## Scope

- Target command: `sh` from `toybox/toybox-src/toys/pending/sh.c`.
- Runtime model: musl userspace on PeetOS.
- Goal: identify shell-critical syscalls first, before broad applet coverage.

## Evidence Sources

- Toybox shell implementation:
  - `toybox/toybox-src/toys/pending/sh.c`
  - `toybox/toybox-src/lib/xwrap.c`
  - `toybox/toybox-src/lib/portability.c`
- musl syscall mappings:
  - `musl/musl-src/src/process/_Fork.c`
  - `musl/musl-src/src/process/vfork.c`
  - `musl/musl-src/src/process/waitpid.c`
  - `musl/musl-src/src/linux/wait4.c`
  - `musl/musl-src/src/unistd/isatty.c`
  - `musl/musl-src/src/misc/ioctl.c`

## Shell-Critical Syscalls

## Tier S0: Parser + Builtins + Script File IO

Required for basic `sh -c`, builtin execution, script loading, and path/env setup.

- `read`, `write`, `openat`/`open`, `close`, `lseek`
- `newfstatat`/`fstat`/`getdents64`
- `access`/`faccessat`
- `chdir`, `getcwd`
- `getpid`, `getppid`, `getuid`, `geteuid`, `getgid`, `getegid`
- `rt_sigaction`, `rt_sigprocmask`, `kill`
- `clock_gettime`, `gettimeofday`, `nanosleep` (time variables/shell helpers)

Status in PeetOS: mostly implemented.

## Tier S1: External Commands, Pipelines, Background Jobs

Required for normal shell behavior beyond builtin-only commands.

- Process creation:
  - `fork` or compatible path (`clone`/`vfork` behavior used by libc/toolchain)
- Child reaping:
  - `wait4` (musl `waitpid()` uses `wait4`)
- Pipeline/process plumbing:
  - `pipe`/`pipe2`, `dup2`/`dup3`, `fcntl`, `execve` (already present)

Status in PeetOS:

- `execve`, `pipe*`, `dup*`, `fcntl` are implemented.
- `fork` baseline is implemented (address-space clone; no COW).
- `wait4` baseline is implemented (`pid=-1` or `pid>0`, plus `WNOHANG`; `rusage` pointer currently unsupported).
- `clone`/`vfork` baseline compatibility paths are now implemented for fork-like process creation.

## Tier S2: Interactive Shell / TTY Comfort

Needed for robust interactive behavior and terminal semantics.

- `ioctl` baseline tty support:
  - `TIOCGWINSZ` (used by `isatty`/stdio behavior)
  - `TCGETS`, `TCSETS*` (termios path)
  - `TIOCGPGRP`/`TIOCSPGRP` for process group terminal control
- Session/process-group controls:
  - `setpgid`, `setsid`, `getsid`

Status in PeetOS:

- `ioctl` baseline tty subset is implemented for console fds:
  - `TIOCGWINSZ`, `TCGETS`, `TCSETS`, `TCSETSW`, `TCSETSF`, `TIOCGPGRP`, `TIOCSPGRP`.
- `setpgid`/`setsid`/`getsid` baseline implementations are now wired.

## What Breaks Today

- Baseline `sh -c 'echo ...'` now works in the `musl-ext2-exec` shell bring-up path (Toybox `sh` via `/mnt/ext2/bin/toybox`), and non-test boots now hand off to interactive `sh` on the kernel TTY.
- Full interactive job control is still incomplete even with `setpgid`/`setsid`/`getsid` present; signal and controlling-tty semantics are still baseline-only.
- `ioctl` semantics are intentionally shallow (static/default termios + fixed winsize), so advanced terminal behavior may not match Linux.
- `clone`/`vfork` coverage is limited to fork-compatible semantics; full Linux clone-flag behavior is not yet implemented.

## Recommended Hardening Order (Shell-First)

1. Land baseline process/tty primitives (done):
   - `fork`, `wait4`, and minimal tty `ioctl` subset.
2. Keep hardening process-group/session behavior:
   - `setpgid`, `setsid`, `getsid` edge-case handling + job-control interactions.
3. Harden process-creation compatibility:
   - expand beyond fork-compatible `clone`/`vfork` subset as required by selected musl/toolchain configuration.
4. Optional hardening:
   - Expand symlink-follow path semantics beyond baseline `symlinkat`/`readlinkat` support.

## Suggested Smoke Milestones

1. `sh -c 'echo hi'` (builtin-only path) - done.
2. `sh -c '/bin/echo hi'` (external exec path).
3. `sh -c 'echo hi | cat'` (pipeline path).
4. `sh -c 'sleep 1 & wait'` (background + wait path).
5. Interactive prompt sanity on serial console.
6. Feature-gated scripted utility smoke (`scripts/run_toybox_next_smoke.sh`) to record syscall-gap failures for `cp`/`mv`/`ln`/`touch`.

## Limitations / Simplifications

- This is a shell-focused syscall subset, not full Toybox coverage.
- Toybox `sh` still has internal TODOs for complete job-control semantics; kernel support alone does not guarantee full bash-equivalent behavior.
- musl/toolchain variants may route process creation through different syscalls (`fork` vs `clone`), so implementation should match the selected ABI/runtime path in this repo.
- `wait4` is currently minimal (`WNOHANG` only, `rusage` not yet supported).
- `clone`/`vfork` are currently compatibility-focused and do not provide full Linux shared-VM/thread semantics.
