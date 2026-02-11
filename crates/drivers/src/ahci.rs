pub const SATA_SIG_ATA: u32 = 0x0000_0101;
pub const SATA_SIG_ATAPI: u32 = 0xEB14_0101;
pub const SATA_SIG_SEMB: u32 = 0xC33C_0101;
pub const SATA_SIG_PM: u32 = 0x9669_0101;

pub const AHCI_REG_CAP: usize = 0x00;
pub const AHCI_REG_GHC: usize = 0x04;
pub const AHCI_REG_IS: usize = 0x08;
pub const AHCI_REG_PI: usize = 0x0C;
pub const AHCI_REG_VS: usize = 0x10;

pub const AHCI_GHC_AE: u32 = 1 << 31;

pub const AHCI_PORT_BASE: usize = 0x100;
pub const AHCI_PORT_STRIDE: usize = 0x80;

pub const AHCI_PORT_CLB: usize = 0x00;
pub const AHCI_PORT_CLBU: usize = 0x04;
pub const AHCI_PORT_FB: usize = 0x08;
pub const AHCI_PORT_FBU: usize = 0x0C;
pub const AHCI_PORT_IS: usize = 0x10;
pub const AHCI_PORT_IE: usize = 0x14;
pub const AHCI_PORT_CMD: usize = 0x18;
pub const AHCI_PORT_TFD: usize = 0x20;
pub const AHCI_PORT_SIG: usize = 0x24;
pub const AHCI_PORT_SSTS: usize = 0x28;
pub const AHCI_PORT_SERR: usize = 0x30;
pub const AHCI_PORT_SACT: usize = 0x34;
pub const AHCI_PORT_CI: usize = 0x38;

pub const AHCI_PORT_CMD_ST: u32 = 1 << 0;
pub const AHCI_PORT_CMD_SUD: u32 = 1 << 1;
pub const AHCI_PORT_CMD_POD: u32 = 1 << 2;
pub const AHCI_PORT_CMD_FRE: u32 = 1 << 4;
pub const AHCI_PORT_CMD_FR: u32 = 1 << 14;
pub const AHCI_PORT_CMD_CR: u32 = 1 << 15;

pub const AHCI_PORT_IS_TFES: u32 = 1 << 30;

pub const AHCI_SSTS_DET_MASK: u32 = 0x0F;
pub const AHCI_SSTS_IPM_MASK: u32 = 0x0F00;
pub const AHCI_SSTS_IPM_SHIFT: u32 = 8;

pub const AHCI_SSTS_DET_PRESENT: u8 = 3;
pub const AHCI_SSTS_IPM_ACTIVE: u8 = 1;

pub const AHCI_FIS_TYPE_REG_H2D: u8 = 0x27;
pub const AHCI_FIS_H2D_COMMAND: u8 = 1 << 7;

