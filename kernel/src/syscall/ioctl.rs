//! ioctl dispatch for console, pty and framebuffer devices.

use super::*;

pub(super) fn default_termios() -> LinuxTermios {
    tty::default_termios()
}

pub(super) fn process_group_exists_in_session(sid: i32, pgid: i32) -> bool {
    for idx in 0..task::MAX_TASKS {
        let task_ptr = task::get_by_index(idx);
        if task_ptr.is_null() {
            continue;
        }
        let (task_sid, task_pgid, state) =
            unsafe { ((*crate::task::leader_of(task_ptr)).sid, (*crate::task::leader_of(task_ptr)).pgid, (*task_ptr).state) };
        if state == TaskState::Dead {
            continue;
        }
        if task_sid == sid && task_pgid == pgid {
            return true;
        }
    }
    false
}

pub(super) fn process_group_exists(pgid: i32) -> bool {
    for idx in 0..task::MAX_TASKS {
        let task_ptr = task::get_by_index(idx);
        if task_ptr.is_null() {
            continue;
        }
        let (task_pgid, state) = unsafe { ((*crate::task::leader_of(task_ptr)).pgid, (*task_ptr).state) };
        if state == TaskState::Dead {
            continue;
        }
        if task_pgid == pgid {
            return true;
        }
    }
    false
}

pub(super) fn ioctl_console(ctx: &SyscallContext, request: u64, argp: u64) -> i64 {
    match request {
        TIOCGWINSZ => {
            if argp == 0 {
                return -EFAULT;
            }
            let ws = LinuxWinSize {
                ws_row: 25,
                ws_col: 80,
                ws_xpixel: 0,
                ws_ypixel: 0,
            };
            match write_struct_to_user(ctx.current, argp, &ws) {
                Ok(()) => 0,
                Err(err) => -err,
            }
        }
        TCGETS => {
            if argp == 0 {
                return -EFAULT;
            }
            let tio = tty::console_termios();
            match write_struct_to_user(ctx.current, argp, &tio) {
                Ok(()) => 0,
                Err(err) => -err,
            }
        }
        TCSETS | TCSETSW | TCSETSF => {
            if argp == 0 {
                return -EFAULT;
            }
            match read_struct_from_user::<LinuxTermios>(ctx.current, argp) {
                Ok(tio) => {
                    tty::set_console_termios(&tio);
                    0
                }
                Err(err) => -err,
            }
        }
        TIOCGPGRP => {
            if argp == 0 {
                return -EFAULT;
            }
            let pgrp = unsafe { (*crate::task::leader_of(ctx.current)).pgid };
            match write_i32_to_user(ctx.current, argp, pgrp) {
                Ok(()) => 0,
                Err(err) => -err,
            }
        }
        TIOCSPGRP => {
            if argp == 0 {
                return -EFAULT;
            }
            let requested = match read_i32_from_user(ctx.current, argp) {
                Ok(value) => value,
                Err(err) => return -err,
            };
            if requested <= 0 {
                return -EINVAL;
            }
            let sid = unsafe { (*crate::task::leader_of(ctx.current)).sid };
            if !process_group_exists_in_session(sid, requested) {
                return -EPERM;
            }
            0
        }
        _ => -ENOTTY,
    }
}

pub(super) fn ioctl_pty(ctx: &SyscallContext, pty_id: u16, request: u64, argp: u64) -> i64 {
    match request {
        TIOCGWINSZ => {
            if argp == 0 {
                return -EFAULT;
            }
            let (rows, cols) = crate::pty::get_winsize(pty_id);
            let ws = LinuxWinSize {
                ws_row: rows,
                ws_col: cols,
                ws_xpixel: 0,
                ws_ypixel: 0,
            };
            match write_struct_to_user(ctx.current, argp, &ws) {
                Ok(()) => 0,
                Err(err) => -err,
            }
        }
        TIOCSWINSZ => {
            if argp == 0 {
                return -EFAULT;
            }
            match read_struct_from_user::<LinuxWinSize>(ctx.current, argp) {
                Ok(ws) => {
                    crate::pty::set_winsize(pty_id, ws.ws_row, ws.ws_col);
                    0
                }
                Err(err) => -err,
            }
        }
        TCGETS => {
            if argp == 0 {
                return -EFAULT;
            }
            let tio = crate::pty::get_termios(pty_id);
            match write_struct_to_user(ctx.current, argp, &tio) {
                Ok(()) => 0,
                Err(err) => -err,
            }
        }
        TCSETS | TCSETSW | TCSETSF => {
            if argp == 0 {
                return -EFAULT;
            }
            match read_struct_from_user::<LinuxTermios>(ctx.current, argp) {
                Ok(tio) => {
                    crate::pty::set_termios(pty_id, &tio);
                    0
                }
                Err(err) => -err,
            }
        }
        TIOCSCTTY => {
            unsafe {
                (*crate::task::leader_of(ctx.current)).ctty_type = task::CTTY_PTY;
                (*crate::task::leader_of(ctx.current)).ctty_id = pty_id;
            }
            0
        }
        TIOCNOTTY => {
            unsafe {
                (*crate::task::leader_of(ctx.current)).ctty_type = task::CTTY_NONE;
                (*crate::task::leader_of(ctx.current)).ctty_id = 0;
            }
            0
        }
        TIOCGPGRP => {
            if argp == 0 {
                return -EFAULT;
            }
            let pgrp = unsafe { (*crate::task::leader_of(ctx.current)).pgid };
            match write_i32_to_user(ctx.current, argp, pgrp) {
                Ok(()) => 0,
                Err(err) => -err,
            }
        }
        TIOCSPGRP => {
            if argp == 0 {
                return -EFAULT;
            }
            let requested = match read_i32_from_user(ctx.current, argp) {
                Ok(value) => value,
                Err(err) => return -err,
            };
            if requested <= 0 {
                return -EINVAL;
            }
            let sid = unsafe { (*crate::task::leader_of(ctx.current)).sid };
            if !process_group_exists_in_session(sid, requested) {
                return -EPERM;
            }
            0
        }
        TIOCSPTLCK => {
            // Lock/unlock the slave side. argp points to an int: 0 = unlock, nonzero = lock.
            if argp == 0 {
                return -EFAULT;
            }
            let lock_val = match read_i32_from_user(ctx.current, argp) {
                Ok(v) => v,
                Err(err) => return -err,
            };
            if lock_val == 0 {
                crate::pty::unlock(pty_id);
            }
            // lock_val != 0: re-lock (not typically used, ignore for simplicity)
            0
        }
        TIOCGPTN => {
            // Return the PTY number (slave index)
            if argp == 0 {
                return -EFAULT;
            }
            match write_i32_to_user(ctx.current, argp, pty_id as i32) {
                Ok(()) => 0,
                Err(err) => -err,
            }
        }
        _ => -ENOTTY,
    }
}

