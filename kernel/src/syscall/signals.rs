//! Signal syscalls and user signal-frame delivery.

use super::*;

pub(super) fn read_sigaction_from_user(
    task_ptr: *mut task::Task,
    user_src: u64,
) -> Result<LinuxSigAction, i64> {
    let mut raw = [0u8; core::mem::size_of::<LinuxSigAction>()];
    copy_from_user_task(task_ptr, user_src, &mut raw)?;
    let ptr = raw.as_ptr() as *const LinuxSigAction;
    Ok(unsafe { ptr.read_unaligned() })
}

pub(super) fn write_sigaction_to_user(
    task_ptr: *mut task::Task,
    user_dst: u64,
    action: LinuxSigAction,
) -> Result<(), i64> {
    let bytes = unsafe {
        core::slice::from_raw_parts(
            (&action as *const LinuxSigAction) as *const u8,
            core::mem::size_of::<LinuxSigAction>(),
        )
    };
    copy_to_user_task(task_ptr, user_dst, bytes)
}

pub(super) fn sys_kill(_ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let pid = call.arg(0) as i64;
    let sig = call.arg(1) as i64;

    if pid <= 0 {
        return -EINVAL;
    }
    if sig < 0 {
        return -EINVAL;
    }

    let target = task::get_by_pid(pid as i32);
    if target.is_null() {
        return -ESRCH;
    }
    if sig == 0 {
        return 0;
    }

    match signal::send(target, sig as u32) {
        Ok(()) => 0,
        Err(err) => -signal_to_errno(err),
    }
}

pub(super) fn sys_rt_sigaction(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let signum = call.arg(0) as u32;
    let act_ptr = call.arg(1);
    let oldact_ptr = call.arg(2);
    let sigsetsize = call.arg(3);

    if sigsetsize != 8 {
        return -EINVAL;
    }

    if oldact_ptr != 0 {
        let old = match signal::read_action(ctx.current, signum) {
            Ok(old) => old,
            Err(err) => return -signal_to_errno(err),
        };
        let old_linux = LinuxSigAction {
            sa_handler: old.handler,
            sa_flags: old.flags,
            sa_restorer: old.restorer,
            sa_mask: old.mask,
        };
        if let Err(err) = write_sigaction_to_user(ctx.current, oldact_ptr, old_linux) {
            return -err;
        }
    }

    if act_ptr != 0 {
        let act = match read_sigaction_from_user(ctx.current, act_ptr) {
            Ok(act) => act,
            Err(err) => return -err,
        };
        let new_action = signal::SigAction {
            handler: act.sa_handler,
            flags: act.sa_flags,
            restorer: act.sa_restorer,
            mask: act.sa_mask,
        };
        if let Err(err) = signal::write_action(ctx.current, signum, new_action) {
            return -signal_to_errno(err);
        }
    }

    0
}

pub(super) fn sys_rt_sigprocmask(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let how = call.arg(0) as i32;
    let set_ptr = call.arg(1);
    let oldset_ptr = call.arg(2);
    let sigsetsize = call.arg(3);

    if sigsetsize != 8 {
        return -EINVAL;
    }

    if oldset_ptr != 0 {
        let mask = signal::current_mask(ctx.current);
        if let Err(err) = write_u64_to_user(ctx.current, oldset_ptr, mask) {
            return -err;
        }
    }

    if set_ptr != 0 {
        let set = match read_u64_from_user(ctx.current, set_ptr) {
            Ok(set) => set,
            Err(err) => return -err,
        };
        if let Err(err) = signal::update_mask(ctx.current, how, set) {
            return -signal_to_errno(err);
        }
    }

    0
}

pub(super) fn sys_rt_sigreturn(_ctx: &SyscallContext, _call: &SyscallArgs) -> i64 {
    // rt_sigreturn requires direct access to the live trap frame and is
    // handled in the entry stubs before table dispatch.
    -ENOSYS
}

