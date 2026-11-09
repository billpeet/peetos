//! `/dev/dri/card0` uapi: open, ioctl, read (events), poll and mmap for the
//! minimal KMS device in `crate::drm`.
//!
//! ioctl arguments are copied in as raw bytes (the size encoded in the
//! request, capped at the kernel's struct size and zero-extended, as Linux's
//! `drm_ioctl` does) and fields are read and written at the offsets of the
//! uapi structs in libdrm's `drm.h` / `drm_mode.h`.
//!
//! Dumb buffers are hidden, unlinked shmem files (the same storage as memfd
//! and `MAP_SHARED | MAP_ANONYMOUS`), so mmap and PRIME reuse the existing
//! file mapping and fd machinery. Scanout copies the framebuffer object's
//! pages to the VESA framebuffer on `SETCRTC`, `PAGE_FLIP` and `DIRTYFB`.

use super::*;
use crate::display_mode;
use crate::drm;

const DRM_IOCTL_TYPE: u64 = 0x64; // 'd'
const ARG_MAX: usize = 128;

const DRM_CAP_DUMB_BUFFER: u64 = 0x1;
const DRM_CAP_VBLANK_HIGH_CRTC: u64 = 0x2;
const DRM_CAP_DUMB_PREFERRED_DEPTH: u64 = 0x3;
const DRM_CAP_DUMB_PREFER_SHADOW: u64 = 0x4;
const DRM_CAP_PRIME: u64 = 0x5;
const DRM_CAP_TIMESTAMP_MONOTONIC: u64 = 0x6;
const DRM_CAP_ASYNC_PAGE_FLIP: u64 = 0x7;
const DRM_CAP_CURSOR_WIDTH: u64 = 0x8;
const DRM_CAP_CURSOR_HEIGHT: u64 = 0x9;
const DRM_CAP_ADDFB2_MODIFIERS: u64 = 0x10;
const DRM_CAP_PAGE_FLIP_TARGET: u64 = 0x11;
const DRM_CAP_CRTC_IN_VBLANK_EVENT: u64 = 0x12;
const DRM_CAP_SYNCOBJ: u64 = 0x13;

const DRM_CLIENT_CAP_STEREO_3D: u64 = 1;
const DRM_CLIENT_CAP_UNIVERSAL_PLANES: u64 = 2;
const DRM_CLIENT_CAP_ATOMIC: u64 = 3;
const DRM_CLIENT_CAP_ASPECT_RATIO: u64 = 4;

const DRM_MODE_OBJECT_CRTC: u32 = 0xcccc_cccc;
const DRM_MODE_OBJECT_CONNECTOR: u32 = 0xc0c0_c0c0;
const DRM_MODE_OBJECT_ENCODER: u32 = 0xe0e0_e0e0;
const DRM_MODE_OBJECT_PLANE: u32 = 0xeeee_eeee;
const DRM_MODE_PROP_ENUM: u32 = 1 << 3;
const DRM_MODE_PROP_IMMUTABLE: u32 = 1 << 2;
const DRM_MODE_PAGE_FLIP_EVENT: u32 = 0x01;
const DRM_MODE_PAGE_FLIP_ASYNC: u32 = 0x02;
const DRM_MODE_FB_MODIFIERS: u32 = 0x02;
const DRM_MODE_CONNECTOR_VIRTUAL: u32 = 15;
const DRM_MODE_ENCODER_VIRTUAL: u32 = 5;
const DRM_MODE_CONNECTED: u32 = 1;
const DRM_MODE_SUBPIXEL_UNKNOWN: u32 = 1;
const DRM_CLOEXEC: u32 = 0o2000000;

/// Character device number reported by fstat on a card fd (226:0).
pub(super) const DRM_CARD0_RDEV: u64 = 226 << 8;

// ---------------------------------------------------------------------------
// Byte-buffer field access
// ---------------------------------------------------------------------------

fn get_u32(b: &[u8], off: usize) -> u32 {
    u32::from_ne_bytes(b[off..off + 4].try_into().unwrap())
}

fn get_u64(b: &[u8], off: usize) -> u64 {
    u64::from_ne_bytes(b[off..off + 8].try_into().unwrap())
}

fn put_u32(b: &mut [u8], off: usize, v: u32) {
    b[off..off + 4].copy_from_slice(&v.to_ne_bytes());
}

