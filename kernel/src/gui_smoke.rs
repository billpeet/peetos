#[cfg(target_os = "none")]
use core::sync::atomic::{AtomicBool, Ordering};

#[cfg(target_os = "none")]
use arch_x86_64::pit;

#[cfg(target_os = "none")]
static INPUT_INJECTED: AtomicBool = AtomicBool::new(false);

#[cfg(target_os = "none")]
const INJECT_AFTER_MS: u64 = 750;

#[cfg(target_os = "none")]
pub fn tick() {
    if INPUT_INJECTED.load(Ordering::Acquire) || pit::uptime_ms() < INJECT_AFTER_MS {
        return;
    }
    if INPUT_INJECTED
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }

    crate::input::push_pointer_relative(24, -12, 0);
    crate::input::push_pointer_relative(0, 0, 1);
    crate::input::push_pointer_relative(0, 0, 0);
    crate::input::push_keyboard_key(crate::input::KEY_A, true);
    crate::input::push_keyboard_key(crate::input::KEY_A, false);
    crate::serial_println!("[GUI-SMOKE] injected synthetic keyboard/pointer events");
}

#[cfg(not(target_os = "none"))]
pub fn tick() {}