pub(super) fn install_user_signal_frame_syscall(
    task_ptr: *mut task::Task,
    frame: &mut SyscallFrame,
    delivery: signal::UserDelivery,
) -> Result<(), i64> {
    let frame_size = size_of::<UserSignalFrameSyscall>() as u64;
    let frame_rsp = frame.rsp.checked_sub(frame_size).ok_or(EFAULT)?;
    let frame_rsp = align_down(frame_rsp, 16);
    let return_rsp = frame_rsp
        .checked_sub(size_of::<u64>() as u64)
        .ok_or(EFAULT)?;

    let signal_frame = UserSignalFrameSyscall {
        magic: SIGNAL_FRAME_MAGIC_SYSCALL,
        saved_mask: delivery.saved_mask,
        signum: delivery.signum as u64,
        r15: frame.r15,
        r14: frame.r14,
        r13: frame.r13,
        r12: frame.r12,
        rbp: frame.rbp,
        rbx: frame.rbx,
        r9: frame.r9,
        r8: frame.r8,
        r10: frame.r10,
        rdx: frame.rdx,
        rsi: frame.rsi,
        rdi: frame.rdi,
        rax: frame.rax,
        r11: frame.r11,
        rcx: frame.rcx,
        rsp: frame.rsp,
        fpu: capture_user_fpu(),
    };

    write_struct_to_user(task_ptr, frame_rsp, &signal_frame)?;
    write_u64_to_user(task_ptr, return_rsp, delivery.restorer)?;

    // Like Linux, the handler starts from a clean FPU/SSE state.
    fpu::reset_current();
    frame.rcx = delivery.handler;
    frame.rsp = return_rsp;
    frame.rdi = delivery.signum as u64;
    Ok(())
}

pub(super) fn install_user_signal_frame_int80(
    task_ptr: *mut task::Task,
    frame: &mut InterruptFrame,
    delivery: signal::UserDelivery,
) -> Result<(), i64> {
    let frame_size = size_of::<UserSignalFrameInt80>() as u64;
    let frame_rsp = frame.rsp.checked_sub(frame_size).ok_or(EFAULT)?;
    let frame_rsp = align_down(frame_rsp, 16);
    let return_rsp = frame_rsp
        .checked_sub(size_of::<u64>() as u64)
        .ok_or(EFAULT)?;

    let signal_frame = UserSignalFrameInt80 {
        magic: SIGNAL_FRAME_MAGIC_INT80,
        saved_mask: delivery.saved_mask,
        signum: delivery.signum as u64,
        r15: frame.r15,
        r14: frame.r14,
        r13: frame.r13,
        r12: frame.r12,
        r11: frame.r11,
        r10: frame.r10,
        r9: frame.r9,
        r8: frame.r8,
        rbp: frame.rbp,
        rdi: frame.rdi,
        rsi: frame.rsi,
        rdx: frame.rdx,
        rcx: frame.rcx,
        rbx: frame.rbx,
        rax: frame.rax,
        rip: frame.rip,
        cs: frame.cs,
        rflags: frame.rflags,
        rsp: frame.rsp,
        ss: frame.ss,
        fpu: capture_user_fpu(),
    };

    write_struct_to_user(task_ptr, frame_rsp, &signal_frame)?;
    write_u64_to_user(task_ptr, return_rsp, delivery.restorer)?;

    // Like Linux, the handler starts from a clean FPU/SSE state.
    fpu::reset_current();
    frame.rip = delivery.handler;
    frame.rsp = return_rsp;
    frame.rdi = delivery.signum as u64;
    Ok(())
}

pub(super) fn restore_user_signal_frame_syscall(
    task_ptr: *mut task::Task,
    frame: &mut SyscallFrame,
) -> i64 {
    let signal_frame = match read_struct_from_user::<UserSignalFrameSyscall>(task_ptr, frame.rsp) {
        Ok(signal_frame) => signal_frame,
        Err(err) => return -err,
    };
    if signal_frame.magic != SIGNAL_FRAME_MAGIC_SYSCALL {
        return -EINVAL;
    }
    if !user_range_ok(signal_frame.rsp, 1) || !user_range_ok(signal_frame.rcx, 1) {
        return -EFAULT;
    }
    if let Err(err) = signal::restore_mask(task_ptr, signal_frame.saved_mask) {
        return -signal_to_errno(err);
    }

    frame.r15 = signal_frame.r15;
    frame.r14 = signal_frame.r14;
    frame.r13 = signal_frame.r13;
    frame.r12 = signal_frame.r12;
    frame.rbp = signal_frame.rbp;
    frame.rbx = signal_frame.rbx;
    frame.r9 = signal_frame.r9;
    frame.r8 = signal_frame.r8;
    frame.r10 = signal_frame.r10;
    frame.rdx = signal_frame.rdx;
    frame.rsi = signal_frame.rsi;
    frame.rdi = signal_frame.rdi;
    frame.rax = signal_frame.rax;
    frame.r11 = signal_frame.r11;
    frame.rcx = signal_frame.rcx;
    frame.rsp = signal_frame.rsp;
    restore_user_fpu(signal_frame.fpu);
    0
}