fn put_u64(b: &mut [u8], off: usize, v: u64) {
    b[off..off + 8].copy_from_slice(&v.to_ne_bytes());
}

/// Decode a DRM ioctl request: `Some((nr, size))` for type `'d'`.
pub(super) fn drm_ioctl_decode(request: u64) -> Option<(u8, usize)> {
    if (request >> 8) & 0xff != DRM_IOCTL_TYPE {
        return None;
    }
    Some(((request & 0xff) as u8, ((request >> 16) & 0x3fff) as usize))
}

/// Copy a `u32` id list to user memory if the user's array is big enough
/// (Linux semantics: the count is always updated, the array only filled when
/// it can hold everything).
fn put_ids(task: *mut task::Task, ptr: u64, user_count: u32, ids: &[u32]) -> Result<(), i64> {
    if ptr == 0 || (user_count as usize) < ids.len() || ids.is_empty() {
        return Ok(());
    }
    let mut bytes = [0u8; 4 * drm::MAX_FBS];
    for (i, id) in ids.iter().enumerate() {
        bytes[i * 4..i * 4 + 4].copy_from_slice(&id.to_ne_bytes());
    }
    copy_to_user_task(task, ptr, &bytes[..ids.len() * 4]).map_err(|e| -e)
}

fn put_u64s(task: *mut task::Task, ptr: u64, user_count: u32, vals: &[u64]) -> Result<(), i64> {
    if ptr == 0 || (user_count as usize) < vals.len() || vals.is_empty() {
        return Ok(());
    }
    let mut bytes = [0u8; 8 * 4];
    for (i, v) in vals.iter().enumerate() {
        bytes[i * 8..i * 8 + 8].copy_from_slice(&v.to_ne_bytes());
    }
    copy_to_user_task(task, ptr, &bytes[..vals.len() * 8]).map_err(|e| -e)
}

fn put_str(task: *mut task::Task, buf: &mut [u8], len_off: usize, ptr_off: usize, s: &[u8]) -> Result<(), i64> {
    let user_len = get_u64(buf, len_off) as usize;
    let ptr = get_u64(buf, ptr_off);
    if ptr != 0 && user_len > 0 {
        let n = user_len.min(s.len());
        copy_to_user_task(task, ptr, &s[..n]).map_err(|e| -e)?;
    }
    put_u64(buf, len_off, s.len() as u64);
    Ok(())
}

// ---------------------------------------------------------------------------
// Mode, scanout copy, console handover
// ---------------------------------------------------------------------------

fn current_mode() -> Option<drm::Mode> {
    let fb = framebuffer::info();
    if !fb.available || fb.bpp != 32 {
        return None;
    }
    Some(drm::Mode { width: fb.width as u32, height: fb.height as u32 })
}

/// Copy rows `[y0, y1)` x columns `[x0, x1)` (screen coordinates) of the
/// scanout buffer to the VESA framebuffer.
fn blit(s: &drm::Scanout, x0: u32, y0: u32, x1: u32, y1: u32) {
    let fb = framebuffer::info();
    if !fb.available || fb.bpp != 32 || fb.buffer_virt == 0 || !drm::dpms_on() {
        return;
    }
    let w = (fb.width as u32).min(s.width.saturating_sub(s.x));
    let h = (fb.height as u32).min(s.height.saturating_sub(s.y));
    let (x0, x1) = (x0.min(w), x1.min(w));
    let (y0, y1) = (y0.min(h), y1.min(h));
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let row_bytes = ((x1 - x0) * 4) as usize;
    for y in y0..y1 {
        let src = (s.y + y) as u64 * s.pitch as u64 + (s.x + x0) as u64 * 4;
        let dst_off = y as usize * fb.pitch as usize + x0 as usize * 4;
        let dst = unsafe { core::slice::from_raw_parts_mut((fb.buffer_virt as *mut u8).add(dst_off), row_bytes) };
        let _ = crate::shmem::read(s.inode, src, dst);
    }
}

fn blit_all(s: &drm::Scanout) {
    blit(s, 0, 0, u32::MAX, u32::MAX);
}

/// The compositor (DRM master) takes the display from the console.
fn take_display() {
    if !display_mode::compositor_active() {
        display_mode::activate_compositor();
        #[cfg(not(test))]
        {
            crate::fb_console::set_active(false, false);
            crate::mouse::sync_display_owner();
        }
    }
}

