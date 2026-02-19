# Device Drivers (Phase 7 Progress)

This document covers current Phase-7 driver work: PCI enumeration plus ATA/AHCI bring-up through AHCI command-engine `IDENTIFY`, AHCI LBA48 DMA read/write commands, baseline block-device API integration, keyboard/mouse input wiring, UHCI USB HID-tablet polling, and framebuffer setup.

**Where It Lives**
- `crates/drivers/src/pci.rs`
- `crates/drivers/src/storage.rs`
- `crates/drivers/src/ahci.rs`
- `crates/arch_x86_64/src/port.rs`
- `kernel/src/storage.rs`
- `kernel/src/block.rs`
- `kernel/src/keyboard.rs`
- `kernel/src/mouse.rs`
- `kernel/src/usb.rs`
- `kernel/src/tty.rs`
- `kernel/src/framebuffer.rs`
- `kernel/src/main.rs`

**PCI Enumeration**
The kernel performs a boot-time PCI config-space scan using PCI configuration mechanism #1 (`0xCF8`/`0xCFC`). The scan walks all bus/device/function tuples (`256 x 32 x 8`) and records present functions (`vendor_id != 0xFFFF`).

Each discovered function captures:
- BDF address (`bus:device.function`)
- `vendor_id` / `device_id`
- `class_code` / `subclass` / `prog_if`
- `header_type`
- PCI command/status, IRQ line/pin, and decoded BAR metadata

During `kmain()` bring-up, the scanner runs after heap initialization and prints a bounded serial inventory (`PCI_LOG_LIMIT` entries) plus a summary.

**ATA/AHCI Discovery and Command Paths**
After PCI scan, storage controllers are classified from class/subclass/prog-if:
- IDE controllers (`class=0x01`, `subclass=0x01`) are decoded as legacy/native/mixed mode from `prog_if`.
- AHCI controllers (`class=0x01`, `subclass=0x06`, `prog_if=0x01`) expose ABAR from BAR5.

The kernel then performs:
- PCI command-bit enable (`MEM_SPACE` + `BUS_MASTER`) for AHCI functions.
- AHCI HBA MMIO mapping into dedicated high-half windows.
- Capability/port probing (`CAP`, `GHC`, `PI`, `VS`, `SSTS`, `SIG`).
- AHCI port command-engine setup for active SATA ports:
  - stop engine
  - program CLB/FB DMA regions
  - start engine
  - issue ATA `IDENTIFY` as a DMA AHCI command (CFIS + PRDT)
  - issue ATA `READ DMA EXT` / `WRITE DMA EXT` commands through block-I/O callbacks
  - poll completion and parse IDENTIFY words
  - stop engine and release temporary DMA buffers
- ATA PIO `IDENTIFY` probes over IDE primary/secondary master/slave channels.

**Block Device Integration**
A minimal kernel block layer now provides:
- device inventory/registration
- per-device read/write callback dispatch (`block::read`, `block::write`)

The registry records discovered disks from both paths:
- transport (`ata-pio` or `ahci-dma`)
- controller BDF + target (IDE drive slot or AHCI port)
- logical block size + total sectors + LBA48 capability

Boot logs include per-device registration lines, an aggregated `[BLK] Inventory` summary, and a best-effort AHCI LBA0 read probe when AHCI devices are present.

**Keyboard + TTY Input Baseline**
- Keyboard IRQ1 is registered in the IDT and unmasked on the PIC during bring-up.
- PS/2 set-1 scancodes are decoded with basic modifier handling (left/right shift, caps lock, printable keys, backspace, enter).
- Decoded bytes are fed into a kernel TTY line buffer:
  - canonical mode only (input is committed on newline)
  - basic backspace editing in the pending line
  - serial echo for typed bytes/newline/backspace
- Console `read` in syscall dispatch now pulls from this TTY queue and blocks while no full line is available.

