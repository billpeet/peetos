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
    let request = call.arg(1);
    let argp = call.arg(2);
    if kind == task::USER_FD_KIND_CONSOLE {
        ioctl_console(ctx, request, argp)
    } else if kind == task::USER_FD_KIND_PTY_MASTER || kind == task::USER_FD_KIND_PTY_SLAVE {
        ioctl_pty(ctx, inode, request, argp)
    } else if kind == task::USER_FD_KIND_VFS && fs::inode_is_framebuffer(inode) {
        ioctl_framebuffer(ctx, request, argp)
    } else {
        -ENOTTY
    }
}