fn restore_console() {
    display_mode::activate_console();
    #[cfg(not(test))]
    {
        crate::fb_console::set_active(true, true);
        crate::mouse::sync_display_owner();
    }
}

fn release_inode(inode: u16) {
    if fs::inode_open_release(inode) {
        vfs_inode_freed(inode);
    }
}

// ---------------------------------------------------------------------------
// fd object lifetime (called from fd.rs)
// ---------------------------------------------------------------------------

pub(super) fn drm_open_fd(ctx: &SyscallContext, fd_flags: u32) -> i64 {
    if current_mode().is_none() {
        return -ENXIO;
    }
    let file = match drm::open() {
        Ok(f) => f,
        Err(e) => return e,
    };
    match fd_alloc(ctx.current, task::USER_FD_KIND_DRM, file, fd_flags) {
        Ok(fd) => fd,
        Err(err) => {
            drm_release(file);
            err
        }
    }
}

pub(super) fn drm_retain(file: u16) -> Result<(), i64> {
    drm::retain(file)
}

pub(super) fn drm_release(file: u16) {
    let released = drm::release(file);
    for inode in released.iter() {
        release_inode(inode);
    }
    if released.restore_console {
        restore_console();
    }
}

// ---------------------------------------------------------------------------
// read / poll / mmap
// ---------------------------------------------------------------------------

/// `read(2)`: whole `drm_event_vblank` records for completed flips.
pub(super) fn drm_read(ctx: &SyscallContext, file: u16, flags: u32, buf: u64, len: usize) -> i64 {
    if len < 32 {
        return -EINVAL;
    }
    let nonblocking = (fd_open_flags(flags) & O_NONBLOCK) != 0;
    let mut copied = 0usize;
    loop {
        let now = pit::uptime_ms();
        match drm::take_event(file, now) {
            Ok(Some(ev)) => {
                let bytes = drm::flip_event_bytes(&ev, now);
                if let Err(e) = copy_to_user_task(ctx.current, buf + copied as u64, &bytes) {
                    return if copied > 0 { copied as i64 } else { -e };
                }
                copied += 32;
                if copied + 32 > len {
                    return copied as i64;
                }
            }
            Ok(None) => {
                if copied > 0 {
                    return copied as i64;
                }
                if nonblocking {
                    return -EAGAIN;
                }
                if signal::wait_interrupted(ctx.current) {
                    return -ERESTARTSYS;
                }
                #[cfg(not(test))]
                crate::sched::yield_now();
                #[cfg(test)]
                return -EAGAIN;
            }
            Err(e) => return e,
        }
    }
}

pub(super) fn drm_poll_revents(file: u16, requested: i16) -> i16 {
    let mut revents = 0;
    if (requested & POLLIN) != 0 && drm::has_ready_event(file, pit::uptime_ms()) {
        revents |= POLLIN;
    }
    if (requested & POLLOUT) != 0 {
        revents |= POLLOUT;
    }
    revents
}

/// mmap of a card fd: `(inode, offset)` of the dumb buffer behind the fake
/// offset from `MAP_DUMB`.
pub(super) fn drm_mmap_target(file: u16, offset: u64, len: u64) -> Result<(u16, u64), i64> {
    drm::dumb_mmap_target(file, offset, len)
}

// ---------------------------------------------------------------------------
// ioctl
// ---------------------------------------------------------------------------

pub(super) fn ioctl_drm(ctx: &SyscallContext, file: u16, request: u64, argp: u64) -> i64 {
    let Some((nr, size)) = drm_ioctl_decode(request) else {
        return -ENOTTY;
    };
    let mut buf = [0u8; ARG_MAX];
    let n = size.min(ARG_MAX);
    if n > 0 {
        if argp == 0 {
            return -EFAULT;
        }
        if let Err(e) = copy_from_user_task(ctx.current, argp, &mut buf[..n]) {
            return -e;
        }
    }
    let result = drm_ioctl_dispatch(ctx, file, nr, &mut buf);
    match result {
        Ok(()) => {
            let dir_read = (request >> 30) & 2 != 0; // _IOC_READ: kernel writes back
            if n > 0 && dir_read {
                if let Err(e) = copy_to_user_task(ctx.current, argp, &buf[..n]) {
                    return -e;
                }
            }
            0
        }
        Err(e) => e,
    }
}

