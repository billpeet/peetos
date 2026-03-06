#[cfg(target_os = "none")]
use arch_x86_64::pit;

use core::mem::size_of;
use core::ptr;
#[cfg(not(target_os = "none"))]
use core::sync::atomic::{AtomicU64, Ordering};

use crate::sync::SpinLock;
use crate::task::Task;

pub const EAGAIN: i64 = 11;
pub const EINVAL: i64 = 22;

pub const DEVICE_KEYBOARD: u16 = 0;
pub const DEVICE_POINTER: u16 = 1;

pub const EV_SYN: u16 = 0x00;
pub const EV_KEY: u16 = 0x01;
pub const EV_REL: u16 = 0x02;
pub const EV_ABS: u16 = 0x03;

pub const SYN_REPORT: u16 = 0;
pub const SYN_DROPPED: u16 = 3;

pub const REL_X: u16 = 0x00;
pub const REL_Y: u16 = 0x01;

pub const ABS_X: u16 = 0x00;
pub const ABS_Y: u16 = 0x01;

pub const KEY_ESC: u16 = 1;
pub const KEY_1: u16 = 2;
pub const KEY_2: u16 = 3;
pub const KEY_3: u16 = 4;
pub const KEY_4: u16 = 5;
pub const KEY_5: u16 = 6;
pub const KEY_6: u16 = 7;
pub const KEY_7: u16 = 8;
pub const KEY_8: u16 = 9;
pub const KEY_9: u16 = 10;
pub const KEY_0: u16 = 11;
pub const KEY_MINUS: u16 = 12;
pub const KEY_EQUAL: u16 = 13;
pub const KEY_BACKSPACE: u16 = 14;
pub const KEY_TAB: u16 = 15;
pub const KEY_Q: u16 = 16;
pub const KEY_W: u16 = 17;
pub const KEY_E: u16 = 18;
pub const KEY_R: u16 = 19;
pub const KEY_T: u16 = 20;
pub const KEY_Y: u16 = 21;
pub const KEY_U: u16 = 22;
pub const KEY_I: u16 = 23;
pub const KEY_O: u16 = 24;
pub const KEY_P: u16 = 25;
pub const KEY_LEFTBRACE: u16 = 26;
pub const KEY_RIGHTBRACE: u16 = 27;
pub const KEY_ENTER: u16 = 28;
pub const KEY_LEFTCTRL: u16 = 29;
pub const KEY_A: u16 = 30;
pub const KEY_S: u16 = 31;
pub const KEY_D: u16 = 32;
pub const KEY_F: u16 = 33;
pub const KEY_G: u16 = 34;
pub const KEY_H: u16 = 35;
pub const KEY_J: u16 = 36;
pub const KEY_K: u16 = 37;
pub const KEY_L: u16 = 38;
pub const KEY_SEMICOLON: u16 = 39;
pub const KEY_APOSTROPHE: u16 = 40;
pub const KEY_GRAVE: u16 = 41;
pub const KEY_LEFTSHIFT: u16 = 42;
pub const KEY_BACKSLASH: u16 = 43;
pub const KEY_Z: u16 = 44;
pub const KEY_X: u16 = 45;
pub const KEY_C: u16 = 46;
pub const KEY_V: u16 = 47;
pub const KEY_B: u16 = 48;
pub const KEY_N: u16 = 49;
pub const KEY_M: u16 = 50;
pub const KEY_COMMA: u16 = 51;
pub const KEY_DOT: u16 = 52;
pub const KEY_SLASH: u16 = 53;
pub const KEY_RIGHTSHIFT: u16 = 54;
pub const KEY_LEFTALT: u16 = 56;
pub const KEY_SPACE: u16 = 57;
pub const KEY_CAPSLOCK: u16 = 58;
pub const KEY_F1: u16 = 59;
pub const KEY_F2: u16 = 60;
pub const KEY_F3: u16 = 61;
pub const KEY_F4: u16 = 62;
pub const KEY_F5: u16 = 63;
pub const KEY_F6: u16 = 64;
pub const KEY_F7: u16 = 65;
pub const KEY_F8: u16 = 66;
pub const KEY_F9: u16 = 67;
pub const KEY_F10: u16 = 68;
pub const KEY_NUMLOCK: u16 = 69;
pub const KEY_SCROLLLOCK: u16 = 70;
pub const KEY_F11: u16 = 87;
pub const KEY_F12: u16 = 88;
pub const KEY_KPENTER: u16 = 96;
pub const KEY_RIGHTCTRL: u16 = 97;
pub const KEY_KPSLASH: u16 = 98;
pub const KEY_RIGHTALT: u16 = 100;
pub const KEY_HOME: u16 = 102;
pub const KEY_UP: u16 = 103;
pub const KEY_PAGEUP: u16 = 104;
pub const KEY_LEFT: u16 = 105;
pub const KEY_RIGHT: u16 = 106;
pub const KEY_END: u16 = 107;
pub const KEY_DOWN: u16 = 108;
pub const KEY_PAGEDOWN: u16 = 109;
pub const KEY_INSERT: u16 = 110;
pub const KEY_DELETE: u16 = 111;

