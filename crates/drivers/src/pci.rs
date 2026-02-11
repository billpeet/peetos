use arch_x86_64::port::{inl, outl};

pub const CONFIG_ADDRESS_PORT: u16 = 0xCF8;
pub const CONFIG_DATA_PORT: u16 = 0xCFC;
const CONFIG_ENABLE: u32 = 1 << 31;

pub const PCI_CLASS_MASS_STORAGE: u8 = 0x01;

pub const PCI_STORAGE_SUBCLASS_IDE: u8 = 0x01;
pub const PCI_STORAGE_SUBCLASS_SATA: u8 = 0x06;

pub const PCI_SATA_PROG_IF_AHCI: u8 = 0x01;

const REG_ID: u8 = 0x00;
const REG_COMMAND_STATUS: u8 = 0x04;
const REG_CLASS_REVISION: u8 = 0x08;
const REG_HEADER_TYPE: u8 = 0x0C;
const REG_BAR0: u8 = 0x10;
const REG_INTERRUPT: u8 = 0x3C;

const PCI_COMMAND_MEMORY_SPACE: u16 = 1 << 1;
const PCI_COMMAND_BUS_MASTER: u16 = 1 << 2;

const BAR_COUNT_TYPE0: usize = 6;
const BAR_COUNT_TYPE1: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum PciBarKind {
    None = 0,
    Io = 1,
    Memory32 = 2,
    Memory64 = 3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PciBar {
    pub kind: PciBarKind,
    pub addr: u64,
    pub prefetchable: bool,
}

impl PciBar {
    pub const EMPTY: Self = Self {
        kind: PciBarKind::None,
        addr: 0,
        prefetchable: false,
    };
}

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
    pub command: u16,
    pub status: u16,
    pub irq_line: u8,
    pub irq_pin: u8,
    pub bars: [PciBar; 6],
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
        command: 0,
        status: 0,
        irq_line: 0,
        irq_pin: 0,
        bars: [PciBar::EMPTY; 6],
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

#[cfg(target_os = "none")]
pub fn read_config_dword(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
    unsafe {
        outl(
            CONFIG_ADDRESS_PORT,
            config_address(bus, device, function, offset),
        );
        inl(CONFIG_DATA_PORT)
    }
}

#[cfg(not(target_os = "none"))]
pub fn read_config_dword(_bus: u8, _device: u8, _function: u8, _offset: u8) -> u32 {
    0xFFFF_FFFF
}

#[cfg(target_os = "none")]
pub fn write_config_dword(bus: u8, device: u8, function: u8, offset: u8, value: u32) {
    unsafe {
        outl(
            CONFIG_ADDRESS_PORT,
            config_address(bus, device, function, offset),
        );
        outl(CONFIG_DATA_PORT, value);
    }
}

#[cfg(not(target_os = "none"))]
pub fn write_config_dword(_bus: u8, _device: u8, _function: u8, _offset: u8, _value: u32) {}

pub fn read_config_word(bus: u8, device: u8, function: u8, offset: u8) -> u16 {
    let shift = (offset & 0x2) * 8;
    ((read_config_dword(bus, device, function, offset & !0x2) >> shift) & 0xFFFF) as u16
}

pub fn write_config_word(bus: u8, device: u8, function: u8, offset: u8, value: u16) {
    let aligned = offset & !0x2;
    let shift = (offset & 0x2) * 8;
    let mut dword = read_config_dword(bus, device, function, aligned);
    dword &= !(0xFFFF << shift);
    dword |= (value as u32) << shift;
    write_config_dword(bus, device, function, aligned, dword);
}

#[cfg(target_os = "none")]
pub fn enable_mem_and_bus_master(bus: u8, device: u8, function: u8) -> u16 {
    let mut command = read_config_word(bus, device, function, REG_COMMAND_STATUS);
    command |= PCI_COMMAND_MEMORY_SPACE | PCI_COMMAND_BUS_MASTER;
    write_config_word(bus, device, function, REG_COMMAND_STATUS, command);
    read_config_word(bus, device, function, REG_COMMAND_STATUS)
}

#[cfg(not(target_os = "none"))]
pub fn enable_mem_and_bus_master(_bus: u8, _device: u8, _function: u8) -> u16 {
    0
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
        read_config_dword(bus, device, function, offset)
    }
}

#[cfg(any(target_os = "none", test))]
fn read_vendor_id(access: &impl ConfigAccess, bus: u8, device: u8, function: u8) -> u16 {
    (access.read_u32(bus, device, function, REG_ID) & 0xFFFF) as u16
}

#[cfg(any(target_os = "none", test))]
fn bar_count(header_type: u8) -> usize {
    match header_type & 0x7F {
        0x00 => BAR_COUNT_TYPE0,
        0x01 => BAR_COUNT_TYPE1,
        _ => 0,
    }
}

#[cfg(any(target_os = "none", test))]
fn parse_bars(
    access: &impl ConfigAccess,
    bus: u8,
    device: u8,
    function: u8,
    header_type: u8,
) -> [PciBar; 6] {
    let mut bars = [PciBar::EMPTY; 6];
    let mut index = 0usize;
    let max = bar_count(header_type);

    while index < max {
        let offset = REG_BAR0 + (index as u8 * 4);
        let low = access.read_u32(bus, device, function, offset);

        if low == 0 || low == 0xFFFF_FFFF {
            index += 1;
            continue;
        }

        if (low & 0x1) != 0 {
            bars[index] = PciBar {
                kind: PciBarKind::Io,
                addr: (low & 0xFFFF_FFFC) as u64,
                prefetchable: false,
            };
            index += 1;
            continue;
        }

        let prefetchable = (low & 0x8) != 0;
        let mem_type = (low >> 1) & 0x3;

        if mem_type == 0x2 && (index + 1) < max {
            let high = access.read_u32(bus, device, function, offset + 4);
            bars[index] = PciBar {
                kind: PciBarKind::Memory64,
                addr: ((high as u64) << 32) | ((low & 0xFFFF_FFF0) as u64),
                prefetchable,
            };
            index += 2;
            continue;
        }

        bars[index] = PciBar {
            kind: PciBarKind::Memory32,
            addr: (low & 0xFFFF_FFF0) as u64,
            prefetchable,
        };
        index += 1;
    }

    bars
}

