# Framebuffer Bring-Up

This document describes the framebuffer initialization path currently wired into the Rust kernel.

**Where It Lives**
- `kernel/src/framebuffer.rs`
- `kernel/src/mouse.rs`
- `kernel/src/usb.rs`
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
- `0xFFFF_FFFF_8800_0000` (size: `16 MiB`)

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

`kernel/src/fb_console.rs` exposes:
- `init()` - initialize a styled framebuffer terminal with an 8x16 bitmap text grid, a top `PeetOS` desktop header, and a `Terminal` title bar
- `write_bytes(bytes)` - mirror console/serial output into the framebuffer text grid
- `tick()` - advance cursor blink timing and refresh cursor visibility
- `set_theme(preset)` - switch between built-in terminal theme presets (`Midnight`, `Amber`, `Green`, `Solarized`)

`kernel/src/mouse.rs` exposes:
- `init()` - initialize PS/2 mouse streaming and render the initial software cursor when framebuffer is available
- `begin_framebuffer_update()` / `end_framebuffer_update()` - hide/redraw cursor around framebuffer console updates
- `tick()` - apply queued mouse deltas and move the software cursor
- `set_relative_enabled(enabled)` - enable/disable relative PS/2 motion application
- `set_absolute_position(raw_x, raw_y, min_x, max_x, min_y, max_y, buttons)` - map absolute coordinates into cursor space and redraw

`kernel/src/usb.rs` exposes:
- `init(pci_devices)` - discover and initialize UHCI + HID-tablet path if present
- `poll()` - fetch interrupt-IN tablet reports and update cursor absolute position
This USB path is feature-gated behind `KERNEL_FEATURES=usb-tablet`.

Color values are treated as 32-bit packed pixels (current baseline assumes 32-bpp layout from VBE mode).

Limitations / Simplifications
- Only bootloader-provided VESA mode is supported; kernel-side mode discovery/switching is not implemented.
- Pixel format is treated as packed 32-bpp without decoding VBE channel masks.
- Text rendering is still baseline: fixed 8x16 VGA-compatible bitmap glyphs, static panel chrome, a top desktop header, a `Terminal` title bar with cwd path rendering, a blinking underline cursor, and partial ANSI CSI support (`H`/`f`, `A`/`B`/`C`/`D`, `J`, `K`, `m`) with SGR limited to 16-color fg/bg + bold/default resets.
- Mouse cursor rendering is software-only over the same framebuffer; it relies on save/restore and can still show artifacts if non-console code draws directly into cursor-covered pixels without using cursor hide/show hooks.
- USB absolute cursor mapping currently assumes a single directly attached HID tablet on UHCI and maps reported logical X/Y ranges linearly to cursor coordinates.
- Full VT terminal semantics are not implemented (no scrollback buffer, no DEC private modes, and no 256-color/truecolor SGR).
- Title-bar cwd uses bounded in-kernel traversal from the active task inode and is polled periodically rather than event-driven, so updates are near-real-time but not instantaneous on every internal cwd mutation.
- No double buffering, dirty-region tracking, or acceleration path exists.
- Mapping is boot-time fixed-size (`16 MiB`) and not dynamically expanded/remapped.
- Kernel bring-up does not clear the full framebuffer during early boot; only probe/logging is done at init.
