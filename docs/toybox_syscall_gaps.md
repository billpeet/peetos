# Toybox Syscall Gaps

Last reviewed: 2026-02-28

This document tracks remaining Linux syscalls needed for broader Toybox utility coverage on PeetOS.

For shell-specific bring-up priorities, see `docs/toybox_shell_syscalls.md`.

## Scope

- Focus first on core file/text/process utilities (`ls`, `cat`, `cp`, `mv`, `rm`, `mkdir`, `ln`, `touch`, `find`, `tar`).
- Network/system-management tools were initially deferred; seventh-wave staging now includes baseline networking bring-up (`netcat`/`wget`/`httpd`/`sntp`/`microcom`/`ftpget`/`ping`) while admin-heavy tooling remains deferred.
- QuickJS runtime smoke is staged through the same ext2 launcher path (`musl-ext2-quickjs-smoke`) to validate JavaScript userspace execution on top of the current toybox/musl baseline.
- MicroPython runtime smoke is staged through the same ext2 launcher path (`musl-ext2-micropython-smoke`) to validate Python userspace execution on top of the current toybox/musl baseline.
- Dropbear runtime smoke is staged through the same ext2 launcher path (`musl-ext2-dropbear-smoke`) to validate SSH daemon/key tool userspace execution on top of the current toybox/musl baseline.
- Use `kernel/src/syscall.rs` as current implemented surface.

## Current Kernel Syscalls

Currently implemented in `kernel/src/syscall.rs`:

- `read`, `write`, `writev`, `readv`, `pread64`, `pwrite64`, `open`, `openat`, `close`, `lseek`
- `stat`, `lstat`, `fstat`, `newfstatat`, `getdents64`
- `mkdirat`, `unlinkat`, `renameat`, `linkat`, `symlinkat`, `readlink`, `readlinkat`, `utimensat`
- legacy compatibility aliases: `rename` (82), `link` (86), `readlink` (89 -> `readlinkat` + `AT_FDCWD`)
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
- other: `ascii`, `base64`, `bunzip2`, `bzcat`, `clear`, `dos2unix`, `unix2dos`, `factor`, `fmt`, `getopt`, `hd`, `help`, `mcookie`, `nologin`, `pwgen`, `readelf`, `reset`, `setsid`, `shred`, `shuf`, `ts`, `usleep`, `uuidgen`, `xxd`, `hexdump`, `csplit`

Sixth-wave applets (16 additions, no new syscalls required):
- posix aliases: `whoami` (id.c alias), `arch` (uname.c), `egrep`/`fgrep` (grep.c aliases), `crc32` (cksum.c), `[` test glue (test.c alias)
- lsb: `zcat` (gzip.c), `sha1sum`/`sha224sum`/`sha256sum`/`sha384sum`/`sha512sum` (md5sum.c aliases)
- other: `base32` (base64.c), `unicode` (ascii.c), `sha3sum` (sha3sum.c), `mountpoint` (mountpoint.c)

Seventh-wave networking applets (7 additions, staged for BSD sockets + poll/recvmsg):
- TCP/UDP tools: `netcat` (SOCK_STREAM/DGRAM), `wget` (TCP), `httpd` (TCP server), `ftpget` (TCP dual-channel), `sntp` (UDP)
- Serial/TTY: `microcom` (uses poll() on TTY, not network)
- ICMP: `ping` (SOCK_DGRAM + `IPPROTO_ICMP` via ICMP datagram socket support)
- dedicated smoke target: `make run-ping-smoke` (feature `musl-ext2-ping-smoke`) validates userspace `toybox ping` against `10.0.2.2`
- All use `xgetaddrinfo()` → musl `getaddrinfo()` which handles numeric IPs via `inet_pton()` without DNS; hostname resolution requires `/etc/resolv.conf` (staged with QEMU DNS forwarder at 10.0.2.3)
- Blocked applets (not staged): `host` (needs libresolv)
- Interactive networking available via `make run-net-shell` (ext2 shell + E1000)

P0 metadata syscalls (`fchmodat`, `fchownat`, `mknodat`, `utimes`, `futimesat`)
are now implemented; `chmod`/`chown` applets use the new handlers.

