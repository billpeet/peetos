use core::sync::atomic::{AtomicU64, Ordering};

#[cfg(target_os = "none")]
use crate::idt::{self, InterruptFrame};
#[cfg(target_os = "none")]
use crate::{irq, pic};
#[cfg(target_os = "none")]
use crate::port::{inb, outb};

pub const PIT_CHANNEL0_DATA: u16 = 0x40;
pub const PIT_COMMAND: u16 = 0x43;

pub const PIT_CHANNEL0: u8 = 0 << 6;
pub const PIT_ACCESS_LOHI: u8 = 3 << 4;
pub const PIT_MODE2: u8 = 2 << 1;
pub const PIT_BINARY: u8 = 0;
pub const PIT_LATCH_CHANNEL0: u8 = 0;

pub const PIT_FREQUENCY: u32 = 1_193_182;
pub const TIMER_FREQUENCY: u32 = 100;

static TICKS: AtomicU64 = AtomicU64::new(0);
#[cfg(target_os = "none")]
static mut TICK_HOOK: Option<fn()> = None;

#[inline]
pub const fn pit_divisor(freq: u32) -> u16 {
    (PIT_FREQUENCY / freq) as u16
}

/// Lock-free part of IRQ0: advance the tick counter before the kernel takes
/// any lock, so code waiting on `ticks()` on another CPU always progresses.
#[cfg(target_os = "none")]
extern "C" fn timer_fast_handler(_frame: &mut InterruptFrame) {
    TICKS.fetch_add(1, Ordering::Relaxed);
}

#[cfg(target_os = "none")]
extern "C" fn timer_handler(_frame: &mut InterruptFrame) {
    if let Some(hook) = unsafe { TICK_HOOK } {
        hook();
    }
    // EOI is sent by the IDT dispatcher.
}

#[cfg(target_os = "none")]
pub fn init() {
    let divisor = pit_divisor(TIMER_FREQUENCY);
    let command = PIT_CHANNEL0 | PIT_ACCESS_LOHI | PIT_MODE2 | PIT_BINARY;

    unsafe {
        outb(PIT_COMMAND, command);
        outb(PIT_CHANNEL0_DATA, (divisor & 0xFF) as u8);
        outb(PIT_CHANNEL0_DATA, (divisor >> 8) as u8);
    }

    idt::register_fast_handler(irq::irq_to_vector(pic::IRQ_TIMER), timer_fast_handler);
    idt::register_handler(irq::irq_to_vector(pic::IRQ_TIMER), timer_handler);
    irq::unmask(pic::IRQ_TIMER);
}

#[cfg(not(target_os = "none"))]
pub fn init() {}

#[cfg(target_os = "none")]
pub fn set_tick_hook(hook: fn()) {
    unsafe {
        TICK_HOOK = Some(hook);
    }
}

#[cfg(not(target_os = "none"))]
pub fn set_tick_hook(_hook: fn()) {}

/// Elapsed PIT input clocks between two channel-0 counter reads, given the
/// reload value. The counter counts down from `reload` and wraps at most once
/// between reads when polled faster than one timer period.
pub const fn counter_elapsed(prev: u16, now: u16, reload: u16) -> u32 {
    if now <= prev {
        (prev - now) as u32
    } else {
        prev as u32 + (reload - now) as u32
    }
}

#[cfg(target_os = "none")]
fn read_counter() -> u16 {
    crate::interrupts::without_interrupts(|| unsafe {
        outb(PIT_COMMAND, PIT_LATCH_CHANNEL0);
        let lo = inb(PIT_CHANNEL0_DATA) as u16;
        let hi = inb(PIT_CHANNEL0_DATA) as u16;
        (hi << 8) | lo
    })
}

/// Busy-wait for roughly `us` microseconds by polling the channel-0 counter.
/// Works with interrupts disabled (unlike [`sleep_ticks`]); requires [`init`].
#[cfg(target_os = "none")]
pub fn busy_wait_us(us: u64) {
    let reload = pit_divisor(TIMER_FREQUENCY);
    let target = us.saturating_mul(PIT_FREQUENCY as u64) / 1_000_000;
    let mut elapsed: u64 = 0;
    let mut prev = read_counter();
    while elapsed < target {
        core::hint::spin_loop();
        let now = read_counter();
        elapsed += counter_elapsed(prev, now, reload) as u64;
        prev = now;
    }
}

#[cfg(not(target_os = "none"))]
pub fn busy_wait_us(_us: u64) {}

pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

pub fn uptime_ms() -> u64 {
    ticks() * 1000 / TIMER_FREQUENCY as u64
}

pub fn uptime_seconds() -> u64 {
    ticks() / TIMER_FREQUENCY as u64
}

pub fn sleep_ticks(ticks: u64) {
    let target = ticks.saturating_add(TICKS.load(Ordering::Relaxed));
    while TICKS.load(Ordering::Relaxed) < target {
        #[cfg(target_os = "none")]
        unsafe {
            core::arch::asm!("hlt", options(nomem, nostack, preserves_flags));
        }
        #[cfg(not(target_os = "none"))]
        core::hint::spin_loop();
    }
}

pub fn sleep_ms(ms: u64) {
    let mut ticks = (ms * TIMER_FREQUENCY as u64 + 999) / 1000;
    if ms > 0 && ticks == 0 {
        ticks = 1;
    }
    sleep_ticks(ticks);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counter_elapsed_handles_wrap() {
        assert_eq!(counter_elapsed(1000, 400, 11_931), 600);
        assert_eq!(counter_elapsed(500, 500, 11_931), 0);
        // Counter reloaded between reads: 100 clocks to zero, then down from reload.
        assert_eq!(counter_elapsed(100, 11_900, 11_931), 131);
    }

    #[test]
    fn pit_divisor_matches_reference() {
        assert_eq!(pit_divisor(100), 11_931);
        assert_eq!(pit_divisor(1_000), 1_193);
    }
}