pub const BTN_LEFT: u16 = 0x110;
pub const BTN_RIGHT: u16 = 0x111;
pub const BTN_MIDDLE: u16 = 0x112;

const INPUT_QUEUE_CAPACITY: usize = 128;
const INPUT_MAX_WAITERS: usize = 8;
const INPUT_WAKE_BATCH: usize = 8;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LinuxInputEvent {
    pub tv_sec: i64,
    pub tv_usec: i64,
    pub type_: u16,
    pub code: u16,
    pub value: i32,
}

impl LinuxInputEvent {
    const EMPTY: Self = Self {
        tv_sec: 0,
        tv_usec: 0,
        type_: 0,
        code: 0,
        value: 0,
    };
}

#[derive(Clone, Copy)]
struct Waiter {
    used: bool,
    task: usize,
}

impl Waiter {
    const fn empty() -> Self {
        Self {
            used: false,
            task: 0,
        }
    }
}

#[derive(Clone, Copy)]
struct InputQueue {
    head: u16,
    tail: u16,
    len: u16,
    dropped: bool,
    events: [LinuxInputEvent; INPUT_QUEUE_CAPACITY],
    waiters: [Waiter; INPUT_MAX_WAITERS],
}

impl InputQueue {
    const fn new() -> Self {
        Self {
            head: 0,
            tail: 0,
            len: 0,
            dropped: false,
            events: [LinuxInputEvent::EMPTY; INPUT_QUEUE_CAPACITY],
            waiters: [Waiter::empty(); INPUT_MAX_WAITERS],
        }
    }

    fn reset(&mut self) {
        *self = Self::new();
    }

    fn available(&self) -> usize {
        INPUT_QUEUE_CAPACITY - self.len as usize
    }

    fn push_event(&mut self, event: LinuxInputEvent) {
        let idx = self.head as usize;
        self.events[idx] = event;
        self.head = ((idx + 1) % INPUT_QUEUE_CAPACITY) as u16;
        self.len += 1;
    }

    fn pop_event(&mut self) -> Option<LinuxInputEvent> {
        if self.len == 0 {
            return None;
        }
        let idx = self.tail as usize;
        let event = self.events[idx];
        self.tail = ((idx + 1) % INPUT_QUEUE_CAPACITY) as u16;
        self.len -= 1;
        Some(event)
    }

    fn register_waiter(&mut self, task: *mut Task) -> Result<(), i64> {
        if task.is_null() {
            return Err(EAGAIN);
        }
        let mut idx = 0usize;
        while idx < INPUT_MAX_WAITERS {
            let waiter = &self.waiters[idx];
            if waiter.used && waiter.task == task as usize {
                return Ok(());
            }
            idx += 1;
        }
        idx = 0;
        while idx < INPUT_MAX_WAITERS {
            if !self.waiters[idx].used {
                self.waiters[idx] = Waiter {
                    used: true,
                    task: task as usize,
                };
                return Ok(());
            }
            idx += 1;
        }
        Err(EAGAIN)
    }

    fn remove_waiter(&mut self, task: *mut Task) {
        let mut idx = 0usize;
        while idx < INPUT_MAX_WAITERS {
            let waiter = &mut self.waiters[idx];
            if waiter.used && waiter.task == task as usize {
                *waiter = Waiter::empty();
            }
            idx += 1;
        }
    }

