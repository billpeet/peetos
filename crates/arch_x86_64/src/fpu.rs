//! x87/SSE register state save and restore (FXSAVE/FXRSTOR).
//!
//! The kernel is built with `-mmx,-sse,-sse2`, so kernel code never touches
//! the FPU/SSE register file. That means the live hardware FPU state always
//! belongs to the *current user task*, and the kernel only needs to:
//!
//! - save it into the outgoing task and restore the incoming task's copy at
//!   every context switch,
//! - copy it into a forked child,
//! - reset it to the ABI default on `execve` and for signal handlers,
//! - stash it in signal frames and restore it (sanitised) on `rt_sigreturn`.
//!
//! Only the legacy 512-byte FXSAVE image is handled. The kernel does not
//! enable `CR4.OSXSAVE`, so AVX (YMM/ZMM) state is architecturally unusable by
//! user code and needs no saving.

use core::sync::atomic::{AtomicU32, Ordering};

/// Size of the legacy FXSAVE/FXRSTOR memory image.
pub const FXSAVE_AREA_SIZE: usize = 512;

/// x87 control word after `FNINIT`: all exceptions masked, 64-bit precision,
/// round-to-nearest. This is also the SysV x86_64 ABI initial value.
pub const DEFAULT_FCW: u16 = 0x037F;

/// MXCSR power-on/ABI default: all SIMD exceptions masked, round-to-nearest.
pub const DEFAULT_MXCSR: u32 = 0x1F80;

/// MXCSR mask assumed when the CPU reports 0 in the FXSAVE image (per the
/// Intel SDM, a zero mask means bits 0-15 except DAZ are writable).
pub const FALLBACK_MXCSR_MASK: u32 = 0x0000_FFBF;

const FCW_OFFSET: usize = 0;
const MXCSR_OFFSET: usize = 24;
const MXCSR_MASK_OFFSET: usize = 28;

/// Writable MXCSR bits on this CPU. `FXRSTOR` raises #GP if a reserved bit is
/// set, so any MXCSR value that came from user memory is masked with this.
static MXCSR_MASK: AtomicU32 = AtomicU32::new(FALLBACK_MXCSR_MASK);

/// One FXSAVE image. The instruction requires 16-byte alignment.
#[repr(C, align(16))]
#[derive(Clone, Copy)]
pub struct FxSaveArea {
    pub bytes: [u8; FXSAVE_AREA_SIZE],
}

impl FxSaveArea {
    /// The register state a fresh process starts with (`FNINIT` x87 state,
    /// default MXCSR, all data registers zero).
    pub const fn new_default() -> Self {
        let mut bytes = [0u8; FXSAVE_AREA_SIZE];
        let fcw = DEFAULT_FCW.to_le_bytes();
        bytes[FCW_OFFSET] = fcw[0];
        bytes[FCW_OFFSET + 1] = fcw[1];
        let mxcsr = DEFAULT_MXCSR.to_le_bytes();
        bytes[MXCSR_OFFSET] = mxcsr[0];
        bytes[MXCSR_OFFSET + 1] = mxcsr[1];
        bytes[MXCSR_OFFSET + 2] = mxcsr[2];
        bytes[MXCSR_OFFSET + 3] = mxcsr[3];
        FxSaveArea { bytes }
    }

    pub fn fcw(&self) -> u16 {
        u16::from_le_bytes([self.bytes[FCW_OFFSET], self.bytes[FCW_OFFSET + 1]])
    }

    pub fn mxcsr(&self) -> u32 {
        read_u32(&self.bytes, MXCSR_OFFSET)
    }

    pub fn set_mxcsr(&mut self, value: u32) {
        self.bytes[MXCSR_OFFSET..MXCSR_OFFSET + 4].copy_from_slice(&value.to_le_bytes());
    }

    /// MXCSR_MASK field as written by `FXSAVE` (0 on some older CPUs).
    pub fn mxcsr_mask_field(&self) -> u32 {
        read_u32(&self.bytes, MXCSR_MASK_OFFSET)
    }

    /// Clear MXCSR bits that are reserved under `mask`, so that a subsequent
    /// `FXRSTOR` cannot fault. Used for images that came from user memory.
    pub fn sanitize_with_mask(&mut self, mask: u32) {
        let mxcsr = self.mxcsr();
        self.set_mxcsr(mxcsr & mask);
    }

