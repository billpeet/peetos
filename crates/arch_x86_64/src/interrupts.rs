#[cfg(not(target_os = "none"))]
use core::sync::atomic::{AtomicBool, Ordering};

#[cfg(not(target_os = "none"))]
static IF_STATE: AtomicBool = AtomicBool::new(true);

#[cfg(target_os = "none")]
use core::arch::asm;

#[cfg(target_os = "none")]
#[inline]
pub fn enable() {
    unsafe {
        asm!("sti", options(nomem, nostack, preserves_flags));
    }
}

#[cfg(target_os = "none")]
#[inline]
pub fn disable() {
    unsafe {
        asm!("cli", options(nomem, nostack, preserves_flags));
    }
}

#[cfg(target_os = "none")]
#[inline]
pub fn are_enabled() -> bool {
    let rflags: u64;
    unsafe {
        asm!("pushfq; pop {}", out(reg) rflags, options(preserves_flags));
    }
    (rflags & (1 << 9)) != 0
}

#[cfg(not(target_os = "none"))]
#[inline]
pub fn enable() {
    IF_STATE.store(true, Ordering::SeqCst);
}

#[cfg(not(target_os = "none"))]
#[inline]
pub fn disable() {
    IF_STATE.store(false, Ordering::SeqCst);
}

#[cfg(not(target_os = "none"))]
#[inline]
pub fn are_enabled() -> bool {
    IF_STATE.load(Ordering::SeqCst)
}

#[inline]
pub fn without_interrupts<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    let was_enabled = are_enabled();
    disable();
    let result = f();
    if was_enabled {
        enable();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::{are_enabled, disable, enable, without_interrupts};

    #[test]
    fn without_interrupts_restores_enabled_state() {
        enable();
        assert!(are_enabled());

        let inside = without_interrupts(|| are_enabled());
        assert!(!inside);
        assert!(are_enabled());
    }

    #[test]
    fn without_interrupts_keeps_disabled_state() {
        disable();
        assert!(!are_enabled());

        let inside = without_interrupts(|| are_enabled());
        assert!(!inside);
        assert!(!are_enabled());

        enable();
    }
}
