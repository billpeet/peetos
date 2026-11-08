//! Event-style file descriptor objects: `eventfd`, `timerfd`, and `signalfd`.
//!
//! These are small, refcounted kernel objects referenced from task fd slots
//! (`USER_FD_KIND_EVENTFD` / `_TIMERFD` / `_SIGNALFD`, with the object index
//! stored in the slot's inode field). The syscall layer
//! (`kernel/src/syscall/eventfds.rs`) owns user-memory copies, blocking, and
//! fd-table plumbing; this module only holds object state and semantics so it
//! can be unit tested on the host.
//!
//! Time is passed in explicitly (`now_ns`, nanoseconds on the boot-relative
//! clock that backs both `CLOCK_MONOTONIC` and `CLOCK_REALTIME`), and readiness
//! is computed lazily whenever an object is read or polled. That fits the
//! kernel's poll/select/epoll loops, which re-evaluate readiness on every
//! scheduler pass instead of relying on wakeups.

use crate::sync::SpinLock;

const EAGAIN: i64 = 11;
const EBADF: i64 = 9;
const EINVAL: i64 = 22;
const EMFILE: i64 = 24;

pub const EVENTFD_MAX_OBJECTS: usize = 32;
pub const TIMERFD_MAX_OBJECTS: usize = 32;
pub const SIGNALFD_MAX_OBJECTS: usize = 16;

/// Largest value an eventfd counter may hold (`0xffff_ffff_ffff_fffe`).
pub const EVENTFD_COUNTER_MAX: u64 = u64::MAX - 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Event,
    Timer,
    Signal,
}

#[derive(Clone, Copy)]
struct EventFd {
    used: bool,
    refs: u16,
    counter: u64,
    semaphore: bool,
}

impl EventFd {
    const EMPTY: Self = Self {
        used: false,
        refs: 0,
        counter: 0,
        semaphore: false,
    };
}

#[derive(Clone, Copy)]
struct TimerFd {
    used: bool,
    refs: u16,
    clock_id: i32,
    /// Absolute deadline of the next expiration; 0 means disarmed.
    next_ns: u64,
    interval_ns: u64,
    /// Expirations accumulated since the last successful read.
    expirations: u64,
}

impl TimerFd {
    const EMPTY: Self = Self {
        used: false,
        refs: 0,
        clock_id: 0,
        next_ns: 0,
        interval_ns: 0,
        expirations: 0,
    };

    /// Fold any deadlines that have passed by `now_ns` into `expirations`.
    fn advance(&mut self, now_ns: u64) {
        if self.next_ns == 0 || now_ns < self.next_ns {
            return;
        }
        if self.interval_ns == 0 {
            self.expirations = self.expirations.saturating_add(1);
            self.next_ns = 0;
            return;
        }
        let missed = (now_ns - self.next_ns) / self.interval_ns + 1;
        self.expirations = self.expirations.saturating_add(missed);
        self.next_ns = self
            .next_ns
            .saturating_add(missed.saturating_mul(self.interval_ns));
    }

    fn remaining(&self, now_ns: u64) -> u64 {
        if self.next_ns == 0 {
            0
        } else {
            self.next_ns.saturating_sub(now_ns).max(1)
        }
    }
}

#[derive(Clone, Copy)]
struct SignalFd {
    used: bool,
    refs: u16,
    /// Signals this fd reports (Linux `sigset_t` layout: signal n is bit n-1).
    mask: u64,
}

impl SignalFd {
    const EMPTY: Self = Self {
        used: false,
        refs: 0,
        mask: 0,
    };
}

struct EvfdState {
    events: [EventFd; EVENTFD_MAX_OBJECTS],
    timers: [TimerFd; TIMERFD_MAX_OBJECTS],
    signals: [SignalFd; SIGNALFD_MAX_OBJECTS],
}

impl EvfdState {
    const fn new() -> Self {
        Self {
            events: [EventFd::EMPTY; EVENTFD_MAX_OBJECTS],
            timers: [TimerFd::EMPTY; TIMERFD_MAX_OBJECTS],
            signals: [SignalFd::EMPTY; SIGNALFD_MAX_OBJECTS],
        }
    }
}

static STATE: SpinLock<EvfdState> = SpinLock::new(EvfdState::new());

fn event_mut(state: &mut EvfdState, id: u16) -> Result<&mut EventFd, i64> {
    match state.events.get_mut(id as usize) {
        Some(obj) if obj.used => Ok(obj),
        _ => Err(-EBADF),
    }
}

fn timer_mut(state: &mut EvfdState, id: u16) -> Result<&mut TimerFd, i64> {
    match state.timers.get_mut(id as usize) {
        Some(obj) if obj.used => Ok(obj),
        _ => Err(-EBADF),
    }
}

