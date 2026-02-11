use arch_x86_64::addr::{PhysAddr, VirtAddr, PAGE_SIZE};
use arch_x86_64::port::{inb, inw, outb};
use drivers::{ahci, pci, storage};

use crate::memory::vmm;
use crate::serial_println;

const STORAGE_LOG_LIMIT: usize = 8;
const ATA_IDENTIFY_TIMEOUT: usize = 100_000;

const ATA_REG_DATA: u16 = 0x00;
const ATA_REG_SECCOUNT0: u16 = 0x02;
const ATA_REG_LBA0: u16 = 0x03;
const ATA_REG_LBA1: u16 = 0x04;
const ATA_REG_LBA2: u16 = 0x05;
const ATA_REG_DRIVE: u16 = 0x06;
const ATA_REG_STATUS_COMMAND: u16 = 0x07;

const ATA_SR_ERR: u8 = 1 << 0;
const ATA_SR_DRQ: u8 = 1 << 3;
const ATA_SR_BSY: u8 = 1 << 7;

const ATA_CMD_IDENTIFY: u8 = 0xEC;

const ATA_DRIVE_MASTER: u8 = 0xA0;
const ATA_DRIVE_SLAVE: u8 = 0xB0;

const AHCI_REG_CAP: usize = 0x00;
const AHCI_REG_GHC: usize = 0x04;
const AHCI_REG_PI: usize = 0x0C;
const AHCI_REG_VS: usize = 0x10;
const AHCI_GHC_AE: u32 = 1 << 31;

const AHCI_PORT_BASE: usize = 0x100;
const AHCI_PORT_STRIDE: usize = 0x80;
const AHCI_PORT_SIG: usize = 0x24;
const AHCI_PORT_SSTS: usize = 0x28;

const AHCI_MMIO_WINDOW_BASE: u64 = 0xFFFF_FFFF_9000_0000;
const AHCI_MMIO_WINDOW_STRIDE: u64 = 0x20_000;
const AHCI_MMIO_WINDOW_LIMIT: usize = 8;
const AHCI_MMIO_READ_SIZE: usize = 0x1200;

static mut AHCI_MMIO_WINDOW_NEXT: usize = 0;

#[derive(Clone, Copy)]
struct AtaChannel {
    cmd: u16,
    ctrl: u16,
}

#[derive(Clone, Copy)]
struct AtaIdentify {
    sectors: u64,
    lba48: bool,
}

pub fn init(pci_devices: &[pci::DeviceInfo]) {
    let mut storage_ctrls = [storage::StorageController::EMPTY; STORAGE_LOG_LIMIT];
    let storage_summary = storage::enumerate(pci_devices, &mut storage_ctrls);

    serial_println!(
        "[STORAGE] Detected {} controller(s): ATA={} AHCI={} (showing {})",
        storage_summary.total_controllers,
        storage_summary.ata_controllers,
        storage_summary.ahci_controllers,
        storage_summary.stored_controllers
    );

    for ctrl in storage_ctrls
        .iter()
        .take(storage_summary.stored_controllers)
    {
        serial_println!(
            "[STORAGE] {:02X}:{:02X}.{} {:04X}:{:04X} kind={} bus_master={} irq={:02X}",
            ctrl.bus,
            ctrl.device,
            ctrl.function,
            ctrl.vendor_id,
            ctrl.device_id,
            storage::kind_name(ctrl.kind),
            ctrl.bus_master_enabled as u8,
            ctrl.irq_line
        );

        let Some(dev) = find_pci_device(pci_devices, ctrl) else {
            continue;
        };

        match ctrl.kind {
            storage::StorageControllerKind::Ahci => probe_ahci(ctrl, dev),
            _ => probe_ata(ctrl, dev),
        }
    }

    if storage_summary.overflow {
        serial_println!(
            "[STORAGE] {} additional controller(s) not shown",
            storage_summary.total_controllers - storage_summary.stored_controllers
        );
    }
}

fn find_pci_device<'a>(
    devices: &'a [pci::DeviceInfo],
    ctrl: &storage::StorageController,
) -> Option<&'a pci::DeviceInfo> {
    devices.iter().find(|dev| {
        dev.bus == ctrl.bus && dev.device == ctrl.device && dev.function == ctrl.function
    })
}