Next-wave syscalls now implemented:
- `ftruncate` (77) - done (truncate-to-zero and truncate-to-length for tmpfs/ext2 regular files)
- `getrandom` (318) - done (XORshift PRNG, capped at 256 bytes per call)
- `sysinfo` (99) - done (PIT uptime, PMM memory stats)
- `prlimit64` (302) - done (stub returning RLIM_INFINITY)
- `getrusage` (98) - done (stub returning zeroed rusage)
- `statfs` (137) / `fstatfs` (138) - done (tmpfs/ext2 backend-aware values)

Third-wave syscalls now implemented:
- `fchmod` (91) - done (fd-based permission-bit update for VFS/ext2)
- `fchown` (93) - done (fd-based uid/gid update for VFS/ext2)
- `truncate` (76) - done (path-based truncate via VFS lookup)
- `sync` (162) - done (ext2 dirty-run enqueue + cache flush)
- `pread64` (17) - done (positioned read without fd offset change)
- `pwrite64` (18) - done (positioned write without fd offset change)
- `readv` (19) - done (scatter/gather read via iovec entries)
- `sched_getaffinity` (204) - done (single-CPU affinity stub)
- `getpgid` (121) - done (return process group ID for pid)

Virtual device additions:
- `/dev/urandom` - done (XORshift PRNG read, DevNull-like write)

## Latest Smoke Observations (2026-02-18)

From `scripts/run_toybox_next_smoke.sh` on `KERNEL_FEATURES=musl-ext2-cmd-smoke`:

- Observed syscall stub hits: none
- No additional unknown-syscall logs were emitted in that run (legacy syscall numbers now map to named handlers/stubs).

From `scripts/run_toybox_ping_smoke.sh` / `make run-ping-smoke` on `KERNEL_FEATURES=musl-ext2-ping-smoke`:

- Completion marker observed: `[musl-ext2-ping-smoke] done`
- Failure marker not observed: `[musl-ext2-ping-smoke] failures`
- Reply evidence observed: `bytes from 10.0.2.2`
- Implementation note: ICMP datagram socket send path now normalizes/recomputes ICMP checksum for toybox `ping` payloads before transmit.

From `scripts/run_toybox_quickjs_smoke.sh` / `make run-quickjs-smoke` on `KERNEL_FEATURES=musl-ext2-quickjs-smoke`:

- Completion marker observed: `[musl-ext2-quickjs-smoke] done`
- Failure marker not observed: `[musl-ext2-quickjs-smoke] failures`
- Output evidence observed: `Hello from QuickJS on PeetOS!`
- Staging note: ext2 payload includes `/mnt/ext2/bin/qjs` plus `/mnt/ext2/tmp/test_hello.js` and `/mnt/ext2/tmp/test_math.js`.

From `scripts/run_toybox_micropython_smoke.sh` / `make run-micropython-smoke` on `KERNEL_FEATURES=musl-ext2-micropython-smoke`:

- Completion marker observed: `[musl-ext2-micropython-smoke] done`
- Failure marker not observed: `[musl-ext2-micropython-smoke] failures`
- Output evidence observed: `Hello from MicroPython on PeetOS!`
- Bring-up note: MicroPython probe paths required legacy syscall `readlink` (89); kernel now exposes it as a compatibility wrapper over `readlinkat`.
- Staging note: ext2 payload includes `/mnt/ext2/bin/micropython` plus `/mnt/ext2/tmp/test_hello.py` and `/mnt/ext2/tmp/test_math.py`.

From `scripts/run_toybox_dropbear_smoke.sh` / `make run-dropbear-smoke` on `KERNEL_FEATURES=musl-ext2-dropbear-smoke`:

- Completion marker observed: `[musl-ext2-dropbear-smoke] done`
- Failure marker not observed: `[musl-ext2-dropbear-smoke] failures`
- Output evidence observed: `Dropbear` (version/help output)
- Runtime marker observed: `[musl-ext2-dropbear-smoke] daemon running`
- Bring-up note: smoke now stages host key material and launches daemon mode (`dropbear ... -p 2222`), then validates detached startup by forcing a same-port bind collision from a probe launch.
- Staging note: ext2 payload includes `/mnt/ext2/bin/dropbear`, `/mnt/ext2/bin/dropbearkey`, `/mnt/ext2/etc/dropbear/dropbear_ed25519_host_key`, and `/mnt/ext2/bin/dropbear_smoke.sh`.

