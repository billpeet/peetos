use arch_x86_64::port::{inl, outl};

pub const CONFIG_ADDRESS_PORT: u16 = 0xCF8;
pub const CONFIG_DATA_PORT: u16 = 0xCFC;
const CONFIG_ENABLE: u32 = 1 << 31;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceInfo {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub vendor_id: u16,
    pub device_id: u16,
    pub class_code: u8,
    pub subclass: u8,
    pub prog_if: u8,
    pub header_type: u8,
}

impl DeviceInfo {
    pub const EMPTY: Self = Self {
        bus: 0,
        device: 0,
        function: 0,
        vendor_id: 0,
        device_id: 0,
        class_code: 0,
        subclass: 0,
        prog_if: 0,
        header_type: 0,
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScanSummary {
    pub total_devices: usize,
    pub stored_devices: usize,
    pub overflow: bool,
}

impl ScanSummary {
    pub const EMPTY: Self = Self {
        total_devices: 0,
        stored_devices: 0,
        overflow: false,
    };
}

pub const fn config_address(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
    CONFIG_ENABLE
        | ((bus as u32) << 16)
        | ((device as u32) << 11)
        | ((function as u32) << 8)
        | ((offset as u32) & 0xFC)
}

#[cfg(any(target_os = "none", test))]
trait ConfigAccess {
    fn read_u32(&self, bus: u8, device: u8, function: u8, offset: u8) -> u32;
}

#[cfg(target_os = "none")]
struct PortConfigAccess;

#[cfg(target_os = "none")]
impl ConfigAccess for PortConfigAccess {
    #[inline]
    fn read_u32(&self, bus: u8, device: u8, function: u8, offset: u8) -> u32 {
        unsafe {
            outl(
                CONFIG_ADDRESS_PORT,
                config_address(bus, device, function, offset),
            );
            inl(CONFIG_DATA_PORT)
        }
    }
}

#[cfg(any(target_os = "none", test))]
fn read_vendor_id(access: &impl ConfigAccess, bus: u8, device: u8, function: u8) -> u16 {
    (access.read_u32(bus, device, function, 0x00) & 0xFFFF) as u16
}

#[cfg(any(target_os = "none", test))]
fn read_device_info(access: &impl ConfigAccess, bus: u8, device: u8, function: u8) -> DeviceInfo {
    let id = access.read_u32(bus, device, function, 0x00);
    let class = access.read_u32(bus, device, function, 0x08);
    let header = access.read_u32(bus, device, function, 0x0C);

    DeviceInfo {
        bus,
        device,
        function,
        vendor_id: (id & 0xFFFF) as u16,
        device_id: ((id >> 16) & 0xFFFF) as u16,
        prog_if: ((class >> 8) & 0xFF) as u8,
        subclass: ((class >> 16) & 0xFF) as u8,
        class_code: ((class >> 24) & 0xFF) as u8,
        header_type: ((header >> 16) & 0xFF) as u8,
    }
}

#[cfg(any(target_os = "none", test))]
fn scan_with_access(access: &impl ConfigAccess, devices: &mut [DeviceInfo]) -> ScanSummary {
    let mut summary = ScanSummary::EMPTY;

    for bus in 0..=u8::MAX {
        for device in 0..32u8 {
            let vendor = read_vendor_id(access, bus, device, 0);
            if vendor == 0xFFFF {
                continue;
            }

            let header = read_device_info(access, bus, device, 0).header_type;
            let function_count = if (header & 0x80) != 0 { 8 } else { 1 };

            for function in 0..function_count {
                if read_vendor_id(access, bus, device, function) == 0xFFFF {
                    continue;
                }

                let info = read_device_info(access, bus, device, function);
                summary.total_devices += 1;

                if summary.stored_devices < devices.len() {
                    devices[summary.stored_devices] = info;
                    summary.stored_devices += 1;
                } else {
                    summary.overflow = true;
                }
            }
        }
    }

    summary
}

#[cfg(target_os = "none")]
pub fn enumerate(devices: &mut [DeviceInfo]) -> ScanSummary {
    scan_with_access(&PortConfigAccess, devices)
}

#[cfg(not(target_os = "none"))]
pub fn enumerate(_devices: &mut [DeviceInfo]) -> ScanSummary {
    ScanSummary::EMPTY
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct MockConfig {
        regs: BTreeMap<(u8, u8, u8, u8), u32>,
    }

    impl MockConfig {
        fn set(&mut self, bus: u8, device: u8, function: u8, offset: u8, value: u32) {
            self.regs
                .insert((bus, device, function, offset & 0xFC), value);
        }
    }

    impl ConfigAccess for MockConfig {
        fn read_u32(&self, bus: u8, device: u8, function: u8, offset: u8) -> u32 {
            *self
                .regs
                .get(&(bus, device, function, offset & 0xFC))
                .unwrap_or(&0xFFFF_FFFF)
        }
    }

    #[test]
    fn config_address_packs_fields() {
        assert_eq!(
            config_address(0x12, 0x1B, 0x03, 0x0D),
            0x8000_0000 | (0x12 << 16) | (0x1B << 11) | (0x03 << 8) | 0x0C
        );
    }

    #[test]
    fn scan_finds_single_function_device() {
        let mut cfg = MockConfig::default();
        cfg.set(0, 1, 0, 0x00, 0x5678_1234);
        cfg.set(0, 1, 0, 0x08, 0x0106_0100);
        cfg.set(0, 1, 0, 0x0C, 0x0000_0000);

        let mut devices = [DeviceInfo::EMPTY; 4];
        let summary = scan_with_access(&cfg, &mut devices);

        assert_eq!(summary.total_devices, 1);
        assert_eq!(summary.stored_devices, 1);
        assert!(!summary.overflow);
        assert_eq!(devices[0].bus, 0);
        assert_eq!(devices[0].device, 1);
        assert_eq!(devices[0].function, 0);
        assert_eq!(devices[0].vendor_id, 0x1234);
        assert_eq!(devices[0].device_id, 0x5678);
        assert_eq!(devices[0].class_code, 0x01);
        assert_eq!(devices[0].subclass, 0x06);
        assert_eq!(devices[0].prog_if, 0x01);
    }

    #[test]
    fn scan_handles_multifunction_and_overflow() {
        let mut cfg = MockConfig::default();

        cfg.set(0, 2, 0, 0x00, 0x1111_8086);
        cfg.set(0, 2, 0, 0x08, 0x0604_0000);
        cfg.set(0, 2, 0, 0x0C, 0x0080_0000);

        cfg.set(0, 2, 1, 0x00, 0x2222_10EC);
        cfg.set(0, 2, 1, 0x08, 0x0200_0000);
        cfg.set(0, 2, 1, 0x0C, 0x0000_0000);

        let mut devices = [DeviceInfo::EMPTY; 1];
        let summary = scan_with_access(&cfg, &mut devices);

        assert_eq!(summary.total_devices, 2);
        assert_eq!(summary.stored_devices, 1);
        assert!(summary.overflow);
        assert_eq!(devices[0].vendor_id, 0x8086);
    }
}