    /// Sanitise against the mask detected by [`init`].
    pub fn sanitize(&mut self) {
        self.sanitize_with_mask(MXCSR_MASK.load(Ordering::Relaxed));
    }

    /// Capture the live hardware x87/SSE state into this image.
    #[inline]
    pub fn save(&mut self) {
        #[cfg(target_os = "none")]
        unsafe {
            core::arch::asm!(
                "fxsave64 [{}]",
                in(reg) self.bytes.as_mut_ptr(),
                options(nostack, preserves_flags)
            );
        }
    }

    /// Load this image into the hardware x87/SSE registers.
    ///
    /// The image must be either produced by [`save`](Self::save) /
    /// [`new_default`](Self::new_default) or passed through
    /// [`sanitize`](Self::sanitize); otherwise reserved MXCSR bits can #GP.
    #[inline]
    pub fn restore(&self) {
        #[cfg(target_os = "none")]
        unsafe {
            core::arch::asm!(
                "fxrstor64 [{}]",
                in(reg) self.bytes.as_ptr(),
                options(nostack, preserves_flags, readonly)
            );
        }
    }
}

impl Default for FxSaveArea {
    fn default() -> Self {
        Self::new_default()
    }
}

fn read_u32(bytes: &[u8; FXSAVE_AREA_SIZE], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

/// Convert the MXCSR_MASK field reported by `FXSAVE` into the effective mask.
pub fn effective_mxcsr_mask(reported: u32) -> u32 {
    if reported == 0 {
        FALLBACK_MXCSR_MASK
    } else {
        reported
    }
}

/// Detect the writable MXCSR bits and load the default FPU state into the
/// hardware. Must run after `CR4.OSFXSR` is set (done in `entry.asm`) and
/// before any user task runs.
pub fn init() {
    let mut probe = FxSaveArea::new_default();
    probe.save();
    MXCSR_MASK.store(effective_mxcsr_mask(probe.mxcsr_mask_field()), Ordering::Relaxed);
    reset_current();
}

/// Writable MXCSR bits detected at boot.
pub fn mxcsr_mask() -> u32 {
    MXCSR_MASK.load(Ordering::Relaxed)
}

/// Load the ABI-default state into the live registers (used on `execve` and
/// before entering a signal handler).
pub fn reset_current() {
    FxSaveArea::new_default().restore();
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::{align_of, size_of};

    #[test]
    fn area_is_512_bytes_and_16_aligned() {
        assert_eq!(size_of::<FxSaveArea>(), 512);
        assert_eq!(align_of::<FxSaveArea>(), 16);
    }

    #[test]
    fn default_image_matches_abi_initial_state() {
        let area = FxSaveArea::new_default();
        assert_eq!(area.fcw(), 0x037F);
        assert_eq!(area.mxcsr(), 0x1F80);
        // FSW, abridged FTW, and every data register start out zero.
        assert!(area.bytes[2..24].iter().all(|b| *b == 0));
        assert!(area.bytes[28..].iter().all(|b| *b == 0));
    }

    #[test]
    fn sanitize_clears_reserved_mxcsr_bits() {
        let mut area = FxSaveArea::new_default();
        area.set_mxcsr(0xFFFF_7F80 | (1 << 6));
        area.sanitize_with_mask(FALLBACK_MXCSR_MASK);
        // Reserved high bits and the unsupported DAZ bit are gone; rounding
        // control and exception masks survive.
        assert_eq!(area.mxcsr(), 0x7F80);
    }

    #[test]
    fn sanitize_keeps_daz_when_cpu_supports_it() {
        let mut area = FxSaveArea::new_default();
        area.set_mxcsr(0x1F80 | (1 << 6));
        area.sanitize_with_mask(0xFFFF);
        assert_eq!(area.mxcsr(), 0x1FC0);
    }

    #[test]
    fn zero_reported_mask_uses_fallback() {
        assert_eq!(effective_mxcsr_mask(0), FALLBACK_MXCSR_MASK);
        assert_eq!(effective_mxcsr_mask(0xFFFF), 0xFFFF);
    }
}