fn drm_ioctl_dispatch(ctx: &SyscallContext, file: u16, nr: u8, b: &mut [u8; ARG_MAX]) -> Result<(), i64> {
    let task = ctx.current;
    match nr {
        // DRM_IOCTL_VERSION
        0x00 => {
            put_u32(b, 0, 1);
            put_u32(b, 4, 0);
            put_u32(b, 8, 0);
            put_str(task, b, 16, 24, b"simpledrm")?;
            put_str(task, b, 32, 40, b"20231012")?;
            put_str(task, b, 48, 56, b"PeetOS DRM driver for the boot framebuffer")?;
            Ok(())
        }
        // GET_UNIQUE: no bus id.
        0x01 => {
            put_u64(b, 0, 0);
            Ok(())
        }
        // GET_MAGIC
        0x02 => {
            put_u32(b, 0, file as u32 + 1);
            Ok(())
        }
        // SET_VERSION: report interface 1.4 and driver 1.0.
        0x07 => {
            put_u32(b, 0, 1);
            put_u32(b, 4, 4);
            put_u32(b, 8, 1);
            put_u32(b, 12, 0);
            Ok(())
        }
        // GEM_CLOSE
        0x09 => {
            if let Some(inode) = drm::gem_close(file, get_u32(b, 0))? {
                release_inode(inode);
            }
            Ok(())
        }
        // GET_CAP
        0x0c => {
            let value = match get_u64(b, 0) {
                DRM_CAP_DUMB_BUFFER => 1,
                DRM_CAP_VBLANK_HIGH_CRTC => 0,
                DRM_CAP_DUMB_PREFERRED_DEPTH => 24,
                DRM_CAP_DUMB_PREFER_SHADOW => 0,
                DRM_CAP_PRIME => 3, // import | export
                DRM_CAP_TIMESTAMP_MONOTONIC => 1,
                DRM_CAP_ASYNC_PAGE_FLIP => 0,
                DRM_CAP_CURSOR_WIDTH | DRM_CAP_CURSOR_HEIGHT => 64,
                DRM_CAP_ADDFB2_MODIFIERS => 0,
                DRM_CAP_PAGE_FLIP_TARGET => 0,
                DRM_CAP_CRTC_IN_VBLANK_EVENT => 1,
                DRM_CAP_SYNCOBJ => 0,
                _ => return Err(-EINVAL),
            };
            put_u64(b, 8, value);
            Ok(())
        }
        // SET_CLIENT_CAP: no atomic, so clients use the legacy KMS API.
        0x0d => match get_u64(b, 0) {
            DRM_CLIENT_CAP_UNIVERSAL_PLANES => drm::set_universal_planes(file, get_u64(b, 8) != 0),
            DRM_CLIENT_CAP_STEREO_3D | DRM_CLIENT_CAP_ASPECT_RATIO => Ok(()),
            DRM_CLIENT_CAP_ATOMIC => Err(-EOPNOTSUPP),
            _ => Err(-EINVAL),
        },
        // AUTH_MAGIC: master only; magic 0 is never valid. libdrm's
        // drmIsMaster() relies on non-masters getting EACCES here.
        0x11 => {
            if !drm::is_master(file) {
                Err(-EACCES)
            } else if get_u32(b, 0) == 0 {
                Err(-EINVAL)
            } else {
                Ok(())
            }
        }
        // SET_MASTER / DROP_MASTER
        0x1e => drm::set_master(file),
        0x1f => drm::drop_master(file),
        // PRIME_HANDLE_TO_FD: a VFS fd on the buffer's hidden shmem file.
        0x2d => {
            let (inode, _) = drm::gem_lookup(file, get_u32(b, 0))?;
            let flags = get_u32(b, 4);
            let mut open_flags = fs::O_RDWR;
            if flags & DRM_CLOEXEC != 0 {
                open_flags |= O_CLOEXEC;
            }
            let fd = fd_alloc(task, task::USER_FD_KIND_VFS, inode, fd_flags_from_open_flags(open_flags))?;
            put_u32(b, 8, fd as u32);
            Ok(())
        }
        // PRIME_FD_TO_HANDLE: any fd on a shmem-backed file (exported
        // buffers, memfds) becomes a GEM handle.
        0x2e => {
            let fd = get_u32(b, 8) as i32;
            let (inode, _) = mmap_file_inode(task, fd, false, false)?;
            let size = crate::shmem::size(inode).ok_or(-EINVAL)?;
            fs::inode_open_retain(inode);
            match drm::gem_add(file, inode, size) {
                Ok((handle, fresh)) => {
                    if !fresh {
                        release_inode(inode);
                    }
                    put_u32(b, 0, handle);
                    Ok(())
                }
                Err(e) => {
                    release_inode(inode);
                    Err(e)
                }
            }
        }
        // MODE_GETRESOURCES
        0xA0 => {
            let mut fbs = [0u32; drm::MAX_FBS];
            let nfb = drm::fb_ids(file, &mut fbs);
            put_ids(task, get_u64(b, 0), get_u32(b, 32), &fbs[..nfb])?;
            put_ids(task, get_u64(b, 8), get_u32(b, 36), &[drm::CRTC_ID])?;
            put_ids(task, get_u64(b, 16), get_u32(b, 40), &[drm::CONNECTOR_ID])?;
            put_ids(task, get_u64(b, 24), get_u32(b, 44), &[drm::ENCODER_ID])?;
            put_u32(b, 32, nfb as u32);
            put_u32(b, 36, 1);
            put_u32(b, 40, 1);
            put_u32(b, 44, 1);
            put_u32(b, 48, 1);
            put_u32(b, 52, drm::MAX_DIMENSION);
            put_u32(b, 56, 1);
            put_u32(b, 60, drm::MAX_DIMENSION);
            Ok(())
        }
        // MODE_GETCRTC
        0xA1 => {
            if get_u32(b, 12) != drm::CRTC_ID {
                return Err(-ENOENT);
            }
            let st = drm::crtc_state();
            put_u32(b, 16, st.fb_id);
            put_u32(b, 20, st.x);
            put_u32(b, 24, st.y);
            put_u32(b, 28, 0);
            put_u32(b, 32, st.active as u32);
            if st.active {
                let mode = current_mode().ok_or(-ENXIO)?;
                b[36..104].copy_from_slice(&drm::modeinfo_bytes(mode));
            } else {
                b[36..104].fill(0);
            }
            Ok(())
        }
        // MODE_SETCRTC
        0xA2 => {
            if get_u32(b, 12) != drm::CRTC_ID {
                return Err(-ENOENT);
            }
            let mode = current_mode().ok_or(-ENXIO)?;
            let fb_id = get_u32(b, 16);
            if fb_id != 0 {
                if get_u32(b, 32) == 0 {
                    return Err(-EINVAL);
                }
                if drm::modeinfo_size(&b[36..104]) != (mode.width, mode.height) {
                    return Err(-EINVAL);
                }
                let count = get_u32(b, 8);
                if count != 1 {
                    return Err(-EINVAL);
                }
                let mut conn = [0u8; 4];
                copy_from_user_task(task, get_u64(b, 0), &mut conn).map_err(|e| -e)?;
                if u32::from_ne_bytes(conn) != drm::CONNECTOR_ID {
                    return Err(-ENOENT);
                }
            }
            let fb_id = if fb_id == u32::MAX { drm::crtc_state().fb_id } else { fb_id };
            match drm::set_crtc(file, fb_id, get_u32(b, 20), get_u32(b, 24), mode)? {
                Some(scanout) => {
                    take_display();
                    blit_all(&scanout);
                }
                None => {}
            }
            Ok(())
        }
        // MODE_CURSOR / CURSOR2: no hardware cursor (clients fall back to
        // drawing it themselves). Hiding it (DRM_MODE_CURSOR_BO with handle
        // 0) and moving the hidden cursor succeed: wlroots' legacy commit
        // hides the cursor on every modeset and fails the commit otherwise.
        0xA3 | 0xBB => cursor_request_result(get_u32(b, 0), get_u32(b, 24)),
        // MODE_GETGAMMA / SETGAMMA: gamma_size is 0.
        0xA4 | 0xA5 => Err(-EINVAL),
        // MODE_GETENCODER
        0xA6 => {
            if get_u32(b, 0) != drm::ENCODER_ID {
                return Err(-ENOENT);
            }
            put_u32(b, 4, DRM_MODE_ENCODER_VIRTUAL);
            put_u32(b, 8, drm::CRTC_ID);
            put_u32(b, 12, 1);
            put_u32(b, 16, 0);
            Ok(())
        }
        // MODE_GETCONNECTOR
        0xA7 => {
            if get_u32(b, 48) != drm::CONNECTOR_ID {
                return Err(-ENOENT);
            }
            let mode = current_mode().ok_or(-ENXIO)?;
            put_ids(task, get_u64(b, 0), get_u32(b, 40), &[drm::ENCODER_ID])?;
            let modes_ptr = get_u64(b, 8);
            if modes_ptr != 0 && get_u32(b, 32) >= 1 {
                copy_to_user_task(task, modes_ptr, &drm::modeinfo_bytes(mode)).map_err(|e| -e)?;
            }
            put_ids(task, get_u64(b, 16), get_u32(b, 36), &[drm::PROP_DPMS])?;
            let dpms = if drm::dpms_on() { 0 } else { 3 };
            put_u64s(task, get_u64(b, 24), get_u32(b, 36), &[dpms])?;
            put_u32(b, 32, 1);
            put_u32(b, 36, 1);
            put_u32(b, 40, 1);
            put_u32(b, 44, drm::ENCODER_ID);
            put_u32(b, 52, DRM_MODE_CONNECTOR_VIRTUAL);
            put_u32(b, 56, 1);
            put_u32(b, 60, DRM_MODE_CONNECTED);
            // Physical size at 96 dpi.
            put_u32(b, 64, mode.width * 254 / 960);
            put_u32(b, 68, mode.height * 254 / 960);
            put_u32(b, 72, DRM_MODE_SUBPIXEL_UNKNOWN);
            Ok(())
        }
        // MODE_GETPROPERTY
        0xAA => drm_get_property(task, b),
        // MODE_SETPROPERTY (connector): DPMS only.
        0xAB => {
            if get_u32(b, 12) != drm::CONNECTOR_ID || get_u32(b, 8) != drm::PROP_DPMS {
                return Err(-EINVAL);
            }
            set_dpms(get_u64(b, 0))
        }
        // MODE_GETPROPBLOB: no blobs (no EDID).
        0xAC => Err(-ENOENT),
        // MODE_GETFB
        0xAD => {
            let fb = drm::fb_get(get_u32(b, 0))?;
            put_u32(b, 4, fb.width);
            put_u32(b, 8, fb.height);
            put_u32(b, 12, fb.pitch);
            put_u32(b, 16, 32);
            put_u32(b, 20, if fb.format == drm::FORMAT_ARGB8888 { 32 } else { 24 });
            put_u32(b, 24, 0);
            Ok(())
        }
        // MODE_ADDFB (legacy bpp/depth)
        0xAE => {
            let (bpp, depth) = (get_u32(b, 16), get_u32(b, 20));
            let format = match (bpp, depth) {
                (32, 24) => drm::FORMAT_XRGB8888,
                (32, 32) => drm::FORMAT_ARGB8888,
                _ => return Err(-EINVAL),
            };
            let id = drm::fb_add(file, get_u32(b, 24), get_u32(b, 4), get_u32(b, 8), get_u32(b, 12), format)?;
            put_u32(b, 0, id);
            Ok(())
        }
        // MODE_RMFB
        0xAF => {
            if let Some(inode) = drm::fb_remove(file, get_u32(b, 0))? {
                release_inode(inode);
            }
            Ok(())
        }
        // MODE_PAGE_FLIP
        0xB0 => {
            if get_u32(b, 0) != drm::CRTC_ID {
                return Err(-ENOENT);
            }
            let flags = get_u32(b, 8);
            if flags & !(DRM_MODE_PAGE_FLIP_EVENT | DRM_MODE_PAGE_FLIP_ASYNC) != 0 || flags & DRM_MODE_PAGE_FLIP_ASYNC != 0 {
                return Err(-EINVAL);
            }
            let scanout = drm::page_flip(
                file,
                get_u32(b, 4),
                flags & DRM_MODE_PAGE_FLIP_EVENT != 0,
                get_u64(b, 16),
                pit::uptime_ms(),
            )?;
            blit_all(&scanout);
            Ok(())
        }
        // MODE_DIRTYFB: copy the clip rectangles (or everything) if the
        // framebuffer is on screen.
        0xB1 => {
            let fb_id = get_u32(b, 0);
            drm::fb_get(fb_id)?;
            if let Some(scanout) = drm::scanout_if(fb_id) {
                let num = get_u32(b, 12) as usize;
                let clips = get_u64(b, 16);
                if num == 0 || clips == 0 {
                    blit_all(&scanout);
                } else {
                    for i in 0..num.min(256) {
                        let mut r = [0u8; 8];
                        copy_from_user_task(task, clips + (i * 8) as u64, &mut r).map_err(|e| -e)?;
                        let c = |o: usize| u16::from_ne_bytes([r[o], r[o + 1]]) as u32;
                        blit(&scanout, c(0), c(2), c(4), c(6));
                    }
                }
            }
            Ok(())
        }
        // MODE_CREATE_DUMB
        0xB2 => {
            let (pitch, size) = drm::dumb_layout(get_u32(b, 4), get_u32(b, 0), get_u32(b, 8))?;
            let backing = shmem_hidden_file(task, size, b"drm-dumb")?;
            let inode = backing.inode();
            fs::inode_open_retain(inode); // owned by the GEM entry
            drop(backing);
            match drm::gem_add(file, inode, size) {
                Ok((handle, _)) => {
                    put_u32(b, 16, handle);
                    put_u32(b, 20, pitch);
                    put_u64(b, 24, size);
                    Ok(())
                }
                Err(e) => {
                    release_inode(inode);
                    Err(e)
                }
            }
        }
        // MODE_MAP_DUMB
        0xB3 => {
            let offset = drm::dumb_map_offset(file, get_u32(b, 0))?;
            put_u64(b, 8, offset);
            Ok(())
        }
        // MODE_DESTROY_DUMB
        0xB4 => {
            if let Some(inode) = drm::gem_close(file, get_u32(b, 0))? {
                release_inode(inode);
            }
            Ok(())
        }
        // MODE_GETPLANERESOURCES: the primary plane, for universal-plane clients.
        0xB5 => {
            let planes: &[u32] = if drm::universal_planes(file) { &[drm::PLANE_ID] } else { &[] };
            put_ids(task, get_u64(b, 0), get_u32(b, 8), planes)?;
            put_u32(b, 8, planes.len() as u32);
            Ok(())
        }
        // MODE_GETPLANE
        0xB6 => {
            if get_u32(b, 0) != drm::PLANE_ID {
                return Err(-ENOENT);
            }
            let st = drm::crtc_state();
            put_u32(b, 4, if st.active { drm::CRTC_ID } else { 0 });
            put_u32(b, 8, st.fb_id);
            put_u32(b, 12, 1);
            put_u32(b, 16, 0);
            put_ids(task, get_u64(b, 24), get_u32(b, 20), &[drm::FORMAT_XRGB8888, drm::FORMAT_ARGB8888])?;
            put_u32(b, 20, 2);
            Ok(())
        }
        // MODE_ADDFB2 (single plane, offset 0, linear)
        0xB8 => {
            let flags = get_u32(b, 16);
            if flags & DRM_MODE_FB_MODIFIERS != 0 && get_u64(b, 72) != 0 {
                return Err(-EINVAL);
            }
            if get_u32(b, 52) != 0 || get_u32(b, 24) != 0 {
                return Err(-EINVAL);
            }
            let id = drm::fb_add(file, get_u32(b, 20), get_u32(b, 4), get_u32(b, 8), get_u32(b, 36), get_u32(b, 12))?;
            put_u32(b, 0, id);
            Ok(())
        }
        // MODE_OBJ_GETPROPERTIES
        0xB9 => {
            let obj_id = get_u32(b, 20);
            let obj_type = drm_object_type_for(get_u32(b, 24), obj_id);
            let (ids, vals): (&[u32], [u64; 1]) = match (obj_type, obj_id) {
                (DRM_MODE_OBJECT_CONNECTOR, drm::CONNECTOR_ID) => {
                    (&[drm::PROP_DPMS], [if drm::dpms_on() { 0 } else { 3 }])
                }
                (DRM_MODE_OBJECT_PLANE, drm::PLANE_ID) => (&[drm::PROP_PLANE_TYPE], [1]),
                (DRM_MODE_OBJECT_CRTC, drm::CRTC_ID) | (DRM_MODE_OBJECT_ENCODER, drm::ENCODER_ID) => (&[], [0]),
                _ => return Err(-ENOENT),
            };
            put_ids(task, get_u64(b, 0), get_u32(b, 16), ids)?;
            put_u64s(task, get_u64(b, 8), get_u32(b, 16), &vals[..ids.len()])?;
            put_u32(b, 16, ids.len() as u32);
            Ok(())
        }
        // MODE_OBJ_SETPROPERTY: connector DPMS only.
        0xBA => {
            if get_u32(b, 12) != drm::CONNECTOR_ID
                || get_u32(b, 16) != DRM_MODE_OBJECT_CONNECTOR
                || get_u32(b, 8) != drm::PROP_DPMS
            {
                return Err(-EINVAL);
            }
            set_dpms(get_u64(b, 0))
        }
        // MODE_ATOMIC: not supported (the client cap is refused).
        0xBC => Err(-EOPNOTSUPP),
        _ => Err(-EINVAL),
    }
}

