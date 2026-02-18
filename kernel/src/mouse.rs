#[cfg(target_os = "none")]
use arch_x86_64::idt::{self, InterruptFrame};
#[cfg(target_os = "none")]
use arch_x86_64::{interrupts, pic, port};

#[cfg(target_os = "none")]
use core::ptr;
#[cfg(target_os = "none")]
use core::sync::atomic::{AtomicI32, AtomicU8, Ordering};

#[cfg(target_os = "none")]
use crate::framebuffer;
#[cfg(target_os = "none")]
use crate::sync::SpinLock;

#[cfg(target_os = "none")]
const PS2_DATA_PORT: u16 = 0x60;
#[cfg(target_os = "none")]
const PS2_STATUS_PORT: u16 = 0x64;
#[cfg(target_os = "none")]
const PS2_COMMAND_PORT: u16 = 0x64;

#[cfg(target_os = "none")]
const PS2_STATUS_OUTPUT_FULL: u8 = 1 << 0;
#[cfg(target_os = "none")]
const PS2_STATUS_INPUT_FULL: u8 = 1 << 1;

#[cfg(target_os = "none")]
const PS2_CMD_ENABLE_AUX: u8 = 0xA8;
#[cfg(target_os = "none")]
const PS2_CMD_READ_CONFIG: u8 = 0x20;
#[cfg(target_os = "none")]
const PS2_CMD_WRITE_CONFIG: u8 = 0x60;
#[cfg(target_os = "none")]
const PS2_CMD_WRITE_MOUSE: u8 = 0xD4;

#[cfg(target_os = "none")]
const PS2_MOUSE_CMD_DEFAULTS: u8 = 0xF6;
#[cfg(target_os = "none")]
const PS2_MOUSE_CMD_ENABLE_STREAM: u8 = 0xF4;
#[cfg(target_os = "none")]
const PS2_MOUSE_ACK: u8 = 0xFA;

#[cfg(target_os = "none")]
const PS2_CONFIG_IRQ12: u8 = 1 << 1;
#[cfg(target_os = "none")]
const PS2_CONFIG_DISABLE_AUX_CLOCK: u8 = 1 << 5;

#[cfg(target_os = "none")]
const PS2_IO_TIMEOUT: usize = 100_000;

#[cfg(target_os = "none")]
const CURSOR_WIDTH: usize = 10;
#[cfg(target_os = "none")]
const CURSOR_HEIGHT: usize = 16;
#[cfg(target_os = "none")]
const CURSOR_PIXELS: usize = CURSOR_WIDTH * CURSOR_HEIGHT;

#[cfg(target_os = "none")]
const CURSOR_BORDER_COLOR: u32 = 0x0000_0000;
#[cfg(target_os = "none")]
const CURSOR_FILL_COLOR: u32 = 0x00FF_FFFF;

#[cfg(target_os = "none")]
const CURSOR_BITMAP: [&[u8; CURSOR_WIDTH]; CURSOR_HEIGHT] = [
    b"X.........",
    b"XX........",
    b"XoX.......",
    b"XooX......",
    b"XoooX.....",
    b"XooooX....",
    b"XoooooX...",
    b"XooooooX..",
    b"XoooooooX.",
    b"XooooooooX",
    b"XXXXXXXXXX",
    b"...XX.....",
    b"..XooX....",
    b".X.oo.X...",
    b"X..oo..X..",
    b"...oo...X.",
];

#[derive(Clone, Copy)]
struct PacketAssembler {
    bytes: [u8; 3],
    index: usize,
}

impl PacketAssembler {
    const fn new() -> Self {
        Self {
            bytes: [0; 3],
            index: 0,
        }
    }

