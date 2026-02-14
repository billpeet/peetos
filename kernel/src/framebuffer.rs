use crate::sync::SpinLock;

const VESA_MODE_INFO_ADDR: usize = 0xFFFF_FFFF_8000_0A00;
const VESA_STATUS_ADDR: usize = 0xFFFF_FFFF_8000_0B00;

const FRAMEBUFFER_WINDOW_BASE: u64 = 0xFFFF_FFFF_8800_0000;
const FRAMEBUFFER_WINDOW_SIZE: u64 = 16 * 1024 * 1024;
const FRAMEBUFFER_WINDOW_OFFSET_MASK: u64 = 0x1F_FFFF;
const FRAMEBUFFER_MIN_PHYS_ADDR: u32 = 16 * 1024 * 1024;
const FRAMEBUFFER_MAX_DIMENSION: u16 = 8192;

const MODE_INFO_OFFSET_PITCH: usize = 16;
const MODE_INFO_OFFSET_WIDTH: usize = 18;
const MODE_INFO_OFFSET_HEIGHT: usize = 20;
const MODE_INFO_OFFSET_BPP: usize = 25;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FramebufferInfo {
    pub available: bool,
    pub mode: u16,
    pub phys_addr: u32,
    pub width: u16,
    pub height: u16,
    pub pitch: u16,
    pub bpp: u8,
    pub buffer_virt: u64,
}

impl FramebufferInfo {
    pub const fn unavailable() -> Self {
        Self {
            available: false,
            mode: 0,
            phys_addr: 0,
            width: 0,
            height: 0,
            pitch: 0,
            bpp: 0,
            buffer_virt: 0,
        }
    }
}

#[derive(Clone, Copy)]
struct BootVesaStatus {
    available: u8,
    mode: u16,
    phys_addr: u32,
}

static FRAMEBUFFER_INFO: SpinLock<FramebufferInfo> = SpinLock::new(FramebufferInfo::unavailable());

fn parse_status(raw: &[u8; 8]) -> BootVesaStatus {
    BootVesaStatus {
        available: raw[0],
        mode: u16::from_le_bytes([raw[2], raw[3]]),
        phys_addr: u32::from_le_bytes([raw[4], raw[5], raw[6], raw[7]]),
    }
}

fn parse_mode_info(raw: &[u8; 26]) -> (u16, u16, u16, u8) {
    let pitch = u16::from_le_bytes([raw[MODE_INFO_OFFSET_PITCH], raw[MODE_INFO_OFFSET_PITCH + 1]]);
    let width = u16::from_le_bytes([raw[MODE_INFO_OFFSET_WIDTH], raw[MODE_INFO_OFFSET_WIDTH + 1]]);
    let height = u16::from_le_bytes([
        raw[MODE_INFO_OFFSET_HEIGHT],
        raw[MODE_INFO_OFFSET_HEIGHT + 1],
    ]);
    let bpp = raw[MODE_INFO_OFFSET_BPP];
    (pitch, width, height, bpp)
}

fn framebuffer_virt_from_phys(phys_addr: u32) -> u64 {
    FRAMEBUFFER_WINDOW_BASE + (phys_addr as u64 & FRAMEBUFFER_WINDOW_OFFSET_MASK)
}

fn framebuffer_phys_addr_valid(phys_addr: u32) -> bool {
    phys_addr >= FRAMEBUFFER_MIN_PHYS_ADDR && (phys_addr & 0xFFF) == 0
}

fn framebuffer_geometry_valid(pitch: u16, width: u16, height: u16, bpp: u8) -> bool {
    if pitch == 0 || width == 0 || height == 0 {
        return false;
    }
    if width > FRAMEBUFFER_MAX_DIMENSION || height > FRAMEBUFFER_MAX_DIMENSION {
        return false;
    }
    if bpp != 32 {
        return false;
    }
    if pitch % 4 != 0 {
        return false;
    }
    let min_pitch = match (width as u32).checked_mul(4) {
        Some(v) if v <= u16::MAX as u32 => v as u16,
        _ => return false,
    };
    pitch >= min_pitch
}

fn framebuffer_bytes_required(pitch: u16, height: u16) -> Option<u64> {
    (pitch as u64).checked_mul(height as u64)
}

fn mapping_fits_window(phys_addr: u32, bytes_required: u64) -> bool {
    let offset = phys_addr as u64 & FRAMEBUFFER_WINDOW_OFFSET_MASK;
    match offset.checked_add(bytes_required) {
        Some(end) => end <= FRAMEBUFFER_WINDOW_SIZE,
        None => false,
    }
}

