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

/// Process id reported as the sender in `siginfo_t`.
fn sender_pid(ctx: &SyscallContext) -> i32 {
    if ctx.current.is_null() {
        0
    } else {
        unsafe { (*ctx.current).tgid }
    }
}

pub(super) fn sys_kill(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
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
    // Like Linux, signalling an exited but unreaped process succeeds and
    // has no effect.
    if sig == 0 || !group_has_live_thread(target) {
        return 0;
    }

    match signal::send_to_process_with_info(target, sig as u32, signal::SI_USER, sender_pid(ctx)) {
        Ok(()) => 0,
        Err(err) => -signal_to_errno(err),
    }
}

pub(super) fn group_has_live_thread(t: *mut task::Task) -> bool {
    task::live_threads_in_group(unsafe { (*t).tgid }) > 0
}

/// Send `sig` to one specific thread (tkill/tgkill).
pub(super) fn signal_thread(ctx: &SyscallContext, target: *mut task::Task, sig: i64) -> i64 {
    if target.is_null() || !task::is_live(target) {
        return -ESRCH;
    }
    if sig < 0 {
        return -EINVAL;
    }
    if sig == 0 {
        return 0;
    }
    match signal::send_with_info(target, sig as u32, signal::SI_TKILL, sender_pid(ctx)) {
        Ok(()) => 0,
        Err(err) => -signal_to_errno(err),
    }
}

pub(super) fn sys_tkill(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let tid = call.arg(0) as i64;
    if tid <= 0 || tid > i32::MAX as i64 {
        return -EINVAL;
    }
    signal_thread(ctx, task::get_by_pid(tid as i32), call.arg(1) as i64)
}

