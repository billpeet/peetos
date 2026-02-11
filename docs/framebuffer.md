# Framebuffer Bring-Up

This document describes the phase-7 framebuffer initialization path currently wired into the Rust kernel.

**Where It Lives**
- `kernel/src/framebuffer.rs`
- `kernel/src/main.rs`
- `boot/stage2/vesa.asm`
- `boot/stage2/paging.asm`
- `boot/include/constants.inc`

**Boot Handoff**
The stage2 bootloader attempts to configure VESA mode (`1280x1024x32`) and writes handoff data into fixed low-memory buffers that are mapped into higher-half virtual space:
- VESA status: `0xFFFF_FFFF_8000_0B00`
- VESA mode info: `0xFFFF_FFFF_8000_0A00`

The kernel reads:
- availability flag + selected mode + framebuffer physical address (status block)
- pitch + width + height + bpp (mode info block)

**Kernel Mapping Model**
Boot paging pre-maps a framebuffer window at:
- `0xFFFF_FFFF_8100_0000` (size: `16 MiB`)

The kernel computes the active framebuffer virtual base as:
- `window_base + (phys_addr & 0x1F_FFFF)`

Initialization validates that:
- mode is explicitly marked available (`status.available == 1`)
- mode id is non-zero
- framebuffer physical address is page-aligned and outside low memory bootstrap ranges
- geometry fields are non-zero
- bpp is `32`
- pitch is 4-byte aligned
- pitch is wide enough for `width * 4`
- `(phys_offset + pitch * height)` stays within the 16 MiB window

If validation fails, framebuffer is marked unavailable and kernel continues with serial-only output.

**Current API**
`kernel/src/framebuffer.rs` exposes:
- `init()` - probe handoff + validate mode/mapping
- `info()` - snapshot of current framebuffer state
- `is_available()` - convenience availability check
- `set_pixel(x, y, color)` - single-pixel volatile write
- `clear(color)` - full-screen fill (including pitch padding)

Color values are treated as 32-bit packed pixels (current baseline assumes 32-bpp layout from VBE mode).

Limitations / Simplifications
- Only bootloader-provided VESA mode is supported; kernel-side mode discovery/switching is not implemented.
- Pixel format is treated as packed 32-bpp without decoding VBE channel masks.
- No text renderer, font compositor, or terminal emulation is connected yet.
- No double buffering, dirty-region tracking, or acceleration path exists.
- Mapping is boot-time fixed-size (`16 MiB`) and not dynamically expanded/remapped.
- Kernel bring-up does not clear the full framebuffer during early boot; only probe/logging is done at init.
