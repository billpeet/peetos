# Toybox Syscall Gaps

Last reviewed: 2026-02-14

This document tracks missing Linux syscalls needed to bring up Toybox utilities on PeetOS in a staged way.

For shell-specific bring-up priorities, see `docs/toybox_shell_syscalls.md`.

## Scope

- Focus first on core file/text/process utilities (`ls`, `cat`, `cp`, `mv`, `rm`, `mkdir`, `ln`, `touch`, `find`, `tar`).
- Defer network and system-management tools until core userspace is stable.
- Use `kernel/src/syscall.rs` as current implemented surface.

## Current Kernel Syscalls (Baseline)

Currently implemented in `kernel/src/syscall.rs`:

- `read`, `write`, `writev`, `readv`, `pread64`, `pwrite64`, `open`, `openat`, `close`, `lseek`
- `stat`, `lstat`, `fstat`, `newfstatat`, `getdents64`
- `mkdirat`, `unlinkat`, `renameat`, `linkat`, `symlinkat`, `readlinkat`, `utimensat`
- legacy compatibility aliases: `rename` (82), `link` (86)
- `mmap`, `mprotect`, `munmap`, `brk`
- `dup`, `dup2`, `dup3`, `fcntl`, `pipe`, `pipe2`
- `ioctl` (console tty baseline)
- `rt_sigaction`, `rt_sigprocmask`, `rt_sigreturn`, `kill`
- `futex`, `shmget`, `shmat`, `shmdt`, `shmctl`
- `clone` (fork-compatible subset), `fork`, `vfork` (fork-compatible path), `wait4`, `copy_file_range`, `execve`, `exit`, `exit_group`, `sched_yield`
- `getpid`, `getppid`, `gettid`, `set_tid_address`, `getpgid`
- `uname`
- `arch_prctl`, `umask`
- `ftruncate`, `truncate`, `getrandom`, `sysinfo`, `prlimit64`, `getrusage`, `statfs`, `fstatfs`
- `fchmod`, `fchown`, `sync`, `sched_getaffinity`

## Current Staged Applets

The `musl-ext2-exec` smoke image now stages a shell-core set from one Toybox
multicall binary:

- `sh`, `echo`, `cat`, `ls`, `mkdir`, `rm`, `pwd`, `cp`, `mv`, `ln`, `touch`
- `true`, `false`, `test`, `sleep`, `seq`, `env`, `basename`, `dirname`
- `wc`, `head`, `tail`, `tee`, `sort`, `uniq`, `tr`, `cut`
- `chmod`, `chown`, `uname`

Next-wave applets (26 additions):
- Scripting essentials: `find`, `grep`, `sed`, `xargs`
- Common utilities: `date`, `id`, `printf`, `readlink`, `stat`, `du`, `kill`, `which`, `dd`, `mktemp`, `yes`, `nohup`, `rmdir`, `cmp`, `strings`
- Text processing: `paste`, `nl`, `rev`, `comm`, `fold`
- System info: `free`, `uptime`

Third-wave applets (26 additions):
- Archive/compression: `tar`, `gzip`, `gunzip`
- File comparison: `diff`, `patch`, `cmp` (already in next-wave)
- Checksums: `md5sum`
- Data inspection: `od`, `tac`, `file`, `strings` (already in next-wave)
- Path/system: `realpath`, `hostname`, `expr`, `truncate`, `expand`, `df`, `printenv`, `unlink`, `split`, `tty`, `sync`, `cal`, `groups`, `logname`, `nproc`

Fourth-wave applets (2 additions):
- Text processing: `awk`
- File installation: `install`

Fifth-wave applets (35 additions, no new syscalls required):
- posix: `chgrp`, `cksum`, `cpio`, `getconf`, `iconv`, `link`, `time`, `tsort`, `uudecode`, `uuencode`, `ulimit`
- other: `ascii`, `base64`, `bunzip2`, `bzcat`, `clear`, `dos2unix`, `unix2dos`, `factor`, `fmt`, `getopt`, `hd`, `help`, `mcookie`, `nologin`, `pwgen`, `readelf`, `reset`, `setsid`, `shred`, `shuf`, `ts`, `usleep`, `uuidgen`, `xxd`
- pending: `hexdump`, `csplit`

Sixth-wave applets (15 additions, no new syscalls required):
- posix aliases: `whoami` (id.c alias), `arch` (uname.c), `egrep`/`fgrep` (grep.c aliases), `crc32` (cksum.c), `[` test glue (test.c alias)
- lsb: `zcat` (gzip.c), `sha1sum`/`sha224sum`/`sha256sum`/`sha384sum`/`sha512sum` (md5sum.c aliases)
- other: `base32` (base64.c), `unicode` (ascii.c), `sha3sum` (sha3sum.c), `mountpoint` (mountpoint.c)

P0 metadata syscalls (`fchmodat`, `fchownat`, `mknodat`, `utimes`, `futimesat`)
are now implemented; `chmod`/`chown` applets use the new handlers.