## Definitive Remaining Applets + Blockers (2026-02-18)

This section is generated from the current tree by diffing:

- all Toybox applets discovered in `toybox/toybox-src/generated/newtoys.h`
- enabled symbols in `toybox/configs/shell-core.config`

Summary:

- total applets discovered: 350
- currently staged/enabled: 143
- remaining (unstaged): 181

Blocker buckets (high-confidence):

1. Explicitly blocked in this document:
- `host` (needs `libresolv`)

2. Blocked by pending mount/namespace syscall surface (P2):
- `mount`, `umount`, `switch_root`, `pivot_root`, `nsenter`, `unshare`

3. Blocked by pending scheduler/ioprio syscall surface (P2):
- `taskset`, `uclampset`, `ionice`, `iorenice`

4. Blocked by pending module syscall surface (P2):
- `insmod`, `lsmod`, `modinfo`, `modprobe`, `rmmod`

5. Remaining unstaged applets (165) are not enabled in `shell-core.config` yet; per-applet blockers are not fully triaged in this doc and may include the pending syscall/subsystem items listed in P1/P2.

Remaining unstaged applets (165):

```text
acpi
addgroup
adduser
arp
arping
bc
blkdiscard
blkid
blockdev
bootchartd
brctl
chattr
chcon
chroot
chrt
chsh
chvt
count
crond
crontab
deallocvt
delgroup
deluser
demo_many_options
demo_number
demo_scankey
demo_utf8towc
devmem
dhcp
dhcp6
dhcpd
dmesg
dnsdomainname
dumpleases
eject
fallocate
fdisk
flock
freeramdisk
fsck
fsfreeze
fstype
fsync
ftpput
getenforce
getfattr
getty
gitcheckout
gitclone
gitfetch
gitinit
gitremote
gpiodetect
gpiofind
gpioget
gpioinfo
gpioset
groupadd
groupdel
halt
hello
hexedit
hostid
hwclock
i2cdetect
i2cdump
i2cget
i2cset
i2ctransfer
ifconfig
init
inotifyd
iotop
ip
ipaddr
ipcrm
ipcs
iplink
iproute
iprule
iptunnel
killall
killall5
klogd
last
linux32
load_policy
log
logger
login
logpath
losetup
lsattr
lsof
lspci
lsusb
makedevs
man
mdev
memeater
mix
mkfifo
mknod
mkpasswd
mkswap
more
nbd-client
nbd-server
nbd_client
nbd_server
netstat
nice
oneit
openvt
partprobe
passwd
pgrep
pidof
pkill
pmap
poweroff
ps
pwdx
readahead
reboot
renice
restorecon
rfkill
route
rtcwake
runcon
sendevent
setenforce
setfattr
skeleton
skeleton_alias
strace
stty
su
sulogin
swapoff
swapon
sysctl
syslogd
tcpsvd
telnet
telnetd
tftp
tftpd
timeout
top
traceroute
traceroute6
tunctl
udpsvd
useradd
userdel
vconfig
vi
vmstat
w
watch
watchdog
who
xzcat
```

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

1. Networking tools (`wget`, `ifconfig`, `netcat`, etc.):
- Core userspace networking syscalls needed by staged seventh-wave applets are now implemented:
  `socket`/`connect`/`accept`/`sendto`/`recvfrom`, `bind`/`listen`/`setsockopt`/`getsockopt`, `poll`/`ppoll`/`recvmsg`.
- Remaining blockers are now mostly subsystem/runtime level, not syscall-number gaps:
  - resolver/library dependency for `host` (`libresolv`)
  - net-admin/ioctl/netlink-heavy tools (`ifconfig`, `ip*`, `route`, etc.)
  - advanced diagnostic/raw-socket behavior (for example traceroute-family semantics)

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
- `ping`: `socket` (`SOCK_DGRAM` + `IPPROTO_ICMP`), `sendto`, `recvmsg`/`recvfrom`, `poll`/`ppoll`
- `ps`/`time` and process wrappers: `wait4`, session/process-group syscalls
- `cpio`: `open`, `read`, `write`, `stat`, `fchown`, `fchmod`, `mkdir`, `symlink`, `utimensat`, `fork`
- `iconv`: `read`, `write` (musl self-contained iconv)
- `readelf`: `open`, `fstat`, `mmap`, `munmap`
- `time`: `clock_gettime`, `vfork`, `wait4`
- `setsid`: `setsid`, `setpgid`, `vfork`, `wait4`, `execve`
- `shred`: `open`, `write`, `lseek`, `getrandom`, `chmod`, `unlink`