    fn collect_waiters(&mut self, out: &mut [*mut Task; INPUT_WAKE_BATCH]) -> usize {
        let mut count = 0usize;
        let mut idx = 0usize;
        while idx < INPUT_MAX_WAITERS && count < out.len() {
            let waiter = &mut self.waiters[idx];
            if waiter.used && waiter.task != 0 {
                out[count] = waiter.task as *mut Task;
                *waiter = Waiter::empty();
                count += 1;
            }
            idx += 1;
        }
        count
    }
}

#[derive(Clone, Copy)]
struct PointerState {
    buttons: u8,
    abs_x: i32,
    abs_y: i32,
    abs_valid: bool,
}

impl PointerState {
    const fn new() -> Self {
        Self {
            buttons: 0,
            abs_x: 0,
            abs_y: 0,
            abs_valid: false,
        }
    }
}

#[derive(Clone, Copy)]
struct InputState {
    keyboard: InputQueue,
    pointer: InputQueue,
    pointer_state: PointerState,
}

impl InputState {
    const fn new() -> Self {
        Self {
            keyboard: InputQueue::new(),
            pointer: InputQueue::new(),
            pointer_state: PointerState::new(),
        }
    }

    fn queue_mut(&mut self, device_id: u16) -> Option<&mut InputQueue> {
        match device_id {
            DEVICE_KEYBOARD => Some(&mut self.keyboard),
            DEVICE_POINTER => Some(&mut self.pointer),
            _ => None,
        }
    }
}

static INPUT_STATE: SpinLock<InputState> = SpinLock::new(InputState::new());
#[cfg(not(target_os = "none"))]
static TEST_TIMESTAMP_MS: AtomicU64 = AtomicU64::new(0);

fn event_with_timestamp(ts: (i64, i64), type_: u16, code: u16, value: i32) -> LinuxInputEvent {
    LinuxInputEvent {
        tv_sec: ts.0,
        tv_usec: ts.1,
        type_,
        code,
        value,
    }
}

#[cfg(target_os = "none")]
fn current_timestamp() -> (i64, i64) {
    let ms = pit::uptime_ms();
    ((ms / 1000) as i64, ((ms % 1000) * 1000) as i64)
}

#[cfg(not(target_os = "none"))]
fn current_timestamp() -> (i64, i64) {
    let ms = TEST_TIMESTAMP_MS.fetch_add(1, Ordering::Relaxed);
    ((ms / 1000) as i64, ((ms % 1000) * 1000) as i64)
}

pub fn init() {
    let mut state = INPUT_STATE.lock();
    *state = InputState::new();
    #[cfg(not(target_os = "none"))]
    TEST_TIMESTAMP_MS.store(0, Ordering::Relaxed);
}

fn enqueue_packet(
    queue: &mut InputQueue,
    packet: &[LinuxInputEvent],
    dropped_event: LinuxInputEvent,
    waiters: &mut [*mut Task; INPUT_WAKE_BATCH],
) -> usize {
    let required = packet.len() + usize::from(queue.dropped);
    if required > queue.available() {
        queue.dropped = true;
        return 0;
    }

    if queue.dropped {
        queue.push_event(dropped_event);
        queue.dropped = false;
    }
    for event in packet {
        queue.push_event(*event);
    }

    queue.collect_waiters(waiters)
}

fn wake_tasks(tasks: &[*mut Task], count: usize) {
    let mut idx = 0usize;
    while idx < count {
        wake_task(tasks[idx]);
        idx += 1;
    }
}

fn append_button_changes(
    prev_buttons: u8,
    next_buttons: u8,
    ts: (i64, i64),
    out: &mut [LinuxInputEvent],
) -> usize {
    let mappings = [(0u8, BTN_LEFT), (1u8, BTN_RIGHT), (2u8, BTN_MIDDLE)];
    let mut count = 0usize;
    for (bit, code) in mappings {
        let mask = 1u8 << bit;
        if (prev_buttons & mask) != (next_buttons & mask) {
            out[count] =
                event_with_timestamp(ts, EV_KEY, code, i32::from((next_buttons & mask) != 0));
            count += 1;
        }
    }
    count
}