/// `struct drm_mode_cursor` flags and handle -> result: only "no cursor".
pub(super) fn cursor_request_result(flags: u32, handle: u32) -> Result<(), i64> {
    const DRM_MODE_CURSOR_BO: u32 = 0x01;
    if (flags & DRM_MODE_CURSOR_BO) != 0 && handle != 0 {
        return Err(-ENXIO);
    }
    Ok(())
}

/// `DRM_MODE_OBJECT_ANY` (0) asks the kernel to look the type up from the id
/// (wlroots reads every property that way); object ids are unique here.
pub(super) fn drm_object_type_for(obj_type: u32, obj_id: u32) -> u32 {
    if obj_type != 0 {
        return obj_type;
    }
    match obj_id {
        drm::CRTC_ID => DRM_MODE_OBJECT_CRTC,
        drm::ENCODER_ID => DRM_MODE_OBJECT_ENCODER,
        drm::CONNECTOR_ID => DRM_MODE_OBJECT_CONNECTOR,
        drm::PLANE_ID => DRM_MODE_OBJECT_PLANE,
        _ => 0,
    }
}

fn set_dpms(value: u64) -> Result<(), i64> {
    if value > 3 {
        return Err(-EINVAL);
    }
    let on = value == 0;
    drm::set_dpms(on);
    if on {
        if let Some(s) = drm::scanout_if(drm::crtc_state().fb_id) {
            blit_all(&s);
        }
    }
    Ok(())
}

