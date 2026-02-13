#[cfg(target_os = "none")]
use arch_x86_64::idt::{self, InterruptFrame};
#[cfg(target_os = "none")]
use arch_x86_64::{pic, port};

use crate::sync::SpinLock;

const PS2_DATA_PORT: u16 = 0x60;
const SCANCODE_RELEASE_MASK: u8 = 0x80;
const SCANCODE_PREFIX_EXTENDED: u8 = 0xE0;

#[derive(Clone, Copy)]
struct KeyboardState {
    left_shift: bool,
    right_shift: bool,
    caps_lock: bool,
    extended: bool,
}

impl KeyboardState {
    const fn new() -> Self {
        Self {
            left_shift: false,
            right_shift: false,
            caps_lock: false,
            extended: false,
        }
    }

    #[inline]
    fn shifted(&self) -> bool {
        self.left_shift || self.right_shift
    }
}

static KBD_STATE: SpinLock<KeyboardState> = SpinLock::new(KeyboardState::new());

#[cfg(target_os = "none")]
pub fn init() {
    idt::register_handler(pic::irq_to_vector(pic::IRQ_KEYBOARD), keyboard_irq_handler);
    pic::unmask_irq(pic::IRQ_KEYBOARD);
}

#[cfg(not(target_os = "none"))]
pub fn init() {}

#[cfg(target_os = "none")]
extern "C" fn keyboard_irq_handler(_frame: &mut InterruptFrame) {
    let scancode = unsafe { port::inb(PS2_DATA_PORT) };
    let byte = {
        let mut state = KBD_STATE.lock();
        decode_scancode(&mut state, scancode)
    };

    if let Some(byte) = byte {
        crate::tty::ingest_byte(byte);
    }

    pic::send_eoi(pic::IRQ_KEYBOARD);
}

fn decode_scancode(state: &mut KeyboardState, scancode: u8) -> Option<u8> {
    if scancode == SCANCODE_PREFIX_EXTENDED {
        state.extended = true;
        return None;
    }

    let release = (scancode & SCANCODE_RELEASE_MASK) != 0;
    let code = scancode & !SCANCODE_RELEASE_MASK;

    if state.extended {
        // Extended keys are mostly ignored for now, but keypad Enter should
        // behave like regular Enter for shell/TTY usability.
        state.extended = false;
        if !release && code == 0x1C {
            return Some(b'\n');
        }
        return None;
    }

    match code {
        0x2A => {
            state.left_shift = !release;
            return None;
        }
        0x36 => {
            state.right_shift = !release;
            return None;
        }
        0x3A => {
            if !release {
                state.caps_lock = !state.caps_lock;
            }
            return None;
        }
        _ => {}
    }

    if release {
        return None;
    }

    if let Some(mut ch) = map_letter(code) {
        if state.caps_lock ^ state.shifted() {
            ch = ch - b'a' + b'A';
        }
        return Some(ch);
    }

    map_non_letter(code, state.shifted())
}

fn map_letter(code: u8) -> Option<u8> {
    match code {
        0x10 => Some(b'q'),
        0x11 => Some(b'w'),
        0x12 => Some(b'e'),
        0x13 => Some(b'r'),
        0x14 => Some(b't'),
        0x15 => Some(b'y'),
        0x16 => Some(b'u'),
        0x17 => Some(b'i'),
        0x18 => Some(b'o'),
        0x19 => Some(b'p'),
        0x1E => Some(b'a'),
        0x1F => Some(b's'),
        0x20 => Some(b'd'),
        0x21 => Some(b'f'),
        0x22 => Some(b'g'),
        0x23 => Some(b'h'),
        0x24 => Some(b'j'),
        0x25 => Some(b'k'),
        0x26 => Some(b'l'),
        0x2C => Some(b'z'),
        0x2D => Some(b'x'),
        0x2E => Some(b'c'),
        0x2F => Some(b'v'),
        0x30 => Some(b'b'),
        0x31 => Some(b'n'),
        0x32 => Some(b'm'),
        _ => None,
    }
}

