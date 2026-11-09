# Device Drivers

This document covers current driver work: PCI enumeration, ATA/AHCI bring-up through AHCI command-engine `IDENTIFY` and LBA48 DMA read/write commands, block-device API integration, keyboard/mouse input wiring, UHCI USB HID-tablet polling, and framebuffer setup.

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
- a bounded page-sized write-back cache for block I/O (`block::write_cached`, `block::read_cached`, `block::flush_cached_device`). The uncached `block::read`/`block::write` stay coherent with it: a raw read first writes back dirty lines it overlaps, and a raw write writes back and drops them. Cached I/O of a different size does the same for overlapping lines. (The ext2 backend writes through the cache but reads raw; before this, a dirty metadata block that ext2 evicted and reloaded came back stale and runtime-created inodes reverted to mode 0.)
- an optional per-device `BlockOps.flush` (AHCI: ATA `FLUSH CACHE EXT`, issued as a non-data command) and `block::flush_device(index)`, a durability barrier that writes back the device's dirty cache lines and then flushes the drive's volatile write cache. The ext3 journal uses it to order log, commit, and checkpoint writes, and `sync`/`fsync`/`msync` use it so data reaches stable media before returning.

The registry records discovered disks from both paths:
- transport (`ata-pio` or `ahci-dma`)
- controller BDF + target (IDE drive slot or AHCI port)
- logical block size + total sectors + LBA48 capability

Boot logs include per-device registration lines, an aggregated `[BLK] Inventory` summary, and a best-effort AHCI LBA0 read probe plus a `flush_device` barrier probe (`[BLK] devN flush ok`) when AHCI devices are present; `scripts/run_toybox_proc_smoke.sh` asserts the flush line.

**Keyboard + TTY Input Baseline**
- Keyboard IRQ1 is registered in the IDT and unmasked on the PIC during bring-up.
- PS/2 set-1 scancodes are decoded with basic modifier handling (left/right shift, caps lock, printable keys, backspace, enter).
- The same decode path now also feeds `/dev/input/event0` with evdev-style
  `EV_KEY` records plus `SYN_REPORT`.
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
- Decoded pointer activity feeds the evdev devices:
  - PS/2 packets emit `EV_REL` (`REL_X`, `REL_Y`) plus `EV_KEY` button changes, using the Linux evdev convention (`REL_Y` positive = down; PS/2 reports positive = up, so the driver flips it)
  - USB tablet absolute updates emit `EV_ABS` (`ABS_X`, `ABS_Y`, screen pixels) plus button changes
  - each packet/report ends with `SYN_REPORT`
  - queue overflow is reported with `SYN_DROPPED` on the next successfully queued packet
- Devices (`kernel/src/input.rs`), each a character device 13:64+N:
  - `event0`: the keyboard ("AT Translated Set 2 keyboard")
  - `event1`: a legacy aggregate of both pointers (relative and absolute events on one device), read by `peetwl` and the older compositors; like Linux's `/dev/input/mice` it is not listed in `/sys`
  - `event2`: the PS/2 mouse alone ("PS/2 Generic Mouse")
  - `event3`: the USB tablet alone ("QEMU QEMU USB Tablet")
