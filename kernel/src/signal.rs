use crate::task::{Task, TaskState, SIG_MAX_TASK_SIGNALS};

pub const EINVAL: i64 = 22;
pub const ESRCH: i64 = 3;
pub const ENOSYS: i64 = 38;
pub const EINTR: i64 = 4;
/// Kernel-internal errno (positive form): a blocking wait was interrupted by
/// a signal. The syscall layer restarts the call or turns this into `EINTR`.
pub const ERESTARTSYS: i64 = 512;

pub const SIGHUP: u32 = 1;
pub const SIGINT: u32 = 2;
pub const SIGQUIT: u32 = 3;
pub const SIGILL: u32 = 4;
pub const SIGTRAP: u32 = 5;
pub const SIGABRT: u32 = 6;
pub const SIGBUS: u32 = 7;
pub const SIGFPE: u32 = 8;
pub const SIGKILL: u32 = 9;
pub const SIGUSR1: u32 = 10;
pub const SIGSEGV: u32 = 11;
pub const SIGUSR2: u32 = 12;
pub const SIGPIPE: u32 = 13;
pub const SIGALRM: u32 = 14;
pub const SIGTERM: u32 = 15;
pub const SIGCHLD: u32 = 17;
pub const SIGCONT: u32 = 18;
pub const SIGSTOP: u32 = 19;
pub const SIGTSTP: u32 = 20;
pub const SIGTTIN: u32 = 21;
pub const SIGTTOU: u32 = 22;
pub const SIGURG: u32 = 23;
pub const SIGXCPU: u32 = 24;
pub const SIGXFSZ: u32 = 25;
pub const SIGVTALRM: u32 = 26;
pub const SIGPROF: u32 = 27;
pub const SIGWINCH: u32 = 28;
pub const SIGIO: u32 = 29;
pub const SIGPWR: u32 = 30;
pub const SIGSYS: u32 = 31;
/// First real-time signal number. musl reserves 32..=34 for timers,
/// thread cancellation and setxid.
pub const SIGRTMIN: u32 = 32;
/// Highest valid signal number (`_NSIG - 1`).
pub const SIGRTMAX: u32 = 64;

pub const SIG_DFL: u64 = 0;
pub const SIG_IGN: u64 = 1;

pub const SIG_BLOCK: i32 = 0;
pub const SIG_UNBLOCK: i32 = 1;
pub const SIG_SETMASK: i32 = 2;

pub const SA_SIGINFO: u64 = 0x0000_0004;
pub const SA_RESTORER: u64 = 0x0400_0000;
pub const SA_ONSTACK: u64 = 0x0800_0000;
pub const SA_RESTART: u64 = 0x1000_0000;
pub const SA_NODEFER: u64 = 0x4000_0000;
pub const SA_RESETHAND: u64 = 0x8000_0000;