fn probe_ahci(ctrl: &storage::StorageController, dev: &pci::DeviceInfo) {
    let command = pci::enable_mem_and_bus_master(dev.bus, dev.device, dev.function);
    if ctrl.ahci_abar == 0 {
        serial_println!(
            "[AHCI] {:02X}:{:02X}.{} missing ABAR",
            ctrl.bus,
            ctrl.device,
            ctrl.function
        );
        return;
    }

    let Some(base) = map_mmio_window(ctrl.ahci_abar, AHCI_MMIO_READ_SIZE) else {
        serial_println!(
            "[AHCI] {:02X}:{:02X}.{} failed to map ABAR 0x{:x}",
            ctrl.bus,
            ctrl.device,
            ctrl.function,
            ctrl.ahci_abar
        );
        return;
    };

    let mut ghc = mmio_read32(base, AHCI_REG_GHC);
    if (ghc & AHCI_GHC_AE) == 0 {
        ghc |= AHCI_GHC_AE;
        mmio_write32(base, AHCI_REG_GHC, ghc);
    }

    let cap = mmio_read32(base, AHCI_REG_CAP);
    let pi = mmio_read32(base, AHCI_REG_PI);
    let vs = mmio_read32(base, AHCI_REG_VS);
    let caps = ahci::decode_capabilities(cap);

    serial_println!(
        "[AHCI] {:02X}:{:02X}.{} ABAR=0x{:x} command=0x{:04X} v={}.{} ports={} slots={} ncq={} s64a={}",
        ctrl.bus,
        ctrl.device,
        ctrl.function,
        ctrl.ahci_abar,
        command,
        (vs >> 16) & 0xFFFF,
        vs & 0xFFFF,
        caps.port_count,
        caps.command_slot_count,
        caps.supports_ncq as u8,
        caps.supports_64bit as u8
    );

    let mut ports = [0u8; 32];
    let port_count = ahci::list_implemented_ports(pi, &mut ports);
    for port in ports.iter().take(port_count) {
        let port_base = AHCI_PORT_BASE + (*port as usize) * AHCI_PORT_STRIDE;
        let ssts = mmio_read32(base, port_base + AHCI_PORT_SSTS);
        let sig = mmio_read32(base, port_base + AHCI_PORT_SIG);
        let device = ahci::classify_port_signature(sig);
        let det = (ssts & 0x0F) as u8;

        serial_println!(
            "[AHCI] port={} det={} sig=0x{:08x} dev={:?}",
            *port,
            det,
            sig,
            device
        );
    }
}

fn probe_ata(ctrl: &storage::StorageController, dev: &pci::DeviceInfo) {
    let (primary, secondary) = ide_channels(dev);
    serial_println!(
        "[ATA] {:02X}:{:02X}.{} primary=0x{:x}/0x{:x} secondary=0x{:x}/0x{:x}",
        ctrl.bus,
        ctrl.device,
        ctrl.function,
        primary.cmd,
        primary.ctrl,
        secondary.cmd,
        secondary.ctrl
    );

    probe_ata_drive("pri-master", primary, false);
    probe_ata_drive("pri-slave", primary, true);
    probe_ata_drive("sec-master", secondary, false);
    probe_ata_drive("sec-slave", secondary, true);
}

fn probe_ata_drive(label: &str, channel: AtaChannel, slave: bool) {
    let Some(info) = ata_identify(channel, slave) else {
        return;
    };

    serial_println!(
        "[ATA] {} present sectors={} lba48={}",
        label,
        info.sectors,
        info.lba48 as u8
    );
}

fn ata_identify(channel: AtaChannel, slave: bool) -> Option<AtaIdentify> {
    unsafe {
        outb(
            channel.cmd + ATA_REG_DRIVE,
            if slave {
                ATA_DRIVE_SLAVE
            } else {
                ATA_DRIVE_MASTER
            },
        );
        io_delay(channel.ctrl);

        outb(channel.cmd + ATA_REG_SECCOUNT0, 0);
        outb(channel.cmd + ATA_REG_LBA0, 0);
        outb(channel.cmd + ATA_REG_LBA1, 0);
        outb(channel.cmd + ATA_REG_LBA2, 0);
        outb(channel.cmd + ATA_REG_STATUS_COMMAND, ATA_CMD_IDENTIFY);

        let status = inb(channel.cmd + ATA_REG_STATUS_COMMAND);
        if status == 0 {
            return None;
        }

        let mut ready = false;
        for _ in 0..ATA_IDENTIFY_TIMEOUT {
            let s = inb(channel.cmd + ATA_REG_STATUS_COMMAND);
            if (s & ATA_SR_ERR) != 0 {
                return None;
            }
            if (s & ATA_SR_BSY) == 0 && (s & ATA_SR_DRQ) != 0 {
                ready = true;
                break;
            }
        }

        if !ready {
            return None;
        }

        let mut identify_words = [0u16; 256];
        for word in identify_words.iter_mut() {
            *word = inw(channel.cmd + ATA_REG_DATA);
        }

        let lba48 = (identify_words[83] & (1 << 10)) != 0;
        let sectors28 = (identify_words[60] as u64) | ((identify_words[61] as u64) << 16);
        let sectors48 = (identify_words[100] as u64)
            | ((identify_words[101] as u64) << 16)
            | ((identify_words[102] as u64) << 32)
            | ((identify_words[103] as u64) << 48);

        Some(AtaIdentify {
            sectors: if lba48 && sectors48 != 0 {
                sectors48
            } else {
                sectors28
            },
            lba48,
        })
    }
}

