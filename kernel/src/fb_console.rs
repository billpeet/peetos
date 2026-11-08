use core::ptr;

use crate::font8x16_vga::FONT8X16_VGA;
use crate::framebuffer::{self, FramebufferInfo};
use crate::sync::SpinLock;

const GLYPH_WIDTH: usize = 8;
const GLYPH_HEIGHT: usize = 16;
const TAB_WIDTH: usize = 4;

const PANEL_MARGIN_X: usize = 24;
const PANEL_MARGIN_Y: usize = 24;
const PANEL_BORDER: usize = 2;
const PANEL_PADDING_X: usize = 12;
const PANEL_PADDING_Y: usize = 12;
const PANEL_TITLE_BAR_HEIGHT: usize = 20;
const DESKTOP_HEADER_HEIGHT: usize = 24;
const DESKTOP_HEADER_GAP: usize = 10;
const STATUS_TEXT_PAD_X: usize = 6;
const STATUS_SEG_GAP_X: usize = 4;
const STATUS_POLL_TICKS: u8 = 20;
const STATUS_PATH_MAX_DEPTH: usize = 32;
const DESKTOP_HEADER_LABEL: &[u8] = b"PeetOS";
const STATUS_TITLE_LABEL: &[u8] = b"Terminal";

const ANSI_MAX_PARAMS: usize = 4;

const DEFAULT_FG_INDEX: u8 = 7;
const DEFAULT_BG_INDEX: u8 = 0;
const CURSOR_BLINK_TICKS: u64 = 50;
const CURSOR_INVERT_MASK: u32 = 0x00FF_FFFF;

const DEFAULT_THEME: ThemePreset = ThemePreset::Midnight;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemePreset {
    Midnight = 0,
    Amber = 1,
    Green = 2,
    Solarized = 3,
}

#[derive(Clone, Copy)]
struct ThemeColors {
    desktop_bg: u32,
    panel_border: u32,
    panel_bg: u32,
    panel_title: u32,
    default_fg: u8,
    default_bg: u8,
    palette: [u32; 16],
}

const THEME_MIDNIGHT: ThemeColors = ThemeColors {
    desktop_bg: 0x0016_1F_2F,
    panel_border: 0x003A_4E_68,
    panel_bg: 0x000C_11_18,
    panel_title: 0x001F_2D_41,
    default_fg: 7,
    default_bg: 0,
    palette: [
        0x000C_1118,
        0x00C8_5353,
        0x007E_BA63,
        0x00DB_B767,
        0x0073_98DC,
        0x00BA_8EDB,
        0x0069_BD_CA,
        0x00D4_D9_E4,
        0x004E_596A,
        0x00E2_7E7E,
        0x0098_CC7C,
        0x00E6_C67F,
        0x0089_AEEA,
        0x00CE_A4E9,
        0x0089_D0DA,
        0x00F1_F4_F8,
    ],
};

const THEME_AMBER: ThemeColors = ThemeColors {
    desktop_bg: 0x001B_1710,
    panel_border: 0x0058_4323,
    panel_bg: 0x000F_0B_06,
    panel_title: 0x0032_2715,
    default_fg: 14,
    default_bg: 0,
    palette: [
        0x000F_0B_06,
        0x00C7_6D_43,
        0x0092_A4_67,
        0x00C9_9732,
        0x0086_9C_B5,
        0x00B2_8E_C4,
        0x006C_A8_A0,
        0x00DE_D1_BA,
        0x0057_4C_3A,
        0x00E5_8C_63,
        0x00B3_C7_78,
        0x00E7_B7_4A,
        0x00A4_BA_D1,
        0x00D0_A8_E0,
        0x0088_C7_BE,
        0x00FF_E9_C5,
    ],
};

const THEME_GREEN: ThemeColors = ThemeColors {
    desktop_bg: 0x000E_1A_12,
    panel_border: 0x0029_5B_3E,
    panel_bg: 0x0008_10_0B,
    panel_title: 0x0015_31_23,
    default_fg: 10,
    default_bg: 0,
    palette: [
        0x0008_100B,
        0x00A5_5A_5A,
        0x0069_BA_72,
        0x00B0_B2_63,
        0x0067_8E_B8,
        0x0092_74_AE,
        0x0052_A0_9A,
        0x00C4_D7_CC,
        0x003A_4F_42,
        0x00CC_7676,
        0x0088_E9_95,
        0x00D0_D3_79,
        0x0080_AE_DD,
        0x00B6_96_D1,
        0x0066_C2_BA,
        0x00EA_F7_F0,
    ],
};

const THEME_SOLARIZED: ThemeColors = ThemeColors {
    desktop_bg: 0x0000_2B_36,
    panel_border: 0x0007_36_42,
    panel_bg: 0x0000_25_2E,
    panel_title: 0x0007_36_42,
    default_fg: 12,
    default_bg: 0,
    palette: [
        0x0000_25_2E,
        0x00DC_322F,
        0x0085_99_00,
        0x00B5_89_00,
        0x0026_8B_D2,
        0x00D3_36_82,
        0x002A_A1_98,
        0x0093_A1_A1,
        0x0065_7B_83,
        0x00CB_4B_16,
        0x0058_6E_75,
        0x0065_7B_83,
        0x0083_94_96,
        0x006C_71_C4,
        0x0093_A1_A1,
        0x00FD_F6_E3,
    ],
};

