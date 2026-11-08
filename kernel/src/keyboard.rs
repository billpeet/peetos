#[cfg(target_os = "none")]
use arch_x86_64::idt::{self, InterruptFrame};
#[cfg(target_os = "none")]
use arch_x86_64::{irq, pic, port};

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DecodedScancode {
    tty_byte: Option<u8>,
    input_event: Option<(u16, i32)>,
}

impl DecodedScancode {
    const fn none() -> Self {
        Self {
            tty_byte: None,
            input_event: None,
        }
    }
}

#[cfg(target_os = "none")]
pub fn init() {
    idt::register_handler(irq::irq_to_vector(pic::IRQ_KEYBOARD), keyboard_irq_handler);
    irq::unmask(pic::IRQ_KEYBOARD);
}

#[cfg(not(target_os = "none"))]
pub fn init() {}

#[cfg(target_os = "none")]
extern "C" fn keyboard_irq_handler(_frame: &mut InterruptFrame) {
    let scancode = unsafe { port::inb(PS2_DATA_PORT) };
    let decoded = {
        let mut state = KBD_STATE.lock();
        decode_scancode(&mut state, scancode)
    };

    if let Some((code, value)) = decoded.input_event {
        crate::input::push_keyboard_key(code, value != 0);
    }

    if let Some(byte) = decoded.tty_byte {
        crate::tty::ingest_byte(byte);
    }

    // EOI is sent by the IDT dispatcher.
}

fn decode_scancode(state: &mut KeyboardState, scancode: u8) -> DecodedScancode {
    if scancode == SCANCODE_PREFIX_EXTENDED {
        state.extended = true;
        return DecodedScancode::none();
    }

    let release = (scancode & SCANCODE_RELEASE_MASK) != 0;
    let code = scancode & !SCANCODE_RELEASE_MASK;
    let extended = state.extended;
    state.extended = false;

    let keycode = map_keycode(code, extended);
    let input_event = keycode.map(|code| (code, i32::from(!release)));

    if extended {
        let tty_byte = if !release && code == 0x1C {
            Some(b'\n')
        } else {
            None
        };
        return DecodedScancode {
            tty_byte,
            input_event,
        };
    }

    match code {
        0x2A => {
            state.left_shift = !release;
            return DecodedScancode {
                tty_byte: None,
                input_event,
            };
        }
        0x36 => {
            state.right_shift = !release;
            return DecodedScancode {
                tty_byte: None,
                input_event,
            };
        }
        0x3A => {
            if !release {
                state.caps_lock = !state.caps_lock;
            }
            return DecodedScancode {
                tty_byte: None,
                input_event,
            };
        }
        _ => {}
    }

    if release {
        return DecodedScancode {
            tty_byte: None,
            input_event,
        };
    }

    if let Some(mut ch) = map_letter(code) {
        if state.caps_lock ^ state.shifted() {
            ch = ch - b'a' + b'A';
        }
        return DecodedScancode {
            tty_byte: Some(ch),
            input_event,
        };
    }

    DecodedScancode {
        tty_byte: map_non_letter(code, state.shifted()),
        input_event,
    }
}