#[cfg(any(target_os = "none", test))]
fn read_device_info(access: &impl ConfigAccess, bus: u8, device: u8, function: u8) -> DeviceInfo {
    let id = access.read_u32(bus, device, function, REG_ID);
    let command_status = access.read_u32(bus, device, function, REG_COMMAND_STATUS);
    let class_revision = access.read_u32(bus, device, function, REG_CLASS_REVISION);
    let header = access.read_u32(bus, device, function, REG_HEADER_TYPE);
    let interrupt = access.read_u32(bus, device, function, REG_INTERRUPT);
    let header_type = ((header >> 16) & 0xFF) as u8;

    DeviceInfo {
        bus,
        device,
        function,
        vendor_id: (id & 0xFFFF) as u16,
        device_id: ((id >> 16) & 0xFFFF) as u16,
        prog_if: ((class_revision >> 8) & 0xFF) as u8,
        subclass: ((class_revision >> 16) & 0xFF) as u8,
        class_code: ((class_revision >> 24) & 0xFF) as u8,
        header_type,
        command: (command_status & 0xFFFF) as u16,
        status: ((command_status >> 16) & 0xFFFF) as u16,
        irq_line: (interrupt & 0xFF) as u8,
        irq_pin: ((interrupt >> 8) & 0xFF) as u8,
        bars: parse_bars(access, bus, device, function, header_type),
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
        cfg.set(0, 1, 0, REG_ID, 0x5678_1234);
        cfg.set(0, 1, 0, REG_CLASS_REVISION, 0x0106_0100);
        cfg.set(0, 1, 0, REG_HEADER_TYPE, 0x0000_0000);
        cfg.set(0, 1, 0, REG_COMMAND_STATUS, 0x0010_0007);
        cfg.set(0, 1, 0, REG_INTERRUPT, 0x0001_0E0A);
        cfg.set(0, 1, 0, 0x24, 0xFEBF_0000);

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
        assert_eq!(devices[0].command, 0x0007);
        assert_eq!(devices[0].status, 0x0010);
        assert_eq!(devices[0].irq_line, 0x0A);
        assert_eq!(devices[0].irq_pin, 0x0E);
        assert_eq!(devices[0].bars[5].kind, PciBarKind::Memory32);
        assert_eq!(devices[0].bars[5].addr, 0xFEBF_0000);
    }

    #[test]
    fn scan_handles_multifunction_and_overflow() {
        let mut cfg = MockConfig::default();

        cfg.set(0, 2, 0, REG_ID, 0x1111_8086);
        cfg.set(0, 2, 0, REG_CLASS_REVISION, 0x0604_0000);
        cfg.set(0, 2, 0, REG_HEADER_TYPE, 0x0080_0000);

        cfg.set(0, 2, 1, REG_ID, 0x2222_10EC);
        cfg.set(0, 2, 1, REG_CLASS_REVISION, 0x0200_0000);
        cfg.set(0, 2, 1, REG_HEADER_TYPE, 0x0000_0000);

        let mut devices = [DeviceInfo::EMPTY; 1];
        let summary = scan_with_access(&cfg, &mut devices);

        assert_eq!(summary.total_devices, 2);
        assert_eq!(summary.stored_devices, 1);
        assert!(summary.overflow);
        assert_eq!(devices[0].vendor_id, 0x8086);
    }

    #[test]
    fn parse_64bit_bar_uses_next_slot_for_address_upper() {
        let mut cfg = MockConfig::default();
        cfg.set(0, 3, 0, REG_ID, 0x0001_1234);
        cfg.set(0, 3, 0, REG_CLASS_REVISION, 0x0106_0100);
        cfg.set(0, 3, 0, REG_HEADER_TYPE, 0x0000_0000);
        cfg.set(0, 3, 0, REG_BAR0, 0xABCD_0004);
        cfg.set(0, 3, 0, REG_BAR0 + 4, 0x0000_0001);

        let info = read_device_info(&cfg, 0, 3, 0);

        assert_eq!(info.bars[0].kind, PciBarKind::Memory64);
        assert_eq!(info.bars[0].addr, 0x0000_0001_ABCD_0000);
        assert_eq!(info.bars[1].kind, PciBarKind::None);
    }

    #[test]
    fn header_type_bridge_limits_bar_scan_to_two() {
        let mut cfg = MockConfig::default();
        cfg.set(0, 4, 0, REG_ID, 0x0002_1234);
        cfg.set(0, 4, 0, REG_CLASS_REVISION, 0x0604_0000);
        cfg.set(0, 4, 0, REG_HEADER_TYPE, 0x0001_0000);
        cfg.set(0, 4, 0, REG_BAR0, 0xD000_0000);
        cfg.set(0, 4, 0, REG_BAR0 + 4, 0xD000_1000);
        cfg.set(0, 4, 0, REG_BAR0 + 8, 0xD000_2000);

        let info = read_device_info(&cfg, 0, 4, 0);

        assert_eq!(info.bars[0].kind, PciBarKind::Memory32);
        assert_eq!(info.bars[1].kind, PciBarKind::Memory32);
        assert_eq!(info.bars[2].kind, PciBarKind::None);
    }
}