pub(super) fn restore_user_signal_frame_int80(
    task_ptr: *mut task::Task,
    frame: &mut InterruptFrame,
) -> i64 {
    let signal_frame = match read_struct_from_user::<UserSignalFrameInt80>(task_ptr, frame.rsp) {
        Ok(signal_frame) => signal_frame,
        Err(err) => return -err,
    };
    if signal_frame.magic != SIGNAL_FRAME_MAGIC_INT80 {
        return -EINVAL;
    }
    if !user_range_ok(signal_frame.rsp, 1) || !user_range_ok(signal_frame.rip, 1) {
        return -EFAULT;
    }
    if let Err(err) = signal::restore_mask(task_ptr, signal_frame.saved_mask) {
        return -signal_to_errno(err);
    }

    frame.r15 = signal_frame.r15;
    frame.r14 = signal_frame.r14;
    frame.r13 = signal_frame.r13;
    frame.r12 = signal_frame.r12;
    frame.r11 = signal_frame.r11;
    frame.r10 = signal_frame.r10;
    frame.r9 = signal_frame.r9;
    frame.r8 = signal_frame.r8;
    frame.rbp = signal_frame.rbp;
    frame.rdi = signal_frame.rdi;
    frame.rsi = signal_frame.rsi;
    frame.rdx = signal_frame.rdx;
    frame.rcx = signal_frame.rcx;
    frame.rbx = signal_frame.rbx;
    frame.rax = signal_frame.rax;
    frame.rip = signal_frame.rip;
    frame.cs = signal_frame.cs;
    frame.rflags = signal_frame.rflags;
    frame.rsp = signal_frame.rsp;
    frame.ss = signal_frame.ss;
    restore_user_fpu(signal_frame.fpu);
    0
}

/// Snapshot the running task's live FPU/SSE registers for a signal frame.
pub(super) fn capture_user_fpu() -> FxSaveArea {
    let mut area = FxSaveArea::new_default();
    area.save();
    area
}

/// Load FPU/SSE state from a user-controlled signal frame. Reserved MXCSR
/// bits are cleared first so a corrupted frame cannot #GP in `FXRSTOR`.
pub(super) fn restore_user_fpu(mut area: FxSaveArea) {
    area.sanitize();
    area.restore();
}

pub(super) fn can_process_pending_signals(current: *mut task::Task) -> bool {
    if current.is_null() {
        return false;
    }
    unsafe { (*current).state != TaskState::Zombie && (*current).state != TaskState::Dead }
}

pub(super) fn apply_pending_signals_syscall(frame: &mut SyscallFrame) -> Option<i64> {
    let current = task::current();
    if !can_process_pending_signals(current) {
        return None;
    }

    loop {
        match signal::consume_next(current) {
            signal::DeliveryAction::None => return None,
            signal::DeliveryAction::Ignore => continue,
            signal::DeliveryAction::Terminate(code) => {
                let ctx = SyscallContext::new(current);
                return Some(sys_exit_common(&ctx, code));
            }
            signal::DeliveryAction::Handle(delivery) => {
                if install_user_signal_frame_syscall(current, frame, delivery).is_err() {
                    let ctx = SyscallContext::new(current);
                    return Some(sys_exit_common(&ctx, 128 + signal::SIGSEGV as i32));
                }
                return None;
            }
        }
    }
}

pub(super) fn apply_pending_signals_int80(frame: &mut InterruptFrame) -> Option<i64> {
    let current = task::current();
    if !can_process_pending_signals(current) {
        return None;
    }

    loop {
        match signal::consume_next(current) {
            signal::DeliveryAction::None => return None,
            signal::DeliveryAction::Ignore => continue,
            signal::DeliveryAction::Terminate(code) => {
                let ctx = SyscallContext::new(current);
                return Some(sys_exit_common(&ctx, code));
            }
            signal::DeliveryAction::Handle(delivery) => {
                if install_user_signal_frame_int80(current, frame, delivery).is_err() {
                    let ctx = SyscallContext::new(current);
                    return Some(sys_exit_common(&ctx, 128 + signal::SIGSEGV as i32));
                }
                return None;
            }
        }
    }
}