pub fn push_keyboard_key(code: u16, pressed: bool) {
    let ts = current_timestamp();
    let mut waiters = [ptr::null_mut::<Task>(); INPUT_WAKE_BATCH];
    let dropped_event = event_with_timestamp(ts, EV_SYN, SYN_DROPPED, 0);
    let packet = [
        event_with_timestamp(ts, EV_KEY, code, i32::from(pressed)),
        event_with_timestamp(ts, EV_SYN, SYN_REPORT, 0),
    ];

    let woke = {
        let mut state = INPUT_STATE.lock();
        enqueue_packet(&mut state.keyboard, &packet, dropped_event, &mut waiters)
    };
    if woke > 0 {
        wake_tasks(&waiters, woke);
    }
}

pub fn push_pointer_relative(dx: i16, dy: i16, buttons: u8) {
    let ts = current_timestamp();
    let dropped_event = event_with_timestamp(ts, EV_SYN, SYN_DROPPED, 0);
    let mut packet = [LinuxInputEvent::EMPTY; 6];
    let mut waiters = [ptr::null_mut::<Task>(); INPUT_WAKE_BATCH];

    let woke = {
        let mut state = INPUT_STATE.lock();
        let mut count = 0usize;
        if dx != 0 {
            packet[count] = event_with_timestamp(ts, EV_REL, REL_X, dx as i32);
            count += 1;
        }
        if dy != 0 {
            packet[count] = event_with_timestamp(ts, EV_REL, REL_Y, dy as i32);
            count += 1;
        }
        count += append_button_changes(
            state.pointer_state.buttons,
            buttons,
            ts,
            &mut packet[count..],
        );
        state.pointer_state.buttons = buttons;
        if count == 0 {
            0
        } else {
            packet[count] = event_with_timestamp(ts, EV_SYN, SYN_REPORT, 0);
            count += 1;

            enqueue_packet(
                &mut state.pointer,
                &packet[..count],
                dropped_event,
                &mut waiters,
            )
        }
    };
    if woke > 0 {
        wake_tasks(&waiters, woke);
    }
}

pub fn push_pointer_absolute(x: i32, y: i32, buttons: u8) {
    let ts = current_timestamp();
    let dropped_event = event_with_timestamp(ts, EV_SYN, SYN_DROPPED, 0);
    let mut packet = [LinuxInputEvent::EMPTY; 6];
    let mut waiters = [ptr::null_mut::<Task>(); INPUT_WAKE_BATCH];

    let woke = {
        let mut state = INPUT_STATE.lock();
        let mut count = 0usize;

        if !state.pointer_state.abs_valid || state.pointer_state.abs_x != x {
            packet[count] = event_with_timestamp(ts, EV_ABS, ABS_X, x);
            count += 1;
        }
        if !state.pointer_state.abs_valid || state.pointer_state.abs_y != y {
            packet[count] = event_with_timestamp(ts, EV_ABS, ABS_Y, y);
            count += 1;
        }
        count += append_button_changes(
            state.pointer_state.buttons,
            buttons,
            ts,
            &mut packet[count..],
        );

        state.pointer_state.buttons = buttons;
        state.pointer_state.abs_x = x;
        state.pointer_state.abs_y = y;
        state.pointer_state.abs_valid = true;

        if count == 0 {
            0
        } else {
            packet[count] = event_with_timestamp(ts, EV_SYN, SYN_REPORT, 0);
            count += 1;

            enqueue_packet(
                &mut state.pointer,
                &packet[..count],
                dropped_event,
                &mut waiters,
            )
        }
    };
    if woke > 0 {
        wake_tasks(&waiters, woke);
    }
}

pub fn device_has_ready_data(device_id: u16) -> bool {
    let state = INPUT_STATE.lock();
    match device_id {
        DEVICE_KEYBOARD => state.keyboard.len > 0,
        DEVICE_POINTER => state.pointer.len > 0,
        _ => false,
    }
}

pub fn read_device(
    task: *mut Task,
    device_id: u16,
    out: &mut [u8],
    nonblocking: bool,
) -> Result<usize, i64> {
    let event_size = size_of::<LinuxInputEvent>();
    if out.len() < event_size {
        return Err(EINVAL);
    }

    loop {
        {
            let mut state = INPUT_STATE.lock();
            let queue = match state.queue_mut(device_id) {
                Some(queue) => queue,
                None => return Err(EINVAL),
            };

            let max_events = out.len() / event_size;
            let mut count = 0usize;
            while count < max_events {
                let Some(event) = queue.pop_event() else {
                    break;
                };
                let start = count * event_size;
                let bytes = unsafe {
                    core::slice::from_raw_parts(
                        (&event as *const LinuxInputEvent).cast::<u8>(),
                        event_size,
                    )
                };
                out[start..start + event_size].copy_from_slice(bytes);
                count += 1;
            }

            if count > 0 {
                return Ok(count * event_size);
            }

            if nonblocking || !can_block(task) {
                return Err(EAGAIN);
            }

            queue.register_waiter(task)?;
        }

        block_current("input-read");
        let mut state = INPUT_STATE.lock();
        if let Some(queue) = state.queue_mut(device_id) {
            queue.remove_waiter(task);
        }
    }
}