/// `si_code` values recorded with a pending signal.
pub const SI_USER: i32 = 0;
pub const SI_KERNEL: i32 = 0x80;
pub const SI_TKILL: i32 = -6;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UserDelivery {
    pub signum: u32,
    pub handler: u64,
    pub restorer: u64,
    pub saved_mask: u64,
    /// The handler's `sa_flags` (SA_SIGINFO, SA_RESTART, ...).
    pub flags: u64,
    /// `si_code` and sending process for the `siginfo_t` given to SA_SIGINFO
    /// handlers.
    pub code: i32,
    pub sender: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeliveryAction {
    None,
    Ignore,
    Terminate(i32),
    Handle(UserDelivery),
}

#[derive(Clone, Copy, Debug)]
pub struct SigAction {
    pub handler: u64,
    pub flags: u64,
    pub restorer: u64,
    pub mask: u64,
}

const SIG_ACTION_TERMINATE: u8 = 0;
const SIG_ACTION_IGNORE: u8 = 1;

const DEFAULT_ACTIONS: [u8; SIG_MAX_TASK_SIGNALS] = default_actions();

/// Default dispositions: classic signals as listed below; every real-time
/// signal (32..=64) terminates.
const fn default_actions() -> [u8; SIG_MAX_TASK_SIGNALS] {
    let mut out = [SIG_ACTION_TERMINATE; SIG_MAX_TASK_SIGNALS];
    let mut idx = 0;
    while idx < CLASSIC_DEFAULT_ACTIONS.len() {
        out[idx] = CLASSIC_DEFAULT_ACTIONS[idx];
        idx += 1;
    }
    out
}

const CLASSIC_DEFAULT_ACTIONS: [u8; 32] = [
    SIG_ACTION_TERMINATE, // 0 (unused)
    SIG_ACTION_TERMINATE, // SIGHUP
    SIG_ACTION_TERMINATE, // SIGINT
    SIG_ACTION_TERMINATE, // SIGQUIT
    SIG_ACTION_TERMINATE, // SIGILL
    SIG_ACTION_TERMINATE, // SIGTRAP
    SIG_ACTION_TERMINATE, // SIGABRT
    SIG_ACTION_TERMINATE, // SIGBUS
    SIG_ACTION_TERMINATE, // SIGFPE
    SIG_ACTION_TERMINATE, // SIGKILL
    SIG_ACTION_TERMINATE, // SIGUSR1
    SIG_ACTION_TERMINATE, // SIGSEGV
    SIG_ACTION_TERMINATE, // SIGUSR2
    SIG_ACTION_TERMINATE, // SIGPIPE
    SIG_ACTION_TERMINATE, // SIGALRM
    SIG_ACTION_TERMINATE, // SIGTERM
    SIG_ACTION_IGNORE,    // 16 reserved
    SIG_ACTION_IGNORE,    // SIGCHLD
    SIG_ACTION_IGNORE,    // SIGCONT (continue not implemented yet)
    SIG_ACTION_IGNORE,    // SIGSTOP (stop not implemented yet)
    SIG_ACTION_IGNORE,    // SIGTSTP
    SIG_ACTION_IGNORE,    // SIGTTIN
    SIG_ACTION_IGNORE,    // SIGTTOU
    SIG_ACTION_IGNORE,    // SIGURG
    SIG_ACTION_TERMINATE, // SIGXCPU
    SIG_ACTION_TERMINATE, // SIGXFSZ
    SIG_ACTION_TERMINATE, // SIGVTALRM
    SIG_ACTION_TERMINATE, // SIGPROF
    SIG_ACTION_IGNORE,    // SIGWINCH
    SIG_ACTION_TERMINATE, // SIGIO
    SIG_ACTION_TERMINATE, // SIGPWR
    SIG_ACTION_TERMINATE, // SIGSYS
];

/// Bit for `signum` in a signal set. This is the Linux `sigset_t` layout
/// (signal N is bit N-1), so masks from userspace are used unchanged.
#[inline]
pub const fn signal_bit(signum: u32) -> u64 {
    1u64 << (signum - 1)
}

#[inline]
fn pack_info(code: i32, sender: i32) -> u64 {
    ((code as u32 as u64) << 32) | (sender as u32 as u64)
}

#[inline]
fn unpack_info(info: u64) -> (i32, i32) {
    ((info >> 32) as u32 as i32, info as u32 as i32)
}

#[inline]
pub fn valid_signal(signum: u32) -> bool {
    signum > 0 && (signum as usize) < SIG_MAX_TASK_SIGNALS
}

#[inline]
fn can_catch(signum: u32) -> bool {
    signum != SIGKILL && signum != SIGSTOP
}

#[inline]
fn default_action(signum: u32) -> u8 {
    if (signum as usize) < DEFAULT_ACTIONS.len() {
        DEFAULT_ACTIONS[signum as usize]
    } else {
        SIG_ACTION_TERMINATE
    }
}

/// True when `task`'s process discards `signum`: SIG_IGN, or SIG_DFL with
/// a default "ignore" action.
fn is_ignored(task: *mut Task, signum: u32) -> bool {
    if !can_catch(signum) {
        return false;
    }
    let handler = unsafe { (*crate::task::leader_of(task)).signal_handlers[signum as usize] };
    handler == SIG_IGN || (handler == SIG_DFL && default_action(signum) == SIG_ACTION_IGNORE)
}

/// Queue a kernel-generated signal on one thread.
pub fn send(task: *mut Task, signum: u32) -> Result<(), i64> {
    send_with_info(task, signum, SI_KERNEL, 0)
}

/// Queue a signal on one thread, recording its `si_code` and sender.
pub fn send_with_info(task: *mut Task, signum: u32, code: i32, sender: i32) -> Result<(), i64> {
    if task.is_null() {
        return Err(ESRCH);
    }
    if !valid_signal(signum) {
        return Err(EINVAL);
    }

    unsafe {
        // Like Linux, an ignored signal that is not blocked is discarded when
        // it is generated. A blocked one stays pending, because the
        // disposition may change before it is unblocked.
        if ((*task).signal_blocked & signal_bit(signum)) == 0 && is_ignored(task, signum) {
            return Ok(());
        }
        (*task).sig_info[signum as usize] = pack_info(code, sender);
        (*task).sig_pending |= signal_bit(signum);
        if ((*task).state == TaskState::Blocked || (*task).state == TaskState::Sleeping)
            && ((*task).signal_blocked & signal_bit(signum)) == 0
        {
            crate::sched::unblock(task);
        }
    }
    Ok(())
}

pub fn current_mask(task: *mut Task) -> u64 {
    if task.is_null() {
        return 0;
    }
    unsafe { (*task).signal_blocked }
}

pub fn update_mask(task: *mut Task, how: i32, set: u64) -> Result<(), i64> {
    if task.is_null() {
        return Err(ESRCH);
    }

    unsafe {
        match how {
            SIG_BLOCK => (*task).signal_blocked |= set,
            SIG_UNBLOCK => (*task).signal_blocked &= !set,
            SIG_SETMASK => (*task).signal_blocked = set,
            _ => return Err(EINVAL),
        }

        (*task).signal_blocked &= !signal_bit(SIGKILL);
        (*task).signal_blocked &= !signal_bit(SIGSTOP);
    }
    Ok(())
}

pub fn read_action(task: *mut Task, signum: u32) -> Result<SigAction, i64> {
    if task.is_null() {
        return Err(ESRCH);
    }
    if !valid_signal(signum) {
        return Err(EINVAL);
    }

    let idx = signum as usize;
    unsafe {
        Ok(SigAction {
            handler: (*crate::task::leader_of(task)).signal_handlers[idx],
            flags: (*crate::task::leader_of(task)).signal_flags[idx],
            restorer: (*crate::task::leader_of(task)).signal_restorer,
            mask: (*crate::task::leader_of(task)).signal_mask_saved[idx],
        })
    }
}

pub fn write_action(task: *mut Task, signum: u32, action: SigAction) -> Result<SigAction, i64> {
    if task.is_null() {
        return Err(ESRCH);
    }
    if !valid_signal(signum) {
        return Err(EINVAL);
    }
    if !can_catch(signum) {
        return Err(EINVAL);
    }

    let idx = signum as usize;
    let old = read_action(task, signum)?;
    unsafe {
        (*crate::task::leader_of(task)).signal_handlers[idx] = action.handler;
        (*crate::task::leader_of(task)).signal_flags[idx] = action.flags;
        (*crate::task::leader_of(task)).signal_mask_saved[idx] = action.mask;
        if (action.flags & SA_RESTORER) != 0 {
            (*crate::task::leader_of(task)).signal_restorer = action.restorer;
        }
    }
    Ok(old)
}

pub fn consume_next(task: *mut Task) -> DeliveryAction {
    if task.is_null() {
        return DeliveryAction::None;
    }

    let deliverable = unsafe { (*task).sig_pending & !(*task).signal_blocked };
    if deliverable == 0 {
        return DeliveryAction::None;
    }

    // Lowest-numbered deliverable signal first.
    let signum = deliverable.trailing_zeros() + 1;
    if !valid_signal(signum) {
        return DeliveryAction::None;
    }

    let idx = signum as usize;
    unsafe {
        (*task).sig_pending &= !signal_bit(signum);
        let leader = crate::task::leader_of(task);
        let handler = (*leader).signal_handlers[idx];
        if handler == SIG_IGN && can_catch(signum) {
            return DeliveryAction::Ignore;
        }
        if handler != SIG_DFL {
            let flags = (*leader).signal_flags[idx];
            let restorer = if (flags & SA_RESTORER) != 0 {
                (*leader).signal_restorer
            } else {
                0
            };
            if restorer == 0 {
                return DeliveryAction::Terminate(128 + signum as i32);
            }

            let saved_mask = (*task).signal_blocked;
            let mut next_mask = saved_mask | (*leader).signal_mask_saved[idx];
            if (flags & SA_NODEFER) == 0 {
                next_mask |= signal_bit(signum);
            }
            next_mask &= !signal_bit(SIGKILL);
            next_mask &= !signal_bit(SIGSTOP);
            (*task).signal_blocked = next_mask;

            if (flags & SA_RESETHAND) != 0 {
                (*leader).signal_handlers[idx] = SIG_DFL;
                (*leader).signal_flags[idx] &= !SA_SIGINFO;
            }

            let (code, sender) = unpack_info((*task).sig_info[idx]);
            return DeliveryAction::Handle(UserDelivery {
                signum,
                handler,
                restorer,
                saved_mask,
                flags,
                code,
                sender,
            });
        }
    }

    match default_action(signum) {
        SIG_ACTION_IGNORE => DeliveryAction::Ignore,
        _ => DeliveryAction::Terminate(128 + signum as i32),
    }
}

/// Choose which thread of `task`'s process receives a process-directed
/// signal: the leader if it is alive and not blocking the signal, otherwise
/// the first live thread that is not blocking it, otherwise any live thread.
fn process_signal_target(task: *mut Task, signum: u32) -> *mut Task {
    let leader = crate::task::leader_of(task);
    let bit = signal_bit(signum);
    let leader_live = crate::task::is_live(leader);
    unsafe {
        if leader_live && ((*leader).signal_blocked & bit) == 0 {
            return leader;
        }
    }

    let tgid = unsafe { (*leader).tgid };
    let mut fallback = if leader_live { leader } else { core::ptr::null_mut() };
    for idx in 0..crate::task::MAX_TASKS {
        let t = crate::task::get_by_index(idx);
        if t.is_null() || t == leader {
            continue;
        }
        unsafe {
            if (*t).tgid != tgid || !crate::task::is_live(t) {
                continue;
            }
            if ((*t).signal_blocked & bit) == 0 {
                return t;
            }
        }
        if fallback.is_null() {
            fallback = t;
        }
    }
    if fallback.is_null() {
        leader
    } else {
        fallback
    }
}

/// Send a signal to a whole process (kill(2) semantics): it is queued on one
/// thread chosen by `process_signal_target`.
pub fn send_to_process(task: *mut Task, signum: u32) -> Result<(), i64> {
    send_to_process_with_info(task, signum, SI_KERNEL, 0)
}

pub fn send_to_process_with_info(
    task: *mut Task,
    signum: u32,
    code: i32,
    sender: i32,
) -> Result<(), i64> {
    if task.is_null() {
        return Err(ESRCH);
    }
    if !valid_signal(signum) {
        return Err(EINVAL);
    }
    send_with_info(process_signal_target(task, signum), signum, code, sender)
}

/// True when a blocked task must give up waiting: it is being killed
/// (SIGKILL pending) or told to exit by a sibling thread.
pub fn must_abort_wait(task: *mut Task) -> bool {
    if task.is_null() {
        return false;
    }
    unsafe {
        ((*task).flags & crate::task::TASK_FLAG_EXITING) != 0
            || ((*task).sig_pending & signal_bit(SIGKILL)) != 0
    }
}

/// True when a blocked syscall must stop waiting: the task is dying, or a
/// signal is pending that will run a handler or terminate the process.
/// Blocked or ignored signals do not interrupt waits.
pub fn wait_interrupted(task: *mut Task) -> bool {
    if must_abort_wait(task) {
        return true;
    }
    if task.is_null() {
        return false;
    }
    let mut deliverable = unsafe { (*task).sig_pending & !(*task).signal_blocked };
    while deliverable != 0 {
        let signum = deliverable.trailing_zeros() + 1;
        deliverable &= deliverable - 1;
        if valid_signal(signum) && !is_ignored(task, signum) {
            return true;
        }
    }
    false
}

/// `wait_interrupted` for the running task.
pub fn current_wait_interrupted() -> bool {
    wait_interrupted(crate::task::current())
}

pub fn send_to_foreground_group(pgid: i32, signum: u32) {
    if !valid_signal(signum) {
        return;
    }
    for idx in 0..crate::task::MAX_TASKS {
        let task_ptr = crate::task::get_by_index(idx);
        if task_ptr.is_null() {
            continue;
        }
        unsafe {
            let task_flags = (*task_ptr).flags;
            // One delivery per process: visit thread group leaders only.
            if (task_flags & crate::task::TASK_FLAG_THREAD) != 0 {
                continue;
            }
            if (task_flags & crate::task::TASK_FLAG_USER) == 0 {
                continue;
            }
            if crate::task::live_threads_in_group((*task_ptr).tgid) == 0 {
                continue;
            }
            if pgid != 0 && (*task_ptr).pgid != pgid {
                continue;
            }
            let _ = send_to_process(task_ptr, signum);
        }
    }
}

pub fn restore_mask(task: *mut Task, mask: u64) -> Result<(), i64> {
    if task.is_null() {
        return Err(ESRCH);
    }

    unsafe {
        (*task).signal_blocked = mask;
        (*task).signal_blocked &= !signal_bit(SIGKILL);
        (*task).signal_blocked &= !signal_bit(SIGSTOP);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task;
    use std::sync::Mutex;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn must_abort_wait_tracks_sigkill_and_exiting_flag() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        let mut t = task::Task::for_test(1);
        let ptr = &mut t as *mut task::Task;
        assert!(!must_abort_wait(ptr));

        send(ptr, SIGTERM).expect("send");
        assert!(!must_abort_wait(ptr));

        send(ptr, SIGKILL).expect("send");
        assert!(must_abort_wait(ptr));

        t.sig_pending = 0;
        t.flags |= task::TASK_FLAG_EXITING;
        assert!(must_abort_wait(&mut t as *mut task::Task));
    }

    #[test]
    fn process_signal_prefers_unblocked_leader() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        let mut leader = task::Task::for_test(40);
        let ptr = &mut leader as *mut task::Task;
        send_to_process(ptr, SIGUSR1).expect("send");
        assert_ne!(leader.sig_pending & signal_bit(SIGUSR1), 0);
    }

    #[test]
    fn invalid_signal_is_rejected() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        let mut t = task::Task::for_test(1);
        let ptr = &mut t as *mut task::Task;
        assert_eq!(send(ptr, 0).unwrap_err(), EINVAL);
        assert_eq!(send(ptr, 65).unwrap_err(), EINVAL);
    }

    #[test]
    fn blocked_signal_waits_until_unblocked() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        let mut t = task::Task::for_test(1);
        let ptr = &mut t as *mut task::Task;

        update_mask(ptr, SIG_BLOCK, signal_bit(SIGTERM)).expect("block");
        send(ptr, SIGTERM).expect("send");
        assert_eq!(consume_next(ptr), DeliveryAction::None);

        update_mask(ptr, SIG_UNBLOCK, signal_bit(SIGTERM)).expect("unblock");
        assert_eq!(
            consume_next(ptr),
            DeliveryAction::Terminate(128 + SIGTERM as i32)
        );
    }

    #[test]
    fn ignored_signal_is_consumed() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        let mut t = task::Task::for_test(1);
        let ptr = &mut t as *mut task::Task;
        write_action(
            ptr,
            SIGTERM,
            SigAction {
                handler: SIG_IGN,
                flags: 0,
                restorer: 0,
                mask: 0,
            },
        )
        .expect("set ignore");
        // An unblocked ignored signal is discarded when sent...
        send(ptr, SIGTERM).expect("send");
        assert_eq!(t.sig_pending, 0);
        assert_eq!(consume_next(ptr), DeliveryAction::None);

        // ...but a blocked one stays pending and is ignored on unblock.
        update_mask(ptr, SIG_BLOCK, signal_bit(SIGTERM)).expect("block");
        send(ptr, SIGTERM).expect("send blocked");
        update_mask(ptr, SIG_UNBLOCK, signal_bit(SIGTERM)).expect("unblock");
        assert_eq!(consume_next(ptr), DeliveryAction::Ignore);
        assert_eq!(consume_next(ptr), DeliveryAction::None);
    }

    #[test]
    fn custom_handler_requires_restorer() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        let mut t = task::Task::for_test(1);
        let ptr = &mut t as *mut task::Task;
        write_action(
            ptr,
            SIGUSR1,
            SigAction {
                handler: 0x1000,
                flags: 0,
                restorer: 0,
                mask: 0,
            },
        )
        .expect("set handler");
        send(ptr, SIGUSR1).expect("send");
        assert_eq!(
            consume_next(ptr),
            DeliveryAction::Terminate(128 + SIGUSR1 as i32)
        );
    }

    #[test]
    fn custom_handler_delivery_saves_and_restores_mask() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        let mut t = task::Task::for_test(1);
        let ptr = &mut t as *mut task::Task;
        let handler = 0x1234_u64;
        let restorer = 0x5678_u64;
        let extra_mask = signal_bit(SIGTERM);

        write_action(
            ptr,
            SIGUSR1,
            SigAction {
                handler,
                flags: SA_RESTORER,
                restorer,
                mask: extra_mask,
            },
        )
        .expect("set handler");

        send(ptr, SIGUSR1).expect("send");
        let delivery = match consume_next(ptr) {
            DeliveryAction::Handle(delivery) => delivery,
            other => panic!("expected user delivery, got {:?}", other),
        };

        assert_eq!(delivery.signum, SIGUSR1);
        assert_eq!(delivery.handler, handler);
        assert_eq!(delivery.restorer, restorer);
        assert_eq!(delivery.saved_mask, 0);
        assert_eq!(current_mask(ptr), signal_bit(SIGUSR1) | extra_mask);

        restore_mask(ptr, delivery.saved_mask).expect("restore mask");
        assert_eq!(current_mask(ptr), 0);
    }

    #[test]
    fn default_sigchld_is_ignored() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        let mut t = task::Task::for_test(1);
        let ptr = &mut t as *mut task::Task;
        send(ptr, SIGCHLD).expect("send");
        assert_eq!(consume_next(ptr), DeliveryAction::None);
        assert!(!wait_interrupted(ptr));
    }

    fn set_handler(ptr: *mut task::Task, signum: u32, flags: u64) {
        write_action(
            ptr,
            signum,
            SigAction {
                handler: 0x4000,
                flags: flags | SA_RESTORER,
                restorer: 0x5000,
                mask: 0,
            },
        )
        .expect("set handler");
    }

    #[test]
    fn signal_sets_use_linux_bit_layout() {
        assert_eq!(signal_bit(1), 1);
        assert_eq!(signal_bit(SIGCHLD), 1 << 16);
        assert_eq!(signal_bit(SIGRTMAX), 1 << 63);
        assert!(valid_signal(33));
        assert!(valid_signal(SIGRTMAX));
        assert!(!valid_signal(0));
        assert!(!valid_signal(65));
    }

    #[test]
    fn realtime_signals_terminate_by_default_and_can_be_caught() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        let mut t = task::Task::for_test(1);
        let ptr = &mut t as *mut task::Task;
        send(ptr, 40).expect("send rt");
        assert_eq!(consume_next(ptr), DeliveryAction::Terminate(128 + 40));

        set_handler(ptr, 33, SA_SIGINFO | SA_RESTART);
        send_with_info(ptr, 33, SI_TKILL, 77).expect("send 33");
        match consume_next(ptr) {
            DeliveryAction::Handle(d) => {
                assert_eq!(d.signum, 33);
                assert_eq!(d.code, SI_TKILL);
                assert_eq!(d.sender, 77);
                assert_ne!(d.flags & SA_RESTART, 0);
                assert_ne!(t.signal_blocked & signal_bit(33), 0);
            }
            other => panic!("unexpected {:?}", other),
        }
    }

    #[test]
    fn nodefer_and_resethand_flags() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        let mut t = task::Task::for_test(1);
        let ptr = &mut t as *mut task::Task;
        set_handler(ptr, SIGUSR1, SA_NODEFER | SA_RESETHAND);
        send(ptr, SIGUSR1).expect("send");
        assert!(matches!(consume_next(ptr), DeliveryAction::Handle(_)));
        // SA_NODEFER: the signal is not blocked while its handler runs.
        assert_eq!(t.signal_blocked & signal_bit(SIGUSR1), 0);
        // SA_RESETHAND: the next one gets the default action.
        send(&mut t as *mut task::Task, SIGUSR1).expect("send again");
        assert_eq!(
            consume_next(&mut t as *mut task::Task),
            DeliveryAction::Terminate(128 + SIGUSR1 as i32)
        );
    }

    #[test]
    fn waits_are_interrupted_only_by_signals_that_will_act() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        let mut t = task::Task::for_test(1);
        let ptr = &mut t as *mut task::Task;
        set_handler(ptr, SIGUSR2, 0);

        update_mask(ptr, SIG_BLOCK, signal_bit(SIGUSR2)).expect("block");
        send(ptr, SIGUSR2).expect("send blocked");
        assert!(!wait_interrupted(ptr), "blocked signals do not interrupt");

        update_mask(ptr, SIG_UNBLOCK, signal_bit(SIGUSR2)).expect("unblock");
        assert!(wait_interrupted(ptr), "a handled signal interrupts");

        let _ = consume_next(ptr);
        assert!(!wait_interrupted(ptr));
        send(ptr, SIGTERM).expect("send fatal");
        assert!(wait_interrupted(ptr), "a fatal signal interrupts");
    }
}