fn map_keycode(code: u8, extended: bool) -> Option<u16> {
    if extended {
        return match code {
            0x1C => Some(crate::input::KEY_KPENTER),
            0x1D => Some(crate::input::KEY_RIGHTCTRL),
            0x35 => Some(crate::input::KEY_KPSLASH),
            0x38 => Some(crate::input::KEY_RIGHTALT),
            0x47 => Some(crate::input::KEY_HOME),
            0x48 => Some(crate::input::KEY_UP),
            0x49 => Some(crate::input::KEY_PAGEUP),
            0x4B => Some(crate::input::KEY_LEFT),
            0x4D => Some(crate::input::KEY_RIGHT),
            0x4F => Some(crate::input::KEY_END),
            0x50 => Some(crate::input::KEY_DOWN),
            0x51 => Some(crate::input::KEY_PAGEDOWN),
            0x52 => Some(crate::input::KEY_INSERT),
            0x53 => Some(crate::input::KEY_DELETE),
            _ => None,
        };
    }

    match code {
        0x01 => Some(crate::input::KEY_ESC),
        0x02 => Some(crate::input::KEY_1),
        0x03 => Some(crate::input::KEY_2),
        0x04 => Some(crate::input::KEY_3),
        0x05 => Some(crate::input::KEY_4),
        0x06 => Some(crate::input::KEY_5),
        0x07 => Some(crate::input::KEY_6),
        0x08 => Some(crate::input::KEY_7),
        0x09 => Some(crate::input::KEY_8),
        0x0A => Some(crate::input::KEY_9),
        0x0B => Some(crate::input::KEY_0),
        0x0C => Some(crate::input::KEY_MINUS),
        0x0D => Some(crate::input::KEY_EQUAL),
        0x0E => Some(crate::input::KEY_BACKSPACE),
        0x0F => Some(crate::input::KEY_TAB),
        0x10 => Some(crate::input::KEY_Q),
        0x11 => Some(crate::input::KEY_W),
        0x12 => Some(crate::input::KEY_E),
        0x13 => Some(crate::input::KEY_R),
        0x14 => Some(crate::input::KEY_T),
        0x15 => Some(crate::input::KEY_Y),
        0x16 => Some(crate::input::KEY_U),
        0x17 => Some(crate::input::KEY_I),
        0x18 => Some(crate::input::KEY_O),
        0x19 => Some(crate::input::KEY_P),
        0x1A => Some(crate::input::KEY_LEFTBRACE),
        0x1B => Some(crate::input::KEY_RIGHTBRACE),
        0x1C => Some(crate::input::KEY_ENTER),
        0x1D => Some(crate::input::KEY_LEFTCTRL),
        0x1E => Some(crate::input::KEY_A),
        0x1F => Some(crate::input::KEY_S),
        0x20 => Some(crate::input::KEY_D),
        0x21 => Some(crate::input::KEY_F),
        0x22 => Some(crate::input::KEY_G),
        0x23 => Some(crate::input::KEY_H),
        0x24 => Some(crate::input::KEY_J),
        0x25 => Some(crate::input::KEY_K),
        0x26 => Some(crate::input::KEY_L),
        0x27 => Some(crate::input::KEY_SEMICOLON),
        0x28 => Some(crate::input::KEY_APOSTROPHE),
        0x29 => Some(crate::input::KEY_GRAVE),
        0x2A => Some(crate::input::KEY_LEFTSHIFT),
        0x2B => Some(crate::input::KEY_BACKSLASH),
        0x2C => Some(crate::input::KEY_Z),
        0x2D => Some(crate::input::KEY_X),
        0x2E => Some(crate::input::KEY_C),
        0x2F => Some(crate::input::KEY_V),
        0x30 => Some(crate::input::KEY_B),
        0x31 => Some(crate::input::KEY_N),
        0x32 => Some(crate::input::KEY_M),
        0x33 => Some(crate::input::KEY_COMMA),
        0x34 => Some(crate::input::KEY_DOT),
        0x35 => Some(crate::input::KEY_SLASH),
        0x36 => Some(crate::input::KEY_RIGHTSHIFT),
        0x38 => Some(crate::input::KEY_LEFTALT),
        0x39 => Some(crate::input::KEY_SPACE),
        0x3A => Some(crate::input::KEY_CAPSLOCK),
        0x3B => Some(crate::input::KEY_F1),
        0x3C => Some(crate::input::KEY_F2),
        0x3D => Some(crate::input::KEY_F3),
        0x3E => Some(crate::input::KEY_F4),
        0x3F => Some(crate::input::KEY_F5),
        0x40 => Some(crate::input::KEY_F6),
        0x41 => Some(crate::input::KEY_F7),
        0x42 => Some(crate::input::KEY_F8),
        0x43 => Some(crate::input::KEY_F9),
        0x44 => Some(crate::input::KEY_F10),
        0x45 => Some(crate::input::KEY_NUMLOCK),
        0x46 => Some(crate::input::KEY_SCROLLLOCK),
        0x57 => Some(crate::input::KEY_F11),
        0x58 => Some(crate::input::KEY_F12),
        _ => None,
    }
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
        assert_eq!(decode_scancode(&mut state, 0x1E).tty_byte, Some(b'a'));
        assert_eq!(decode_scancode(&mut state, 0x9E).tty_byte, None);
        assert_eq!(decode_scancode(&mut state, 0x30).tty_byte, Some(b'b'));
    }

    #[test]
    fn shift_changes_number_and_letter_case() {
        let mut state = KeyboardState::new();
        assert_eq!(decode_scancode(&mut state, 0x2A).tty_byte, None);
        assert_eq!(decode_scancode(&mut state, 0x02).tty_byte, Some(b'!'));
        assert_eq!(decode_scancode(&mut state, 0x1E).tty_byte, Some(b'A'));
        assert_eq!(decode_scancode(&mut state, 0xAA).tty_byte, None);
        assert_eq!(decode_scancode(&mut state, 0x02).tty_byte, Some(b'1'));
    }

    #[test]
    fn caps_lock_and_shift_interact() {
        let mut state = KeyboardState::new();
        assert_eq!(decode_scancode(&mut state, 0x3A).tty_byte, None);
        assert_eq!(decode_scancode(&mut state, 0x1E).tty_byte, Some(b'A'));
        assert_eq!(decode_scancode(&mut state, 0x2A).tty_byte, None);
        assert_eq!(decode_scancode(&mut state, 0x1E).tty_byte, Some(b'a'));
    }

    #[test]
    fn maps_backspace_and_enter() {
        let mut state = KeyboardState::new();
        assert_eq!(decode_scancode(&mut state, 0x0E).tty_byte, Some(0x08));
        assert_eq!(decode_scancode(&mut state, 0x1C).tty_byte, Some(b'\n'));
    }

    #[test]
    fn extended_prefix_is_ignored_except_keypad_enter() {
        let mut state = KeyboardState::new();
        assert_eq!(decode_scancode(&mut state, 0xE0).tty_byte, None);
        let up = decode_scancode(&mut state, 0x48);
        assert_eq!(up.tty_byte, None);
        assert_eq!(up.input_event, Some((crate::input::KEY_UP, 1)));
        assert_eq!(decode_scancode(&mut state, 0xE0).tty_byte, None);
        assert_eq!(decode_scancode(&mut state, 0x1C).tty_byte, Some(b'\n'));
        assert_eq!(decode_scancode(&mut state, 0x1E).tty_byte, Some(b'a'));
    }

    #[test]
    fn release_events_preserve_input_keycode() {
        let mut state = KeyboardState::new();
        assert_eq!(
            decode_scancode(&mut state, 0x9E).input_event,
            Some((crate::input::KEY_A, 0))
        );
    }
}