fn io_delay(ctrl_port: u16) {
    unsafe {
        let _ = inb(ctrl_port);
        let _ = inb(ctrl_port);
        let _ = inb(ctrl_port);
        let _ = inb(ctrl_port);
    }
}

fn ide_channels(dev: &pci::DeviceInfo) -> (AtaChannel, AtaChannel) {
    (
        AtaChannel {
            cmd: io_bar_base(dev.bars[0], 0x1F0),
            ctrl: control_bar_base(dev.bars[1], 0x3F6),
        },
        AtaChannel {
            cmd: io_bar_base(dev.bars[2], 0x170),
            ctrl: control_bar_base(dev.bars[3], 0x376),
        },
    )
}

fn io_bar_base(bar: pci::PciBar, default: u16) -> u16 {
    match bar.kind {
        pci::PciBarKind::Io => {
            if bar.addr == 0 {
                default
            } else {
                (bar.addr & 0xFFFC) as u16
            }
        }
        _ => default,
    }
}

fn control_bar_base(bar: pci::PciBar, default: u16) -> u16 {
    match bar.kind {
        pci::PciBarKind::Io => {
            if bar.addr == 0 {
                default
            } else {
                ((bar.addr & 0xFFFC) as u16).wrapping_add(2)
            }
        }
        _ => default,
    }
}

fn map_mmio_window(phys_addr: u64, size: usize) -> Option<*mut u8> {
    let window_index = unsafe {
        if AHCI_MMIO_WINDOW_NEXT >= AHCI_MMIO_WINDOW_LIMIT {
            return None;
        }
        let current = AHCI_MMIO_WINDOW_NEXT;
        AHCI_MMIO_WINDOW_NEXT += 1;
        current
    };

    let virt_base = AHCI_MMIO_WINDOW_BASE + (window_index as u64) * AHCI_MMIO_WINDOW_STRIDE;
    let phys_base = phys_addr & !(PAGE_SIZE - 1);
    let page_offset = (phys_addr - phys_base) as usize;
    let map_size = page_offset.saturating_add(size);
    let page_count = (map_size as u64 + PAGE_SIZE - 1) / PAGE_SIZE;

    for page in 0..page_count {
        let virt = VirtAddr::new(virt_base + page * PAGE_SIZE);
        let phys = PhysAddr::new(phys_base + page * PAGE_SIZE);
        if vmm::map_page(virt, phys, vmm::VMM_FLAG_KERNEL_DATA) != 0 {
            return None;
        }
    }

    Some((virt_base as usize + page_offset) as *mut u8)
}

fn mmio_read32(base: *mut u8, offset: usize) -> u32 {
    unsafe { core::ptr::read_volatile(base.add(offset) as *const u32) }
}

fn mmio_write32(base: *mut u8, offset: usize, value: u32) {
    unsafe {
        core::ptr::write_volatile(base.add(offset) as *mut u32, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ide_channel_defaults_when_bars_missing() {
        let dev = pci::DeviceInfo::EMPTY;
        let (primary, secondary) = ide_channels(&dev);

        assert_eq!(primary.cmd, 0x1F0);
        assert_eq!(primary.ctrl, 0x3F6);
        assert_eq!(secondary.cmd, 0x170);
        assert_eq!(secondary.ctrl, 0x376);
    }

    #[test]
    fn ide_channel_uses_io_bars_when_present() {
        let mut dev = pci::DeviceInfo::EMPTY;
        dev.bars[0] = pci::PciBar {
            kind: pci::PciBarKind::Io,
            addr: 0x1E8,
            prefetchable: false,
        };
        dev.bars[1] = pci::PciBar {
            kind: pci::PciBarKind::Io,
            addr: 0x3E4,
            prefetchable: false,
        };

        let (primary, _) = ide_channels(&dev);
        assert_eq!(primary.cmd, 0x1E8);
        assert_eq!(primary.ctrl, 0x3E6);
    }
}
