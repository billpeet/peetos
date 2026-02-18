# PeetOS Rust Porting Plan

This document captures a high-level roadmap to reimplement the existing PeetOS C kernel in Rust from scratch, while preserving feature parity with the current C implementation.

## Parity Target (Source of Truth)

The current feature set is summarized in `../peetos_c/docs/ROADMAP.md` and the kernel sources in `../peetos_c/kernel/src`. The Rust port should aim for parity with Stages 1–9 before attempting Stage 10 optimizations.

## Current Status (Last reviewed: 2026-02-18)

- Phase 0: Complete. Workspace, subsystem crates, bootloader/linker reuse, Makefile, `.cargo/config.toml`, and kernel entry assembly are present.
- Phase 1: Complete. The kernel boots to Rust `kmain()`, initializes serial output, prints a banner, and halts in a loop. The panic handler reports over serial before halting.
- Phase 2: Complete (baseline). Added `arch_x86_64` port I/O helpers, interrupt enable/disable wrappers (plus RAII guard), GDT/IDT setup with interrupt stubs and handler registration, PIC remap, PIT timer init, syscall entry/MSR setup, and a serial exception report.
- Phase 3: Complete (baseline). Added typed address helpers, bitmap-based PMM, VMM mapping/address-space support, and a kernel heap allocator.
- Phase 4: In progress. Baseline tasking + scheduler (kernel threads, run queue, PIT-driven preemption, yield/block/sleep paths) are in place. A minimal user-mode task now boots via `int 0x80` with a Linux-numbered syscall subset (`open`/`openat`, `mkdirat`, `close`, `read`, `write`, `writev`, `lseek`, `dup`/`dup2`/`dup3`, `fcntl`, `ioctl`, `stat`/`lstat`/`fstat`/`newfstatat`, `getdents64`, `unlinkat`, `renameat`, `linkat`, `symlinkat`, `readlinkat`, `utimensat`, `access`/`faccessat`/`faccessat2`, `chdir`/`fchdir`/`getcwd`, `getuid`/`geteuid`/`getgid`/`getegid`, `uname`, `gettimeofday`/`clock_gettime`/`nanosleep`, `mmap`, `mprotect`, `munmap`, `clone`, `fork`, `vfork`, `wait4`, `copy_file_range`, `rt_sigaction`, `rt_sigprocmask`, `rt_sigreturn`, `kill`, `shmget`, `shmat`, `shmdt`, `shmctl`, `umask`, `execve`, `exit`/`exit_group`, `yield`, `getpid`, `getppid`, `setpgid`, `setsid`, `getsid`, `arch_prctl`, `gettid`, `set_tid_address`, `brk`) backed by a table-driven shared SYSCALL/int80 dispatcher and per-task user metadata (`brk`, stack bounds, initial stack pointers, ELF PHDR/base/dynamic flags, per-task FS-base TLS state, per-task FD table, cwd inode). The SYSCALL fast path now uses `swapgs` and per-task kernel stacks via `TSS.rsp0`. Initial user bootstrap now builds `argc`/`argv`/`envp`/`auxv` for ring-3 entry (including baseline identity/random auxv entries), and a minimal dynamic-linking bootstrap is wired (`PT_INTERP` mapping + interpreter-first entry with `AT_ENTRY`/`AT_BASE`). The ELF loader includes stricter header/segment validation (alignment checks, malformed `PT_INTERP` rejection, RWX-segment rejection, entrypoint-in-exec-range checks), plus `ET_EXEC` loading and `ET_DYN` metadata support (`PT_INTERP`, `PT_DYNAMIC`, `load_at`). NXE is enabled in `IA32_EFER` so non-executable mappings are enforced. Baseline userspace VM mapping support now includes private anonymous/file-backed `mmap`, `mprotect`, and `munmap` to unblock external dynamic-linker integration. `execve` now replaces the current image with caller-provided `argv`/`envp` and ELF/VFS-backed load, including close-on-exec descriptor teardown for `FD_CLOEXEC`. Toybox shell-critical syscall bring-up now includes baseline `fork`/`clone`/`vfork` compatibility, `wait4`, `stat`/`lstat`/`uname`, console `ioctl` (`TIOCGWINSZ`, `TCGETS`, `TCSETS*`, `TIOCGPGRP`, `TIOCSPGRP`), and baseline process/session controls (`setpgid`, `setsid`, `getsid`). P0 VFS metadata syscalls (`fchmodat`, `fchownat`, `mknodat`, `utimes`, `futimesat`) are now implemented with baseline semantics. Next-wave syscalls (`ftruncate`, `getrandom`, `sysinfo`, `prlimit64`, `getrusage`, `statfs`, `fstatfs`) are now implemented for expanded toybox applet coverage. Third-wave syscalls (`fchmod`, `fchown`, `truncate`, `sync`, `pread64`, `pwrite64`, `readv`, `sched_getaffinity`, `getpgid`) are now implemented for third-wave applet coverage. Full musl/toybox syscall coverage and full reloc/symbol dynamic linker semantics remain outstanding.
- Phase 5: In progress (expanded baseline). Added a minimal VFS with concrete tmpfs regular-file behavior, virtual fs backends for `devfs` (`/dev/null`, `/dev/zero`, `/dev/console`, `/dev/urandom`) and `procfs` (`/proc/uptime`, `/proc/meminfo`, `/proc/mounts`, `/proc/version`), a mount table abstraction, and a baseline ext2 importer (`mount_ext2_image`) used to mount ext2 at `/mnt/ext2` (preferring block-backed images discovered via the kernel block layer; current bare-metal kernel builds do not embed the sample ext2 fallback image). Ext2 read/write/create/mkdir/truncate/unlink/rmdir/rename support updates inode/block bitmaps and ext2 metadata in the mounted image, with logical-block reads now handling direct + single/double/triple-indirect mappings; mutating VFS syscalls trigger best-effort sync-back to the block device when block-backed ext2 is active. Public `truncate`/`truncate_to` VFS wrappers and `statfs` metadata helpers are now available.
- Phase 6: In progress (baseline+). Added in-kernel synchronization primitives (`SpinLock`, `Mutex`, `Semaphore`) plus IPC plumbing for anonymous pipes, System V-style shared memory, futex wait/wake, and signal delivery (`kill`, `rt_sigaction`, `rt_sigprocmask`, `rt_sigreturn`, default/ignore actions, user-handler delivery via `SA_RESTORER`, syscall-boundary pending delivery, `SIGPIPE` generation on broken pipes). Syscall integration includes `pipe`/`pipe2`, `shmget`/`shmat`/`shmdt`/`shmctl(IPC_RMID)`, `futex`, and signal syscalls, with fd-close teardown for pipe endpoints, shared-memory detach on task exit, and best-effort `clear_child_tid` wake on task exit.
- Phase 7: In progress (baseline+). Added a PCI config-space enumerator (mechanism #1 via `0xCF8`/`0xCFC`) in the `drivers` crate, wired during kernel bring-up with bounded serial reporting of discovered device functions. Added ATA/AHCI discovery over PCI class codes (IDE mode classification + AHCI controller detection/ABAR capture), AHCI command-register setup (`MEM_SPACE` + `BUS_MASTER`), AHCI HBA MMIO capability/port signature probing, AHCI command-engine `IDENTIFY` DMA on active SATA ports, AHCI LBA48 DMA read/write command execution through the kernel block API, ATA PIO `IDENTIFY` probing on IDE channels, and a minimal in-kernel block-device inventory. Added baseline PS/2 keyboard IRQ handling and a canonical line-buffered TTY input path wired to console `read`. Added baseline framebuffer bring-up using bootloader-provided VESA status/mode info with higher-half mapped framebuffer window validation, plus styled serial-to-framebuffer terminal mirroring (8x16 VGA-compatible glyphs, panel chrome, theme presets, cursor blink, and baseline ANSI 16-color SGR handling) for visible on-screen console output.
- Phase 8: In progress (Chunk 1 + 2a + 2b + 2c complete). Ported E1000 NIC driver (PCI discovery, MMIO BAR mapping, EEPROM MAC read, 256-entry TX/RX descriptor rings with DMA buffers, IRQ handler, polling path), Ethernet frame layer (handler dispatch by EtherType, frame build/pad/send), ARP protocol (cache with 32 entries, request/reply/gratuitous, blocking resolve), IPv4 layer (header build/validate/checksum, routing via subnet check + gateway, protocol handler dispatch), ICMP (echo request/reply, blocking ping with RTT), UDP (header build/parse with pseudo-header checksum, port binding table, per-binding receive queue, blocking/non-blocking recv, ephemeral port allocation), TCP (RFC 793 state machine with all 11 states, 16-slot connection table, active/passive open, 3-way handshake, data transfer with sequence tracking and peer windowing, FIN teardown, RST handling, retransmit timer, 4KB per-connection RX/TX buffers), and BSD sockets interface (32-slot socket table, 16 Linux-compatible socket syscalls, sockaddr_in parsing, FD integration for read/write/close, auto-bind for UDP, nonblocking support, plus AF_INET `SOCK_DGRAM` + `IPPROTO_ICMP` path for userspace ping). Static IP config (10.0.2.15/24, gw 10.0.2.2) for QEMU user-mode NAT. Shared MMIO window allocator extracted from storage.rs. `make run-net` target added. Added `poll`/`ppoll` syscalls with per-FD-kind dispatch (console, VFS, pipe, socket) and non-blocking/timeout/infinite-wait support, plus full `recvmsg` (iovec scatter, source address writeback). Full select/epoll, sendmsg, and Unix domain sockets remain pending.
- Phase 9: In progress (bootstrap + preparation). Dynamic-linker bootstrap exists (`PT_INTERP` + auxv handoff), baseline VM mapping syscalls (`mmap`/`mprotect`/`munmap`) are wired, and a baseline `execve` path now replaces process images with caller `argv`/`envp`. A musl bootstrap workspace now exists in-repo (`musl/`, with `make musl-fetch`, `make musl-configure`, `make musl`) to pull source and build a PeetOS-oriented sysroot for cross-compilation bring-up, plus feature-gated runtime paths: `KERNEL_FEATURES=musl-smoke` (direct musl static init), `KERNEL_FEATURES=musl-ext2-exec` (interactive Toybox `sh` launched via `/mnt/ext2/bin/toybox`), `KERNEL_FEATURES=musl-ext2-cmd-smoke` (scripted shell sequence via `/mnt/ext2/bin/next_smoke.sh` for syscall-gap capture), `KERNEL_FEATURES=musl-ext2-ping-smoke` (scripted userspace ICMP smoke via `/mnt/ext2/bin/ping_smoke.sh`), and `KERNEL_FEATURES=musl-ext2-quickjs-smoke` (scripted userspace QuickJS smoke via `/mnt/ext2/bin/quickjs_smoke.sh`). Toybox staging is now wired in-repo (`toybox/`, with `make toybox-fetch`, `make toybox`), and the ext2 `execve` path now stages a shell-core utility set (`sh`, `echo`, `cat`, `ls`, `mkdir`, `rm`, `pwd`, `cp`, `mv`, `ln`, `touch`) via one multicall binary at `/mnt/ext2/bin/toybox`. Networking applets (`netcat`, `wget`, `httpd`, `sntp`, `microcom`, `ftpget`, `ping`) are now staged in the toybox config, enabled by `poll`/`ppoll`/`recvmsg` syscalls and the BSD sockets interface (including datagram ICMP sockets); `/etc/resolv.conf` is staged for QEMU DNS forwarding; `make run-net-shell` provides an interactive shell with E1000 networking and `make run-ping-smoke` provides a focused gateway ping smoke path. QuickJS staging is now wired in-repo (`quickjs/`, with `make quickjs-fetch`, `make quickjs`) with `qjs` staged at `/mnt/ext2/bin/qjs` and smoke scripts staged under `/mnt/ext2/tmp`; `make run-quickjs-smoke` validates interpreter bring-up. External musl dynamic linker integration and broader userspace runtime/tool coverage remain pending.
- Phase 10: Not started.

## Phase 0: Project Scaffolding

- Establish a Rust workspace with `kernel` plus subsystem crates (`arch_x86_64`, `drivers`, `fs`, `net`, `syscall`, `userabi`).
- Reuse bootloader assembly and linker scripts from the C codebase.
- Add build tooling (`Makefile`, `.cargo/config.toml`) and a minimal kernel entry.

## Phase 1: Kernel Bring-Up

- Boot to a Rust `kmain()` using the existing stage1/stage2 bootloader.
- Implement serial output and a minimal panic handler.
- Verify a clean boot to `hlt` loop in QEMU.

## Phase 2: CPU + Interrupts

- Port I/O helpers and safe interrupt enable/disable wrappers.
- RAII-style critical section guard for interrupt masking.
- GDT/IDT setup with interrupt stubs and handler registration.
- PIC remap and PIT timer init (IRQ0).
- Syscall entry stub and MSR setup.
- Exception handling details (serial report; VGA optional).
- Syscall entry stub and interrupt context framing.
- Establish safe Rust wrappers for interrupt enable/disable and critical sections (basic `enable`/`disable` + `without_interrupts` done; scoped guards pending).

## Phase 3: Memory Management

- Physical frame allocator (bitmap allocator) - done.
- Virtual memory manager (4-level paging, higher-half kernel) - done.
- Kernel heap allocator with safe wrappers - done.
- Introduce typed addresses (`PhysAddr`, `VirtAddr`) and RAII mappings - done.

## Phase 4: Tasking + Syscalls

- Task structs, context switching, scheduler.
- User mode, ELF loader, per-process address spaces.
- Implement syscall dispatch surface required by musl/toybox.

## Phase 5: VFS + Filesystems

- VFS core (inode/dentry model).
- tmpfs, devfs, procfs.
- ext2 baseline support (image import + mount + read/write/create/mkdir/truncate with inode/block bitmap updates) done; logical-block reads now include indirect mapping (single/double/triple), block-backed mount + coarse sync-back bridge done; native block-cache/page-cache-backed persistence, indirect-block allocation/write growth, and journal semantics pending.
- FAT32 support pending.
- Block device layer and partitioning.

## Phase 6: IPC + Sync

- Spinlocks, mutexes, semaphores.
- Pipes, shared memory, futexes, and signal delivery baseline (including `rt_sigreturn` + `SA_RESTORER` handler return) done.
- Advanced signal semantics (alternate stacks, queued RT signals, full `siginfo_t`/ucontext compatibility) pending.

## Phase 7: Device Drivers

- PCI enumeration baseline done (full bus/device/function scan over PCI config mechanism #1 with serial inventory output during boot).
- ATA/AHCI bring-up in progress: PCI-based controller discovery + AHCI BAR capture/command-register enablement + AHCI MMIO capability/port probing + AHCI `IDENTIFY` DMA + AHCI LBA48 DMA read/write + ATA `IDENTIFY` probing + minimal block-device inventory/block API are done; a baseline VFS ext2 block bridge (mount + sync-back) is now wired; partition parsing, interrupt-driven AHCI completion, and full native block-fs integration remain pending.
- Keyboard + baseline TTY input are now wired (PS/2 set-1 decode, canonical line buffering, blocking console reads).
- Framebuffer baseline is now wired (VESA handoff parsing, mapped-window validation, and basic clear/pixel primitives), and a styled 8x16 bitmap text mirror now renders serial output onscreen with baseline ANSI 16-color SGR handling, cursor blink, and theme presets; full terminal emulation and VT/pty integration remain pending.

## Phase 8: Networking

- E1000 NIC driver — done (Chunk 1). PCI scan, MMIO BAR0 mapping, EEPROM MAC read, 256-entry TX/RX descriptor rings with DMA buffers, IRQ handler on PIC, polling path. Shared MMIO window allocator extracted into `kernel/src/mmio.rs`.
- Ethernet frame layer — done (Chunk 1). Handler dispatch by EtherType, frame build/pad/send via E1000.
- ARP protocol — done (Chunk 1). 32-entry cache, request/reply, gratuitous ARP, blocking resolve with timeout.
- IPv4 layer — done (Chunk 1). Header construction/validation/checksum, subnet-based routing + gateway, protocol handler dispatch.
- ICMP protocol — done (Chunk 1). Echo request/reply (ping responder), blocking ping with PIT-based RTT measurement.
- Static IP config for QEMU user-mode NAT (10.0.2.15/24, gw 10.0.2.2) — done (Chunk 1).
- Common net types crate (`crates/net`) — done. Byte-order helpers, MacAddr, Ipv4Addr, NetConfig, RFC 1071 checksum.
- `make run-net` and `make run-net-smoke` targets — done.
- UDP protocol — done (Chunk 2a). Header build/parse with pseudo-header checksum, port binding table (16 slots), per-binding receive queue (8 datagrams), blocking/non-blocking recv, ephemeral port allocation, `net-udp-smoke` feature gate + `make run-net-udp-smoke` target.
- TCP protocol — done (Chunk 2b). RFC 793 state machine (CLOSED through TIME_WAIT, all 11 states), connection table (16 slots), active open (`connect` with 3-way SYN handshake + blocking wait), passive open (`listen`/`accept`), data send/recv with sequence tracking and peer windowing, graceful close (FIN handshake with FinWait1/FinWait2/Closing/LastAck/TimeWait), RST handling (send + receive), retransmit timer (1s RTO, 5 max retries), per-connection 4KB RX/TX buffers, in-order data delivery with duplicate ACK for out-of-order, pseudo-header checksum, ephemeral port allocation, `net-tcp-smoke` feature gate + `make run-net-tcp-smoke` target. Sliding window, Nagle, congestion control, out-of-order reassembly, and SACK remain pending.
- BSD sockets interface — done (Chunk 2c). 32-slot socket table bridging userspace FDs to kernel TCP/UDP APIs, 16 Linux-compatible socket syscalls (`socket`, `bind`, `listen`, `connect`, `accept`/`accept4`, `sendto`, `recvfrom`, `shutdown`, `getsockname`, `getpeername`, `setsockopt`, `getsockopt`; `sendmsg`/`recvmsg`/`socketpair` stubbed), `sockaddr_in` parsing/serialization, `read`/`write`/`close` FD integration for `USER_FD_KIND_SOCKET`, auto-bind for UDP sendto, nonblocking support via `O_NONBLOCK`/`SOCK_NONBLOCK`, `net-socket-smoke` feature gate + `make run-net-socket-smoke` target. Full `poll`/`select`/`epoll`, `sendmsg`/`recvmsg`, and Unix domain sockets remain pending.

## Phase 9: User Space Integration

- Dynamic linker support.
- musl source/sysroot bootstrap is now wired (`make musl-fetch`, `make musl-configure`, `make musl`); full runtime integration remains pending.
- toybox staging is now wired (`make toybox-fetch`, `make toybox`) with shell-first ext2 `execve` integration (Toybox `sh` via `/mnt/ext2/bin/toybox`) and an expanded shell-core utility set (31 applets: `echo`, `cat`, `ls`, `mkdir`, `rm`, `pwd`, `cp`, `mv`, `ln`, `touch`, `true`, `false`, `test`, `sleep`, `seq`, `env`, `basename`, `dirname`, `wc`, `head`, `tail`, `tee`, `sort`, `uniq`, `tr`, `cut`, `chmod`, `chown`, `uname`) plus next-wave applets (26 additions: `find`, `grep`, `sed`, `xargs`, `date`, `id`, `printf`, `readlink`, `stat`, `du`, `kill`, `which`, `dd`, `mktemp`, `yes`, `nohup`, `rmdir`, `cmp`, `strings`, `paste`, `nl`, `rev`, `comm`, `fold`, `free`, `uptime`) plus third-wave applets (26 additions: `tar`, `diff`, `patch`, `md5sum`, `od`, `tac`, `realpath`, `hostname`, `expr`, `truncate`, `expand`, `df`, `printenv`, `unlink`, `split`, `tty`, `sync`, `cal`, `groups`, `logname`, `nproc`, `file`, `gzip`, `gunzip`) plus fourth-wave applets (2 additions: `awk`, `install`) plus fifth-wave applets (35 additions: `chgrp`, `cksum`, `cpio`, `getconf`, `iconv`, `link`, `time`, `tsort`, `uudecode`, `uuencode`, `ulimit`, `ascii`, `base64`, `bunzip2`, `bzcat`, `clear`, `dos2unix`, `unix2dos`, `factor`, `fmt`, `getopt`, `hd`, `help`, `mcookie`, `nologin`, `pwgen`, `readelf`, `reset`, `setsid`, `shred`, `shuf`, `ts`, `usleep`, `uuidgen`, `xxd`, `hexdump`, `csplit`) plus sixth-wave applets (16 additions: `whoami`, `arch`, `egrep`, `fgrep`, `crc32`, `[` test glue, `zcat`, `sha1sum`, `sha224sum`, `sha256sum`, `sha384sum`, `sha512sum`, `base32`, `unicode`, `sha3sum`, `mountpoint`) plus seventh-wave networking applets (7 additions: `netcat`, `wget`, `httpd`, `sntp`, `microcom`, `ftpget`, `ping`) via `/mnt/ext2/bin/toybox`, with a `/bin/sh` hardlink for musl `popen()`/`system()` support and `/etc/resolv.conf` staged for QEMU DNS forwarding; `make run-net-shell` provides interactive shell with E1000 networking and `make run-ping-smoke` provides a focused ping smoke for `10.0.2.2`; quickjs staging is now wired (`make quickjs-fetch`, `make quickjs`) with `make run-quickjs-smoke` validating `/mnt/ext2/bin/qjs` against staged scripts in `/mnt/ext2/tmp`; broader utility/runtime bring-up and full POSIX compliance remain pending.
- Validate full shell usage and test scripts.

## Phase 10: Post-Parity Improvements (Optional)

- SMP, ACPI, demand paging, COW fork.
- Page cache, async I/O, security hardening.

## Parity Verification

- Re-run the shell test scripts from the C project (`../peetos_c/tests/*.sh`).
- Validate the “What Works” list in `../peetos_c/docs/ROADMAP.md`.
- Use `make test` for Rust-port checks (host unit tests plus QEMU serial banner check; QEMU exits via `isa-debug-exit`).
