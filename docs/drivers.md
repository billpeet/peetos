# Device Drivers (Phase 7 Baseline)

This document covers the first Phase-7 driver milestone: PCI enumeration.

**Where It Lives**
- `crates/drivers/src/pci.rs`
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

Limitations / Simplifications
- Read-only enumeration only; no BAR sizing/probing, capability walking, IRQ routing, or driver binding yet.
- Uses legacy config-space mechanism #1, not ECAM/MMCONFIG.
- Inventory is bounded for log safety; additional devices are counted but not printed once the limit is reached.
- No persistent in-kernel device registry yet; data is currently used for diagnostics and bring-up verification.