- evdev ioctls for libevdev/libinput: `EVIOCGVERSION`, `EVIOCGID`, `EVIOCGNAME`/`PHYS` (`UNIQ` gives `ENOENT`), `EVIOCGPROP`, `EVIOCGBIT` for every event type, `EVIOCGKEY`/`LED`/`SND`/`SW` (all released), `EVIOCGABS` (range 0 to the cursor's maximum position), and `EVIOCGRAB`, `EVIOCREVOKE` and `EVIOCSCLOCKID` accepted. The same capability bitmaps are published in `/sys` (`uevent` `EV=`/`KEY=`/..., `capabilities/*`) for libudev.
- Motion deltas are accumulated in IRQ context and applied in the main loop to a software framebuffer cursor.
- Cursor rendering uses save/restore of covered pixels and clamps movement to screen bounds.
- Framebuffer console writes/ticks temporarily hide the cursor and redraw it after updates so text rendering and cursor overlay stay coherent.
- `KERNEL_FEATURES=compositor-first` disables kernel cursor composition and framebuffer-console updates so userspace can own the display without kernel redraw interference; fatal panic/exception paths, init-launch failure, and tracked compositor/init exit reactivate the kernel console fallback.

**USB UHCI Tablet Baseline**
- Feature-gated behind `KERNEL_FEATURES=usb-tablet` so default builds stay minimal and the optional UHCI path remains opt-in.
- UHCI host-controller discovery is wired from PCI class/subclass/prog-if (`0x0C:0x03:0x00`) and currently uses I/O BAR programming.
- The kernel initializes a minimal UHCI schedule (frame list + queue head + TD pool), performs root-port reset/enable, and issues control transfers for device/config/report descriptors.
- HID configuration parsing selects an interrupt-IN endpoint and a report descriptor parser extracts absolute X/Y + button fields when possible.
- On successful enumeration, USB interrupt-IN polling drives `mouse::set_absolute_position(...)` for absolute framebuffer cursor updates.
- Relative PS/2 cursor motion is disabled while USB tablet mode is active to avoid mixed-input drift.

**Userspace Input Device ABI**
- `devfs` now exposes:
  - `/dev/input/event0` for keyboard
  - `/dev/input/event1` for pointer
- Records use a fixed 24-byte layout:
  - `tv_sec: i64`
  - `tv_usec: i64`
  - `type: u16`
  - `code: u16`
  - `value: i32`
- Timestamps are sourced from monotonic PIT uptime.
- `read` is packet-preserving at event-record granularity, nonblocking empty reads return `-EAGAIN`, and readiness is wired through `poll`/`select`/`epoll` as `POLLIN`.
- The dedicated compositor-first smoke profile now launches a userspace compositor process that consumes these devices while compositing multiple client surfaces to `/dev/fb0`; clients attach shared-memory buffers with `memfd_create` + `SCM_RIGHTS`, giving the current Phase 4 compositor slice a direct framebuffer/input regression path.

**Framebuffer Baseline**
- Bootloader-provided VESA handoff structures are consumed from fixed higher-half addresses:
  - status block: `0xFFFF_FFFF_8000_0B00`
  - mode info block: `0xFFFF_FFFF_8000_0A00`
- The kernel validates mode geometry and bpp (`32` for current baseline) and computes framebuffer virtual address inside the pre-mapped boot window at `0xFFFF_FFFF_8800_0000`.
- The active window size is treated as `16 MiB`; initialization rejects modes whose offset+size exceeds that window.
- Baseline drawing primitives are provided in-kernel:
  - pixel write (`set_pixel`)
  - full clear (`clear`)
- Boot logs now include framebuffer activation/fallback status over serial.

Limitations / Simplifications
- AHCI command execution is polling-based; interrupts and async completion are not wired.
- Barriers are full drains (all dirty lines of the device plus a device flush); there is no FUA or tagged ordering, so each journal commit costs two device flushes.
- Block-cache coherence for uncached and mixed-size I/O is a linear scan of the 64 lines per request, and the cache is one global array with no lock of its own (it relies on the callers' serialisation).
- AHCI DMA transfer size is currently capped to one page (`4 KiB`) per command through a bounce buffer.
- ATA (`ata-pio`) devices are inventory-only for now; read/write callbacks are currently wired for AHCI devices.
- VFS now mounts ext2 through a native block backend with sparse/lazy block residency and dirty-run sync-back; it no longer preloads a full image into RAM at mount.
- No partition parsing (MBR/GPT), and full VM/fs page-cache unification is still incomplete (notably writable shared-mmap writeback semantics).
- MMIO window mapping is currently bounded (`AHCI_MMIO_WINDOW_LIMIT`) and boot-time only.
- Hotplug, power management, and controller reset/error-recovery flows are minimal.
- Keyboard input is US-layout set-1 only; extended keys and key-repeat handling are minimal.
- Mouse support is baseline PS/2 packet mode plus a minimal UHCI USB HID-tablet absolute-input path; there is no generic USB hub stack, transfer scheduler, or broad HID-class coverage yet.
- Input-event ABI coverage is intentionally narrow: fixed `event0`..`event3`
  numbering, no hotplug, no multi-seat, and one event queue per device shared
  by every reader (Linux gives each open file its own buffer), so only one
  compositor should read a device at a time. `EVIOCSABS`, force feedback,
  `EV_REP` and multitouch slots are not implemented; `EVIOCGRAB` is accepted
  without excluding other readers.
- Headless GUI smoke coverage currently injects synthetic keyboard/pointer
  events from a smoke-only kernel hook because the CI/QEMU harness does not yet
  provide direct host-input injection.
- The current compositor smoke still routes input only to the compositor itself;
  client-facing input dispatch/focus policy is a later Phase 4 step.
- Cursor composition is software-only over the shared framebuffer; there is no true hardware cursor plane, alpha blending, or z-order compositor yet.
- Display ownership is still coarse-grained: the current model is console-or-compositor with a kernel-internal mode switch, not a runtime userspace lock/lease ABI.
- TTY is canonical-line buffered only; no `termios`, ptys, or virtual terminal switching yet.
- Framebuffer currently assumes packed 32-bpp layout and does not yet decode VBE color masks.
- Framebuffer text rendering is still baseline: serial output is mirrored into a fixed 8x16 bitmap text grid with terminal panel styling, preset themes, a blinking cursor, and partial ANSI/SGR handling (16-color fg/bg + bold/default), but full VT emulation is not implemented.
- No acceleration, double buffering, mode switching, or runtime remap support is implemented.