#[cfg(target_os = "none")]
fn probe_framebuffer() -> FramebufferInfo {
    let mut status_raw = [0u8; 8];
    let mut mode_raw = [0u8; 26];

    unsafe {
        let status_ptr = VESA_STATUS_ADDR as *const u8;
        let mut idx = 0usize;
        while idx < status_raw.len() {
            status_raw[idx] = core::ptr::read_volatile(status_ptr.add(idx));
            idx += 1;
        }

        let mode_ptr = VESA_MODE_INFO_ADDR as *const u8;
        idx = 0;
        while idx < mode_raw.len() {
            mode_raw[idx] = core::ptr::read_volatile(mode_ptr.add(idx));
            idx += 1;
        }
    }

    let status = parse_status(&status_raw);
    if status.available != 1 || status.mode == 0 {
        return FramebufferInfo::unavailable();
    }
    if !framebuffer_phys_addr_valid(status.phys_addr) {
        return FramebufferInfo::unavailable();
    }

    let (pitch, width, height, bpp) = parse_mode_info(&mode_raw);
    if !framebuffer_geometry_valid(pitch, width, height, bpp) {
        return FramebufferInfo::unavailable();
    }

    let bytes_required = match framebuffer_bytes_required(pitch, height) {
        Some(bytes) if bytes > 0 => bytes,
        _ => return FramebufferInfo::unavailable(),
    };
    if !mapping_fits_window(status.phys_addr, bytes_required) {
        return FramebufferInfo::unavailable();
    }

    FramebufferInfo {
        available: true,
        mode: status.mode,
        phys_addr: status.phys_addr,
        width,
        height,
        pitch,
        bpp,
        buffer_virt: framebuffer_virt_from_phys(status.phys_addr),
    }
}

#[cfg(not(target_os = "none"))]
fn probe_framebuffer() -> FramebufferInfo {
    FramebufferInfo::unavailable()
}

pub fn init() {
    let info = probe_framebuffer();
    let mut state = FRAMEBUFFER_INFO.lock();
    *state = info;
}

pub fn info() -> FramebufferInfo {
    *FRAMEBUFFER_INFO.lock()
}

pub fn is_available() -> bool {
    info().available
}

pub fn set_pixel(x: u32, y: u32, color: u32) -> bool {
    let fb = info();
    if !fb.available {
        return false;
    }

    if x >= fb.width as u32 || y >= fb.height as u32 {
        return false;
    }

    let pitch = fb.pitch as usize;
    let limit = pitch.saturating_mul(fb.height as usize);
    let offset = (y as usize)
        .saturating_mul(pitch)
        .saturating_add((x as usize).saturating_mul(4));
    if offset.checked_add(4).is_none() || offset + 4 > limit {
        return false;
    }

    unsafe {
        let dst = (fb.buffer_virt as *mut u8).add(offset) as *mut u32;
        core::ptr::write_volatile(dst, color);
    }
    true
}

pub fn clear(color: u32) -> bool {
    let fb = info();
    if !fb.available {
        return false;
    }

    let pitch_pixels = fb.pitch as usize / 4;
    let total_pixels = match pitch_pixels.checked_mul(fb.height as usize) {
        Some(v) => v,
        None => return false,
    };
    if total_pixels == 0 {
        return false;
    }
    unsafe {
        let mut idx = 0usize;
        while idx < total_pixels {
            core::ptr::write_volatile((fb.buffer_virt as *mut u32).add(idx), color);
            idx += 1;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_status_extracts_fields() {
        let raw = [1, 0, 0x34, 0x12, 0x78, 0x56, 0x34, 0x12];
        let status = parse_status(&raw);
        assert_eq!(status.available, 1);
        assert_eq!(status.mode, 0x1234);
        assert_eq!(status.phys_addr, 0x1234_5678);
    }

    #[test]
    fn parse_mode_info_extracts_fields() {
        let mut raw = [0u8; 26];
        raw[MODE_INFO_OFFSET_PITCH] = 0x00;
        raw[MODE_INFO_OFFSET_PITCH + 1] = 0x14; // 5120
        raw[MODE_INFO_OFFSET_WIDTH] = 0x00;
        raw[MODE_INFO_OFFSET_WIDTH + 1] = 0x05; // 1280
        raw[MODE_INFO_OFFSET_HEIGHT] = 0x00;
        raw[MODE_INFO_OFFSET_HEIGHT + 1] = 0x04; // 1024
        raw[MODE_INFO_OFFSET_BPP] = 32;

        let (pitch, width, height, bpp) = parse_mode_info(&raw);
        assert_eq!(pitch, 5120);
        assert_eq!(width, 1280);
        assert_eq!(height, 1024);
        assert_eq!(bpp, 32);
    }

    #[test]
    fn framebuffer_virt_uses_window_offset() {
        assert_eq!(
            framebuffer_virt_from_phys(0xFD12_3456),
            FRAMEBUFFER_WINDOW_BASE + 0x123456
        );
    }

    #[test]
    fn mapping_fits_window_checks_end_boundary() {
        let phys = 0xFD00_1000;
        assert!(mapping_fits_window(phys, FRAMEBUFFER_WINDOW_SIZE - 0x1000));
        assert!(!mapping_fits_window(phys, FRAMEBUFFER_WINDOW_SIZE));
    }

    #[test]
    fn bytes_required_multiplies_pitch_and_height() {
        assert_eq!(framebuffer_bytes_required(5120, 1024), Some(5_242_880));
    }

    #[test]
    fn phys_addr_validation_requires_high_aligned_addr() {
        assert!(framebuffer_phys_addr_valid(0xFD00_0000));
        assert!(!framebuffer_phys_addr_valid(0x000F_F000));
        assert!(!framebuffer_phys_addr_valid(0xFD00_0001));
    }

    #[test]
    fn geometry_validation_rejects_invalid_shapes() {
        assert!(framebuffer_geometry_valid(5120, 1280, 1024, 32));
        assert!(!framebuffer_geometry_valid(5116, 1280, 1024, 32));
        assert!(!framebuffer_geometry_valid(5120, 1280, 1024, 24));
        assert!(!framebuffer_geometry_valid(4096, 4096, 9000, 32));
    }
}