fn signal_mut(state: &mut EvfdState, id: u16) -> Result<&mut SignalFd, i64> {
    match state.signals.get_mut(id as usize) {
        Some(obj) if obj.used => Ok(obj),
        _ => Err(-EBADF),
    }
}

/// Take an additional reference (fd duplication, fork, fd passing).
pub fn retain(kind: Kind, id: u16) -> Result<(), i64> {
    let mut state = STATE.lock();
    let refs = match kind {
        Kind::Event => &mut event_mut(&mut state, id)?.refs,
        Kind::Timer => &mut timer_mut(&mut state, id)?.refs,
        Kind::Signal => &mut signal_mut(&mut state, id)?.refs,
    };
    *refs = refs.checked_add(1).ok_or(-EMFILE)?;
    Ok(())
}

/// Drop a reference; the object is freed when the last one goes away.
pub fn release(kind: Kind, id: u16) {
    let mut state = STATE.lock();
    match kind {
        Kind::Event => {
            if let Ok(obj) = event_mut(&mut state, id) {
                obj.refs = obj.refs.saturating_sub(1);
                if obj.refs == 0 {
                    *obj = EventFd::EMPTY;
                }
            }
        }
        Kind::Timer => {
            if let Ok(obj) = timer_mut(&mut state, id) {
                obj.refs = obj.refs.saturating_sub(1);
                if obj.refs == 0 {
                    *obj = TimerFd::EMPTY;
                }
            }
        }
        Kind::Signal => {
            if let Ok(obj) = signal_mut(&mut state, id) {
                obj.refs = obj.refs.saturating_sub(1);
                if obj.refs == 0 {
                    *obj = SignalFd::EMPTY;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// eventfd
// ---------------------------------------------------------------------------

pub fn eventfd_create(initval: u32, semaphore: bool) -> Result<u16, i64> {
    let mut state = STATE.lock();
    let idx = state
        .events
        .iter()
        .position(|e| !e.used)
        .ok_or(-EMFILE)?;
    state.events[idx] = EventFd {
        used: true,
        refs: 1,
        counter: initval as u64,
        semaphore,
    };
    Ok(idx as u16)
}

/// Consume the counter. Returns `-EAGAIN` when it is zero (the caller blocks
/// or reports `EAGAIN` depending on `O_NONBLOCK`).
pub fn eventfd_read(id: u16) -> Result<u64, i64> {
    let mut state = STATE.lock();
    let obj = event_mut(&mut state, id)?;
    if obj.counter == 0 {
        return Err(-EAGAIN);
    }
    let value = if obj.semaphore { 1 } else { obj.counter };
    obj.counter -= value;
    Ok(value)
}

/// Add to the counter. Returns `-EAGAIN` if it would exceed
/// [`EVENTFD_COUNTER_MAX`], and `-EINVAL` for the reserved value `u64::MAX`.
pub fn eventfd_write(id: u16, value: u64) -> Result<(), i64> {
    if value == u64::MAX {
        return Err(-EINVAL);
    }
    let mut state = STATE.lock();
    let obj = event_mut(&mut state, id)?;
    if value > EVENTFD_COUNTER_MAX - obj.counter {
        return Err(-EAGAIN);
    }
    obj.counter += value;
    Ok(())
}

/// `(readable, writable)` readiness.
pub fn eventfd_poll(id: u16) -> Result<(bool, bool), i64> {
    let mut state = STATE.lock();
    let obj = event_mut(&mut state, id)?;
    Ok((obj.counter > 0, obj.counter < EVENTFD_COUNTER_MAX))
}

// ---------------------------------------------------------------------------
// timerfd
// ---------------------------------------------------------------------------

/// Current setting of a timer: time until next expiry and reload interval.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct TimerSetting {
    pub value_ns: u64,
    pub interval_ns: u64,
}

pub fn timerfd_create(clock_id: i32) -> Result<u16, i64> {
    let mut state = STATE.lock();
    let idx = state
        .timers
        .iter()
        .position(|t| !t.used)
        .ok_or(-EMFILE)?;
    state.timers[idx] = TimerFd {
        used: true,
        refs: 1,
        clock_id,
        ..TimerFd::EMPTY
    };
    Ok(idx as u16)
}

pub fn timerfd_clock(id: u16) -> Result<i32, i64> {
    let mut state = STATE.lock();
    Ok(timer_mut(&mut state, id)?.clock_id)
}

/// Arm or disarm a timer. `value_ns == 0` disarms. With `absolute`, the value
/// is a deadline on the timer's clock; otherwise it is relative to `now_ns`.
/// Returns the previous setting. Re-arming discards unread expirations.
pub fn timerfd_settime(
    id: u16,
    now_ns: u64,
    absolute: bool,
    value_ns: u64,
    interval_ns: u64,
) -> Result<TimerSetting, i64> {
    let mut state = STATE.lock();
    let obj = timer_mut(&mut state, id)?;
    obj.advance(now_ns);
    let old = TimerSetting {
        value_ns: obj.remaining(now_ns),
        interval_ns: obj.interval_ns,
    };
    obj.expirations = 0;
    obj.interval_ns = interval_ns;
    obj.next_ns = if value_ns == 0 {
        0
    } else if absolute {
        // A deadline already in the past expires on the next read/poll.
        value_ns.max(1)
    } else {
        now_ns.saturating_add(value_ns)
    };
    Ok(old)
}

pub fn timerfd_gettime(id: u16, now_ns: u64) -> Result<TimerSetting, i64> {
    let mut state = STATE.lock();
    let obj = timer_mut(&mut state, id)?;
    obj.advance(now_ns);
    Ok(TimerSetting {
        value_ns: obj.remaining(now_ns),
        interval_ns: obj.interval_ns,
    })
}

/// Consume accumulated expirations. `-EAGAIN` if none are pending.
pub fn timerfd_read(id: u16, now_ns: u64) -> Result<u64, i64> {
    let mut state = STATE.lock();
    let obj = timer_mut(&mut state, id)?;
    obj.advance(now_ns);
    if obj.expirations == 0 {
        return Err(-EAGAIN);
    }
    let count = obj.expirations;
    obj.expirations = 0;
    Ok(count)
}

pub fn timerfd_poll(id: u16, now_ns: u64) -> Result<bool, i64> {
    let mut state = STATE.lock();
    let obj = timer_mut(&mut state, id)?;
    obj.advance(now_ns);
    Ok(obj.expirations > 0)
}

/// Absolute deadline of the next expiry (0 if disarmed or already expired).
pub fn timerfd_next_deadline(id: u16) -> Result<u64, i64> {
    let mut state = STATE.lock();
    Ok(timer_mut(&mut state, id)?.next_ns)
}

// ---------------------------------------------------------------------------
// signalfd
// ---------------------------------------------------------------------------

pub fn signalfd_create(mask: u64) -> Result<u16, i64> {
    let mut state = STATE.lock();
    let idx = state
        .signals
        .iter()
        .position(|s| !s.used)
        .ok_or(-EMFILE)?;
    state.signals[idx] = SignalFd {
        used: true,
        refs: 1,
        mask,
    };
    Ok(idx as u16)
}

pub fn signalfd_set_mask(id: u16, mask: u64) -> Result<(), i64> {
    let mut state = STATE.lock();
    signal_mut(&mut state, id)?.mask = mask;
    Ok(())
}

pub fn signalfd_mask(id: u16) -> Result<u64, i64> {
    let mut state = STATE.lock();
    Ok(signal_mut(&mut state, id)?.mask)
}

/// Pick the lowest-numbered pending signal reported by a signalfd with `mask`.
/// Both masks use the Linux `sigset_t` layout (signal `n` is bit `n - 1`).
pub fn signalfd_pick(pending: u64, mask: u64) -> Option<u32> {
    let ready = pending & mask;
    if ready == 0 {
        None
    } else {
        Some(ready.trailing_zeros() + 1)
    }
}

#[cfg(test)]
pub(crate) fn reset_for_test() {
    *STATE.lock() = EvfdState::new();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn setup() -> std::sync::MutexGuard<'static, ()> {
        let guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_for_test();
        guard
    }

    const MS: u64 = 1_000_000;

    #[test]
    fn eventfd_counter_semantics() {
        let _g = setup();
        let id = eventfd_create(0, false).unwrap();
        assert_eq!(eventfd_read(id), Err(-EAGAIN));
        assert_eq!(eventfd_poll(id), Ok((false, true)));
        eventfd_write(id, 3).unwrap();
        eventfd_write(id, 4).unwrap();
        assert_eq!(eventfd_poll(id), Ok((true, true)));
        assert_eq!(eventfd_read(id), Ok(7));
        assert_eq!(eventfd_read(id), Err(-EAGAIN));
    }

    #[test]
    fn eventfd_semaphore_mode_decrements_by_one() {
        let _g = setup();
        let id = eventfd_create(2, true).unwrap();
        assert_eq!(eventfd_read(id), Ok(1));
        assert_eq!(eventfd_read(id), Ok(1));
        assert_eq!(eventfd_read(id), Err(-EAGAIN));
    }

    #[test]
    fn eventfd_rejects_overflow_and_reserved_value() {
        let _g = setup();
        let id = eventfd_create(0, false).unwrap();
        assert_eq!(eventfd_write(id, u64::MAX), Err(-EINVAL));
        eventfd_write(id, EVENTFD_COUNTER_MAX).unwrap();
        assert_eq!(eventfd_poll(id), Ok((true, false)));
        assert_eq!(eventfd_write(id, 1), Err(-EAGAIN));
    }

    #[test]
    fn refcount_frees_object_on_last_release() {
        let _g = setup();
        let id = eventfd_create(5, false).unwrap();
        retain(Kind::Event, id).unwrap();
        release(Kind::Event, id);
        assert_eq!(eventfd_read(id), Ok(5), "still alive after one release");
        release(Kind::Event, id);
        assert_eq!(eventfd_read(id), Err(-EBADF));
        assert_eq!(retain(Kind::Event, id), Err(-EBADF));
        // The slot is reusable.
        assert_eq!(eventfd_create(0, false), Ok(id));
    }

    #[test]
    fn tables_report_exhaustion() {
        let _g = setup();
        for _ in 0..SIGNALFD_MAX_OBJECTS {
            signalfd_create(0).unwrap();
        }
        assert_eq!(signalfd_create(0), Err(-EMFILE));
    }

    #[test]
    fn oneshot_timer_expires_once() {
        let _g = setup();
        let id = timerfd_create(1).unwrap();
        assert_eq!(timerfd_read(id, 0), Err(-EAGAIN), "disarmed");
        timerfd_settime(id, 100 * MS, false, 50 * MS, 0).unwrap();
        assert_eq!(timerfd_poll(id, 149 * MS), Ok(false));
        assert_eq!(timerfd_gettime(id, 120 * MS).unwrap().value_ns, 30 * MS);
        assert_eq!(timerfd_poll(id, 150 * MS), Ok(true));
        assert_eq!(timerfd_read(id, 400 * MS), Ok(1));
        assert_eq!(timerfd_read(id, 900 * MS), Err(-EAGAIN));
        assert_eq!(timerfd_gettime(id, 900 * MS), Ok(TimerSetting::default()));
    }

    #[test]
    fn periodic_timer_counts_missed_expirations() {
        let _g = setup();
        let id = timerfd_create(1).unwrap();
        timerfd_settime(id, 0, false, 10 * MS, 10 * MS).unwrap();
        // Deadlines at 10, 20, 30 ms have all passed by 35 ms.
        assert_eq!(timerfd_read(id, 35 * MS), Ok(3));
        assert_eq!(timerfd_gettime(id, 35 * MS).unwrap().value_ns, 5 * MS);
        assert_eq!(timerfd_read(id, 39 * MS), Err(-EAGAIN));
        assert_eq!(timerfd_read(id, 40 * MS), Ok(1));
    }

    #[test]
    fn absolute_timer_in_the_past_fires_immediately() {
        let _g = setup();
        let id = timerfd_create(0).unwrap();
        timerfd_settime(id, 500 * MS, true, 100 * MS, 0).unwrap();
        assert_eq!(timerfd_poll(id, 500 * MS), Ok(true));
        assert_eq!(timerfd_read(id, 500 * MS), Ok(1));
    }

    #[test]
    fn settime_returns_old_value_and_disarm_clears_pending() {
        let _g = setup();
        let id = timerfd_create(1).unwrap();
        timerfd_settime(id, 0, false, 100 * MS, 25 * MS).unwrap();
        let old = timerfd_settime(id, 40 * MS, false, 0, 0).unwrap();
        assert_eq!(
            old,
            TimerSetting {
                value_ns: 60 * MS,
                interval_ns: 25 * MS
            }
        );
        assert_eq!(timerfd_poll(id, 1_000 * MS), Ok(false), "disarmed");
        // Re-arming drops expirations accumulated under the old setting.
        timerfd_settime(id, 0, false, 1 * MS, 0).unwrap();
        timerfd_settime(id, 10 * MS, false, 50 * MS, 0).unwrap();
        assert_eq!(timerfd_read(id, 20 * MS), Err(-EAGAIN));
    }

    #[test]
    fn signalfd_picks_lowest_masked_pending_signal() {
        let _g = setup();
        let bit = |signum: u32| 1u64 << (signum - 1);
        let id = signalfd_create(bit(10)).unwrap();
        assert_eq!(signalfd_mask(id), Ok(bit(10)));
        signalfd_set_mask(id, bit(2) | bit(15)).unwrap();
        let mask = signalfd_mask(id).unwrap();
        assert_eq!(signalfd_pick(bit(10), mask), None);
        assert_eq!(signalfd_pick(bit(15) | bit(2) | bit(3), mask), Some(2));
        assert_eq!(signalfd_pick(bit(15), mask), Some(15));
        // Real-time signals above 32 are representable too.
        assert_eq!(signalfd_pick(bit(64), bit(64)), Some(64));
    }
}