fn theme_colors(preset: ThemePreset) -> ThemeColors {
    match preset {
        ThemePreset::Midnight => THEME_MIDNIGHT,
        ThemePreset::Amber => THEME_AMBER,
        ThemePreset::Green => THEME_GREEN,
        ThemePreset::Solarized => THEME_SOLARIZED,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
enum AnsiState {
    Normal = 0,
    Esc = 1,
    Csi = 2,
}

#[derive(Clone, Copy)]
struct FbConsoleState {
    initialized: bool,
    fb: FramebufferInfo,
    theme: ThemePreset,
    desktop_bg: u32,
    panel_border: u32,
    panel_bg: u32,
    panel_title: u32,
    palette: [u32; 16],
    chrome_enabled: bool,
    panel_x: usize,
    panel_y: usize,
    panel_width: usize,
    panel_height: usize,
    text_origin_x: usize,
    text_origin_y: usize,
    cols: usize,
    rows: usize,
    cursor_col: usize,
    cursor_row: usize,
    default_fg_index: u8,
    default_bg_index: u8,
    fg_index: u8,
    bg_index: u8,
    bold: bool,
    cursor_drawn: bool,
    blink_ticks: u64,
    ansi_state: AnsiState,
    ansi_params: [u16; ANSI_MAX_PARAMS],
    ansi_param_count: u8,
    ansi_current_param: u16,
    status_poll_ticks: u8,
    status_cwd_len: usize,
    status_cwd: [u8; fs::VFS_MAX_PATH],
}

impl FbConsoleState {
    const fn unavailable() -> Self {
        Self {
            initialized: false,
            fb: FramebufferInfo::unavailable(),
            theme: DEFAULT_THEME,
            desktop_bg: THEME_MIDNIGHT.desktop_bg,
            panel_border: THEME_MIDNIGHT.panel_border,
            panel_bg: THEME_MIDNIGHT.panel_bg,
            panel_title: THEME_MIDNIGHT.panel_title,
            palette: THEME_MIDNIGHT.palette,
            chrome_enabled: false,
            panel_x: 0,
            panel_y: 0,
            panel_width: 0,
            panel_height: 0,
            text_origin_x: 0,
            text_origin_y: 0,
            cols: 0,
            rows: 0,
            cursor_col: 0,
            cursor_row: 0,
            default_fg_index: DEFAULT_FG_INDEX,
            default_bg_index: DEFAULT_BG_INDEX,
            fg_index: DEFAULT_FG_INDEX,
            bg_index: DEFAULT_BG_INDEX,
            bold: false,
            cursor_drawn: false,
            blink_ticks: 0,
            ansi_state: AnsiState::Normal,
            ansi_params: [0; ANSI_MAX_PARAMS],
            ansi_param_count: 0,
            ansi_current_param: 0,
            status_poll_ticks: 0,
            status_cwd_len: 0,
            status_cwd: [0; fs::VFS_MAX_PATH],
        }
    }
}

static FB_CONSOLE: SpinLock<FbConsoleState> = SpinLock::new(FbConsoleState::unavailable());

#[inline]
fn console_active() -> bool {
    crate::display_mode::console_active()
}

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
    let offset = y.saturating_mul(pitch_pixels(fb)).saturating_add(x);
    unsafe {
        ptr::write_volatile((fb.buffer_virt as *mut u32).add(offset), color);
    }
}

fn fill_rect(fb: FramebufferInfo, x: usize, y: usize, width: usize, height: usize, color: u32) {
    let x0 = core::cmp::min(x, fb.width as usize);
    let y0 = core::cmp::min(y, fb.height as usize);
    let x1 = core::cmp::min(x0.saturating_add(width), fb.width as usize);
    let y1 = core::cmp::min(y0.saturating_add(height), fb.height as usize);
    for py in y0..y1 {
        for px in x0..x1 {
            put_pixel(fb, px, py, color);
        }
    }
}

fn xor_pixel(fb: FramebufferInfo, x: usize, y: usize, mask: u32) {
    if x >= fb.width as usize || y >= fb.height as usize {
        return;
    }
    let offset = y.saturating_mul(pitch_pixels(fb)).saturating_add(x);
    unsafe {
        let dst = (fb.buffer_virt as *mut u32).add(offset);
        let current = ptr::read_volatile(dst);
        ptr::write_volatile(dst, current ^ mask);
    }
}

fn toggle_cursor(state: &mut FbConsoleState) {
    let x0 = state
        .text_origin_x
        .saturating_add(state.cursor_col.saturating_mul(GLYPH_WIDTH));
    let y0 = state
        .text_origin_y
        .saturating_add(state.cursor_row.saturating_mul(GLYPH_HEIGHT));
    let cursor_h = 2usize;
    let y_start = y0.saturating_add(GLYPH_HEIGHT.saturating_sub(cursor_h));
    for y in y_start..y0.saturating_add(GLYPH_HEIGHT) {
        for x in x0..x0.saturating_add(GLYPH_WIDTH) {
            xor_pixel(state.fb, x, y, CURSOR_INVERT_MASK);
        }
    }
    state.cursor_drawn = !state.cursor_drawn;
}

fn hide_cursor(state: &mut FbConsoleState) {
    if state.cursor_drawn {
        toggle_cursor(state);
    }
}

fn cursor_should_be_visible(state: &FbConsoleState) -> bool {
    ((state.blink_ticks / CURSOR_BLINK_TICKS) & 1) == 0
}

fn sync_cursor(state: &mut FbConsoleState) {
    let should_show = cursor_should_be_visible(state);
    if should_show != state.cursor_drawn {
        toggle_cursor(state);
    }
}

fn draw_panel(state: &FbConsoleState) {
    if !state.chrome_enabled {
        fill_rect(
            state.fb,
            0,
            0,
            state.fb.width as usize,
            state.fb.height as usize,
            current_bg_color(state),
        );
        return;
    }

    fill_rect(
        state.fb,
        state.panel_x,
        state.panel_y,
        state.panel_width,
        state.panel_height,
        state.panel_border,
    );
    fill_rect(
        state.fb,
        state.panel_x.saturating_add(PANEL_BORDER),
        state.panel_y.saturating_add(PANEL_BORDER),
        state.panel_width.saturating_sub(PANEL_BORDER * 2),
        state.panel_height.saturating_sub(PANEL_BORDER * 2),
        state.panel_bg,
    );
    fill_rect(
        state.fb,
        state.panel_x.saturating_add(PANEL_BORDER),
        state.panel_y.saturating_add(PANEL_BORDER),
        state.panel_width.saturating_sub(PANEL_BORDER * 2),
        PANEL_TITLE_BAR_HEIGHT,
        state.panel_title,
    );
}

#[inline]
fn palette_color(state: &FbConsoleState, index: u8) -> u32 {
    state.palette[index as usize & 0x0F]
}

fn current_fg_color(state: &FbConsoleState) -> u32 {
    if state.bold && state.fg_index < 8 {
        palette_color(state, state.fg_index + 8)
    } else {
        palette_color(state, state.fg_index)
    }
}

fn current_bg_color(state: &FbConsoleState) -> u32 {
    palette_color(state, state.bg_index)
}

fn clear_screen(state: &FbConsoleState) {
    if !state.chrome_enabled {
        fill_rect(
            state.fb,
            0,
            0,
            state.fb.width as usize,
            state.fb.height as usize,
            current_bg_color(state),
        );
        return;
    }

    fill_rect(
        state.fb,
        0,
        0,
        state.fb.width as usize,
        state.fb.height as usize,
        state.desktop_bg,
    );
    draw_desktop_header(state);
    draw_panel(state);
    draw_status_line(state);
}

fn draw_desktop_header(state: &FbConsoleState) {
    if !state.chrome_enabled {
        return;
    }

    let header_h = core::cmp::min(DESKTOP_HEADER_HEIGHT, state.fb.height as usize);
    let header_bg = palette_color(state, 2);
    let header_fg = palette_color(state, 15);
    fill_rect(state.fb, 0, 0, state.fb.width as usize, header_h, header_bg);
    if header_h > 0 {
        fill_rect(
            state.fb,
            0,
            header_h.saturating_sub(1),
            state.fb.width as usize,
            1,
            palette_color(state, 8),
        );
    }

    if header_h < GLYPH_HEIGHT {
        return;
    }
    let label_w = DESKTOP_HEADER_LABEL.len().saturating_mul(GLYPH_WIDTH);
    let max_x = (state.fb.width as usize).saturating_sub(label_w);
    let text_x = max_x / 2;
    let text_y = (header_h.saturating_sub(GLYPH_HEIGHT)) / 2;
    let _ = draw_text_raw(
        state,
        text_x,
        text_y,
        DESKTOP_HEADER_LABEL,
        header_fg,
        header_bg,
        DESKTOP_HEADER_LABEL.len(),
    );
}

fn draw_glyph(state: &FbConsoleState, col: usize, row: usize, byte: u8) {
    let x0 = state
        .text_origin_x
        .saturating_add(col.saturating_mul(GLYPH_WIDTH));
    let y0 = state
        .text_origin_y
        .saturating_add(row.saturating_mul(GLYPH_HEIGHT));
    let glyph = &FONT8X16_VGA[byte as usize];
    let fg = current_fg_color(state);
    let bg = current_bg_color(state);

    for (gy, bits) in glyph.iter().copied().enumerate() {
        for gx in 0..8 {
            let on = (bits & (0x80 >> gx)) != 0;
            let color = if on { fg } else { bg };
            put_pixel(state.fb, x0 + gx, y0 + gy, color);
        }
    }
}

fn draw_glyph_raw(state: &FbConsoleState, x0: usize, y0: usize, byte: u8, fg: u32, bg: u32) {
    let glyph_idx = if (0x20..=0x7E).contains(&byte) {
        byte
    } else {
        b'?'
    };
    let glyph = &FONT8X16_VGA[glyph_idx as usize];

    for (gy, bits) in glyph.iter().copied().enumerate() {
        for gx in 0..8 {
            let on = (bits & (0x80 >> gx)) != 0;
            let color = if on { fg } else { bg };
            put_pixel(state.fb, x0 + gx, y0 + gy, color);
        }
    }
}

fn draw_text_raw(
    state: &FbConsoleState,
    x0: usize,
    y0: usize,
    text: &[u8],
    fg: u32,
    bg: u32,
    max_chars: usize,
) -> usize {
    let count = core::cmp::min(text.len(), max_chars);
    let mut idx = 0usize;
    while idx < count {
        let x = x0.saturating_add(idx.saturating_mul(GLYPH_WIDTH));
        draw_glyph_raw(state, x, y0, text[idx], fg, bg);
        idx += 1;
    }
    count
}

#[inline]
fn status_component_is_dot(name: &[u8]) -> bool {
    name.len() == 1 && name[0] == b'.'
}

#[inline]
fn status_component_is_dotdot(name: &[u8]) -> bool {
    name.len() == 2 && name[0] == b'.' && name[1] == b'.'
}

fn find_inode_path_for_status(
    dir_inode: u16,
    target_inode: u16,
    current_len: usize,
    depth: usize,
    path_buf: &mut [u8; fs::VFS_MAX_PATH],
) -> Option<usize> {
    if dir_inode == target_inode {
        return Some(current_len);
    }
    if depth >= STATUS_PATH_MAX_DEPTH {
        return None;
    }

    let mut offset = 0u64;
    loop {
        let entry = match fs::readdir(dir_inode, offset) {
            Ok(Some(entry)) => entry,
            Ok(None) => break,
            Err(_) => return None,
        };
        offset = entry.next_offset;
        if entry.name_len == 0 {
            continue;
        }
        let name = &entry.name[..entry.name_len];
        if status_component_is_dot(name) || status_component_is_dotdot(name) {
            continue;
        }
        let name_str = match core::str::from_utf8(name) {
            Ok(value) => value,
            Err(_) => continue,
        };
        let child_inode = match fs::lookupat(dir_inode, name_str) {
            Ok(inode) => inode,
            Err(_) => continue,
        };

        let next_len;
        if current_len == 0 {
            if 1 + name.len() >= path_buf.len() {
                continue;
            }
            path_buf[0] = b'/';
            path_buf[1..1 + name.len()].copy_from_slice(name);
            next_len = 1 + name.len();
        } else {
            if current_len + 1 + name.len() >= path_buf.len() {
                continue;
            }
            path_buf[current_len] = b'/';
            path_buf[current_len + 1..current_len + 1 + name.len()].copy_from_slice(name);
            next_len = current_len + 1 + name.len();
        }

        if child_inode == target_inode {
            return Some(next_len);
        }

        if matches!(fs::inode_is_dir(child_inode), Ok(true)) {
            if let Some(found_len) =
                find_inode_path_for_status(child_inode, target_inode, next_len, depth + 1, path_buf)
            {
                return Some(found_len);
            }
        }
    }

    None
}

fn query_current_cwd_path(path_buf: &mut [u8; fs::VFS_MAX_PATH]) -> usize {
    let task_ptr = crate::task::current();
    if task_ptr.is_null() {
        path_buf[0] = b'/';
        return 1;
    }

    let cwd = unsafe { (*crate::task::leader_of(task_ptr)).user_cwd_inode };
    if cwd == fs::root_inode() {
        path_buf[0] = b'/';
        return 1;
    }

    match find_inode_path_for_status(fs::root_inode(), cwd, 0, 0, path_buf) {
        Some(len) if len > 0 => len,
        _ => {
            path_buf[0] = b'?';
            1
        }
    }
}

fn format_status_path(path: &[u8], max_chars: usize, out: &mut [u8; fs::VFS_MAX_PATH]) -> usize {
    let sanitize = |byte: u8| {
        if (0x20..=0x7E).contains(&byte) {
            byte
        } else {
            b'?'
        }
    };

    if max_chars == 0 {
        return 0;
    }

    let path_len = core::cmp::min(path.len(), out.len());
    if path_len <= max_chars {
        let mut idx = 0usize;
        while idx < path_len {
            out[idx] = sanitize(path[idx]);
            idx += 1;
        }
        return path_len;
    }

    if max_chars <= 3 {
        let mut idx = 0usize;
        while idx < max_chars {
            out[idx] = b'.';
            idx += 1;
        }
        return max_chars;
    }

    out[0] = b'.';
    out[1] = b'.';
    out[2] = b'.';
    let tail_len = max_chars - 3;
    let tail_start = path_len - tail_len;
    let mut idx = 0usize;
    while idx < tail_len {
        out[3 + idx] = sanitize(path[tail_start + idx]);
        idx += 1;
    }
    max_chars
}

fn refresh_status_path_cache(state: &mut FbConsoleState, force: bool) -> bool {
    if !force {
        if state.status_poll_ticks < STATUS_POLL_TICKS {
            state.status_poll_ticks = state.status_poll_ticks.saturating_add(1);
            return false;
        }
    }

    state.status_poll_ticks = 0;

    let mut latest = [0u8; fs::VFS_MAX_PATH];
    let latest_len = query_current_cwd_path(&mut latest);
    if latest_len == state.status_cwd_len && state.status_cwd[..latest_len] == latest[..latest_len]
    {
        return false;
    }

    state.status_cwd[..latest_len].copy_from_slice(&latest[..latest_len]);
    if latest_len < state.status_cwd_len {
        state.status_cwd[latest_len..state.status_cwd_len].fill(0);
    }
    state.status_cwd_len = latest_len;
    true
}

fn draw_status_segment(
    state: &FbConsoleState,
    x: usize,
    y: usize,
    text: &[u8],
    fg: u32,
    bg: u32,
) -> usize {
    let text_width = text.len().saturating_mul(GLYPH_WIDTH);
    let seg_width = text_width.saturating_add(STATUS_TEXT_PAD_X.saturating_mul(2));
    fill_rect(state.fb, x, y, seg_width, GLYPH_HEIGHT, bg);
    let text_x = x.saturating_add(STATUS_TEXT_PAD_X);
    let _ = draw_text_raw(state, text_x, y, text, fg, bg, text.len());
    x.saturating_add(seg_width)
}

fn draw_status_line(state: &FbConsoleState) {
    if !state.chrome_enabled {
        return;
    }

    let title_x = state.panel_x.saturating_add(PANEL_BORDER);
    let title_y = state.panel_y.saturating_add(PANEL_BORDER);
    let title_w = state
        .panel_width
        .saturating_sub(PANEL_BORDER.saturating_mul(2));
    fill_rect(
        state.fb,
        title_x,
        title_y,
        title_w,
        PANEL_TITLE_BAR_HEIGHT,
        state.panel_title,
    );

    let text_y = title_y.saturating_add(PANEL_TITLE_BAR_HEIGHT.saturating_sub(GLYPH_HEIGHT) / 2);
    let mut x = title_x.saturating_add(STATUS_TEXT_PAD_X);

    let title_bg = palette_color(state, 4);
    let title_fg = palette_color(state, 15);
    let cwd_bg = palette_color(state, 0);
    let cwd_fg = palette_color(state, 14);

    x = draw_status_segment(state, x, text_y, STATUS_TITLE_LABEL, title_fg, title_bg);
    x = x.saturating_add(STATUS_SEG_GAP_X);

    let title_right = title_x.saturating_add(title_w);
    let right_pad = STATUS_TEXT_PAD_X;
    if x.saturating_add(right_pad) >= title_right {
        return;
    }

    let cwd_width = title_right.saturating_sub(x.saturating_add(right_pad));
    fill_rect(state.fb, x, text_y, cwd_width, GLYPH_HEIGHT, cwd_bg);
    let cwd_chars = cwd_width / GLYPH_WIDTH;
    if cwd_chars == 0 {
        return;
    }

    let mut rendered = [0u8; fs::VFS_MAX_PATH];
    let rendered_len = format_status_path(
        &state.status_cwd[..state.status_cwd_len],
        cwd_chars,
        &mut rendered,
    );
    let _ = draw_text_raw(
        state,
        x,
        text_y,
        &rendered[..rendered_len],
        cwd_fg,
        cwd_bg,
        cwd_chars,
    );
}

fn clear_cell(state: &FbConsoleState, col: usize, row: usize) {
    let x0 = state
        .text_origin_x
        .saturating_add(col.saturating_mul(GLYPH_WIDTH));
    let y0 = state
        .text_origin_y
        .saturating_add(row.saturating_mul(GLYPH_HEIGHT));
    let bg = current_bg_color(state);
    for y in 0..GLYPH_HEIGHT {
        for x in 0..GLYPH_WIDTH {
            put_pixel(state.fb, x0 + x, y0 + y, bg);
        }
    }
}

fn scroll_one_text_row(state: &mut FbConsoleState) {
    let fb = state.fb;
    let pitch = fb.pitch as usize;
    let text_height_px = state.rows.saturating_mul(GLYPH_HEIGHT);
    let text_width_px = state.cols.saturating_mul(GLYPH_WIDTH);
    let scroll_px = GLYPH_HEIGHT;
    if scroll_px >= text_height_px || text_width_px == 0 {
        fill_rect(
            fb,
            state.text_origin_x,
            state.text_origin_y,
            text_width_px,
            text_height_px,
            current_bg_color(state),
        );
        return;
    }

    let move_rows = text_height_px - scroll_px;
    let row_copy_bytes = text_width_px.saturating_mul(4);
    unsafe {
        let base = fb.buffer_virt as *mut u8;
        let mut row = 0usize;
        while row < move_rows {
            let dst_off = state
                .text_origin_y
                .saturating_add(row)
                .saturating_mul(pitch)
                .saturating_add(state.text_origin_x.saturating_mul(4));
            let src_off = state
                .text_origin_y
                .saturating_add(row)
                .saturating_add(scroll_px)
                .saturating_mul(pitch)
                .saturating_add(state.text_origin_x.saturating_mul(4));
            ptr::copy(base.add(src_off), base.add(dst_off), row_copy_bytes);
            row += 1;
        }
    }

    let clear_y = state
        .text_origin_y
        .saturating_add(text_height_px)
        .saturating_sub(scroll_px);
    fill_rect(
        fb,
        state.text_origin_x,
        clear_y,
        text_width_px,
        scroll_px,
        current_bg_color(state),
    );
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

fn ansi_get_param(state: &FbConsoleState, idx: usize, default: u16) -> u16 {
    if idx < state.ansi_param_count as usize && state.ansi_params[idx] > 0 {
        state.ansi_params[idx]
    } else {
        default
    }
}

fn ansi_reset(state: &mut FbConsoleState) {
    state.ansi_state = AnsiState::Normal;
    state.ansi_param_count = 0;
    state.ansi_current_param = 0;
    let mut i = 0;
    while i < ANSI_MAX_PARAMS {
        state.ansi_params[i] = 0;
        i += 1;
    }
}

fn apply_theme(state: &mut FbConsoleState, preset: ThemePreset) {
    let theme = theme_colors(preset);
    state.theme = preset;
    state.desktop_bg = theme.desktop_bg;
    state.panel_border = theme.panel_border;
    state.panel_bg = theme.panel_bg;
    state.panel_title = theme.panel_title;
    state.palette = theme.palette;
    state.default_fg_index = theme.default_fg;
    state.default_bg_index = theme.default_bg;
    state.fg_index = state.default_fg_index;
    state.bg_index = state.default_bg_index;
    state.bold = false;
}

fn move_cursor_to(state: &mut FbConsoleState, row: usize, col: usize) {
    state.cursor_row = if row < state.rows {
        row
    } else {
        state.rows.saturating_sub(1)
    };
    state.cursor_col = if col < state.cols {
        col
    } else {
        state.cols.saturating_sub(1)
    };
}

fn clear_from_cursor_to_end(state: &FbConsoleState) {
    let mut col = state.cursor_col;
    while col < state.cols {
        clear_cell(state, col, state.cursor_row);
        col += 1;
    }
    let mut row = state.cursor_row + 1;
    while row < state.rows {
        let mut c = 0;
        while c < state.cols {
            clear_cell(state, c, row);
            c += 1;
        }
        row += 1;
    }
}

fn clear_line_from_cursor(state: &FbConsoleState) {
    let mut col = state.cursor_col;
    while col < state.cols {
        clear_cell(state, col, state.cursor_row);
        col += 1;
    }
}

fn ansi_apply_sgr(state: &mut FbConsoleState) {
    if state.ansi_param_count == 0 {
        state.fg_index = state.default_fg_index;
        state.bg_index = state.default_bg_index;
        state.bold = false;
        return;
    }

    let count = state.ansi_param_count as usize;
    let mut idx = 0;
    while idx < count {
        match state.ansi_params[idx] {
            0 => {
                state.fg_index = state.default_fg_index;
                state.bg_index = state.default_bg_index;
                state.bold = false;
            }
            1 => state.bold = true,
            22 => state.bold = false,
            30..=37 => state.fg_index = (state.ansi_params[idx] - 30) as u8,
            39 => state.fg_index = state.default_fg_index,
            40..=47 => state.bg_index = (state.ansi_params[idx] - 40) as u8,
            49 => state.bg_index = state.default_bg_index,
            90..=97 => state.fg_index = (state.ansi_params[idx] - 90 + 8) as u8,
            100..=107 => state.bg_index = (state.ansi_params[idx] - 100 + 8) as u8,
            _ => {}
        }
        idx += 1;
    }
}

fn ansi_dispatch_csi(state: &mut FbConsoleState, cmd: u8) {
    if state.ansi_current_param > 0 || state.ansi_param_count > 0 || cmd == b'm' {
        if (state.ansi_param_count as usize) < ANSI_MAX_PARAMS {
            state.ansi_params[state.ansi_param_count as usize] = state.ansi_current_param;
            state.ansi_param_count = state.ansi_param_count.saturating_add(1);
        }
    }

    match cmd {
        b'H' | b'f' => {
            let row = ansi_get_param(state, 0, 1).saturating_sub(1) as usize;
            let col = ansi_get_param(state, 1, 1).saturating_sub(1) as usize;
            move_cursor_to(state, row, col);
        }
        b'A' => {
            let n = ansi_get_param(state, 0, 1) as usize;
            state.cursor_row = state.cursor_row.saturating_sub(n);
        }
        b'B' => {
            let n = ansi_get_param(state, 0, 1) as usize;
            let max_row = state.rows.saturating_sub(1);
            state.cursor_row = core::cmp::min(state.cursor_row.saturating_add(n), max_row);
        }
        b'C' => {
            let n = ansi_get_param(state, 0, 1) as usize;
            let max_col = state.cols.saturating_sub(1);
            state.cursor_col = core::cmp::min(state.cursor_col.saturating_add(n), max_col);
        }
        b'D' => {
            let n = ansi_get_param(state, 0, 1) as usize;
            state.cursor_col = state.cursor_col.saturating_sub(n);
        }
        b'J' => {
            let mode = ansi_get_param(state, 0, 0);
            match mode {
                0 => {
                    clear_from_cursor_to_end(state);
                }
                2 => {
                    clear_screen(state);
                    state.cursor_col = 0;
                    state.cursor_row = 0;
                }
                _ => {}
            }
        }
        b'K' => {
            clear_line_from_cursor(state);
        }
        b'm' => {
            ansi_apply_sgr(state);
        }
        _ => {}
    }

    ansi_reset(state);
}

fn ansi_process_char(state: &mut FbConsoleState, byte: u8) -> bool {
    match state.ansi_state {
        AnsiState::Normal => {
            if byte == 0x1B {
                state.ansi_state = AnsiState::Esc;
                state.ansi_param_count = 0;
                state.ansi_current_param = 0;
                let mut i = 0;
                while i < ANSI_MAX_PARAMS {
                    state.ansi_params[i] = 0;
                    i += 1;
                }
                return true;
            }
            false
        }
        AnsiState::Esc => {
            if byte == b'[' {
                state.ansi_state = AnsiState::Csi;
                return true;
            }
            ansi_reset(state);
            false
        }
        AnsiState::Csi => {
            if byte >= b'0' && byte <= b'9' {
                state.ansi_current_param = state
                    .ansi_current_param
                    .saturating_mul(10)
                    .saturating_add((byte - b'0') as u16);
                return true;
            }
            if byte == b';' {
                if (state.ansi_param_count as usize) < ANSI_MAX_PARAMS {
                    state.ansi_params[state.ansi_param_count as usize] = state.ansi_current_param;
                    state.ansi_param_count += 1;
                }
                state.ansi_current_param = 0;
                return true;
            }
            if byte >= 0x40 && byte <= 0x7E {
                ansi_dispatch_csi(state, byte);
                return true;
            }
            ansi_reset(state);
            false
        }
    }
}

fn write_byte_locked(state: &mut FbConsoleState, byte: u8) {
    if ansi_process_char(state, byte) {
        return;
    }

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

    let mut cols = (fb.width as usize) / GLYPH_WIDTH;
    let mut rows = (fb.height as usize) / GLYPH_HEIGHT;
    let mut panel_x = 0usize;
    let mut panel_y = 0usize;
    let mut panel_width = fb.width as usize;
    let mut panel_height = fb.height as usize;
    let mut text_origin_x = 0usize;
    let mut text_origin_y = 0usize;
    let mut chrome_enabled = false;

    let chrome_w = PANEL_MARGIN_X * 2 + PANEL_BORDER * 2 + PANEL_PADDING_X * 2;
    let chrome_h = PANEL_MARGIN_Y * 2
        + PANEL_BORDER * 2
        + PANEL_PADDING_Y * 2
        + PANEL_TITLE_BAR_HEIGHT
        + DESKTOP_HEADER_HEIGHT
        + DESKTOP_HEADER_GAP;

    if (fb.width as usize) > chrome_w && (fb.height as usize) > chrome_h {
        let max_text_width = (fb.width as usize).saturating_sub(chrome_w);
        let max_text_height = (fb.height as usize).saturating_sub(chrome_h);
        let panel_cols = max_text_width / GLYPH_WIDTH;
        let panel_rows = max_text_height / GLYPH_HEIGHT;
        if panel_cols > 0 && panel_rows > 0 {
            cols = panel_cols;
            rows = panel_rows;
            chrome_enabled = true;

            let text_width = cols.saturating_mul(GLYPH_WIDTH);
            let text_height = rows.saturating_mul(GLYPH_HEIGHT);
            let inner_width = text_width.saturating_add(PANEL_PADDING_X * 2);
            let inner_height =
                text_height.saturating_add(PANEL_PADDING_Y * 2 + PANEL_TITLE_BAR_HEIGHT);
            panel_width = inner_width.saturating_add(PANEL_BORDER * 2);
            panel_height = inner_height.saturating_add(PANEL_BORDER * 2);
            panel_x = (fb.width as usize).saturating_sub(panel_width) / 2;
            let top_reserved = DESKTOP_HEADER_HEIGHT.saturating_add(DESKTOP_HEADER_GAP);
            let usable_height = (fb.height as usize).saturating_sub(top_reserved);
            panel_y = top_reserved.saturating_add(usable_height.saturating_sub(panel_height) / 2);
            text_origin_x = panel_x.saturating_add(PANEL_BORDER + PANEL_PADDING_X);
            text_origin_y =
                panel_y.saturating_add(PANEL_BORDER + PANEL_TITLE_BAR_HEIGHT + PANEL_PADDING_Y);
        }
    }

    if cols == 0 || rows == 0 {
        return;
    }

    let mut state = FB_CONSOLE.lock();
    state.initialized = true;
    state.fb = fb;
    apply_theme(&mut state, DEFAULT_THEME);
    state.chrome_enabled = chrome_enabled;
    state.panel_x = panel_x;
    state.panel_y = panel_y;
    state.panel_width = panel_width;
    state.panel_height = panel_height;
    state.text_origin_x = text_origin_x;
    state.text_origin_y = text_origin_y;
    state.cols = cols;
    state.rows = rows;
    state.cursor_col = 0;
    state.cursor_row = 0;
    state.cursor_drawn = false;
    state.blink_ticks = 0;
    state.status_poll_ticks = STATUS_POLL_TICKS;
    state.status_cwd.fill(0);
    state.status_cwd[0] = b'/';
    state.status_cwd_len = 1;
    ansi_reset(&mut state);
    let _ = refresh_status_path_cache(&mut state, true);
    if console_active() {
        clear_screen(&state);
        sync_cursor(&mut state);
    }
}

pub fn write_bytes(bytes: &[u8]) {
    if bytes.is_empty() || !console_active() {
        return;
    }

    let mut state = FB_CONSOLE.lock();
    if !state.initialized {
        return;
    }

    hide_cursor(&mut state);
    for &byte in bytes {
        write_byte_locked(&mut state, byte);
    }
    sync_cursor(&mut state);
}

pub fn tick() {
    if !console_active() {
        return;
    }
    let mut state = FB_CONSOLE.lock();
    if !state.initialized {
        return;
    }
    if refresh_status_path_cache(&mut state, false) {
        draw_status_line(&state);
    }
    state.blink_ticks = state.blink_ticks.saturating_add(1);
    sync_cursor(&mut state);
}

pub fn set_theme(preset: ThemePreset) {
    let mut state = FB_CONSOLE.lock();
    if !state.initialized {
        return;
    }

    hide_cursor(&mut state);
    apply_theme(&mut state, preset);
    state.cursor_col = 0;
    state.cursor_row = 0;
    let _ = refresh_status_path_cache(&mut state, true);
    ansi_reset(&mut state);
    if console_active() {
        clear_screen(&state);
        sync_cursor(&mut state);
    } else {
        state.cursor_drawn = false;
    }
}

pub fn set_active(active: bool, redraw: bool) {
    let mut state = FB_CONSOLE.lock();
    if !state.initialized {
        return;
    }

    hide_cursor(&mut state);
    state.cursor_drawn = false;
    if !active {
        return;
    }

    let _ = refresh_status_path_cache(&mut state, true);
    if redraw {
        clear_screen(&state);
    }
    sync_cursor(&mut state);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_state(cols: usize, rows: usize) -> FbConsoleState {
        let mut s = FbConsoleState::unavailable();
        s.cols = cols;
        s.rows = rows;
        s
    }

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
        let mut state = test_state(4, 2);
        state.cursor_col = 3;
        state.cursor_row = 0;

        advance_after_printable(&mut state);
        assert_eq!(state.cursor_col, 0);
        assert_eq!(state.cursor_row, 1);
    }

    #[test]
    fn backspace_moves_to_previous_row_tail() {
        let mut state = test_state(10, 3);
        state.cursor_col = 0;
        state.cursor_row = 2;

        backspace_cursor(&mut state);
        assert_eq!(state.cursor_col, 9);
        assert_eq!(state.cursor_row, 1);
    }

    #[test]
    fn ansi_clear_screen_resets_cursor() {
        let mut state = test_state(80, 25);
        state.cursor_col = 10;
        state.cursor_row = 5;

        for &b in b"\x1b[2J" {
            ansi_process_char(&mut state, b);
        }
        assert_eq!(state.cursor_col, 0);
        assert_eq!(state.cursor_row, 0);
    }

    #[test]
    fn ansi_cursor_position() {
        let mut state = test_state(80, 25);

        for &b in b"\x1b[5;10H" {
            ansi_process_char(&mut state, b);
        }
        assert_eq!(state.cursor_row, 4);
        assert_eq!(state.cursor_col, 9);
    }

    #[test]
    fn ansi_cursor_up() {
        let mut state = test_state(80, 25);
        state.cursor_row = 10;
        state.cursor_col = 5;

        for &b in b"\x1b[3A" {
            ansi_process_char(&mut state, b);
        }
        assert_eq!(state.cursor_row, 7);
        assert_eq!(state.cursor_col, 5);
    }

    #[test]
    fn ansi_cursor_down_clamps() {
        let mut state = test_state(80, 25);
        state.cursor_row = 23;

        for &b in b"\x1b[5B" {
            ansi_process_char(&mut state, b);
        }
        assert_eq!(state.cursor_row, 24);
    }

    #[test]
    fn ansi_cursor_forward_and_back() {
        let mut state = test_state(80, 25);
        state.cursor_col = 10;

        for &b in b"\x1b[5C" {
            ansi_process_char(&mut state, b);
        }
        assert_eq!(state.cursor_col, 15);

        for &b in b"\x1b[3D" {
            ansi_process_char(&mut state, b);
        }
        assert_eq!(state.cursor_col, 12);
    }

    #[test]
    fn incomplete_sequence_recovers() {
        let mut state = test_state(80, 25);

        ansi_process_char(&mut state, 0x1B);
        assert_eq!(state.ansi_state, AnsiState::Esc);
        ansi_process_char(&mut state, b'A');
        assert_eq!(state.ansi_state, AnsiState::Normal);
    }

    #[test]
    fn sgr_sets_color_and_bold() {
        let mut state = test_state(80, 25);
        for &b in b"\x1b[31m" {
            ansi_process_char(&mut state, b);
        }
        assert_eq!(state.fg_index, 1);
        assert_eq!(state.bg_index, DEFAULT_BG_INDEX);

        for &b in b"\x1b[1m" {
            ansi_process_char(&mut state, b);
        }
        assert!(state.bold);
        assert_eq!(current_fg_color(&state), state.palette[9]);

        for &b in b"\x1b[44m" {
            ansi_process_char(&mut state, b);
        }
        assert_eq!(state.bg_index, 4);
    }

    #[test]
    fn sgr_reset_and_default_restore() {
        let mut state = test_state(80, 25);
        for &b in b"\x1b[91;103m" {
            ansi_process_char(&mut state, b);
        }
        assert_eq!(state.fg_index, 9);
        assert_eq!(state.bg_index, 11);

        for &b in b"\x1b[39;49m" {
            ansi_process_char(&mut state, b);
        }
        assert_eq!(state.fg_index, DEFAULT_FG_INDEX);
        assert_eq!(state.bg_index, DEFAULT_BG_INDEX);

        for &b in b"\x1b[m" {
            ansi_process_char(&mut state, b);
        }
        assert_eq!(state.fg_index, DEFAULT_FG_INDEX);
        assert_eq!(state.bg_index, DEFAULT_BG_INDEX);
        assert!(!state.bold);
        assert_eq!(state.ansi_state, AnsiState::Normal);
    }

    #[test]
    fn theme_defaults_drive_sgr_reset() {
        let mut state = test_state(80, 25);
        apply_theme(&mut state, ThemePreset::Amber);
        assert_eq!(state.default_fg_index, 14);
        assert_eq!(state.default_bg_index, 0);

        for &b in b"\x1b[31;44m" {
            ansi_process_char(&mut state, b);
        }
        assert_eq!(state.fg_index, 1);
        assert_eq!(state.bg_index, 4);

        for &b in b"\x1b[0m" {
            ansi_process_char(&mut state, b);
        }
        assert_eq!(state.fg_index, 14);
        assert_eq!(state.bg_index, 0);
    }

    #[test]
    fn theme_palette_updates_fg_lookup() {
        let mut state = test_state(80, 25);
        apply_theme(&mut state, ThemePreset::Green);
        state.fg_index = 2;
        assert_eq!(current_fg_color(&state), THEME_GREEN.palette[2]);
    }

    #[test]
    fn status_path_format_keeps_short_paths() {
        let mut out = [0u8; fs::VFS_MAX_PATH];
        let len = format_status_path(b"/mnt/ext2", 32, &mut out);
        assert_eq!(&out[..len], b"/mnt/ext2");
    }

    #[test]
    fn status_path_format_uses_ellipsis_for_long_paths() {
        let mut out = [0u8; fs::VFS_MAX_PATH];
        let len = format_status_path(b"/very/long/path", 8, &mut out);
        assert_eq!(&out[..len], b".../path");
    }

    #[test]
    fn status_path_format_handles_tiny_width() {
        let mut out = [0u8; fs::VFS_MAX_PATH];
        let len = format_status_path(b"/very/long/path", 3, &mut out);
        assert_eq!(&out[..len], b"...");
    }

    #[test]
    fn chrome_labels_match_terminal_ui() {
        assert_eq!(DESKTOP_HEADER_LABEL, b"PeetOS");
        assert_eq!(STATUS_TITLE_LABEL, b"Terminal");
    }
}