pub(super) fn sys_tgkill(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let tgid = call.arg(0) as i64;
    let tid = call.arg(1) as i64;
    if tgid <= 0 || tid <= 0 || tgid > i32::MAX as i64 || tid > i32::MAX as i64 {
        return -EINVAL;
    }
    let target = task::get_by_pid(tid as i32);
    if target.is_null() || unsafe { (*target).tgid } != tgid as i32 {
        return -ESRCH;
    }
    signal_thread(ctx, target, call.arg(2) as i64)
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

// ---------------------------------------------------------------------------
// Linux-compatible signal frames
// ---------------------------------------------------------------------------

/// Register state a syscall returns to, abstracted over the SYSCALL and
/// `int 0x80` entry frames so delivery, `rt_sigreturn` and syscall restart are
/// written once.
pub(super) trait UserRegs {
    fn to_gregs(&self) -> [u64; NGREG];
    /// Load registers from a (user-controlled) `ucontext`. Segment selectors
    /// are forced to user values and RFLAGS is sanitized.
    fn load_gregs(&mut self, gregs: &[u64; NGREG]);
    fn rip(&self) -> u64;
    fn set_rip(&mut self, rip: u64);
    fn result(&self) -> u64;
    fn set_result(&mut self, value: u64);
    fn rsp(&self) -> u64;
    fn enter_handler(&mut self, rip: u64, rsp: u64, args: [u64; 3]);
}

/// RFLAGS bits a signal handler may change through `uc_mcontext`
/// (CF, PF, AF, ZF, SF, DF, OF, AC). IF and the reserved bit 1 are forced.
const USER_RFLAGS_MASK: u64 = 0x1 | 0x4 | 0x10 | 0x40 | 0x80 | 0x400 | 0x800 | 0x4_0000;

pub(super) fn sanitize_rflags(rflags: u64) -> u64 {
    (rflags & USER_RFLAGS_MASK) | 0x202
}

fn user_csgsfs() -> u64 {
    (gdt::USER_CS as u64) | ((gdt::USER_DS as u64) << 48)
}

impl UserRegs for SyscallFrame {
    fn to_gregs(&self) -> [u64; NGREG] {
        let mut g = [0u64; NGREG];
        g[REG_R8] = self.r8;
        g[REG_R9] = self.r9;
        g[REG_R10] = self.r10;
        g[REG_R11] = self.r11;
        g[REG_R12] = self.r12;
        g[REG_R13] = self.r13;
        g[REG_R14] = self.r14;
        g[REG_R15] = self.r15;
        g[REG_RDI] = self.rdi;
        g[REG_RSI] = self.rsi;
        g[REG_RBP] = self.rbp;
        g[REG_RBX] = self.rbx;
        g[REG_RDX] = self.rdx;
        g[REG_RAX] = self.rax;
        g[REG_RCX] = self.rcx;
        g[REG_RSP] = self.rsp;
        // SYSCALL saved the return address in RCX and RFLAGS in R11.
        g[REG_RIP] = self.rcx;
        g[REG_EFL] = self.r11;
        g[REG_CSGSFS] = user_csgsfs();
        g
    }

    fn load_gregs(&mut self, g: &[u64; NGREG]) {
        self.r8 = g[REG_R8];
        self.r9 = g[REG_R9];
        self.r10 = g[REG_R10];
        self.r12 = g[REG_R12];
        self.r13 = g[REG_R13];
        self.r14 = g[REG_R14];
        self.r15 = g[REG_R15];
        self.rdi = g[REG_RDI];
        self.rsi = g[REG_RSI];
        self.rbp = g[REG_RBP];
        self.rbx = g[REG_RBX];
        self.rdx = g[REG_RDX];
        self.rax = g[REG_RAX];
        self.rsp = g[REG_RSP];
        // SYSRET returns through RCX/R11, so their own values cannot be
        // restored on this path (they are clobbered by `syscall` anyway).
        self.rcx = g[REG_RIP];
        self.r11 = sanitize_rflags(g[REG_EFL]);
    }

    fn rip(&self) -> u64 {
        self.rcx
    }
    fn set_rip(&mut self, rip: u64) {
        self.rcx = rip;
    }
    fn result(&self) -> u64 {
        self.rax
    }
    fn set_result(&mut self, value: u64) {
        self.rax = value;
    }
    fn rsp(&self) -> u64 {
        self.rsp
    }
    fn enter_handler(&mut self, rip: u64, rsp: u64, args: [u64; 3]) {
        self.rcx = rip;
        self.rsp = rsp;
        self.rdi = args[0];
        self.rsi = args[1];
        self.rdx = args[2];
        self.rax = 0;
    }
}

impl UserRegs for InterruptFrame {
    fn to_gregs(&self) -> [u64; NGREG] {
        let mut g = [0u64; NGREG];
        g[REG_R8] = self.r8;
        g[REG_R9] = self.r9;
        g[REG_R10] = self.r10;
        g[REG_R11] = self.r11;
        g[REG_R12] = self.r12;
        g[REG_R13] = self.r13;
        g[REG_R14] = self.r14;
        g[REG_R15] = self.r15;
        g[REG_RDI] = self.rdi;
        g[REG_RSI] = self.rsi;
        g[REG_RBP] = self.rbp;
        g[REG_RBX] = self.rbx;
        g[REG_RDX] = self.rdx;
        g[REG_RAX] = self.rax;
        g[REG_RCX] = self.rcx;
        g[REG_RSP] = self.rsp;
        g[REG_RIP] = self.rip;
        g[REG_EFL] = self.rflags;
        g[REG_CSGSFS] = user_csgsfs();
        g
    }

    fn load_gregs(&mut self, g: &[u64; NGREG]) {
        self.r8 = g[REG_R8];
        self.r9 = g[REG_R9];
        self.r10 = g[REG_R10];
        self.r11 = g[REG_R11];
        self.r12 = g[REG_R12];
        self.r13 = g[REG_R13];
        self.r14 = g[REG_R14];
        self.r15 = g[REG_R15];
        self.rdi = g[REG_RDI];
        self.rsi = g[REG_RSI];
        self.rbp = g[REG_RBP];
        self.rbx = g[REG_RBX];
        self.rdx = g[REG_RDX];
        self.rax = g[REG_RAX];
        self.rcx = g[REG_RCX];
        self.rsp = g[REG_RSP];
        self.rip = g[REG_RIP];
        self.rflags = sanitize_rflags(g[REG_EFL]);
        self.cs = gdt::USER_CS as u64;
        self.ss = gdt::USER_DS as u64;
    }

    fn rip(&self) -> u64 {
        self.rip
    }
    fn set_rip(&mut self, rip: u64) {
        self.rip = rip;
    }
    fn result(&self) -> u64 {
        self.rax
    }
    fn set_result(&mut self, value: u64) {
        self.rax = value;
    }
    fn rsp(&self) -> u64 {
        self.rsp
    }
    fn enter_handler(&mut self, rip: u64, rsp: u64, args: [u64; 3]) {
        self.rip = rip;
        self.rsp = rsp;
        self.rdi = args[0];
        self.rsi = args[1];
        self.rdx = args[2];
        self.rax = 0;
    }
}

/// Bytes below the interrupted stack pointer that the SysV ABI lets leaf
/// code use without adjusting RSP; the signal frame must not overwrite them.
const SIGNAL_RED_ZONE: u64 = 128;

/// Push a Linux `rt_sigframe` (return address, `ucontext_t`, `siginfo_t`,
/// FXSAVE area) on the user stack and redirect `regs` into the handler.
pub(super) fn install_rt_signal_frame<R: UserRegs>(
    task_ptr: *mut task::Task,
    regs: &mut R,
    delivery: signal::UserDelivery,
) -> Result<(), i64> {
    let mut sp = regs.rsp().checked_sub(SIGNAL_RED_ZONE).ok_or(EFAULT)?;

    // FPU/SSE state, 64-byte aligned above the frame (as Linux does).
    sp = sp.checked_sub(size_of::<FxSaveArea>() as u64).ok_or(EFAULT)?;
    sp = align_down(sp, 64);
    let fpstate_addr = sp;
    write_struct_to_user(task_ptr, fpstate_addr, &capture_user_fpu())?;

    // The frame starts with the return address, so at handler entry
    // RSP + 8 is 16-byte aligned, exactly as after a `call`.
    sp = sp.checked_sub(size_of::<LinuxRtSigFrame>() as u64).ok_or(EFAULT)?;
    sp = align_down(sp, 16).checked_sub(8).ok_or(EFAULT)?;
    if !user_range_ok(sp, size_of::<LinuxRtSigFrame>()) {
        return Err(EFAULT);
    }

    let frame = LinuxRtSigFrame {
        pretcode: delivery.restorer,
        uc: LinuxUcontext {
            uc_flags: 0,
            uc_link: 0,
            uc_stack: LinuxStackT {
                ss_sp: 0,
                ss_flags: SS_DISABLE,
                _pad: 0,
                ss_size: 0,
            },
            uc_mcontext: LinuxMcontext {
                gregs: regs.to_gregs(),
                fpregs: fpstate_addr,
                reserved: [0; 8],
            },
            uc_sigmask: delivery.saved_mask,
        },
        info: LinuxSigInfo::new(delivery.signum, delivery.code, delivery.sender),
    };
    write_struct_to_user(task_ptr, sp, &frame)?;

    // Like Linux, the handler starts from a clean FPU/SSE state.
    fpu::reset_current();
    let info_addr = sp + offset_of!(LinuxRtSigFrame, info) as u64;
    let uc_addr = sp + offset_of!(LinuxRtSigFrame, uc) as u64;
    regs.enter_handler(
        delivery.handler,
        sp,
        [delivery.signum as u64, info_addr, uc_addr],
    );
    Ok(())
}

/// `rt_sigreturn`: the restorer's `ret` already popped `pretcode`, so the
/// `ucontext_t` sits at the current user RSP. Registers, the signal mask and
/// FPU state come back from it, including any edits the handler made.
pub(super) fn restore_rt_signal_frame<R: UserRegs>(
    task_ptr: *mut task::Task,
    regs: &mut R,
) -> i64 {
    let uc = match read_struct_from_user::<LinuxUcontext>(task_ptr, regs.rsp()) {
        Ok(uc) => uc,
        Err(err) => return -err,
    };
    let gregs = uc.uc_mcontext.gregs;
    if !user_range_ok(gregs[REG_RSP], 1) || !user_range_ok(gregs[REG_RIP], 1) {
        return -EFAULT;
    }
    let fpstate = if uc.uc_mcontext.fpregs != 0 {
        match read_struct_from_user::<FxSaveArea>(task_ptr, uc.uc_mcontext.fpregs) {
            Ok(area) => Some(area),
            Err(err) => return -err,
        }
    } else {
        None
    };
    if let Err(err) = signal::restore_mask(task_ptr, uc.uc_sigmask) {
        return -signal_to_errno(err);
    }

    regs.load_gregs(&gregs);
    match fpstate {
        Some(area) => restore_user_fpu(area),
        None => fpu::reset_current(),
    }
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

/// A thread told to exit (exit_group/execve from a sibling) leaves here, on
/// its way back to user mode.
pub(super) fn exit_if_thread_killed(current: *mut task::Task) -> Option<i64> {
    if current.is_null() || unsafe { ((*current).flags & task::TASK_FLAG_EXITING) == 0 } {
        return None;
    }
    let code = unsafe { (*current).exit_code };
    let ctx = SyscallContext::new(current);
    Some(exit_current_thread(&ctx, code))
}

/// Arrange for the interrupted syscall `nr` to run again when user mode
/// resumes: rewind over the 2-byte `syscall` / `int 0x80` instruction.
pub(super) fn restart_syscall<R: UserRegs>(regs: &mut R, nr: u64) {
    regs.set_result(nr);
    regs.set_rip(regs.rip().wrapping_sub(2));
}

/// Deliver pending signals on the way back to user mode.
///
/// `nr` is the syscall being completed (None for `rt_sigreturn`). When it
/// returned `-ERESTARTSYS` (a wait interrupted by a signal), the syscall is
/// restarted if no handler runs or the handler has `SA_RESTART`, and
/// otherwise fails with `EINTR`.
pub(super) fn apply_pending_signals<R: UserRegs>(regs: &mut R, nr: Option<u64>) -> Option<i64> {
    let current = task::current();
    if !can_process_pending_signals(current) {
        return None;
    }
    if let Some(ret) = exit_if_thread_killed(current) {
        return Some(ret);
    }
    let restart_nr = nr.filter(|_| regs.result() == to_sysret(-ERESTARTSYS));

    loop {
        match signal::consume_next(current) {
            signal::DeliveryAction::None => {
                signal::restore_temporary_mask(current);
                if let Some(nr) = restart_nr {
                    restart_syscall(regs, nr);
                }
                return None;
            }
            signal::DeliveryAction::Ignore => continue,
            signal::DeliveryAction::Terminate(code) => {
                sys_log!(
                    "[SIGNAL] tid {} terminated by signal {}",
                    unsafe { (*current).pid },
                    code - 128
                );
                let ctx = SyscallContext::new(current);
                return Some(sys_exit_common(&ctx, exit_code_for_signal((code - 128) as u32)));
            }
            signal::DeliveryAction::Handle(delivery) => {
                if let Some(nr) = restart_nr {
                    if (delivery.flags & signal::SA_RESTART) != 0 {
                        restart_syscall(regs, nr);
                    } else {
                        regs.set_result(to_sysret(-EINTR));
                    }
                }
                if install_rt_signal_frame(current, regs, delivery).is_err() {
                    let ctx = SyscallContext::new(current);
                    return Some(sys_exit_common(&ctx, exit_code_for_signal(signal::SIGSEGV)));
                }
                return None;
            }
        }
    }
}

pub(super) fn apply_pending_signals_syscall(
    frame: &mut SyscallFrame,
    nr: Option<u64>,
) -> Option<i64> {
    apply_pending_signals(frame, nr)
}

pub(super) fn apply_pending_signals_int80(
    frame: &mut InterruptFrame,
    nr: Option<u64>,
) -> Option<i64> {
    apply_pending_signals(frame, nr)
}