    fn push(&mut self, byte: u8) -> Option<[u8; 3]> {
        if self.index == 0 && (byte & 0x08) == 0 {
            return None;
        }

        self.bytes[self.index] = byte;
        self.index += 1;

        if self.index == 3 {
            self.index = 0;
            return Some(self.bytes);
        }

        None
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MousePacket {
    buttons: u8,
    dx: i16,
    dy: i16,
}

fn decode_packet(bytes: [u8; 3]) -> Option<MousePacket> {
    let status = bytes[0];
    if (status & 0x08) == 0 {
        return None;
    }
    if (status & 0xC0) != 0 {
        return None;
    }

    Some(MousePacket {
        buttons: status & 0x07,
        dx: (bytes[1] as i8) as i16,
        dy: (bytes[2] as i8) as i16,
    })
}

fn apply_motion(x: i32, y: i32, dx: i32, dy: i32, max_x: i32, max_y: i32) -> (i32, i32) {
    let next_x = x.saturating_add(dx).clamp(0, max_x);
    let next_y = y.saturating_sub(dy).clamp(0, max_y);
    (next_x, next_y)
}

#[cfg(target_os = "none")]
#[derive(Clone, Copy)]
struct CursorState {
    initialized: bool,
    drawn: bool,
    fb: framebuffer::FramebufferInfo,
    x: i32,
    y: i32,
    max_x: i32,
    max_y: i32,
    buttons: u8,
    saved: [u32; CURSOR_PIXELS],
}

#[cfg(target_os = "none")]
impl CursorState {
    const fn unavailable() -> Self {
        Self {
            initialized: false,
            drawn: false,
            fb: framebuffer::FramebufferInfo::unavailable(),
            x: 0,
            y: 0,
            max_x: 0,
            max_y: 0,
            buttons: 0,
            saved: [0; CURSOR_PIXELS],
        }
    }
}

#[cfg(target_os = "none")]
static PACKET_ASSEMBLER: SpinLock<PacketAssembler> = SpinLock::new(PacketAssembler::new());
#[cfg(target_os = "none")]
static CURSOR_STATE: SpinLock<CursorState> = SpinLock::new(CursorState::unavailable());
#[cfg(target_os = "none")]
static PENDING_DX: AtomicI32 = AtomicI32::new(0);
#[cfg(target_os = "none")]
static PENDING_DY: AtomicI32 = AtomicI32::new(0);
#[cfg(target_os = "none")]
static BUTTON_STATE: AtomicU8 = AtomicU8::new(0);

#[cfg(target_os = "none")]
#[inline]
fn pixel_offset(fb: framebuffer::FramebufferInfo, x: usize, y: usize) -> Option<usize> {
    if x >= fb.width as usize || y >= fb.height as usize {
        return None;
    }

    let pitch_pixels = fb.pitch as usize / 4;
    y.checked_mul(pitch_pixels)?.checked_add(x)
}

#[cfg(target_os = "none")]
fn read_pixel(fb: framebuffer::FramebufferInfo, x: usize, y: usize) -> Option<u32> {
    let offset = pixel_offset(fb, x, y)?;
    unsafe {
        let src = (fb.buffer_virt as *const u32).add(offset);
        Some(ptr::read_volatile(src))
    }
}

#[cfg(target_os = "none")]
fn write_pixel(fb: framebuffer::FramebufferInfo, x: usize, y: usize, color: u32) {
    if let Some(offset) = pixel_offset(fb, x, y) {
        unsafe {
            let dst = (fb.buffer_virt as *mut u32).add(offset);
            ptr::write_volatile(dst, color);
        }
    }
}

#[cfg(target_os = "none")]
fn wait_for_write_ready() -> bool {
    let mut n = 0usize;
    while n < PS2_IO_TIMEOUT {
        let status = unsafe { port::inb(PS2_STATUS_PORT) };
        if (status & PS2_STATUS_INPUT_FULL) == 0 {
            return true;
        }
        n += 1;
    }
    false
}

#[cfg(target_os = "none")]
fn wait_for_read_ready() -> bool {
    let mut n = 0usize;
    while n < PS2_IO_TIMEOUT {
        let status = unsafe { port::inb(PS2_STATUS_PORT) };
        if (status & PS2_STATUS_OUTPUT_FULL) != 0 {
            return true;
        }
        n += 1;
    }
    false
}

#[cfg(target_os = "none")]
fn write_command(byte: u8) -> bool {
    if !wait_for_write_ready() {
        return false;
    }
    unsafe { port::outb(PS2_COMMAND_PORT, byte) };
    true
}

#[cfg(target_os = "none")]
fn write_data(byte: u8) -> bool {
    if !wait_for_write_ready() {
        return false;
    }
    unsafe { port::outb(PS2_DATA_PORT, byte) };
    true
}

#[cfg(target_os = "none")]
fn read_data() -> Option<u8> {
    if !wait_for_read_ready() {
        return None;
    }
    Some(unsafe { port::inb(PS2_DATA_PORT) })
}

#[cfg(target_os = "none")]
fn flush_output_buffer() {
    let mut n = 0usize;
    while n < 16 {
        let status = unsafe { port::inb(PS2_STATUS_PORT) };
        if (status & PS2_STATUS_OUTPUT_FULL) == 0 {
            return;
        }
        let _ = unsafe { port::inb(PS2_DATA_PORT) };
        n += 1;
    }
}

#[cfg(target_os = "none")]
fn write_mouse_command(byte: u8) -> bool {
    if !write_command(PS2_CMD_WRITE_MOUSE) {
        return false;
    }
    if !write_data(byte) {
        return false;
    }
    matches!(read_data(), Some(PS2_MOUSE_ACK))
}

#[cfg(target_os = "none")]
fn initialize_ps2_mouse() -> bool {
    flush_output_buffer();

    if !write_command(PS2_CMD_ENABLE_AUX) {
        return false;
    }

    if !write_command(PS2_CMD_READ_CONFIG) {
        return false;
    }
    let mut config = match read_data() {
        Some(v) => v,
        None => return false,
    };

    config |= PS2_CONFIG_IRQ12;
    config &= !PS2_CONFIG_DISABLE_AUX_CLOCK;

    if !write_command(PS2_CMD_WRITE_CONFIG) {
        return false;
    }
    if !write_data(config) {
        return false;
    }

    if !write_mouse_command(PS2_MOUSE_CMD_DEFAULTS) {
        return false;
    }
    if !write_mouse_command(PS2_MOUSE_CMD_ENABLE_STREAM) {
        return false;
    }

    true
}

#[cfg(target_os = "none")]
fn draw_cursor(state: &mut CursorState) {
    if !state.initialized || state.drawn {
        return;
    }

    let base_x = state.x as usize;
    let base_y = state.y as usize;
    for (row, pattern) in CURSOR_BITMAP.iter().enumerate() {
        for col in 0..CURSOR_WIDTH {
            let pixel = pattern[col];
            if pixel == b'.' {
                continue;
            }

            let x = base_x.saturating_add(col);
            let y = base_y.saturating_add(row);
            let idx = row * CURSOR_WIDTH + col;
            state.saved[idx] = read_pixel(state.fb, x, y).unwrap_or(0);

            let color = if pixel == b'X' {
                CURSOR_BORDER_COLOR
            } else {
                CURSOR_FILL_COLOR
            };
            write_pixel(state.fb, x, y, color);
        }
    }
    state.drawn = true;
}

#[cfg(target_os = "none")]
fn hide_cursor(state: &mut CursorState) {
    if !state.initialized || !state.drawn {
        return;
    }

    let base_x = state.x as usize;
    let base_y = state.y as usize;
    for (row, pattern) in CURSOR_BITMAP.iter().enumerate() {
        for col in 0..CURSOR_WIDTH {
            if pattern[col] == b'.' {
                continue;
            }

            let x = base_x.saturating_add(col);
            let y = base_y.saturating_add(row);
            let idx = row * CURSOR_WIDTH + col;
            write_pixel(state.fb, x, y, state.saved[idx]);
        }
    }
    state.drawn = false;
}

#[cfg(target_os = "none")]
fn with_cursor_state<F>(f: F)
where
    F: FnOnce(&mut CursorState),
{
    interrupts::without_interrupts(|| {
        let mut state = CURSOR_STATE.lock();
        if !state.initialized {
            return;
        }
        f(&mut state);
    });
}

#[cfg(target_os = "none")]
fn initialize_cursor() {
    let fb = framebuffer::info();
    if !fb.available {
        return;
    }

    let max_x = (fb.width as i32).saturating_sub(CURSOR_WIDTH as i32).max(0);
    let max_y = (fb.height as i32)
        .saturating_sub(CURSOR_HEIGHT as i32)
        .max(0);
    let start_x = (fb.width as i32 / 2).clamp(0, max_x);
    let start_y = (fb.height as i32 / 2).clamp(0, max_y);

    interrupts::without_interrupts(|| {
        let mut state = CURSOR_STATE.lock();
        state.initialized = true;
        state.drawn = false;
        state.fb = fb;
        state.x = start_x;
        state.y = start_y;
        state.max_x = max_x;
        state.max_y = max_y;
        state.buttons = 0;
        draw_cursor(&mut state);
    });
}

#[cfg(target_os = "none")]
fn queue_packet(packet: MousePacket) {
    PENDING_DX.fetch_add(packet.dx as i32, Ordering::Relaxed);
    PENDING_DY.fetch_add(packet.dy as i32, Ordering::Relaxed);
    BUTTON_STATE.store(packet.buttons, Ordering::Relaxed);
}

#[cfg(target_os = "none")]
extern "C" fn mouse_irq_handler(_frame: &mut InterruptFrame) {
    let byte = unsafe { port::inb(PS2_DATA_PORT) };
    let packet = {
        let mut assembler = PACKET_ASSEMBLER.lock();
        assembler.push(byte)
    };

    if let Some(bytes) = packet {
        if let Some(decoded) = decode_packet(bytes) {
            queue_packet(decoded);
        }
    }

    pic::send_eoi(pic::IRQ_MOUSE);
}

#[cfg(target_os = "none")]
pub fn init() {
    idt::register_handler(pic::irq_to_vector(pic::IRQ_MOUSE), mouse_irq_handler);

    if !initialize_ps2_mouse() {
        crate::serial_println!("[MOUSE] PS/2 init failed");
        return;
    }

    pic::unmask_irq(pic::IRQ_MOUSE);
    initialize_cursor();

    crate::serial_println!("[MOUSE] Initialized");
}

#[cfg(not(target_os = "none"))]
pub fn init() {}

#[cfg(target_os = "none")]
pub fn begin_framebuffer_update() {
    with_cursor_state(hide_cursor);
}

#[cfg(not(target_os = "none"))]
pub fn begin_framebuffer_update() {}

#[cfg(target_os = "none")]
pub fn end_framebuffer_update() {
    with_cursor_state(draw_cursor);
}

#[cfg(not(target_os = "none"))]
pub fn end_framebuffer_update() {}

#[cfg(target_os = "none")]
pub fn tick() {
    let dx = PENDING_DX.swap(0, Ordering::AcqRel);
    let dy = PENDING_DY.swap(0, Ordering::AcqRel);
    let buttons = BUTTON_STATE.load(Ordering::Acquire);

    with_cursor_state(|state| {
        state.buttons = buttons;
        if dx == 0 && dy == 0 {
            return;
        }

        hide_cursor(state);
        let (next_x, next_y) = apply_motion(state.x, state.y, dx, dy, state.max_x, state.max_y);
        state.x = next_x;
        state.y = next_y;
        draw_cursor(state);
    });
}

#[cfg(not(target_os = "none"))]
pub fn tick() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packet_assembler_resyncs_to_header_bit() {
        let mut assembler = PacketAssembler::new();

        assert_eq!(assembler.push(0x00), None);
        assert_eq!(assembler.push(0x08), None);
        assert_eq!(assembler.push(0x01), None);
        assert_eq!(assembler.push(0x02), Some([0x08, 0x01, 0x02]));
    }

    #[test]
    fn decode_packet_handles_sign_and_buttons() {
        let packet = decode_packet([0x29, 0xFF, 0x01]).expect("valid packet");
        assert_eq!(packet.buttons, 0x01);
        assert_eq!(packet.dx, -1);
        assert_eq!(packet.dy, 1);
    }

    #[test]
    fn decode_packet_rejects_overflow() {
        assert_eq!(decode_packet([0x48, 0x10, 0x10]), None);
        assert_eq!(decode_packet([0x88, 0x10, 0x10]), None);
    }

    #[test]
    fn apply_motion_clamps_and_inverts_y_axis() {
        let (x, y) = apply_motion(5, 5, 4, 3, 8, 8);
        assert_eq!(x, 8);
        assert_eq!(y, 2);

        let (x2, y2) = apply_motion(x, y, -20, -20, 8, 8);
        assert_eq!(x2, 0);
        assert_eq!(y2, 8);
    }
}
