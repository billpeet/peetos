# DRM/KMS (`/dev/dri/card0`)

PeetOS has a minimal DRM/KMS device in the spirit of Linux's `simpledrm`. It drives the fixed VESA mode the bootloader set, through the standard DRM uapi, so unmodified libdrm and compositors that use legacy KMS can present frames. The VESA framebuffer is still the only real display hardware. Scanout copies the chosen buffer into it.

**Where It Lives**
- `kernel/src/drm.rs`: object state and semantics (files, GEM objects, framebuffers, CRTC, flip events), unit tested on the host.
- `kernel/src/syscall/drmdev.rs`: uapi plumbing (open, ioctl, `read` for events, `poll`, `mmap`, PRIME) and copying the scanout buffer to the VESA framebuffer.
- `crates/fs` creates `/dev/dri/card0` (backend `DevDrm`). Opening it creates a DRM file object, held by an fd of kind `USER_FD_KIND_DRM` (like a PTY master from `/dev/ptmx`).
- libdrm 2.4.123 core (`xf86drm`, `xf86drmMode`) is built by `scripts/build_libdrm.sh`, via `make -C wayland`, into `wayland/out` as `libdrm.a` and `libdrm.so.2`. Headers go in `include/` and `include/libdrm/`. Its configuration is `wayland/peetos/libdrm-config.h`, and small `<linux/types.h>`/`<asm/ioctl.h>` shims stand in for the Linux uapi headers musl lacks.
- `user/drm_smoke.c` and `make run-drm-smoke` (`tests/musl_ext2_drm_smoke.sh`) are the libdrm-based QEMU smoke.

**Objects**
- One connector (id 33, type Virtual, always connected, one mode), one encoder (32), one CRTC (31) and one primary plane (34). The plane is listed only to clients that set `DRM_CLIENT_CAP_UNIVERSAL_PLANES`, as on Linux.
- The mode is the VESA mode, reported at 60 Hz with plausible blanking timings.
- Properties: connector `DPMS` (enum, settable) and plane `type` (immutable enum, Primary).

**Buffers**
- `MODE_CREATE_DUMB` allocates 32 bpp buffers, with rows padded to 64 bytes and up to 256 MiB each. The pages live in a hidden, unlinked shmem file, the same storage memfd and `MAP_SHARED | MAP_ANONYMOUS` use.
- `MODE_MAP_DUMB` returns a fake offset, and mmap of the card fd at that offset maps the buffer `MAP_SHARED`. `GEM_CLOSE`/`DESTROY_DUMB` drop the handle, and framebuffers keep the buffer alive.
- PRIME export (`PRIME_HANDLE_TO_FD`) returns an ordinary fd on the hidden file, which can be mmapped or passed over `SCM_RIGHTS`. Import (`PRIME_FD_TO_HANDLE`) accepts any shmem-backed fd (exported buffers, memfds). Re-importing into the same file returns the existing handle.
- `MODE_ADDFB`/`ADDFB2` create framebuffers in `XRGB8888` or `ARGB8888`: single plane, offset 0, linear (modifier 0 only).

**Mode setting and presentation**
- The first opener with no current master becomes master, and `SET_MASTER`/`DROP_MASTER` move it. `SETCRTC` and `PAGE_FLIP` need master. `AUTH_MAGIC` returns `EACCES` to non-masters, which is how libdrm's `drmIsMaster` detects it.
- `SETCRTC` with a framebuffer takes the display from the text console (as a `/dev/fb0` compositor does) and copies the buffer to the screen. When the file that set the mode is closed, the console takes the display back.
- `PAGE_FLIP` copies the new buffer at once. With `DRM_MODE_PAGE_FLIP_EVENT`, a `DRM_EVENT_FLIP_COMPLETE` event becomes readable from the fd one 60 Hz frame (16 ms) later. Until then further flips fail with `EBUSY`. Events are read as `struct drm_event_vblank` records (`drmHandleEvent`), and `poll`/`epoll` report `POLLIN` when one is ready.
- `DIRTYFB` copies the given clip rectangles (or everything) when the framebuffer is on screen, for front-buffer rendering.
- `peetwl` presents through DRM by default: two dumb buffers, a full-frame copy into the off-screen one, then a page flip. It sends `wl_surface.frame` callbacks when the flip completes. `--fbdev` forces the old `/dev/fb0` path, which is also the fallback when no card is available.

**Testing**
- Host unit tests in `kernel/src/drm.rs` cover master rules, dumb layout, buffer lifetimes, mode-set validation, flip pacing and `EBUSY`, event encoding, cleanup on close, PRIME re-import and mmap offset resolution.
- `make run-drm-smoke` runs `drm_smoke` (libdrm, dynamically linked) and reads `/dev/fb0` back to verify the screen after `SETCRTC`, a page flip and `DIRTYFB`. It also checks capabilities, the plane `type` property, flip events through `poll` + `drmHandleEvent`, PRIME export and import (including into a second, non-master card fd) and `EACCES` on non-master mode sets.
- `make run-wlcomp-smoke` now requires `peetwl` to present through DRM (`[peetwl] output drm`, non-zero `drm flips`), with real Wayland clients and screendump pixel checks.

Limitations / Simplifications
- One fixed mode: the VESA mode chosen at boot. There is no mode switching, no hotplug, no EDID (`GETPROPBLOB` returns `ENOENT`) and no gamma (`gamma_size` 0).
- Legacy KMS only. `DRM_CLIENT_CAP_ATOMIC` is refused and `MODE_ATOMIC` returns `EOPNOTSUPP`. wlroots therefore needs its legacy backend (`WLR_DRM_NO_ATOMIC=1`), and there are no overlay planes or `SETPLANE`.
- No hardware cursor: `MODE_CURSOR`/`CURSOR2` with a buffer return `ENXIO`, so clients draw the cursor themselves. Hiding the cursor (handle 0) and moving it succeed, because wlroots' legacy commit hides it on every modeset.
- Scanout is a CPU copy into the VESA framebuffer on every flip or dirty rectangle (about 3 MiB per 1024x768 frame). There is no real vblank, so flip events are timed with the PIT, and there is no tearing protection.
- Only 32 bpp dumb buffers and `XRGB8888`/`ARGB8888` framebuffers are supported: single plane, offset 0, no modifiers. `GETFB` returns no handle.
- `WAIT_VBLANK`, `CRTC_GET_SEQUENCE`, leases, syncobjs and render nodes (`/dev/dri/renderD128`) are not implemented. There is no GPU and no 3D.
- `/dev/dri/card0` reports `S_IFCHR` 226:0 through `fstat` on an open fd, but `stat` on the path reports a regular file, like the other device nodes. `stat` on `/dev/dri/card0` now reports `S_IFCHR` 226:0 too, and a static `/sys` describes the card as `/sys/devices/platform/simple-framebuffer.0/drm/card0` (`docs/vfs.md`): enough for libudev and `drmGetDeviceNameFromFd2`, but `drmGetDevice` cannot parse the platform bus info and there is no render node.
- Fixed tables: 16 open card files, 64 GEM objects, 32 framebuffers and 8 queued events per file system-wide.
