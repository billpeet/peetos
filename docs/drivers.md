# Device Drivers (Phase 7 Progress)

This document covers current Phase-7 driver work: PCI enumeration plus ATA/AHCI bring-up through AHCI command-engine `IDENTIFY`, AHCI LBA48 DMA read/write commands, and baseline block-device API integration.

**Where It Lives**
- `crates/drivers/src/pci.rs`
- `crates/drivers/src/storage.rs`
- `crates/drivers/src/ahci.rs`
- `crates/arch_x86_64/src/port.rs`
- `kernel/src/storage.rs`
- `kernel/src/block.rs`
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

Limitations / Simplifications
- AHCI command execution is polling-based; interrupts and async completion are not wired.
- AHCI DMA transfer size is currently capped to one page (`4 KiB`) per command through a bounce buffer.
- ATA (`ata-pio`) devices are inventory-only for now; read/write callbacks are currently wired for AHCI devices.
- VFS currently consumes block I/O through an image-style ext2 bridge (read image from block at mount time + best-effort sync-back), not a native block-cache/page-cache-backed ext2 path.
- No partition parsing (MBR/GPT) or block cache/page cache integration yet.
- MMIO window mapping is currently bounded (`AHCI_MMIO_WINDOW_LIMIT`) and boot-time only.
- Hotplug, power management, and controller reset/error-recovery flows are minimal.
