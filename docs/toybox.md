# toybox Bootstrap

This document tracks Stage-9 toybox staging for PeetOS userspace bring-up.

## Scope

- Fetch pinned toybox source into `toybox/toybox-src`.
- Build a minimal static toybox binary with only `echo` enabled.
- Stage that binary into the ext2 exec smoke image at `/mnt/ext2/bin/echo`.

## Commands

```bash
make toybox-fetch
make toybox
make toybox-info
```

Build the ext2 exec smoke path that launches toybox `echo` via `execve`:

```bash
make musl
make toybox
make image PROFILE=release KERNEL_FEATURES=musl-ext2-exec
timeout 8s qemu-system-x86_64 -drive format=raw,file=build/peetos.img -serial stdio -display none -no-reboot -no-shutdown
```

Expected serial output includes `hello from-ext2`.

## Output

- Source tree: `toybox/toybox-src`
- Built utility: `toybox/out/echo`
- Optional copied artifact: `user/toybox_echo.elf`

## Limitations / Simplifications

- Current staging is intentionally narrow and only enables the `echo` applet.
- The smoke path copies the toybox binary as `/bin/echo` instead of relying on symlink/hardlink applet install layouts.
- Broader utility coverage (`ls`, `cat`, shell, etc.) and syscall-gap hardening are still pending.
- The default repository tests avoid networked toybox builds and only validate script behavior/pathing.
