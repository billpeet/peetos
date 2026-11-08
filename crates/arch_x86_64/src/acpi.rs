//! Minimal ACPI table parsing for SMP/APIC discovery.
//!
//! Everything here works on byte slices so it can be unit tested on the host.
//! The kernel is responsible for locating physical memory and handing the
//! relevant ranges to these helpers.

pub const RSDP_SIGNATURE: &[u8; 8] = b"RSD PTR ";
pub const MADT_SIGNATURE: &[u8; 4] = b"APIC";
pub const RSDT_SIGNATURE: &[u8; 4] = b"RSDT";
pub const XSDT_SIGNATURE: &[u8; 4] = b"XSDT";

pub const SDT_HEADER_LEN: usize = 36;
const RSDP_V1_LEN: usize = 20;
const RSDP_V2_LEN: usize = 36;
const MADT_ENTRIES_OFFSET: usize = SDT_HEADER_LEN + 8;

pub const MAX_CPUS: usize = 16;
pub const MAX_IOAPICS: usize = 4;
pub const MAX_OVERRIDES: usize = 16;

const MADT_TYPE_LOCAL_APIC: u8 = 0;
const MADT_TYPE_IOAPIC: u8 = 1;
const MADT_TYPE_ISO: u8 = 2;
const MADT_TYPE_LAPIC_ADDR_OVERRIDE: u8 = 5;
const MADT_TYPE_LOCAL_X2APIC: u8 = 9;

const LAPIC_FLAG_ENABLED: u32 = 1 << 0;
const LAPIC_FLAG_ONLINE_CAPABLE: u32 = 1 << 1;
const MADT_FLAG_PCAT_COMPAT: u32 = 1 << 0;

/// Sum of all bytes must be zero (mod 256) for a valid ACPI structure.
pub fn checksum_ok(bytes: &[u8]) -> bool {
    bytes.iter().fold(0u8, |acc, b| acc.wrapping_add(*b)) == 0
}

fn read_u16(bytes: &[u8], off: usize) -> Option<u16> {
    let b = bytes.get(off..off + 2)?;
    Some(u16::from_le_bytes([b[0], b[1]]))
}

