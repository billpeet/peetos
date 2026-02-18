# toybox Bootstrap

This document tracks Stage-9 toybox staging for PeetOS userspace bring-up.

Shell-focused syscall requirements and priorities are tracked separately in
`docs/toybox_shell_syscalls.md`.

## Scope

- Fetch pinned toybox source into `toybox/toybox-src`.
- Build minimal static toybox binaries for staged applets (`echo`, `sh`) plus a shell-core multicall image.
- Stage shell-focused payload into the ext2 exec image with Toybox `sh` launched from `/mnt/ext2/bin/toybox` and basic file utilities.

## Commands

```bash
make toybox-fetch
make toybox
make toybox-info
scripts/build_toybox_sh.sh
scripts/build_toybox_core.sh
scripts/run_toybox_next_smoke.sh
scripts/run_toybox_ping_smoke.sh
make run-ping-smoke
```

Build the ext2 exec path that launches Toybox `sh` via `execve`:

```bash
make musl
make toybox
make image PROFILE=release KERNEL_FEATURES=musl-ext2-exec
qemu-system-x86_64 \
  -drive id=disk0,file=build/peetos.img,if=none,format=raw \
  -device ich9-ahci,id=ahci \
  -device ide-hd,drive=disk0,bus=ahci.0 \
  -drive id=disk1,file=user/sample_ext2_musl.img,if=none,format=raw \
  -device ide-hd,drive=disk1,bus=ahci.1 \
  -serial stdio -no-reboot -no-shutdown
```

Expected serial output includes `[musl-ext2-exec] launching interactive toybox sh`.
Use the QEMU display window for keyboard input (TTY-backed shell stdin).

## Output

- Source tree: `toybox/toybox-src`
- Built utility: `toybox/out/echo`
- Optional copied artifact: `user/toybox_echo.elf`
- Optional copied artifact: `user/toybox_sh.elf`
- Optional copied artifact: `user/toybox_core.elf`

## Limitations / Simplifications

- Current smoke staging enables a shell-core applet set (31 applets): `sh`, `echo`, `cat`, `ls`, `mkdir`, `rm`, `pwd`, `cp`, `mv`, `ln`, `touch`, `true`, `false`, `test`, `sleep`, `seq`, `env`, `basename`, `dirname`, `wc`, `head`, `tail`, `tee`, `sort`, `uniq`, `tr`, `cut`, `chmod`, `chown`, `uname`.
- Next-wave applets (26 additions): `find`, `grep`, `sed`, `xargs`, `date`, `id`, `printf`, `readlink`, `stat`, `du`, `kill`, `which`, `dd`, `mktemp`, `yes`, `nohup`, `rmdir`, `cmp`, `strings`, `paste`, `nl`, `rev`, `comm`, `fold`, `free`, `uptime`.
- Third-wave applets (26 additions): `tar`, `diff`, `patch`, `md5sum`, `od`, `tac`, `realpath`, `hostname`, `expr`, `truncate`, `expand`, `df`, `printenv`, `unlink`, `split`, `tty`, `sync`, `cal`, `groups`, `logname`, `nproc`, `file`, `gzip`, `gunzip`.
- Fourth-wave applets (2 additions): `awk`, `install`.
- Fifth-wave applets (35 additions): `chgrp`, `cksum`, `cpio`, `getconf`, `iconv`, `link`, `time`, `tsort`, `uudecode`, `uuencode`, `ulimit`, `ascii`, `base64`, `bunzip2`, `bzcat`, `clear`, `dos2unix`, `unix2dos`, `factor`, `fmt`, `getopt`, `hd`, `help`, `mcookie`, `nologin`, `pwgen`, `readelf`, `reset`, `setsid`, `shred`, `shuf`, `ts`, `usleep`, `uuidgen`, `xxd`, `hexdump`, `csplit`. All use syscalls already implemented in the kernel -- no new syscall stubs required.
- Sixth-wave applets (16 additions): `whoami`, `arch`, `egrep`, `fgrep`, `crc32`, `[` (test glue), `zcat`, `sha1sum`, `sha224sum`, `sha256sum`, `sha384sum`, `sha512sum`, `base32`, `unicode`, `sha3sum`, `mountpoint`. Mostly aliases of already-compiled source files or pure-computation utilities -- no new syscalls required.
- Seventh-wave networking applets (7 additions): `netcat`, `wget`, `httpd`, `sntp`, `microcom`, `ftpget`, `ping`. The `ping` applet now runs through `SOCK_DGRAM` + `IPPROTO_ICMP` userspace socket support; `host` remains blocked on resolver-side dependencies.
- The ext2 smoke path installs one `/bin/toybox` multicall binary plus a `/bin/sh` hardlink (so musl `popen()`/`system()` can invoke `/bin/sh -c`); applets are invoked as `toybox <applet>`.
- Runtime shell/smoke commands expect `user/sample_ext2_musl.img` to be attached as a second AHCI disk in QEMU.
- P0 metadata syscalls (`fchmodat`, `fchownat`, `mknodat`, `utimes`, `futimesat`) are now implemented; `chmod`/`chown` applets exercise the new handlers.
- Third-wave syscalls (`fchmod`, `fchown`, `truncate`, `sync`, `pread64`, `pwrite64`, `readv`, `sched_getaffinity`, `getpgid`) are now implemented for third-wave applet coverage.
- `scripts/run_toybox_next_smoke.sh` now continues through command failures and reports both completion and a failure marker so syscall gaps can be captured in one run.
- `scripts/run_toybox_ping_smoke.sh` validates toybox `ping` end-to-end by launching the `musl-ext2-ping-smoke` init path and requiring an ICMP reply from `10.0.2.2`.
- Seventh-wave applets bring the total staged commands to ~143.
- `make toybox` still builds the standalone `echo` staging artifact (`toybox/out/echo`); shell-core staging uses `scripts/build_toybox_core.sh`.
- The default repository tests avoid networked toybox builds and only validate script behavior/pathing.
