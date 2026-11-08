use core::{mem, ptr};

use arch_x86_64::addr::{VirtAddr, PAGE_SIZE};
use arch_x86_64::port::{inb, inw, outb};
use drivers::{ahci, pci, storage};

use crate::block;
use crate::memory::{heap, vmm};
use crate::mmio::{map_mmio_window, mmio_read32, mmio_write32};
use crate::serial_println;

const STORAGE_LOG_LIMIT: usize = 8;
const BLOCK_LOG_LIMIT: usize = 8;

const ATA_IDENTIFY_TIMEOUT: usize = 100_000;
const ATA_IDENTIFY_BYTES: usize = 512;
const ATA_WORD_COUNT: usize = 256;

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
const ATA_CMD_READ_DMA_EXT: u8 = 0x25;
const ATA_CMD_WRITE_DMA_EXT: u8 = 0x35;
const ATA_CMD_FLUSH_CACHE_EXT: u8 = 0xEA;

const ATA_DRIVE_MASTER: u8 = 0xA0;
const ATA_DRIVE_SLAVE: u8 = 0xB0;

const AHCI_MMIO_READ_SIZE: usize = 0x1200;

const AHCI_ENGINE_TIMEOUT: usize = 1_000_000;
const AHCI_CMD_TIMEOUT: usize = 1_000_000;

const AHCI_COMMAND_LIST_BYTES: usize = 1024;
const AHCI_RX_FIS_BYTES: usize = 256;
const AHCI_H2D_FIS_DWORDS: u16 = 5;
const AHCI_DMA_MAX_BYTES: usize = PAGE_SIZE as usize;

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

#[derive(Clone, Copy)]
struct DmaRegion {
    virt: *mut u8,
    phys: u64,
}

#[derive(Clone, Copy)]
struct AhciPortBuffers {
    cmd_list: DmaRegion,
    recv_fis: DmaRegion,
    cmd_table: DmaRegion,
    data: DmaRegion,
}

