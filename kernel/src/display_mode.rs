use core::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum BootDisplayMode {
    ConsoleFirst = 0,
    CompositorFirst = 1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum DisplayOwner {
    Console = 0,
    Compositor = 1,
}

static DISPLAY_OWNER: AtomicU8 = AtomicU8::new(DisplayOwner::Console as u8);

pub const fn boot_display_mode() -> BootDisplayMode {
    if cfg!(feature = "compositor-first") {
        BootDisplayMode::CompositorFirst
    } else {
        BootDisplayMode::ConsoleFirst
    }
}

pub fn init(framebuffer_available: bool) -> DisplayOwner {
    let owner = match (boot_display_mode(), framebuffer_available) {
        (BootDisplayMode::CompositorFirst, true) => DisplayOwner::Compositor,
        _ => DisplayOwner::Console,
    };
    DISPLAY_OWNER.store(owner as u8, Ordering::Release);
    owner
}

pub fn display_owner() -> DisplayOwner {
    match DISPLAY_OWNER.load(Ordering::Acquire) {
        x if x == DisplayOwner::Compositor as u8 => DisplayOwner::Compositor,
        _ => DisplayOwner::Console,
    }
}

pub fn console_active() -> bool {
    display_owner() == DisplayOwner::Console
}

pub fn compositor_active() -> bool {
    display_owner() == DisplayOwner::Compositor
}

pub fn activate_console() {
    DISPLAY_OWNER.store(DisplayOwner::Console as u8, Ordering::Release);
}

pub fn activate_compositor() {
    DISPLAY_OWNER.store(DisplayOwner::Compositor as u8, Ordering::Release);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_boot_mode_is_console_first_without_feature() {
        assert_eq!(boot_display_mode(), BootDisplayMode::ConsoleFirst);
    }

    #[test]
    fn init_defaults_to_console_when_framebuffer_is_missing() {
        activate_compositor();
        assert_eq!(init(false), DisplayOwner::Console);
        assert!(console_active());
        assert!(!compositor_active());
    }

    #[test]
    fn owner_transitions_are_explicit() {
        activate_console();
        assert_eq!(display_owner(), DisplayOwner::Console);

        activate_compositor();
        assert_eq!(display_owner(), DisplayOwner::Compositor);

        activate_console();
        assert_eq!(display_owner(), DisplayOwner::Console);
    }
}