pub(super) fn ioctl_framebuffer(ctx: &SyscallContext, request: u64, argp: u64) -> i64 {
    match request {
        FBIOGETINFO => {
            if argp == 0 {
                return -EFAULT;
            }
            let info = match framebuffer::device_info() {
                Some(info) => info,
                None => return -ENXIO,
            };
            match write_struct_to_user(ctx.current, argp, &info) {
                Ok(()) => 0,
                Err(err) => -err,
            }
        }
        _ => -ENOTTY,
    }
}

const EVIOCGVERSION: u64 = 0x8004_4501;
const EVIOCGID: u64 = 0x8008_4502;
const EVIOCGRAB: u64 = 0x4004_4590;
const EVIOCREVOKE: u64 = 0x4004_4591;
const EVIOCSCLOCKID: u64 = 0x4004_45a0;
const IOC_READ: u64 = 2;

/// What an `EVIOC*` request asks for (see `<linux/input.h>`).
#[derive(Debug, PartialEq, Eq)]
pub(super) enum EvdevRequest {
    Version,
    Id,
    Name(usize),
    Phys(usize),
    Uniq(usize),
    Prop(usize),
    /// `EVIOCGKEY`/`LED`/`SND`/`SW`: current state; nothing is held.
    State { ev_type: u16, len: usize },
    Bits { ev_type: u16, len: usize },
    Abs { axis: u16, len: usize },
    /// `EVIOCGRAB`, `EVIOCREVOKE`, `EVIOCSCLOCKID`: accepted.
    Accepted,
    Unsupported,
}

pub(super) fn decode_evdev_request(request: u64) -> EvdevRequest {
    match request {
        EVIOCGVERSION => return EvdevRequest::Version,
        EVIOCGID => return EvdevRequest::Id,
        EVIOCGRAB | EVIOCREVOKE | EVIOCSCLOCKID => return EvdevRequest::Accepted,
        _ => {}
    }
    let nr = (request & 0xff) as u16;
    let ty = (request >> 8) & 0xff;
    let len = ((request >> 16) & 0x3fff) as usize;
    let dir = (request >> 30) & 0x3;
    if ty != u64::from(b'E') || dir != IOC_READ {
        return EvdevRequest::Unsupported;
    }
    match nr {
        0x06 => EvdevRequest::Name(len),
        0x07 => EvdevRequest::Phys(len),
        0x08 => EvdevRequest::Uniq(len),
        0x09 => EvdevRequest::Prop(len),
        0x18 => EvdevRequest::State { ev_type: input::EV_KEY, len },
        0x19 => EvdevRequest::State { ev_type: input::EV_LED, len },
        0x1a => EvdevRequest::State { ev_type: input::EV_SND, len },
        0x1b => EvdevRequest::State { ev_type: input::EV_SW, len },
        0x20..=0x3f => EvdevRequest::Bits { ev_type: nr - 0x20, len },
        0x40..=0x7f => EvdevRequest::Abs { axis: nr - 0x40, len },
        _ => EvdevRequest::Unsupported,
    }
}