/// `MODE_GETPROPERTY` for the two enum properties: connector "DPMS" and
/// plane "type".
fn drm_get_property(task: *mut task::Task, b: &mut [u8; ARG_MAX]) -> Result<(), i64> {
    let (name, flags, enums): (&[u8], u32, &[(u64, &[u8])]) = match get_u32(b, 16) {
        drm::PROP_DPMS => (
            b"DPMS",
            DRM_MODE_PROP_ENUM,
            &[(0, b"On"), (1, b"Standby"), (2, b"Suspend"), (3, b"Off")],
        ),
        drm::PROP_PLANE_TYPE => (
            b"type",
            DRM_MODE_PROP_ENUM | DRM_MODE_PROP_IMMUTABLE,
            &[(0, b"Overlay"), (1, b"Primary"), (2, b"Cursor")],
        ),
        _ => return Err(-ENOENT),
    };
    let values_ptr = get_u64(b, 0);
    let enum_ptr = get_u64(b, 8);
    let (user_values, user_enums) = (get_u32(b, 56) as usize, get_u32(b, 60) as usize);
    if values_ptr != 0 && user_values >= enums.len() {
        for (i, (v, _)) in enums.iter().enumerate() {
            copy_to_user_task(task, values_ptr + (i * 8) as u64, &v.to_ne_bytes()).map_err(|e| -e)?;
        }
    }
    if enum_ptr != 0 && user_enums >= enums.len() {
        for (i, (v, n)) in enums.iter().enumerate() {
            let mut e = [0u8; 40];
            e[0..8].copy_from_slice(&v.to_ne_bytes());
            e[8..8 + n.len()].copy_from_slice(n);
            copy_to_user_task(task, enum_ptr + (i * 40) as u64, &e).map_err(|e| -e)?;
        }
    }
    put_u32(b, 20, flags);
    b[24..56].fill(0);
    b[24..24 + name.len()].copy_from_slice(name);
    put_u32(b, 56, enums.len() as u32);
    put_u32(b, 60, enums.len() as u32);
    Ok(())
}