fn read_u32(bytes: &[u8], off: usize) -> Option<u32> {
    let b = bytes.get(off..off + 4)?;
    Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn read_u64(bytes: &[u8], off: usize) -> Option<u64> {
    let b = bytes.get(off..off + 8)?;
    let mut raw = [0u8; 8];
    raw.copy_from_slice(b);
    Some(u64::from_le_bytes(raw))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rsdp {
    pub revision: u8,
    pub rsdt_addr: u32,
    /// Zero for ACPI 1.0 RSDPs.
    pub xsdt_addr: u64,
}

impl Rsdp {
    /// Physical address of the root table and whether it is an XSDT.
    pub fn root_table(&self) -> (u64, bool) {
        if self.revision >= 2 && self.xsdt_addr != 0 {
            (self.xsdt_addr, true)
        } else {
            (self.rsdt_addr as u64, false)
        }
    }
}

/// Parse an RSDP that starts at the beginning of `bytes`.
pub fn parse_rsdp(bytes: &[u8]) -> Option<Rsdp> {
    if bytes.get(0..8)? != RSDP_SIGNATURE {
        return None;
    }
    if !checksum_ok(bytes.get(0..RSDP_V1_LEN)?) {
        return None;
    }
    let revision = bytes[15];
    let rsdt_addr = read_u32(bytes, 16)?;
    let mut xsdt_addr = 0;
    if revision >= 2 {
        let length = read_u32(bytes, 20)? as usize;
        if length >= RSDP_V2_LEN {
            if let Some(ext) = bytes.get(0..length) {
                if checksum_ok(ext) {
                    xsdt_addr = read_u64(bytes, 24)?;
                }
            }
        }
    }
    Some(Rsdp {
        revision,
        rsdt_addr,
        xsdt_addr,
    })
}

/// Scan `region` on 16-byte boundaries for a valid RSDP.
/// Returns the byte offset of the RSDP within `region`.
pub fn find_rsdp(region: &[u8]) -> Option<usize> {
    let mut off = 0;
    while off + RSDP_V1_LEN <= region.len() {
        if parse_rsdp(&region[off..]).is_some() {
            return Some(off);
        }
        off += 16;
    }
    None
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SdtHeader {
    pub signature: [u8; 4],
    pub length: u32,
}

pub fn parse_sdt_header(bytes: &[u8]) -> Option<SdtHeader> {
    let sig = bytes.get(0..4)?;
    let length = read_u32(bytes, 4)?;
    let mut signature = [0u8; 4];
    signature.copy_from_slice(sig);
    Some(SdtHeader { signature, length })
}

/// Validate a full SDT: header length fits in `bytes` and the checksum holds.
pub fn validate_sdt<'a>(bytes: &'a [u8], expected: &[u8; 4]) -> Option<&'a [u8]> {
    let header = parse_sdt_header(bytes)?;
    if &header.signature != expected {
        return None;
    }
    let len = header.length as usize;
    if len < SDT_HEADER_LEN {
        return None;
    }
    let table = bytes.get(0..len)?;
    if !checksum_ok(table) {
        return None;
    }
    Some(table)
}

/// Number of entries in an RSDT (4-byte pointers) or XSDT (8-byte pointers).
pub fn root_entry_count(table: &[u8], is_xsdt: bool) -> usize {
    let stride = if is_xsdt { 8 } else { 4 };
    table.len().saturating_sub(SDT_HEADER_LEN) / stride
}

/// Physical address stored in root table entry `index`.
pub fn root_entry(table: &[u8], is_xsdt: bool, index: usize) -> Option<u64> {
    if is_xsdt {
        read_u64(table, SDT_HEADER_LEN + index * 8)
    } else {
        read_u32(table, SDT_HEADER_LEN + index * 4).map(|v| v as u64)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CpuEntry {
    pub acpi_id: u32,
    pub apic_id: u32,
}

impl CpuEntry {
    const EMPTY: Self = Self {
        acpi_id: 0,
        apic_id: 0,
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IoApicEntry {
    pub id: u8,
    pub address: u32,
    pub gsi_base: u32,
}

impl IoApicEntry {
    const EMPTY: Self = Self {
        id: 0,
        address: 0,
        gsi_base: 0,
    };
}

/// MPS INTI polarity/trigger flags from an interrupt source override.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InterruptOverride {
    pub source_irq: u8,
    pub gsi: u32,
    pub flags: u16,
}

impl InterruptOverride {
    const EMPTY: Self = Self {
        source_irq: 0,
        gsi: 0,
        flags: 0,
    };
}

#[derive(Clone, Copy, Debug)]
pub struct MadtInfo {
    pub local_apic_addr: u64,
    pub pcat_compat: bool,
    pub cpus: [CpuEntry; MAX_CPUS],
    pub cpu_count: usize,
    /// CPUs present in the table but dropped because `MAX_CPUS` was reached.
    pub cpus_skipped: usize,
    pub ioapics: [IoApicEntry; MAX_IOAPICS],
    pub ioapic_count: usize,
    pub overrides: [InterruptOverride; MAX_OVERRIDES],
    pub override_count: usize,
}

impl MadtInfo {
    pub const fn empty() -> Self {
        Self {
            local_apic_addr: 0,
            pcat_compat: false,
            cpus: [CpuEntry::EMPTY; MAX_CPUS],
            cpu_count: 0,
            cpus_skipped: 0,
            ioapics: [IoApicEntry::EMPTY; MAX_IOAPICS],
            ioapic_count: 0,
            overrides: [InterruptOverride::EMPTY; MAX_OVERRIDES],
            override_count: 0,
        }
    }

    pub fn cpus(&self) -> &[CpuEntry] {
        &self.cpus[..self.cpu_count]
    }

    pub fn ioapics(&self) -> &[IoApicEntry] {
        &self.ioapics[..self.ioapic_count]
    }

    pub fn overrides(&self) -> &[InterruptOverride] {
        &self.overrides[..self.override_count]
    }

    fn push_cpu(&mut self, cpu: CpuEntry) {
        if self.cpus().iter().any(|c| c.apic_id == cpu.apic_id) {
            return;
        }
        if self.cpu_count < MAX_CPUS {
            self.cpus[self.cpu_count] = cpu;
            self.cpu_count += 1;
        } else {
            self.cpus_skipped += 1;
        }
    }
}

/// Parse a validated MADT (`validate_sdt(.., MADT_SIGNATURE)`).
pub fn parse_madt(table: &[u8]) -> Option<MadtInfo> {
    if table.len() < MADT_ENTRIES_OFFSET {
        return None;
    }
    let mut info = MadtInfo::empty();
    info.local_apic_addr = read_u32(table, SDT_HEADER_LEN)? as u64;
    info.pcat_compat = read_u32(table, SDT_HEADER_LEN + 4)? & MADT_FLAG_PCAT_COMPAT != 0;

    let mut off = MADT_ENTRIES_OFFSET;
    while off + 2 <= table.len() {
        let kind = table[off];
        let len = table[off + 1] as usize;
        if len < 2 || off + len > table.len() {
            break;
        }
        let entry = &table[off..off + len];
        match kind {
            MADT_TYPE_LOCAL_APIC if len >= 8 => {
                let flags = read_u32(entry, 4)?;
                if flags & (LAPIC_FLAG_ENABLED | LAPIC_FLAG_ONLINE_CAPABLE) != 0 {
                    info.push_cpu(CpuEntry {
                        acpi_id: entry[2] as u32,
                        apic_id: entry[3] as u32,
                    });
                }
            }
            MADT_TYPE_LOCAL_X2APIC if len >= 16 => {
                let flags = read_u32(entry, 8)?;
                if flags & (LAPIC_FLAG_ENABLED | LAPIC_FLAG_ONLINE_CAPABLE) != 0 {
                    info.push_cpu(CpuEntry {
                        acpi_id: read_u32(entry, 12)?,
                        apic_id: read_u32(entry, 4)?,
                    });
                }
            }
            MADT_TYPE_IOAPIC if len >= 12 => {
                if info.ioapic_count < MAX_IOAPICS {
                    info.ioapics[info.ioapic_count] = IoApicEntry {
                        id: entry[2],
                        address: read_u32(entry, 4)?,
                        gsi_base: read_u32(entry, 8)?,
                    };
                    info.ioapic_count += 1;
                }
            }
            MADT_TYPE_ISO if len >= 10 => {
                // Bus 0 (ISA) is the only bus defined for overrides.
                if entry[2] == 0 && info.override_count < MAX_OVERRIDES {
                    info.overrides[info.override_count] = InterruptOverride {
                        source_irq: entry[3],
                        gsi: read_u32(entry, 4)?,
                        flags: read_u16(entry, 8)?,
                    };
                    info.override_count += 1;
                }
            }
            MADT_TYPE_LAPIC_ADDR_OVERRIDE if len >= 12 => {
                info.local_apic_addr = read_u64(entry, 4)?;
            }
            _ => {}
        }
        off += len;
    }
    Some(info)
}

/// Resolved routing for one legacy ISA IRQ.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IsaRoute {
    pub gsi: u32,
    pub active_low: bool,
    pub level_triggered: bool,
}

/// Map an ISA IRQ to a GSI using MADT interrupt source overrides.
/// ISA defaults are identity-mapped, edge-triggered, active-high.
pub fn isa_route(overrides: &[InterruptOverride], irq: u8) -> IsaRoute {
    let mut route = IsaRoute {
        gsi: irq as u32,
        active_low: false,
        level_triggered: false,
    };
    if let Some(iso) = overrides.iter().find(|o| o.source_irq == irq) {
        route.gsi = iso.gsi;
        // Polarity bits 1:0 -> 01 active high, 11 active low, 00 bus default.
        route.active_low = iso.flags & 0b11 == 0b11;
        // Trigger bits 3:2 -> 01 edge, 11 level, 00 bus default.
        route.level_triggered = (iso.flags >> 2) & 0b11 == 0b11;
    }
    route
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    fn fix_checksum(bytes: &mut [u8], at: usize) {
        bytes[at] = 0;
        let sum = bytes.iter().fold(0u8, |acc, b| acc.wrapping_add(*b));
        bytes[at] = 0u8.wrapping_sub(sum);
    }

    fn rsdp_v1(rsdt: u32) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(RSDP_SIGNATURE);
        b.push(0); // checksum
        b.extend_from_slice(b"BOCHS ");
        b.push(0); // revision
        b.extend_from_slice(&rsdt.to_le_bytes());
        fix_checksum(&mut b, 8);
        b
    }

    fn rsdp_v2(rsdt: u32, xsdt: u64) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(RSDP_SIGNATURE);
        b.push(0);
        b.extend_from_slice(b"PEETOS");
        b.push(2);
        b.extend_from_slice(&rsdt.to_le_bytes());
        b.extend_from_slice(&36u32.to_le_bytes());
        b.extend_from_slice(&xsdt.to_le_bytes());
        b.push(0); // extended checksum
        b.extend_from_slice(&[0, 0, 0]);
        fix_checksum(&mut b[..20], 8);
        fix_checksum(&mut b, 32);
        b
    }

    fn sdt(signature: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(signature);
        b.extend_from_slice(&((SDT_HEADER_LEN + body.len()) as u32).to_le_bytes());
        b.push(1); // revision
        b.push(0); // checksum
        b.extend_from_slice(b"PEETOS");
        b.extend_from_slice(b"PEETTEST");
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        assert_eq!(b.len(), SDT_HEADER_LEN);
        b.extend_from_slice(body);
        fix_checksum(&mut b, 9);
        b
    }

    /// MADT resembling what QEMU's i440fx machine emits with `-smp 2`.
    fn qemu_like_madt() -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&0xFEE0_0000u32.to_le_bytes());
        body.extend_from_slice(&MADT_FLAG_PCAT_COMPAT.to_le_bytes());
        // Local APIC: acpi 0, apic 0, enabled.
        body.extend_from_slice(&[0, 8, 0, 0]);
        body.extend_from_slice(&1u32.to_le_bytes());
        // Local APIC: acpi 1, apic 1, enabled.
        body.extend_from_slice(&[0, 8, 1, 1]);
        body.extend_from_slice(&1u32.to_le_bytes());
        // Local APIC: acpi 2, apic 2, disabled and not online-capable.
        body.extend_from_slice(&[0, 8, 2, 2]);
        body.extend_from_slice(&0u32.to_le_bytes());
        // IOAPIC id 0 at 0xFEC00000, GSI base 0.
        body.extend_from_slice(&[1, 12, 0, 0]);
        body.extend_from_slice(&0xFEC0_0000u32.to_le_bytes());
        body.extend_from_slice(&0u32.to_le_bytes());
        // ISO: ISA IRQ0 -> GSI2, flags 0 (conforming).
        body.extend_from_slice(&[2, 10, 0, 0]);
        body.extend_from_slice(&2u32.to_le_bytes());
        body.extend_from_slice(&0u16.to_le_bytes());
        // ISO: ISA IRQ11 -> GSI11, level / active high (0x000D).
        body.extend_from_slice(&[2, 10, 0, 11]);
        body.extend_from_slice(&11u32.to_le_bytes());
        body.extend_from_slice(&0x000Du16.to_le_bytes());
        // Local APIC NMI (type 4) - ignored.
        body.extend_from_slice(&[4, 6, 0xFF, 0, 0, 1]);
        sdt(MADT_SIGNATURE, &body)
    }

    #[test]
    fn checksum_detects_corruption() {
        let mut b = rsdp_v1(0x1234);
        assert!(checksum_ok(&b));
        b[16] ^= 1;
        assert!(!checksum_ok(&b));
    }

    #[test]
    fn parse_rsdp_v1_uses_rsdt() {
        let b = rsdp_v1(0x07FE_1234);
        let rsdp = parse_rsdp(&b).unwrap();
        assert_eq!(rsdp.revision, 0);
        assert_eq!(rsdp.root_table(), (0x07FE_1234, false));
    }

    #[test]
    fn parse_rsdp_v2_prefers_xsdt() {
        let b = rsdp_v2(0x1000, 0x1_0000_2000);
        let rsdp = parse_rsdp(&b).unwrap();
        assert_eq!(rsdp.root_table(), (0x1_0000_2000, true));
    }

    #[test]
    fn parse_rsdp_v2_bad_extended_checksum_falls_back_to_rsdt() {
        let mut b = rsdp_v2(0x1000, 0x2000);
        b[32] ^= 0x55;
        let rsdp = parse_rsdp(&b).unwrap();
        assert_eq!(rsdp.root_table(), (0x1000, false));
    }

    #[test]
    fn parse_rsdp_rejects_bad_signature_or_checksum() {
        let mut b = rsdp_v1(0x1000);
        b[0] = b'X';
        assert!(parse_rsdp(&b).is_none());
        let mut b = rsdp_v1(0x1000);
        b[8] ^= 1;
        assert!(parse_rsdp(&b).is_none());
    }

    #[test]
    fn find_rsdp_scans_on_16_byte_boundaries() {
        let mut region = std::vec![0u8; 256];
        let rsdp = rsdp_v1(0xABCD);
        region[0x40..0x40 + rsdp.len()].copy_from_slice(&rsdp);
        assert_eq!(find_rsdp(&region), Some(0x40));

        // Misaligned copies are not found.
        let mut region = std::vec![0u8; 256];
        region[0x41..0x41 + rsdp.len()].copy_from_slice(&rsdp);
        assert_eq!(find_rsdp(&region), None);
    }

    #[test]
    fn root_table_entries_rsdt_and_xsdt() {
        let mut body = Vec::new();
        body.extend_from_slice(&0x1000u32.to_le_bytes());
        body.extend_from_slice(&0x2000u32.to_le_bytes());
        let rsdt = sdt(RSDT_SIGNATURE, &body);
        let table = validate_sdt(&rsdt, RSDT_SIGNATURE).unwrap();
        assert_eq!(root_entry_count(table, false), 2);
        assert_eq!(root_entry(table, false, 1), Some(0x2000));

        let mut body = Vec::new();
        body.extend_from_slice(&0x1_0000_0000u64.to_le_bytes());
        let xsdt = sdt(XSDT_SIGNATURE, &body);
        let table = validate_sdt(&xsdt, XSDT_SIGNATURE).unwrap();
        assert_eq!(root_entry_count(table, true), 1);
        assert_eq!(root_entry(table, true, 0), Some(0x1_0000_0000));
    }

    #[test]
    fn validate_sdt_rejects_wrong_signature_truncation_and_checksum() {
        let madt = qemu_like_madt();
        assert!(validate_sdt(&madt, RSDT_SIGNATURE).is_none());
        assert!(validate_sdt(&madt[..madt.len() - 1], MADT_SIGNATURE).is_none());
        let mut bad = madt.clone();
        bad[40] ^= 1;
        assert!(validate_sdt(&bad, MADT_SIGNATURE).is_none());
        assert!(validate_sdt(&madt, MADT_SIGNATURE).is_some());
    }

    #[test]
    fn parse_madt_collects_cpus_ioapics_and_overrides() {
        let madt = qemu_like_madt();
        let table = validate_sdt(&madt, MADT_SIGNATURE).unwrap();
        let info = parse_madt(table).unwrap();
        assert_eq!(info.local_apic_addr, 0xFEE0_0000);
        assert!(info.pcat_compat);
        assert_eq!(
            info.cpus(),
            &[
                CpuEntry {
                    acpi_id: 0,
                    apic_id: 0
                },
                CpuEntry {
                    acpi_id: 1,
                    apic_id: 1
                }
            ]
        );
        assert_eq!(
            info.ioapics(),
            &[IoApicEntry {
                id: 0,
                address: 0xFEC0_0000,
                gsi_base: 0
            }]
        );
        assert_eq!(info.overrides().len(), 2);
        assert_eq!(info.overrides()[0].gsi, 2);
    }

    #[test]
    fn parse_madt_stops_on_malformed_entry_length() {
        let mut body = Vec::new();
        body.extend_from_slice(&0xFEE0_0000u32.to_le_bytes());
        body.extend_from_slice(&0u32.to_le_bytes());
        body.extend_from_slice(&[0, 8, 0, 0]);
        body.extend_from_slice(&1u32.to_le_bytes());
        // Zero-length entry would loop forever if not rejected.
        body.extend_from_slice(&[0, 0, 1, 1, 1, 0, 0, 0]);
        let madt = sdt(MADT_SIGNATURE, &body);
        let info = parse_madt(validate_sdt(&madt, MADT_SIGNATURE).unwrap()).unwrap();
        assert_eq!(info.cpu_count, 1);
    }

    #[test]
    fn parse_madt_caps_cpu_count_and_dedups() {
        let mut body = Vec::new();
        body.extend_from_slice(&0xFEE0_0000u32.to_le_bytes());
        body.extend_from_slice(&0u32.to_le_bytes());
        for id in 0..(MAX_CPUS as u8 + 3) {
            body.extend_from_slice(&[0, 8, id, id]);
            body.extend_from_slice(&1u32.to_le_bytes());
        }
        // Duplicate APIC id 0.
        body.extend_from_slice(&[0, 8, 0x40, 0]);
        body.extend_from_slice(&1u32.to_le_bytes());
        let madt = sdt(MADT_SIGNATURE, &body);
        let info = parse_madt(validate_sdt(&madt, MADT_SIGNATURE).unwrap()).unwrap();
        assert_eq!(info.cpu_count, MAX_CPUS);
        assert_eq!(info.cpus_skipped, 3);
    }

    #[test]
    fn parse_madt_handles_x2apic_and_lapic_address_override() {
        let mut body = Vec::new();
        body.extend_from_slice(&0xFEE0_0000u32.to_le_bytes());
        body.extend_from_slice(&0u32.to_le_bytes());
        // x2APIC entry: reserved(2), apic id 300, flags 1, acpi uid 7.
        body.extend_from_slice(&[9, 16, 0, 0]);
        body.extend_from_slice(&300u32.to_le_bytes());
        body.extend_from_slice(&1u32.to_le_bytes());
        body.extend_from_slice(&7u32.to_le_bytes());
        // LAPIC address override.
        body.extend_from_slice(&[5, 12, 0, 0]);
        body.extend_from_slice(&0x1_FEE0_0000u64.to_le_bytes());
        let madt = sdt(MADT_SIGNATURE, &body);
        let info = parse_madt(validate_sdt(&madt, MADT_SIGNATURE).unwrap()).unwrap();
        assert_eq!(
            info.cpus(),
            &[CpuEntry {
                acpi_id: 7,
                apic_id: 300
            }]
        );
        assert_eq!(info.local_apic_addr, 0x1_FEE0_0000);
    }

    #[test]
    fn isa_route_applies_overrides_and_defaults() {
        let madt = qemu_like_madt();
        let info = parse_madt(validate_sdt(&madt, MADT_SIGNATURE).unwrap()).unwrap();
        let overrides = info.overrides();

        assert_eq!(
            isa_route(overrides, 0),
            IsaRoute {
                gsi: 2,
                active_low: false,
                level_triggered: false
            }
        );
        assert_eq!(
            isa_route(overrides, 1),
            IsaRoute {
                gsi: 1,
                active_low: false,
                level_triggered: false
            }
        );
        assert_eq!(
            isa_route(overrides, 11),
            IsaRoute {
                gsi: 11,
                active_low: false,
                level_triggered: true
            }
        );

        let low = [InterruptOverride {
            source_irq: 9,
            gsi: 20,
            flags: 0b1111,
        }];
        assert_eq!(
            isa_route(&low, 9),
            IsaRoute {
                gsi: 20,
                active_low: true,
                level_triggered: true
            }
        );
    }
}
