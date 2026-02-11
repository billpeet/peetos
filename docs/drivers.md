# Device Drivers (Phase 7 Baseline)

This document covers the current Phase-7 driver baseline: PCI enumeration plus initial ATA/AHCI discovery.

**Where It Lives**
- `crates/drivers/src/pci.rs`
- `crates/drivers/src/storage.rs`
- `crates/drivers/src/ahci.rs`
- `crates/arch_x86_64/src/port.rs`
- `kernel/src/main.rs`

**PCI Enumeration**
The kernel now performs a boot-time PCI config-space scan using PCI configuration mechanism #1 (`0xCF8`/`0xCFC`). The scan walks all bus/device/function tuples (`256 x 32 x 8`) and records present functions (vendor ID != `0xFFFF`).

Each discovered function captures:
- BDF address (`bus:device.function`)
- `vendor_id` / `device_id`
- `class_code` / `subclass` / `prog_if`
- `header_type`

During `kmain()` bring-up, the scanner runs after heap initialization and prints a bounded serial inventory (`PCI_LOG_LIMIT` entries) plus a summary.

**ATA/AHCI Discovery**
After PCI scan, the driver layer classifies mass-storage controllers:
- IDE controllers (`class=0x01`, `subclass=0x01`) into legacy/native/mixed mode via `prog_if`.
- AHCI controllers (`class=0x01`, `subclass=0x06`, `prog_if=0x01`) with ABAR extraction from BAR5.

Boot logs now print a storage summary and per-controller lines, and AHCI functions are configured with PCI command bits for memory-space and bus-master operation.

The AHCI module currently provides protocol constants plus small decode helpers (`CAP` decode, port-signature mapping, implemented-port bitmap expansion) used as groundwork for full HBA bring-up.

Limitations / Simplifications
- PCI BAR capture currently records address/type only; BAR sizing/probing and capability walking are still pending.
- Uses legacy config-space mechanism #1, not ECAM/MMCONFIG.
- Inventory is bounded for log safety; additional devices are counted but not printed once the limit is reached.
- ATA/AHCI support is discovery/config only right now: no AHCI MMIO mapping, command-engine init, IDENTIFY, or read/write DMA path is implemented yet.
- No persistent in-kernel device registry yet; data is currently used for diagnostics and bring-up verification.