**Mouse Driver + Movable Cursor Baseline**
- Mouse IRQ12 is registered in the IDT and unmasked on the PIC after PS/2 auxiliary-device setup.
- The driver programs the i8042/PS2 controller for mouse streaming (`0xF6` defaults + `0xF4` enable reporting) and parses 3-byte PS/2 packets.
- Packet decode currently supports:
  - button state bits (left/right/middle)
  - signed relative motion (`dx`/`dy`)
  - overflow rejection and first-byte sync-bit resynchronization
- Motion deltas are accumulated in IRQ context and applied in the main loop to a software framebuffer cursor.
- Cursor rendering uses save/restore of covered pixels and clamps movement to screen bounds.
- Framebuffer console writes/ticks temporarily hide the cursor and redraw it after updates so text rendering and cursor overlay stay coherent.

**USB UHCI Tablet Baseline**
- Feature-gated behind `KERNEL_FEATURES=usb-tablet` so default builds remain within the stage2 staging-size limit.
- UHCI host-controller discovery is wired from PCI class/subclass/prog-if (`0x0C:0x03:0x00`) and currently uses I/O BAR programming.
- The kernel initializes a minimal UHCI schedule (frame list + queue head + TD pool), performs root-port reset/enable, and issues control transfers for device/config/report descriptors.
- HID configuration parsing selects an interrupt-IN endpoint and a report descriptor parser extracts absolute X/Y + button fields when possible.
- On successful enumeration, USB interrupt-IN polling drives `mouse::set_absolute_position(...)` for absolute framebuffer cursor updates.
- Relative PS/2 cursor motion is disabled while USB tablet mode is active to avoid mixed-input drift.

**Framebuffer Baseline**
- Bootloader-provided VESA handoff structures are consumed from fixed higher-half addresses:
  - status block: `0xFFFF_FFFF_8000_0B00`
  - mode info block: `0xFFFF_FFFF_8000_0A00`
- The kernel validates mode geometry and bpp (`32` for current baseline) and computes framebuffer virtual address inside the pre-mapped boot window at `0xFFFF_FFFF_8100_0000`.
- The active window size is treated as `16 MiB`; initialization rejects modes whose offset+size exceeds that window.
- Baseline drawing primitives are provided in-kernel:
  - pixel write (`set_pixel`)
  - full clear (`clear`)
- Boot logs now include framebuffer activation/fallback status over serial.

Limitations / Simplifications
- AHCI command execution is polling-based; interrupts and async completion are not wired.
- AHCI DMA transfer size is currently capped to one page (`4 KiB`) per command through a bounce buffer.
- ATA (`ata-pio`) devices are inventory-only for now; read/write callbacks are currently wired for AHCI devices.
- VFS currently consumes block I/O through an image-style ext2 bridge (read image from block at mount time + best-effort sync-back), not a native block-cache/page-cache-backed ext2 path.
- No partition parsing (MBR/GPT) or block cache/page cache integration yet.
- MMIO window mapping is currently bounded (`AHCI_MMIO_WINDOW_LIMIT`) and boot-time only.
- Hotplug, power management, and controller reset/error-recovery flows are minimal.
- Keyboard input is US-layout set-1 only; extended keys and key-repeat handling are minimal.
- Mouse support is baseline PS/2 packet mode plus a minimal UHCI USB HID-tablet absolute-input path; there is no generic USB hub stack, transfer scheduler, or broad HID-class coverage yet.
- Cursor composition is software-only over the shared framebuffer; there is no true hardware cursor plane, alpha blending, or z-order compositor yet.
- TTY is canonical-line buffered only; no `termios`, ptys, or virtual terminal switching yet.
- Framebuffer currently assumes packed 32-bpp layout and does not yet decode VBE color masks.
- Framebuffer text rendering is still baseline: serial output is mirrored into a fixed 8x16 bitmap text grid with terminal panel styling, preset themes, a blinking cursor, and partial ANSI/SGR handling (16-color fg/bg + bold/default), but full VT emulation is not implemented.
- No acceleration, double buffering, mode switching, or runtime remap support is implemented.
