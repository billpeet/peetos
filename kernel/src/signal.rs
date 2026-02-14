use crate::task::{Task, TaskState, SIG_MAX_TASK_SIGNALS};

pub const EINVAL: i64 = 22;
pub const ESRCH: i64 = 3;
pub const ENOSYS: i64 = 38;

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

pub const SIG_DFL: u64 = 0;
pub const SIG_IGN: u64 = 1;

pub const SIG_BLOCK: i32 = 0;
pub const SIG_UNBLOCK: i32 = 1;
pub const SIG_SETMASK: i32 = 2;

pub const SA_RESTORER: u64 = 0x0400_0000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UserDelivery {
    pub signum: u32,
    pub handler: u64,
    pub restorer: u64,
    pub saved_mask: u64,
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

const DEFAULT_ACTIONS: [u8; SIG_MAX_TASK_SIGNALS] = [
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

#[inline]
fn signal_bit(signum: u32) -> u64 {
    1u64 << signum
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

pub fn send(task: *mut Task, signum: u32) -> Result<(), i64> {
    if task.is_null() {
        return Err(ESRCH);
    }
    if !valid_signal(signum) {
        return Err(EINVAL);
    }

    unsafe {
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
            handler: (*task).signal_handlers[idx],
            flags: (*task).signal_flags[idx],
            restorer: (*task).signal_restorer,
            mask: (*task).signal_mask_saved[idx],
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
        (*task).signal_handlers[idx] = action.handler;
        (*task).signal_flags[idx] = action.flags;
        (*task).signal_mask_saved[idx] = action.mask;
        if (action.flags & SA_RESTORER) != 0 {
            (*task).signal_restorer = action.restorer;
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

    let mut signum = 0u32;
    let mut candidate = 1u32;
    while (candidate as usize) < SIG_MAX_TASK_SIGNALS {
        if (deliverable & signal_bit(candidate)) != 0 {
            signum = candidate;
            break;
        }
        candidate += 1;
    }
    if signum == 0 {
        return DeliveryAction::None;
    }

    let idx = signum as usize;
    unsafe {
        (*task).sig_pending &= !signal_bit(signum);
        let handler = (*task).signal_handlers[idx];
        if handler == SIG_IGN && can_catch(signum) {
            return DeliveryAction::Ignore;
        }
        if handler != SIG_DFL {
            let flags = (*task).signal_flags[idx];
            let restorer = if (flags & SA_RESTORER) != 0 {
                (*task).signal_restorer
            } else {
                0
            };
            if restorer == 0 {
                return DeliveryAction::Terminate(128 + signum as i32);
            }

            let saved_mask = (*task).signal_blocked;
            let mut next_mask = saved_mask | (*task).signal_mask_saved[idx];
            next_mask |= signal_bit(signum);
            next_mask &= !signal_bit(SIGKILL);
            next_mask &= !signal_bit(SIGSTOP);
            (*task).signal_blocked = next_mask;

            return DeliveryAction::Handle(UserDelivery {
                signum,
                handler,
                restorer,
                saved_mask,
            });
        }
    }

    match default_action(signum) {
        SIG_ACTION_IGNORE => DeliveryAction::Ignore,
        _ => DeliveryAction::Terminate(128 + signum as i32),
    }
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
            let task_state = (*task_ptr).state;
            let task_pgid = (*task_ptr).pgid;
            let task_flags = (*task_ptr).flags;
            if task_state == TaskState::Dead || task_state == TaskState::Zombie {
                continue;
            }
            if (task_flags & crate::task::TASK_FLAG_USER) == 0 {
                continue;
            }
            if pgid != 0 && task_pgid != pgid {
                continue;
            }
            let _ = send(task_ptr, signum);
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
    fn invalid_signal_is_rejected() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        let mut t = task::Task::for_test(1);
        let ptr = &mut t as *mut task::Task;
        assert_eq!(send(ptr, 0).unwrap_err(), EINVAL);
        assert_eq!(send(ptr, 32).unwrap_err(), EINVAL);
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
        send(ptr, SIGTERM).expect("send");
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
        assert_eq!(consume_next(ptr), DeliveryAction::Ignore);
    }
}
