pub const SATA_SIG_ATA: u32 = 0x0000_0101;
pub const SATA_SIG_ATAPI: u32 = 0xEB14_0101;
pub const SATA_SIG_SEMB: u32 = 0xC33C_0101;
pub const SATA_SIG_PM: u32 = 0x9669_0101;

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
