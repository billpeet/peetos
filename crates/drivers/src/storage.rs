use crate::pci::{
    DeviceInfo, PciBarKind, PCI_CLASS_MASS_STORAGE, PCI_SATA_PROG_IF_AHCI,
    PCI_STORAGE_SUBCLASS_IDE, PCI_STORAGE_SUBCLASS_SATA,
};

pub const AHCI_ABAR_INDEX: usize = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum StorageControllerKind {
    AtaLegacy = 0,
    AtaNative = 1,
    AtaMixed = 2,
    Ahci = 3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StorageController {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub vendor_id: u16,
    pub device_id: u16,
    pub kind: StorageControllerKind,
    pub irq_line: u8,
    pub irq_pin: u8,
    pub bus_master_enabled: bool,
    pub ahci_abar: u64,
    pub ahci_abar_is_64bit: bool,
}

impl StorageController {
    pub const EMPTY: Self = Self {
        bus: 0,
        device: 0,
        function: 0,
        vendor_id: 0,
        device_id: 0,
        kind: StorageControllerKind::AtaLegacy,
        irq_line: 0,
        irq_pin: 0,
        bus_master_enabled: false,
        ahci_abar: 0,
        ahci_abar_is_64bit: false,
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StorageSummary {
    pub total_controllers: usize,
    pub ata_controllers: usize,
    pub ahci_controllers: usize,
    pub stored_controllers: usize,
    pub overflow: bool,
}

impl StorageSummary {
    pub const EMPTY: Self = Self {
        total_controllers: 0,
        ata_controllers: 0,
        ahci_controllers: 0,
        stored_controllers: 0,
        overflow: false,
    };
}

pub const fn kind_name(kind: StorageControllerKind) -> &'static str {
    match kind {
        StorageControllerKind::AtaLegacy => "ata-legacy",
        StorageControllerKind::AtaNative => "ata-native",
        StorageControllerKind::AtaMixed => "ata-mixed",
        StorageControllerKind::Ahci => "ahci",
    }
}

pub fn classify_device(dev: &DeviceInfo) -> Option<StorageControllerKind> {
    if dev.class_code != PCI_CLASS_MASS_STORAGE {
        return None;
    }

    match dev.subclass {
        PCI_STORAGE_SUBCLASS_IDE => {
            let primary_native = (dev.prog_if & 0x01) != 0;
            let secondary_native = (dev.prog_if & 0x04) != 0;

            match (primary_native, secondary_native) {
                (false, false) => Some(StorageControllerKind::AtaLegacy),
                (true, true) => Some(StorageControllerKind::AtaNative),
                _ => Some(StorageControllerKind::AtaMixed),
            }
        }
        PCI_STORAGE_SUBCLASS_SATA if dev.prog_if == PCI_SATA_PROG_IF_AHCI => {
            Some(StorageControllerKind::Ahci)
        }
        _ => None,
    }
}

fn ahci_abar(dev: &DeviceInfo) -> (u64, bool) {
    let bar = dev.bars[AHCI_ABAR_INDEX];
    match bar.kind {
        PciBarKind::Memory32 => (bar.addr, false),
        PciBarKind::Memory64 => (bar.addr, true),
        _ => (0, false),
    }
}

pub fn enumerate(devices: &[DeviceInfo], out: &mut [StorageController]) -> StorageSummary {
    let mut summary = StorageSummary::EMPTY;

    for dev in devices {
        let Some(kind) = classify_device(dev) else {
            continue;
        };

        summary.total_controllers += 1;
        match kind {
            StorageControllerKind::Ahci => summary.ahci_controllers += 1,
            _ => summary.ata_controllers += 1,
        }

        let (abar, abar_64) = if kind == StorageControllerKind::Ahci {
            ahci_abar(dev)
        } else {
            (0, false)
        };

        let controller = StorageController {
            bus: dev.bus,
            device: dev.device,
            function: dev.function,
            vendor_id: dev.vendor_id,
            device_id: dev.device_id,
            kind,
            irq_line: dev.irq_line,
            irq_pin: dev.irq_pin,
            bus_master_enabled: (dev.command & (1 << 2)) != 0,
            ahci_abar: abar,
            ahci_abar_is_64bit: abar_64,
        };

        if summary.stored_controllers < out.len() {
            out[summary.stored_controllers] = controller;
            summary.stored_controllers += 1;
        } else {
            summary.overflow = true;
        }
    }

    summary
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pci::{PciBar, PciBarKind};

    fn base_device() -> DeviceInfo {
        DeviceInfo {
            vendor_id: 0x8086,
            device_id: 0x2922,
            ..DeviceInfo::EMPTY
        }
    }

    #[test]
    fn classify_ide_modes() {
        let mut dev = base_device();
        dev.class_code = PCI_CLASS_MASS_STORAGE;
        dev.subclass = PCI_STORAGE_SUBCLASS_IDE;

        dev.prog_if = 0x00;
        assert_eq!(
            classify_device(&dev),
            Some(StorageControllerKind::AtaLegacy)
        );

        dev.prog_if = 0x05;
        assert_eq!(
            classify_device(&dev),
            Some(StorageControllerKind::AtaNative)
        );

        dev.prog_if = 0x01;
        assert_eq!(classify_device(&dev), Some(StorageControllerKind::AtaMixed));
    }

    #[test]
    fn classify_ahci_requires_standard_prog_if() {
        let mut dev = base_device();
        dev.class_code = PCI_CLASS_MASS_STORAGE;
        dev.subclass = PCI_STORAGE_SUBCLASS_SATA;

        dev.prog_if = 0x00;
        assert_eq!(classify_device(&dev), None);

        dev.prog_if = PCI_SATA_PROG_IF_AHCI;
        assert_eq!(classify_device(&dev), Some(StorageControllerKind::Ahci));
    }

    #[test]
    fn enumerate_tracks_ata_and_ahci() {
        let mut ata = base_device();
        ata.bus = 0;
        ata.device = 1;
        ata.class_code = PCI_CLASS_MASS_STORAGE;
        ata.subclass = PCI_STORAGE_SUBCLASS_IDE;
        ata.prog_if = 0x00;

        let mut ahci = base_device();
        ahci.bus = 0;
        ahci.device = 2;
        ahci.class_code = PCI_CLASS_MASS_STORAGE;
        ahci.subclass = PCI_STORAGE_SUBCLASS_SATA;
        ahci.prog_if = PCI_SATA_PROG_IF_AHCI;
        ahci.command = 0x0006;
        ahci.bars[AHCI_ABAR_INDEX] = PciBar {
            kind: PciBarKind::Memory32,
            addr: 0xFEBF_0000,
            prefetchable: false,
        };

        let mut out = [StorageController::EMPTY; 2];
        let summary = enumerate(&[ata, ahci], &mut out);

        assert_eq!(summary.total_controllers, 2);
        assert_eq!(summary.ata_controllers, 1);
        assert_eq!(summary.ahci_controllers, 1);
        assert_eq!(summary.stored_controllers, 2);
        assert!(!summary.overflow);

        assert_eq!(out[0].kind, StorageControllerKind::AtaLegacy);
        assert_eq!(out[1].kind, StorageControllerKind::Ahci);
        assert_eq!(out[1].ahci_abar, 0xFEBF_0000);
        assert!(out[1].bus_master_enabled);
    }
}