fn map_non_letter(code: u8, shifted: bool) -> Option<u8> {
    match code {
        0x02 => Some(if shifted { b'!' } else { b'1' }),
        0x03 => Some(if shifted { b'@' } else { b'2' }),
        0x04 => Some(if shifted { b'#' } else { b'3' }),
        0x05 => Some(if shifted { b'$' } else { b'4' }),
        0x06 => Some(if shifted { b'%' } else { b'5' }),
        0x07 => Some(if shifted { b'^' } else { b'6' }),
        0x08 => Some(if shifted { b'&' } else { b'7' }),
        0x09 => Some(if shifted { b'*' } else { b'8' }),
        0x0A => Some(if shifted { b'(' } else { b'9' }),
        0x0B => Some(if shifted { b')' } else { b'0' }),
        0x0C => Some(if shifted { b'_' } else { b'-' }),
        0x0D => Some(if shifted { b'+' } else { b'=' }),
        0x0E => Some(0x08),
        0x0F => Some(b'\t'),
        0x1A => Some(if shifted { b'{' } else { b'[' }),
        0x1B => Some(if shifted { b'}' } else { b']' }),
        0x1C => Some(b'\n'),
        0x27 => Some(if shifted { b':' } else { b';' }),
        0x28 => Some(if shifted { b'"' } else { b'\'' }),
        0x29 => Some(if shifted { b'~' } else { b'`' }),
        0x2B => Some(if shifted { b'|' } else { b'\\' }),
        0x33 => Some(if shifted { b'<' } else { b',' }),
        0x34 => Some(if shifted { b'>' } else { b'.' }),
        0x35 => Some(if shifted { b'?' } else { b'/' }),
        0x39 => Some(b' '),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_letters_and_releases() {
        let mut state = KeyboardState::new();
        assert_eq!(decode_scancode(&mut state, 0x1E), Some(b'a'));
        assert_eq!(decode_scancode(&mut state, 0x9E), None);
        assert_eq!(decode_scancode(&mut state, 0x30), Some(b'b'));
    }

    #[test]
    fn shift_changes_number_and_letter_case() {
        let mut state = KeyboardState::new();
        assert_eq!(decode_scancode(&mut state, 0x2A), None);
        assert_eq!(decode_scancode(&mut state, 0x02), Some(b'!'));
        assert_eq!(decode_scancode(&mut state, 0x1E), Some(b'A'));
        assert_eq!(decode_scancode(&mut state, 0xAA), None);
        assert_eq!(decode_scancode(&mut state, 0x02), Some(b'1'));
    }

    #[test]
    fn caps_lock_and_shift_interact() {
        let mut state = KeyboardState::new();
        assert_eq!(decode_scancode(&mut state, 0x3A), None);
        assert_eq!(decode_scancode(&mut state, 0x1E), Some(b'A'));
        assert_eq!(decode_scancode(&mut state, 0x2A), None);
        assert_eq!(decode_scancode(&mut state, 0x1E), Some(b'a'));
    }

    #[test]
    fn maps_backspace_and_enter() {
        let mut state = KeyboardState::new();
        assert_eq!(decode_scancode(&mut state, 0x0E), Some(0x08));
        assert_eq!(decode_scancode(&mut state, 0x1C), Some(b'\n'));
    }

    #[test]
    fn extended_prefix_is_ignored_except_keypad_enter() {
        let mut state = KeyboardState::new();
        assert_eq!(decode_scancode(&mut state, 0xE0), None);
        assert_eq!(decode_scancode(&mut state, 0x48), None);
        assert_eq!(decode_scancode(&mut state, 0xE0), None);
        assert_eq!(decode_scancode(&mut state, 0x1C), Some(b'\n'));
        assert_eq!(decode_scancode(&mut state, 0x1E), Some(b'a'));
    }
}
