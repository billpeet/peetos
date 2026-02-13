use core::ptr;

use crate::font8x8_basic::FONT8X8_BASIC;
use crate::framebuffer::{self, FramebufferInfo};
use crate::sync::SpinLock;

const GLYPH_WIDTH: usize = 8;
const GLYPH_HEIGHT: usize = 8;
const TAB_WIDTH: usize = 4;
const FG_COLOR: u32 = 0x00E6_E6E6;
const BG_COLOR: u32 = 0x0011_1111;

#[derive(Clone, Copy)]
struct FbConsoleState {
    initialized: bool,
    fb: FramebufferInfo,
    cols: usize,
    rows: usize,
    cursor_col: usize,
    cursor_row: usize,
}

impl FbConsoleState {
    const fn unavailable() -> Self {
        Self {
            initialized: false,
            fb: FramebufferInfo::unavailable(),
            cols: 0,
            rows: 0,
            cursor_col: 0,
            cursor_row: 0,
        }
    }
}

static FB_CONSOLE: SpinLock<FbConsoleState> = SpinLock::new(FbConsoleState::unavailable());

#[inline]
fn tab_spaces(col: usize) -> usize {
    TAB_WIDTH - (col % TAB_WIDTH)
}

#[inline]
fn pitch_pixels(fb: FramebufferInfo) -> usize {
    fb.pitch as usize / 4
}

fn put_pixel(fb: FramebufferInfo, x: usize, y: usize, color: u32) {
    if x >= fb.width as usize || y >= fb.height as usize {
        return;
    }
    let offset = y
        .saturating_mul(pitch_pixels(fb))
        .saturating_add(x);
    unsafe {
        ptr::write_volatile((fb.buffer_virt as *mut u32).add(offset), color);
    }
}

fn fill_rows(fb: FramebufferInfo, y_start: usize, y_end: usize, color: u32) {
    let x_max = fb.width as usize;
    let y_max = fb.height as usize;
    let y0 = core::cmp::min(y_start, y_max);
    let y1 = core::cmp::min(y_end, y_max);
    for y in y0..y1 {
        for x in 0..x_max {
            put_pixel(fb, x, y, color);
        }
    }
}

fn clear_screen(state: &FbConsoleState) {
    fill_rows(state.fb, 0, state.fb.height as usize, BG_COLOR);
}

fn draw_glyph(state: &FbConsoleState, col: usize, row: usize, byte: u8) {
    let x0 = col.saturating_mul(GLYPH_WIDTH);
    let y0 = row.saturating_mul(GLYPH_HEIGHT);
    let glyph = &FONT8X8_BASIC[byte as usize];

    for (gy, bits) in glyph.iter().copied().enumerate() {
        for gx in 0..GLYPH_WIDTH {
            let on = ((bits >> gx) & 1) != 0;
            let color = if on { FG_COLOR } else { BG_COLOR };
            put_pixel(state.fb, x0 + gx, y0 + gy, color);
        }
    }
}

fn clear_cell(state: &FbConsoleState, col: usize, row: usize) {
    let x0 = col.saturating_mul(GLYPH_WIDTH);
    let y0 = row.saturating_mul(GLYPH_HEIGHT);
    for y in 0..GLYPH_HEIGHT {
        for x in 0..GLYPH_WIDTH {
            put_pixel(state.fb, x0 + x, y0 + y, BG_COLOR);
        }
    }
}

fn scroll_one_text_row(state: &mut FbConsoleState) {
    let fb = state.fb;
    let pitch = fb.pitch as usize;
    let height = fb.height as usize;
    let scroll_px = GLYPH_HEIGHT;
    if scroll_px >= height {
        clear_screen(state);
        return;
    }

    let move_bytes = (height - scroll_px).saturating_mul(pitch);
    unsafe {
        let dst = fb.buffer_virt as *mut u8;
        let src = dst.add(scroll_px.saturating_mul(pitch));
        ptr::copy(src, dst, move_bytes);
    }

    fill_rows(fb, height - scroll_px, height, BG_COLOR);
}

fn line_feed(state: &mut FbConsoleState) {
    state.cursor_col = 0;
    state.cursor_row = state.cursor_row.saturating_add(1);
    if state.cursor_row >= state.rows {
        scroll_one_text_row(state);
        state.cursor_row = state.rows.saturating_sub(1);
    }
}

fn advance_after_printable(state: &mut FbConsoleState) {
    state.cursor_col = state.cursor_col.saturating_add(1);
    if state.cursor_col >= state.cols {
        line_feed(state);
    }
}

fn backspace_cursor(state: &mut FbConsoleState) {
    if state.cursor_col > 0 {
        state.cursor_col -= 1;
    } else if state.cursor_row > 0 {
        state.cursor_row -= 1;
        state.cursor_col = state.cols.saturating_sub(1);
    }
}

fn write_byte_locked(state: &mut FbConsoleState, byte: u8) {
    match byte {
        b'\r' => {
            state.cursor_col = 0;
        }
        b'\n' => {
            line_feed(state);
        }
        b'\t' => {
            let count = tab_spaces(state.cursor_col);
            for _ in 0..count {
                draw_glyph(state, state.cursor_col, state.cursor_row, b' ');
                advance_after_printable(state);
            }
        }
        0x08 | 0x7F => {
            backspace_cursor(state);
            clear_cell(state, state.cursor_col, state.cursor_row);
        }
        0x20..=0x7E => {
            draw_glyph(state, state.cursor_col, state.cursor_row, byte);
            advance_after_printable(state);
        }
        _ => {}
    }
}

pub fn init() {
    let fb = framebuffer::info();
    if !fb.available {
        return;
    }

    let cols = (fb.width as usize) / GLYPH_WIDTH;
    let rows = (fb.height as usize) / GLYPH_HEIGHT;
    if cols == 0 || rows == 0 {
        return;
    }

    let mut state = FB_CONSOLE.lock();
    state.initialized = true;
    state.fb = fb;
    state.cols = cols;
    state.rows = rows;
    state.cursor_col = 0;
    state.cursor_row = 0;
    clear_screen(&state);
}

pub fn write_bytes(bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }

    let mut state = FB_CONSOLE.lock();
    if !state.initialized {
        return;
    }

    for &byte in bytes {
        write_byte_locked(&mut state, byte);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_stops_are_four_columns() {
        assert_eq!(tab_spaces(0), 4);
        assert_eq!(tab_spaces(1), 3);
        assert_eq!(tab_spaces(2), 2);
        assert_eq!(tab_spaces(3), 1);
        assert_eq!(tab_spaces(4), 4);
    }

    #[test]
    fn printable_wraps_at_end_of_row() {
        let mut state = FbConsoleState::unavailable();
        state.cols = 4;
        state.rows = 2;
        state.cursor_col = 3;
        state.cursor_row = 0;

        advance_after_printable(&mut state);
        assert_eq!(state.cursor_col, 0);
        assert_eq!(state.cursor_row, 1);
    }

    #[test]
    fn backspace_moves_to_previous_row_tail() {
        let mut state = FbConsoleState::unavailable();
        state.cols = 10;
        state.rows = 3;
        state.cursor_col = 0;
        state.cursor_row = 2;

        backspace_cursor(&mut state);
        assert_eq!(state.cursor_col, 9);
        assert_eq!(state.cursor_row, 1);
    }
}