pub const AHCI_CMDH_WRITE: u16 = 1 << 6;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum AhciPortDevice {
    None = 0,
    Sata = 1,
    Satapi = 2,
    EnclosureMgmt = 3,
    PortMultiplier = 4,
    Unknown = 255,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AhciCapabilities {
    pub port_count: u8,
    pub command_slot_count: u8,
    pub supports_64bit: bool,
    pub supports_ncq: bool,
    pub supports_staggered_spinup: bool,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct AhciCommandHeader {
    pub flags: u16,
    pub prdtl: u16,
    pub prdbc: u32,
    pub ctba: u32,
    pub ctbau: u32,
    pub reserved: [u32; 4],
}

impl AhciCommandHeader {
    pub const EMPTY: Self = Self {
        flags: 0,
        prdtl: 0,
        prdbc: 0,
        ctba: 0,
        ctbau: 0,
        reserved: [0; 4],
    };
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct AhciPrdtEntry {
    pub dba: u32,
    pub dbau: u32,
    pub reserved: u32,
    pub dbc_interrupt: u32,
}

impl AhciPrdtEntry {
    pub const EMPTY: Self = Self {
        dba: 0,
        dbau: 0,
        reserved: 0,
        dbc_interrupt: 0,
    };
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct AhciCommandTable1 {
    pub cfis: [u8; 64],
    pub acmd: [u8; 16],
    pub reserved: [u8; 48],
    pub prdt: [AhciPrdtEntry; 1],
}

impl AhciCommandTable1 {
    pub const EMPTY: Self = Self {
        cfis: [0; 64],
        acmd: [0; 16],
        reserved: [0; 48],
        prdt: [AhciPrdtEntry::EMPTY; 1],
    };
}

pub const fn classify_port_signature(signature: u32) -> AhciPortDevice {
    match signature {
        SATA_SIG_ATA => AhciPortDevice::Sata,
        SATA_SIG_ATAPI => AhciPortDevice::Satapi,
        SATA_SIG_SEMB => AhciPortDevice::EnclosureMgmt,
        SATA_SIG_PM => AhciPortDevice::PortMultiplier,
        0 => AhciPortDevice::None,
        _ => AhciPortDevice::Unknown,
    }
}

pub const fn decode_capabilities(cap: u32) -> AhciCapabilities {
    AhciCapabilities {
        port_count: ((cap & 0x1F) as u8).saturating_add(1),
        command_slot_count: (((cap >> 8) & 0x1F) as u8).saturating_add(1),
        supports_64bit: (cap & (1 << 31)) != 0,
        supports_ncq: (cap & (1 << 30)) != 0,
        supports_staggered_spinup: (cap & (1 << 27)) != 0,
    }
}

pub fn list_implemented_ports(pi: u32, out: &mut [u8]) -> usize {
    let mut count = 0usize;
    for port in 0..32u8 {
        if (pi & (1u32 << port)) == 0 {
            continue;
        }
        if count >= out.len() {
            break;
        }
        out[count] = port;
        count += 1;
    }
    count
}

pub const fn port_offset(port: u8, reg: usize) -> usize {
    AHCI_PORT_BASE + (port as usize) * AHCI_PORT_STRIDE + reg
}

pub const fn ssts_det(ssts: u32) -> u8 {
    (ssts & AHCI_SSTS_DET_MASK) as u8
}

pub const fn ssts_ipm(ssts: u32) -> u8 {
    ((ssts & AHCI_SSTS_IPM_MASK) >> AHCI_SSTS_IPM_SHIFT) as u8
}

pub const fn port_has_active_device(ssts: u32) -> bool {
    ssts_det(ssts) == AHCI_SSTS_DET_PRESENT && ssts_ipm(ssts) == AHCI_SSTS_IPM_ACTIVE
}

pub const fn find_free_command_slot(ci: u32, sact: u32, max_slots: u8) -> Option<u8> {
    let busy = ci | sact;
    let mut slot = 0u8;
    while slot < max_slots && slot < 32 {
        if (busy & (1u32 << slot)) == 0 {
            return Some(slot);
        }
        slot += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem;

    #[test]
    fn signatures_map_to_known_device_types() {
        assert_eq!(classify_port_signature(0), AhciPortDevice::None);
        assert_eq!(classify_port_signature(SATA_SIG_ATA), AhciPortDevice::Sata);
        assert_eq!(
            classify_port_signature(SATA_SIG_ATAPI),
            AhciPortDevice::Satapi
        );
        assert_eq!(
            classify_port_signature(SATA_SIG_SEMB),
            AhciPortDevice::EnclosureMgmt
        );
        assert_eq!(
            classify_port_signature(SATA_SIG_PM),
            AhciPortDevice::PortMultiplier
        );
        assert_eq!(
            classify_port_signature(0xDEAD_BEEF),
            AhciPortDevice::Unknown
        );
    }

    #[test]
    fn cap_decode_extracts_basic_flags() {
        let cap = (1 << 31) | (1 << 30) | (1 << 27) | (0x1F << 8) | 0x0F;
        let decoded = decode_capabilities(cap);

        assert_eq!(decoded.port_count, 16);
        assert_eq!(decoded.command_slot_count, 32);
        assert!(decoded.supports_64bit);
        assert!(decoded.supports_ncq);
        assert!(decoded.supports_staggered_spinup);
    }

    #[test]
    fn implemented_port_listing_is_dense_and_bounded() {
        let mut out = [0xFF; 2];
        let count = list_implemented_ports(0b10101, &mut out);

        assert_eq!(count, 2);
        assert_eq!(out[0], 0);
        assert_eq!(out[1], 2);
    }

    #[test]
    fn ssts_helpers_decode_active_link() {
        let ssts = (AHCI_SSTS_IPM_ACTIVE as u32) << AHCI_SSTS_IPM_SHIFT | 0x3;

        assert_eq!(ssts_det(ssts), AHCI_SSTS_DET_PRESENT);
        assert_eq!(ssts_ipm(ssts), AHCI_SSTS_IPM_ACTIVE);
        assert!(port_has_active_device(ssts));
        assert!(!port_has_active_device(0x1));
    }

    #[test]
    fn slot_allocator_ignores_busy_bits() {
        assert_eq!(find_free_command_slot(0b0011, 0, 4), Some(2));
        assert_eq!(find_free_command_slot(0xFFFF_FFFF, 0, 32), None);
        assert_eq!(find_free_command_slot(0, 0b1, 1), None);
    }

    #[test]
    fn ahci_struct_layout_matches_spec() {
        assert_eq!(mem::size_of::<AhciCommandHeader>(), 32);
        assert_eq!(mem::size_of::<AhciPrdtEntry>(), 16);
        assert_eq!(mem::size_of::<AhciCommandTable1>(), 0x90);
    }
}