#[cfg(not(test))]
fn block_current(reason: &str) {
    crate::sched::block(reason);
}

#[cfg(test)]
fn block_current(_reason: &str) {}

#[cfg(not(test))]
fn wake_task(task: *mut Task) {
    if !task.is_null() {
        crate::sched::unblock(task);
    }
}

#[cfg(test)]
fn wake_task(_task: *mut Task) {}

#[cfg(not(test))]
#[inline]
fn can_block(task: *mut Task) -> bool {
    !task.is_null() && crate::sched::is_running()
}

#[cfg(test)]
#[inline]
fn can_block(_task: *mut Task) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn decode_event(bytes: &[u8]) -> LinuxInputEvent {
        assert_eq!(bytes.len(), size_of::<LinuxInputEvent>());
        unsafe { ptr::read_unaligned(bytes.as_ptr().cast::<LinuxInputEvent>()) }
    }

    #[test]
    fn keyboard_events_are_queued_in_order() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        init();

        push_keyboard_key(KEY_A, true);

        let mut buf = [0u8; size_of::<LinuxInputEvent>() * 2];
        let read =
            read_device(ptr::null_mut(), DEVICE_KEYBOARD, &mut buf, true).expect("keyboard read");
        assert_eq!(read, buf.len());

        let ev0 = decode_event(&buf[..size_of::<LinuxInputEvent>()]);
        let ev1 = decode_event(&buf[size_of::<LinuxInputEvent>()..]);
        assert_eq!(ev0.type_, EV_KEY);
        assert_eq!(ev0.code, KEY_A);
        assert_eq!(ev0.value, 1);
        assert_eq!(ev1.type_, EV_SYN);
        assert_eq!(ev1.code, SYN_REPORT);
    }

    #[test]
    fn keyboard_queue_reports_overflow_with_syn_dropped() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        init();

        let packet_events = 2usize;
        let packet_count = INPUT_QUEUE_CAPACITY / packet_events;
        for _ in 0..packet_count {
            push_keyboard_key(KEY_A, true);
        }
        push_keyboard_key(KEY_B, true);

        let mut drain = [0u8; size_of::<LinuxInputEvent>() * 8];
        loop {
            match read_device(ptr::null_mut(), DEVICE_KEYBOARD, &mut drain, true) {
                Ok(_) => {}
                Err(EAGAIN) => break,
                Err(err) => panic!("unexpected drain error {}", err),
            }
        }

        push_keyboard_key(KEY_C, true);

        let mut buf = [0u8; size_of::<LinuxInputEvent>() * 3];
        let read = read_device(ptr::null_mut(), DEVICE_KEYBOARD, &mut buf, true)
            .expect("keyboard read after overflow");
        assert_eq!(read, buf.len());

        let ev0 = decode_event(&buf[..size_of::<LinuxInputEvent>()]);
        let ev1 =
            decode_event(&buf[size_of::<LinuxInputEvent>()..size_of::<LinuxInputEvent>() * 2]);
        let ev2 = decode_event(&buf[size_of::<LinuxInputEvent>() * 2..]);

        assert_eq!(ev0.type_, EV_SYN);
        assert_eq!(ev0.code, SYN_DROPPED);
        assert_eq!(ev1.type_, EV_KEY);
        assert_eq!(ev1.code, KEY_C);
        assert_eq!(ev2.type_, EV_SYN);
        assert_eq!(ev2.code, SYN_REPORT);
    }

    #[test]
    fn empty_nonblocking_read_returns_eagain() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        init();

        let mut buf = [0u8; size_of::<LinuxInputEvent>()];
        assert_eq!(
            read_device(ptr::null_mut(), DEVICE_POINTER, &mut buf, true),
            Err(EAGAIN)
        );
    }
}