pub fn init(pci_devices: &[pci::DeviceInfo]) {
    block::reset();

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

    log_block_inventory();
    run_block_io_probe();
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

    let mut ghc = mmio_read32(base, ahci::AHCI_REG_GHC);
    if (ghc & ahci::AHCI_GHC_AE) == 0 {
        ghc |= ahci::AHCI_GHC_AE;
        mmio_write32(base, ahci::AHCI_REG_GHC, ghc);
    }

    let cap = mmio_read32(base, ahci::AHCI_REG_CAP);
    let pi = mmio_read32(base, ahci::AHCI_REG_PI);
    let vs = mmio_read32(base, ahci::AHCI_REG_VS);
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
        let ssts = mmio_read32(base, ahci::port_offset(*port, ahci::AHCI_PORT_SSTS));
        let sig = mmio_read32(base, ahci::port_offset(*port, ahci::AHCI_PORT_SIG));
        let device = ahci::classify_port_signature(sig);

        serial_println!(
            "[AHCI] port={} det={} ipm={} sig=0x{:08x} dev={:?}",
            *port,
            ahci::ssts_det(ssts),
            ahci::ssts_ipm(ssts),
            sig,
            device
        );

        if !ahci::port_has_active_device(ssts) || device != ahci::AhciPortDevice::Sata {
            continue;
        }

        let Some(info) = ahci_identify_dma(base, *port, caps.command_slot_count) else {
            serial_println!("[AHCI] port={} IDENTIFY DMA failed", *port);
            continue;
        };

        serial_println!(
            "[AHCI] port={} IDENTIFY sectors={} lba48={}",
            *port,
            info.sectors,
            info.lba48 as u8
        );

        register_block_device(
            block::BlockTransport::AhciDma,
            ctrl,
            *port,
            info,
            block::BlockOps {
                read: Some(ahci_block_read),
                write: Some(ahci_block_write),
                flush: Some(ahci_block_flush),
            },
            block::BlockOpContext {
                opaque0: base as u64,
                opaque1: *port as u64,
                opaque2: caps.command_slot_count as u64,
            },
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

    probe_ata_drive(ctrl, "pri-master", primary, false, 0);
    probe_ata_drive(ctrl, "pri-slave", primary, true, 1);
    probe_ata_drive(ctrl, "sec-master", secondary, false, 2);
    probe_ata_drive(ctrl, "sec-slave", secondary, true, 3);
}

fn probe_ata_drive(
    ctrl: &storage::StorageController,
    label: &str,
    channel: AtaChannel,
    slave: bool,
    target: u8,
) {
    let Some(info) = ata_identify(channel, slave) else {
        return;
    };

    serial_println!(
        "[ATA] {} present sectors={} lba48={}",
        label,
        info.sectors,
        info.lba48 as u8
    );

    register_block_device(
        block::BlockTransport::AtaPio,
        ctrl,
        target,
        info,
        block::BlockOps::NONE,
        block::BlockOpContext::EMPTY,
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

        let mut identify_words = [0u16; ATA_WORD_COUNT];
        for word in identify_words.iter_mut() {
            *word = inw(channel.cmd + ATA_REG_DATA);
        }

        Some(decode_identify_words(&identify_words))
    }
}

fn ahci_identify_dma(base: *mut u8, port: u8, max_slots: u8) -> Option<AtaIdentify> {
    let mut identify_data = [0u8; ATA_IDENTIFY_BYTES];
    if !ahci_exec_dma(
        base,
        port,
        max_slots,
        ATA_CMD_IDENTIFY,
        0,
        1,
        false,
        &mut identify_data,
    ) {
        return None;
    }

    let mut identify_words = [0u16; ATA_WORD_COUNT];
    unsafe {
        ptr::copy_nonoverlapping(
            identify_data.as_ptr() as *const u16,
            identify_words.as_mut_ptr(),
            ATA_WORD_COUNT,
        );
    }
    Some(decode_identify_words(&identify_words))
}

fn ahci_block_read(
    ctx: block::BlockOpContext,
    device: &block::BlockDevice,
    lba: u64,
    out: &mut [u8],
) -> Result<(), block::BlockIoError> {
    ahci_block_read_transfer(ctx, device, lba, out)
}

fn ahci_block_write(
    ctx: block::BlockOpContext,
    device: &block::BlockDevice,
    lba: u64,
    data: &[u8],
) -> Result<(), block::BlockIoError> {
    let mut bounce = [0u8; AHCI_DMA_MAX_BYTES];
    let size = validate_ahci_io(device, lba, data.len())?;
    bounce[..size].copy_from_slice(data);

    if !ahci_exec_dma(
        ctx.opaque0 as *mut u8,
        ctx.opaque1 as u8,
        ctx.opaque2 as u8,
        ATA_CMD_WRITE_DMA_EXT,
        lba,
        (size / ATA_IDENTIFY_BYTES) as u16,
        true,
        &mut bounce[..size],
    ) {
        return Err(block::BlockIoError::IoFault);
    }

    Ok(())
}

/// ATA FLUSH CACHE EXT: returns once the drive has written its volatile write
/// cache to media (with QEMU's default cache=writeback this reaches the host
/// file, matching what fsync would guarantee there).
fn ahci_block_flush(
    ctx: block::BlockOpContext,
    device: &block::BlockDevice,
) -> Result<(), block::BlockIoError> {
    if device.transport != block::BlockTransport::AhciDma {
        return Err(block::BlockIoError::Unsupported);
    }
    if !ahci_exec_nodata(
        ctx.opaque0 as *mut u8,
        ctx.opaque1 as u8,
        ctx.opaque2 as u8,
        ATA_CMD_FLUSH_CACHE_EXT,
    ) {
        return Err(block::BlockIoError::IoFault);
    }
    Ok(())
}

fn ahci_block_read_transfer(
    ctx: block::BlockOpContext,
    device: &block::BlockDevice,
    lba: u64,
    out: &mut [u8],
) -> Result<(), block::BlockIoError> {
    let mut bounce = [0u8; AHCI_DMA_MAX_BYTES];
    let size = validate_ahci_io(device, lba, out.len())?;

    if !ahci_exec_dma(
        ctx.opaque0 as *mut u8,
        ctx.opaque1 as u8,
        ctx.opaque2 as u8,
        ATA_CMD_READ_DMA_EXT,
        lba,
        (size / ATA_IDENTIFY_BYTES) as u16,
        false,
        &mut bounce[..size],
    ) {
        return Err(block::BlockIoError::IoFault);
    }

    out[..size].copy_from_slice(&bounce[..size]);
    Ok(())
}

fn validate_ahci_io(
    device: &block::BlockDevice,
    lba: u64,
    bytes: usize,
) -> Result<usize, block::BlockIoError> {
    if device.transport != block::BlockTransport::AhciDma {
        return Err(block::BlockIoError::Unsupported);
    }

    if bytes == 0 {
        return Err(block::BlockIoError::InvalidRequest);
    }

    if bytes > AHCI_DMA_MAX_BYTES {
        return Err(block::BlockIoError::InvalidRequest);
    }

    if (bytes % ATA_IDENTIFY_BYTES) != 0 {
        return Err(block::BlockIoError::UnalignedBuffer);
    }

    let sectors = (bytes / ATA_IDENTIFY_BYTES) as u64;
    if sectors == 0 || sectors > u16::MAX as u64 {
        return Err(block::BlockIoError::InvalidRequest);
    }

    let end = lba
        .checked_add(sectors)
        .ok_or(block::BlockIoError::OutOfRange)?;
    if end > device.sectors {
        return Err(block::BlockIoError::OutOfRange);
    }

    Ok(bytes)
}

fn ahci_exec_dma(
    base: *mut u8,
    port: u8,
    max_slots: u8,
    command: u8,
    lba: u64,
    sector_count: u16,
    write: bool,
    data: &mut [u8],
) -> bool {
    if base.is_null() || max_slots == 0 || sector_count == 0 {
        return false;
    }

    let expected = (sector_count as usize) * ATA_IDENTIFY_BYTES;
    if expected != data.len() || expected > AHCI_DMA_MAX_BYTES {
        return false;
    }

    let Some(buffers) = alloc_ahci_port_buffers(data.len()) else {
        return false;
    };

    if write {
        unsafe {
            ptr::copy_nonoverlapping(data.as_ptr(), buffers.data.virt, data.len());
        }
    } else {
        unsafe {
            ptr::write_bytes(buffers.data.virt, 0, data.len());
        }
    }

    let port_base = ahci::port_offset(port, 0);
    let mut success = false;

    if !ahci_stop_command_engine(base, port_base) {
        free_ahci_port_buffers(buffers);
        return false;
    }

    ahci_configure_port_dma(base, port_base, &buffers);
    ahci_start_command_engine(base, port_base);

    if ahci_wait_port_ready(base, port_base, AHCI_ENGINE_TIMEOUT) {
        let ci = mmio_read32(base, port_base + ahci::AHCI_PORT_CI);
        let sact = mmio_read32(base, port_base + ahci::AHCI_PORT_SACT);
        if let Some(slot) = ahci::find_free_command_slot(ci, sact, max_slots) {
            ahci_prepare_dma_command(
                &buffers,
                slot as usize,
                command,
                lba,
                sector_count,
                write,
                data.len(),
            );
            success = ahci_issue_slot(base, port_base, slot);
        }
    }

    let _ = ahci_stop_command_engine(base, port_base);

    if success && !write {
        unsafe {
            ptr::copy_nonoverlapping(buffers.data.virt, data.as_mut_ptr(), data.len());
        }
    }

    free_ahci_port_buffers(buffers);
    success
}

/// Issues a non-data ATA command (no PRDT entries) and waits for completion.
fn ahci_exec_nodata(base: *mut u8, port: u8, max_slots: u8, command: u8) -> bool {
    if base.is_null() || max_slots == 0 {
        return false;
    }
    let Some(buffers) = alloc_ahci_port_buffers(ATA_IDENTIFY_BYTES) else {
        return false;
    };

    let port_base = ahci::port_offset(port, 0);
    let mut success = false;
    if !ahci_stop_command_engine(base, port_base) {
        free_ahci_port_buffers(buffers);
        return false;
    }
    ahci_configure_port_dma(base, port_base, &buffers);
    ahci_start_command_engine(base, port_base);

    if ahci_wait_port_ready(base, port_base, AHCI_ENGINE_TIMEOUT) {
        let ci = mmio_read32(base, port_base + ahci::AHCI_PORT_CI);
        let sact = mmio_read32(base, port_base + ahci::AHCI_PORT_SACT);
        if let Some(slot) = ahci::find_free_command_slot(ci, sact, max_slots) {
            let headers = buffers.cmd_list.virt as *mut ahci::AhciCommandHeader;
            unsafe {
                let header = &mut *headers.add(slot as usize);
                *header = ahci::AhciCommandHeader::EMPTY;
                header.flags = AHCI_H2D_FIS_DWORDS;
                header.prdtl = 0;
                header.prdbc = 0;
                header.ctba = buffers.cmd_table.phys as u32;
                header.ctbau = (buffers.cmd_table.phys >> 32) as u32;
                let table = buffers.cmd_table.virt as *mut ahci::AhciCommandTable1;
                *table = ahci::AhciCommandTable1::EMPTY;
                fill_h2d_cfis(&mut (*table).cfis, command, 0, 0);
            }
            success = ahci_issue_slot(base, port_base, slot);
        }
    }

    let _ = ahci_stop_command_engine(base, port_base);
    free_ahci_port_buffers(buffers);
    success
}

fn ahci_stop_command_engine(base: *mut u8, port_base: usize) -> bool {
    let mut cmd = mmio_read32(base, port_base + ahci::AHCI_PORT_CMD);
    cmd &= !(ahci::AHCI_PORT_CMD_ST | ahci::AHCI_PORT_CMD_FRE);
    mmio_write32(base, port_base + ahci::AHCI_PORT_CMD, cmd);

    for _ in 0..AHCI_ENGINE_TIMEOUT {
        let running = mmio_read32(base, port_base + ahci::AHCI_PORT_CMD);
        if (running & (ahci::AHCI_PORT_CMD_CR | ahci::AHCI_PORT_CMD_FR)) == 0 {
            return true;
        }
    }

    false
}

fn ahci_start_command_engine(base: *mut u8, port_base: usize) {
    let mut cmd = mmio_read32(base, port_base + ahci::AHCI_PORT_CMD);
    cmd |= ahci::AHCI_PORT_CMD_FRE | ahci::AHCI_PORT_CMD_ST | ahci::AHCI_PORT_CMD_POD;
    mmio_write32(base, port_base + ahci::AHCI_PORT_CMD, cmd);
}

fn ahci_wait_port_ready(base: *mut u8, port_base: usize, timeout: usize) -> bool {
    for _ in 0..timeout {
        let tfd = (mmio_read32(base, port_base + ahci::AHCI_PORT_TFD) & 0xFF) as u8;
        if (tfd & (ATA_SR_BSY | ATA_SR_DRQ)) == 0 {
            return true;
        }
    }
    false
}

fn ahci_configure_port_dma(base: *mut u8, port_base: usize, buffers: &AhciPortBuffers) {
    mmio_write32(
        base,
        port_base + ahci::AHCI_PORT_CLB,
        buffers.cmd_list.phys as u32,
    );
    mmio_write32(
        base,
        port_base + ahci::AHCI_PORT_CLBU,
        (buffers.cmd_list.phys >> 32) as u32,
    );

    mmio_write32(
        base,
        port_base + ahci::AHCI_PORT_FB,
        buffers.recv_fis.phys as u32,
    );
    mmio_write32(
        base,
        port_base + ahci::AHCI_PORT_FBU,
        (buffers.recv_fis.phys >> 32) as u32,
    );

    mmio_write32(base, port_base + ahci::AHCI_PORT_IS, u32::MAX);
    mmio_write32(base, port_base + ahci::AHCI_PORT_SERR, u32::MAX);
    mmio_write32(base, port_base + ahci::AHCI_PORT_IE, 0);
}

fn fill_h2d_cfis(cfis: &mut [u8; 64], command: u8, lba: u64, sector_count: u16) {
    *cfis = [0; 64];
    cfis[0] = ahci::AHCI_FIS_TYPE_REG_H2D;
    cfis[1] = ahci::AHCI_FIS_H2D_COMMAND;
    cfis[2] = command;

    if command == ATA_CMD_IDENTIFY {
        return;
    }

    cfis[4] = (lba & 0xFF) as u8;
    cfis[5] = ((lba >> 8) & 0xFF) as u8;
    cfis[6] = ((lba >> 16) & 0xFF) as u8;
    cfis[7] = 1 << 6;
    cfis[8] = ((lba >> 24) & 0xFF) as u8;
    cfis[9] = ((lba >> 32) & 0xFF) as u8;
    cfis[10] = ((lba >> 40) & 0xFF) as u8;
    cfis[12] = (sector_count & 0xFF) as u8;
    cfis[13] = ((sector_count >> 8) & 0xFF) as u8;
}

fn ahci_prepare_dma_command(
    buffers: &AhciPortBuffers,
    slot: usize,
    command: u8,
    lba: u64,
    sector_count: u16,
    write: bool,
    data_bytes: usize,
) {
    let headers = buffers.cmd_list.virt as *mut ahci::AhciCommandHeader;

    unsafe {
        let header = &mut *headers.add(slot);
        *header = ahci::AhciCommandHeader::EMPTY;
        header.flags = AHCI_H2D_FIS_DWORDS | if write { ahci::AHCI_CMDH_WRITE } else { 0 };
        header.prdtl = 1;
        header.prdbc = 0;
        header.ctba = buffers.cmd_table.phys as u32;
        header.ctbau = (buffers.cmd_table.phys >> 32) as u32;

        let table = buffers.cmd_table.virt as *mut ahci::AhciCommandTable1;
        *table = ahci::AhciCommandTable1::EMPTY;
        fill_h2d_cfis(&mut (*table).cfis, command, lba, sector_count);
        (*table).prdt[0].dba = buffers.data.phys as u32;
        (*table).prdt[0].dbau = (buffers.data.phys >> 32) as u32;
        (*table).prdt[0].dbc_interrupt = (data_bytes as u32 - 1) | (1u32 << 31);
    }
}

fn ahci_issue_slot(base: *mut u8, port_base: usize, slot: u8) -> bool {
    if !ahci_wait_port_ready(base, port_base, AHCI_ENGINE_TIMEOUT) {
        return false;
    }

    let slot_mask = 1u32 << slot;
    mmio_write32(base, port_base + ahci::AHCI_PORT_IS, u32::MAX);
    let ci = mmio_read32(base, port_base + ahci::AHCI_PORT_CI);
    mmio_write32(base, port_base + ahci::AHCI_PORT_CI, ci | slot_mask);

    for _ in 0..AHCI_CMD_TIMEOUT {
        let is = mmio_read32(base, port_base + ahci::AHCI_PORT_IS);
        if (is & ahci::AHCI_PORT_IS_TFES) != 0 {
            return false;
        }

        if (mmio_read32(base, port_base + ahci::AHCI_PORT_CI) & slot_mask) == 0 {
            let tfd = (mmio_read32(base, port_base + ahci::AHCI_PORT_TFD) & 0xFF) as u8;
            return (tfd & ATA_SR_ERR) == 0;
        }
    }

    false
}

fn alloc_ahci_port_buffers(data_bytes: usize) -> Option<AhciPortBuffers> {
    let cmd_table_bytes = mem::size_of::<ahci::AhciCommandTable1>();

    if data_bytes == 0 || data_bytes > AHCI_DMA_MAX_BYTES {
        return None;
    }

    let cmd_list = alloc_dma_region(AHCI_COMMAND_LIST_BYTES, PAGE_SIZE as usize)?;
    let recv_fis = match alloc_dma_region(AHCI_RX_FIS_BYTES, PAGE_SIZE as usize) {
        Some(region) => region,
        None => {
            free_dma_region(cmd_list);
            return None;
        }
    };

    let cmd_table = match alloc_dma_region(cmd_table_bytes, PAGE_SIZE as usize) {
        Some(region) => region,
        None => {
            free_dma_region(recv_fis);
            free_dma_region(cmd_list);
            return None;
        }
    };

    let data = match alloc_dma_region(data_bytes, PAGE_SIZE as usize) {
        Some(region) => region,
        None => {
            free_dma_region(cmd_table);
            free_dma_region(recv_fis);
            free_dma_region(cmd_list);
            return None;
        }
    };

    Some(AhciPortBuffers {
        cmd_list,
        recv_fis,
        cmd_table,
        data,
    })
}

fn free_ahci_port_buffers(buffers: AhciPortBuffers) {
    free_dma_region(buffers.data);
    free_dma_region(buffers.cmd_table);
    free_dma_region(buffers.recv_fis);
    free_dma_region(buffers.cmd_list);
}

fn alloc_dma_region(size: usize, align: usize) -> Option<DmaRegion> {
    let ptr = heap::kmalloc_aligned(size, align);
    if ptr.is_null() {
        return None;
    }

    unsafe {
        ptr::write_bytes(ptr, 0, size);
    }

    let Some(phys) = dma_region_phys(ptr, size) else {
        heap::kfree(ptr);
        return None;
    };

    Some(DmaRegion { virt: ptr, phys })
}

fn free_dma_region(region: DmaRegion) {
    if !region.virt.is_null() {
        heap::kfree(region.virt);
    }
}

fn dma_region_phys(ptr: *mut u8, size: usize) -> Option<u64> {
    if ptr.is_null() || size == 0 {
        return None;
    }

    let start_virt = VirtAddr::new(ptr as u64);
    let end_virt = VirtAddr::new(ptr as u64 + (size as u64 - 1));

    let start_phys = vmm::get_physical(start_virt)?;
    let end_phys = vmm::get_physical(end_virt)?;

    if (start_phys.as_u64() & !(PAGE_SIZE - 1)) != (end_phys.as_u64() & !(PAGE_SIZE - 1)) {
        return None;
    }

    Some(start_phys.as_u64())
}

fn decode_identify_words(identify_words: &[u16; ATA_WORD_COUNT]) -> AtaIdentify {
    let lba48 = (identify_words[83] & (1 << 10)) != 0;
    let sectors28 = (identify_words[60] as u64) | ((identify_words[61] as u64) << 16);
    let sectors48 = (identify_words[100] as u64)
        | ((identify_words[101] as u64) << 16)
        | ((identify_words[102] as u64) << 32)
        | ((identify_words[103] as u64) << 48);

    AtaIdentify {
        sectors: if lba48 && sectors48 != 0 {
            sectors48
        } else {
            sectors28
        },
        lba48,
    }
}

fn register_block_device(
    transport: block::BlockTransport,
    ctrl: &storage::StorageController,
    target: u8,
    identify: AtaIdentify,
    ops: block::BlockOps,
    ctx: block::BlockOpContext,
) {
    let device = block::BlockDevice {
        transport,
        controller_bus: ctrl.bus,
        controller_device: ctrl.device,
        controller_function: ctrl.function,
        target,
        logical_block_size: ATA_IDENTIFY_BYTES as u32,
        sectors: identify.sectors,
        lba48: identify.lba48,
    };

    let registered = if transport == block::BlockTransport::AtaPio {
        block::register(device)
    } else {
        block::register_with_ops(device, ops, ctx)
    };

    if let Some(index) = registered {
        serial_println!(
            "[BLK] dev{} transport={} ctrl={:02X}:{:02X}.{} target={} sectors={} lba48={}",
            index,
            block::transport_name(transport),
            ctrl.bus,
            ctrl.device,
            ctrl.function,
            target,
            identify.sectors,
            identify.lba48 as u8
        );
    } else {
        serial_println!(
            "[BLK] registry full; dropping transport={} ctrl={:02X}:{:02X}.{} target={}",
            block::transport_name(transport),
            ctrl.bus,
            ctrl.device,
            ctrl.function,
            target
        );
    }
}

fn log_block_inventory() {
    let mut snapshot = [block::BlockDevice::EMPTY; BLOCK_LOG_LIMIT];
    let shown = block::snapshot(&mut snapshot);
    let inventory = block::inventory();

    serial_println!(
        "[BLK] Inventory total={} stored={} showing={}",
        inventory.total,
        inventory.stored,
        shown
    );

    for (index, dev) in snapshot.iter().take(shown).enumerate() {
        serial_println!(
            "[BLK] dev{} transport={} ctrl={:02X}:{:02X}.{} target={} lba={} sectors={} bytes={}MiB",
            index,
            block::transport_name(dev.transport),
            dev.controller_bus,
            dev.controller_device,
            dev.controller_function,
            dev.target,
            dev.logical_block_size,
            dev.sectors,
            (dev.sectors.saturating_mul(dev.logical_block_size as u64)) >> 20
        );
    }

    if inventory.overflow {
        serial_println!(
            "[BLK] {} additional block device(s) not shown",
            inventory.total.saturating_sub(inventory.stored)
        );
    }
}

fn run_block_io_probe() {
    let mut snapshot = [block::BlockDevice::EMPTY; BLOCK_LOG_LIMIT];
    let count = block::snapshot(&mut snapshot);

    for (index, dev) in snapshot.iter().take(count).enumerate() {
        if dev.transport != block::BlockTransport::AhciDma || dev.sectors == 0 {
            continue;
        }

        let mut sector0 = [0u8; ATA_IDENTIFY_BYTES];
        match block::read(index, 0, &mut sector0) {
            Ok(()) => {
                serial_println!(
                    "[BLK] dev{} read lba0 ok sig={:02x}{:02x}{:02x}{:02x}",
                    index,
                    sector0[0],
                    sector0[1],
                    sector0[2],
                    sector0[3]
                );
            }
            Err(err) => {
                serial_println!(
                    "[BLK] dev{} read lba0 failed err={}",
                    index,
                    block::io_error_name(err)
                );
            }
        }

        let _ = block::write(index, 0, &[]);
        // Exercise the write barrier the ext3 journal and sync/fsync rely on
        // (block-cache drain + ATA FLUSH CACHE EXT).
        match block::flush_device(index) {
            Ok(()) => serial_println!("[BLK] dev{} flush ok", index),
            Err(err) => serial_println!(
                "[BLK] dev{} flush failed err={}",
                index,
                block::io_error_name(err)
            ),
        }
        break;
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

    #[test]
    fn identify_decode_prefers_lba48_when_present() {
        let mut words = [0u16; ATA_WORD_COUNT];
        words[83] = 1 << 10;
        words[60] = 0xFFFF;
        words[61] = 0x0001;
        words[100] = 0x0004;
        words[101] = 0x0003;
        words[102] = 0x0002;
        words[103] = 0x0001;

        let decoded = decode_identify_words(&words);
        assert!(decoded.lba48);
        assert_eq!(decoded.sectors, 0x0001_0002_0003_0004);
    }

    #[test]
    fn identify_decode_falls_back_to_lba28() {
        let mut words = [0u16; ATA_WORD_COUNT];
        words[60] = 0x5678;
        words[61] = 0x1234;

        let decoded = decode_identify_words(&words);
        assert!(!decoded.lba48);
        assert_eq!(decoded.sectors, 0x1234_5678);
    }

    #[test]
    fn validate_ahci_io_rejects_bad_sizes() {
        let dev = block::BlockDevice {
            transport: block::BlockTransport::AhciDma,
            sectors: 4096,
            ..block::BlockDevice::EMPTY
        };

        assert_eq!(
            validate_ahci_io(&dev, 0, 513),
            Err(block::BlockIoError::UnalignedBuffer)
        );
        assert_eq!(
            validate_ahci_io(&dev, 0, AHCI_DMA_MAX_BYTES + ATA_IDENTIFY_BYTES),
            Err(block::BlockIoError::InvalidRequest)
        );
    }

    #[test]
    fn validate_ahci_io_checks_lba_bounds() {
        let dev = block::BlockDevice {
            transport: block::BlockTransport::AhciDma,
            sectors: 8,
            ..block::BlockDevice::EMPTY
        };

        assert_eq!(
            validate_ahci_io(&dev, 7, ATA_IDENTIFY_BYTES),
            Ok(ATA_IDENTIFY_BYTES)
        );
        assert_eq!(
            validate_ahci_io(&dev, 7, ATA_IDENTIFY_BYTES * 2),
            Err(block::BlockIoError::OutOfRange)
        );
    }

    #[test]
    fn h2d_cfis_encodes_lba48_fields() {
        let mut cfis = [0u8; 64];
        fill_h2d_cfis(&mut cfis, ATA_CMD_READ_DMA_EXT, 0x1122_3344_5566, 0x7788);

        assert_eq!(cfis[0], ahci::AHCI_FIS_TYPE_REG_H2D);
        assert_eq!(cfis[2], ATA_CMD_READ_DMA_EXT);
        assert_eq!(cfis[4], 0x66);
        assert_eq!(cfis[5], 0x55);
        assert_eq!(cfis[6], 0x44);
        assert_eq!(cfis[7], 0x40);
        assert_eq!(cfis[8], 0x33);
        assert_eq!(cfis[9], 0x22);
        assert_eq!(cfis[10], 0x11);
        assert_eq!(cfis[12], 0x88);
        assert_eq!(cfis[13], 0x77);
    }

    #[test]
    fn identify_cfis_keeps_lba_zeroed() {
        let mut cfis = [0xFFu8; 64];
        fill_h2d_cfis(&mut cfis, ATA_CMD_IDENTIFY, 0xFFFF_FFFF_FFFF, 0xFFFF);

        assert_eq!(cfis[2], ATA_CMD_IDENTIFY);
        assert_eq!(cfis[4], 0);
        assert_eq!(cfis[7], 0);
        assert_eq!(cfis[12], 0);
    }
}