Next-wave syscalls now implemented:
- `ftruncate` (77) - done (truncate-to-zero for tmpfs/ext2; truncate-to-length for tmpfs)
- `getrandom` (318) - done (XORshift PRNG, capped at 256 bytes per call)
- `sysinfo` (99) - done (PIT uptime, PMM memory stats)
- `prlimit64` (302) - done (stub returning RLIM_INFINITY)
- `getrusage` (98) - done (stub returning zeroed rusage)
- `statfs` (137) / `fstatfs` (138) - done (tmpfs/ext2 backend-aware values)

Third-wave syscalls now implemented:
- `fchmod` (91) - done (fd-based permission-bit update for VFS/ext2)
- `fchown` (93) - done (fd-based uid/gid update for VFS/ext2)
- `truncate` (76) - done (path-based truncate via VFS lookup)
- `sync` (162) - done (ext2 sync-back)
- `pread64` (17) - done (positioned read without fd offset change)
- `pwrite64` (18) - done (positioned write without fd offset change)
- `readv` (19) - done (scatter/gather read via iovec entries)
- `sched_getaffinity` (204) - done (single-CPU affinity stub)
- `getpgid` (121) - done (return process group ID for pid)

Virtual device additions:
- `/dev/urandom` - done (XORshift PRNG read, DevNull-like write)

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
- `fchmodat` (268) - done (baseline permission-bit update for tmpfs/ext2)
- `fchownat` (260) - done (baseline uid/gid update; `-1` preserve convention; `AT_EMPTY_PATH`/`AT_SYMLINK_NOFOLLOW` accepted)
- `utimensat` (280) - done (baseline atime/mtime update with `UTIME_NOW`/`UTIME_OMIT`)
- `mknodat` (259) - done (baseline `S_IFREG` regular-file creation only; char/block/fifo devices unsupported)
- `utimes` (235) / `futimesat` (261) - done (baseline `struct timeval` time update wrappers)

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
- `sched_setattr` (314), `sched_getattr` (315), `sched_setaffinity` (203)
- `ioprio_set` (251), `ioprio_get` (252)
- module syscalls (`init_module`, `finit_module`, `delete_module`)

## Utility Mapping (Quick Reference)

- `cp`/`mv`/`install`: `faccessat`, `utimensat`, `fchownat`, `fchmodat`
- `ln`: `linkat`, `symlinkat`
- `rm`/`find`: `faccessat`, `fchdir`
- `touch`: `utimensat`, `access`
- `tar`: `linkat`, `symlinkat`, `mknodat`, `fchownat`, `fchmodat`, `ftruncate`, `fchmod`, `fchown`
- `gzip`/`gunzip`: `fchmod`, `fchown`, `truncate`
- `awk`: `read`, `write`, `openat`, `close`, `pipe`, `fork`, `wait4`, `execve` (uses musl `popen()` internally; `/bin/sh` hardlink required)
- `install`: `faccessat`, `utimensat`, `fchmodat`, `fchownat`, `mkdirat` (shares `cp` code path, `CONFIG_CP` required)
- `diff`/`patch`: `read`, `write`, `stat`, `openat`
- `truncate`: `truncate` (76)
- `nproc`: `sched_getaffinity`
- `df`: `statfs`
- `sync`: `sync` (162)
- `ps`/`time` and process wrappers: `wait4`, session/process-group syscalls
- `cpio`: `open`, `read`, `write`, `stat`, `fchown`, `fchmod`, `mkdir`, `symlink`, `utimensat`, `fork`
- `iconv`: `read`, `write` (musl self-contained iconv)
- `readelf`: `open`, `fstat`, `mmap`, `munmap`
- `time`: `clock_gettime`, `vfork`, `wait4`
- `setsid`: `setsid`, `setpgid`, `vfork`, `wait4`, `execve`
- `shred`: `open`, `write`, `lseek`, `getrandom`, `chmod`, `unlink`

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
  - `fchmodat` updates permission bits without full ownership/capability permission checks.
  - `fchownat` truncates uid/gid to u16 to match current VFS inode field width; no privilege checks.
  - `mknodat` only supports `S_IFREG` regular file creation; character/block/fifo device nodes are unsupported.
  - `utimes`/`futimesat` convert `struct timeval` tv_sec directly; microsecond precision is discarded.
  - `fchmod`/`fchown` operate on VFS fd inodes without ownership/capability checks.
  - `truncate` resolves path to VFS inode and delegates to same backend as `ftruncate`.
  - `sync` performs a best-effort ext2 sync-back; no per-filesystem sync granularity.
  - `pread64`/`pwrite64` operate on VFS fds only (pipes/console return `-ESPIPE`).
  - `readv` delegates to `sys_read` per iovec entry (not zero-copy).
  - `sched_getaffinity` returns a single-CPU affinity mask (no SMP awareness).
  - `getpgid` queries live task table; no zombie/exited-task lookup.
