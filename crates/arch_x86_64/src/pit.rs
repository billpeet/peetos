use core::sync::atomic::{AtomicU64, Ordering};

#[cfg(target_os = "none")]
use crate::idt::{self, InterruptFrame};
#[cfg(target_os = "none")]
use crate::pic;
#[cfg(target_os = "none")]
use crate::port::outb;

pub const PIT_CHANNEL0_DATA: u16 = 0x40;
pub const PIT_COMMAND: u16 = 0x43;

pub const PIT_CHANNEL0: u8 = 0 << 6;
pub const PIT_ACCESS_LOHI: u8 = 3 << 4;
pub const PIT_MODE2: u8 = 2 << 1;
pub const PIT_BINARY: u8 = 0;

pub const PIT_FREQUENCY: u32 = 1_193_182;
pub const TIMER_FREQUENCY: u32 = 100;

static TICKS: AtomicU64 = AtomicU64::new(0);

#[inline]
pub const fn pit_divisor(freq: u32) -> u16 {
    (PIT_FREQUENCY / freq) as u16
}

#[cfg(target_os = "none")]
extern "C" fn timer_handler(_frame: &mut InterruptFrame) {
    TICKS.fetch_add(1, Ordering::Relaxed);
    pic::send_eoi(pic::IRQ_TIMER);
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

    idt::register_handler(pic::irq_to_vector(pic::IRQ_TIMER), timer_handler);
    pic::unmask_irq(pic::IRQ_TIMER);
}

#[cfg(not(target_os = "none"))]
pub fn init() {}

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
    fn pit_divisor_matches_reference() {
        assert_eq!(pit_divisor(100), 11_931);
        assert_eq!(pit_divisor(1_000), 1_193);
    }
}