## Execution Plan

1. Keep the generated applet inventory (`newtoys.h` vs `shell-core.config`) current and treat it as the source of truth for "remaining" utilities.
2. Implement known blocker syscall/subsystem gaps first (P1/P2 items), then promote directly affected applets into the staged config.
3. Expand/validate the feature-gated Toybox smoke image in waves and record failures by applet + syscall/subsystem dependency.
4. Triage the 165 "unstaged/unknown blocker" applets into concrete blocker classes (syscall, VFS/device capability, runtime/lib dependency, or explicit out-of-scope).

## Limitations / Simplifications

- This is a staged gap list, not full Linux behavior coverage.
- Some Toybox applets live under `toys/pending/`; those may need extra work unrelated to syscalls.
- Syscall implementation alone is not sufficient for deferred areas (networking, mount namespaces, full tty/pty, multi-process job control).
- `ping` smoke currently asserts only the QEMU user-mode NAT gateway path (`10.0.2.2`); broader ICMP interoperability is not yet a pass criterion in this document.
- QuickJS smoke currently validates interpreter boot and staged script execution only; it is not a toybox syscall-gap oracle and does not replace applet-by-applet syscall triage.
- MicroPython smoke currently validates interpreter boot and staged script execution only; it is not a toybox syscall-gap oracle and does not replace applet-by-applet syscall triage.
- Dropbear smoke currently validates tool bring-up, staged host-key usage, and daemonization startup marker only; full SSH auth/session validation remains pending and it does not replace applet-by-applet syscall triage.
- Newly added P0 handlers are baseline-only:
  - `access`/`faccessat` checks use current VFS metadata and simple uid/gid permission selection.
  - `linkat` currently supports regular-file links only; tmpfs path is a pragmatic clone-style fallback.
  - `symlinkat` creates baseline symlink inodes but general symlink-follow path resolution is not implemented yet.
  - `readlinkat` supports baseline VFS symlink targets plus synthetic `/proc/self/exe`.
  - `utimensat` currently updates inode timestamps in VFS/ext2 metadata without full permission/ownership policy compatibility.
  - `copy_file_range` is currently implemented as a buffered read/write copy for VFS fds (not a zero-copy filesystem-optimized path).
  - time syscalls are PIT tick-derived monotonic values (no RTC or true wall clock).
  - `getcwd` path lookup is bounded and depends on current VFS traversal behavior.
  - `fork` currently clones full userspace mappings (no copy-on-write optimization yet).
  - `clone`/`vfork` currently map to a narrow fork-compatible subset (no full Linux thread/shared-VM semantics).
  - `wait4` currently supports `WNOHANG`; `rusage` output is unimplemented.
  - `ioctl` currently supports a minimal console tty subset with static/default behavior.
  - process-group/session syscalls are baseline and still need hardening for complete job-control behavior.
  - `fchmodat` updates permission bits without full ownership/capability permission checks.
  - `fchownat` truncates uid/gid to u16 to match current VFS inode field width; no privilege checks.
  - `mknodat` only supports `S_IFREG` regular file creation; character/block/fifo device nodes are unsupported.
  - `utimes`/`futimesat` convert `struct timeval` tv_sec directly; microsecond precision is discarded.
  - `fchmod`/`fchown` operate on VFS fd inodes without ownership/capability checks.
  - `truncate` resolves path to VFS inode and delegates to same backend as `ftruncate`.
  - `sync` performs best-effort ext2 dirty-run enqueue plus block-cache flush; no per-filesystem sync granularity.
  - `pread64`/`pwrite64` operate on VFS fds only (pipes/console return `-ESPIPE`).
  - `readv` delegates to `sys_read` per iovec entry (not zero-copy).
  - `sched_getaffinity` returns a single-CPU affinity mask (no SMP awareness).
  - `getpgid` queries live task table; no zombie/exited-task lookup.