/// Copy at most `len` bytes of `data` to user memory; returns the count, as
/// evdev's `bits_to_user`/`str_to_user` do.
fn copy_evdev_bytes(ctx: &SyscallContext, argp: u64, len: usize, data: &[u8]) -> i64 {
    let n = core::cmp::min(len, data.len());
    if n == 0 {
        return 0;
    }
    if argp == 0 {
        return -EFAULT;
    }
    match copy_to_user_task(ctx.current, argp, &data[..n]) {
        Ok(()) => n as i64,
        Err(err) => -err,
    }
}

/// evdev ioctls on `/dev/input/event*`, enough for libevdev/libinput:
/// identity, capability bitmaps, absolute axis ranges, and accepted no-ops
/// for grabbing, revoking and the event clock.
pub(super) fn ioctl_evdev(ctx: &SyscallContext, device_id: u16, request: u64, argp: u64) -> i64 {
    let Some(ident) = input::evdev_identity(device_id) else {
        return -ENODEV;
    };
    match decode_evdev_request(request) {
        EvdevRequest::Version => {
            copy_evdev_bytes(ctx, argp, 4, &input::EVDEV_VERSION.to_ne_bytes()).min(0)
        }
        EvdevRequest::Id => {
            let mut id = [0u8; 8];
            id[0..2].copy_from_slice(&ident.bustype.to_ne_bytes());
            id[2..4].copy_from_slice(&ident.vendor.to_ne_bytes());
            id[4..6].copy_from_slice(&ident.product.to_ne_bytes());
            id[6..8].copy_from_slice(&ident.version.to_ne_bytes());
            copy_evdev_bytes(ctx, argp, 8, &id).min(0)
        }
        EvdevRequest::Name(len) | EvdevRequest::Phys(len) => {
            let text = if matches!(decode_evdev_request(request), EvdevRequest::Name(_)) {
                ident.name
            } else {
                ident.phys
            };
            let mut buf = [0u8; 64];
            let n = core::cmp::min(text.len(), buf.len() - 1);
            buf[..n].copy_from_slice(&text.as_bytes()[..n]);
            copy_evdev_bytes(ctx, argp, len, &buf[..n + 1])
        }
        EvdevRequest::Uniq(_) => -ENOENT,
        EvdevRequest::Prop(len) => copy_evdev_bytes(ctx, argp, len, &[0u8; 8]),
        EvdevRequest::State { ev_type, len } => {
            let size = input::evdev_bitmap_bytes(ev_type).unwrap_or(8);
            let zeros = [0u8; input::EVDEV_KEY_BITMAP_BYTES];
            copy_evdev_bytes(ctx, argp, len, &zeros[..size])
        }
        EvdevRequest::Bits { ev_type, len } => {
            let Some(size) = input::evdev_bitmap_bytes(ev_type) else {
                return -EINVAL;
            };
            let mut bits = [0u8; input::EVDEV_KEY_BITMAP_BYTES];
            input::evdev_capabilities(device_id, ev_type, &mut bits[..size]);
            copy_evdev_bytes(ctx, argp, len, &bits[..size])
        }
        EvdevRequest::Abs { axis, len } => {
            let Some(info) = input::evdev_absinfo(device_id, axis) else {
                return -EINVAL;
            };
            let mut raw = [0u8; 24];
            for (i, v) in info.iter().enumerate() {
                raw[i * 4..i * 4 + 4].copy_from_slice(&v.to_ne_bytes());
            }
            copy_evdev_bytes(ctx, argp, len, &raw).min(0)
        }
        EvdevRequest::Accepted => 0,
        EvdevRequest::Unsupported => -EINVAL,
    }
}

#[inline]
pub(super) fn ioctl_request(raw: u64) -> u64 {
    raw as u32 as u64
}

pub(super) fn sys_ioctl(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let idx = match fd_index(call.arg(0) as i32) {
        Ok(idx) => idx,
        Err(err) => return err,
    };
    if !fd_slot_is_open(ctx.current, idx) {
        return -EBADF;
    }

    let kind = unsafe { (*crate::task::leader_of(ctx.current)).user_fd_kind[idx] };
    let inode = unsafe { (*crate::task::leader_of(ctx.current)).user_fd_inode[idx] };
    // The request is an `unsigned int` in Linux. musl's ioctl() takes an
    // `int`, so requests with bit 31 set (every _IOR, e.g. EVIOCGID) arrive
    // sign-extended to 64 bits; compare only the low 32.
    let request = ioctl_request(call.arg(1));
    let argp = call.arg(2);
    if kind == task::USER_FD_KIND_CONSOLE {
        ioctl_console(ctx, request, argp)
    } else if kind == task::USER_FD_KIND_PTY_MASTER || kind == task::USER_FD_KIND_PTY_SLAVE {
        ioctl_pty(ctx, inode, request, argp)
    } else if kind == task::USER_FD_KIND_VFS && fs::inode_is_framebuffer(inode) {
        ioctl_framebuffer(ctx, request, argp)
    } else if let Some(device_id) =
        if kind == task::USER_FD_KIND_VFS { fs::inode_input_device_id(inode) } else { None }
    {
        ioctl_evdev(ctx, device_id, request, argp)
    } else if kind == task::USER_FD_KIND_DRM {
        ioctl_drm(ctx, inode, request, argp)
    } else {
        -ENOTTY
    }
}
