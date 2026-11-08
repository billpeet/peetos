use core::ptr;

use arch_x86_64::addr::PAGE_SIZE;
use arch_x86_64::{idt, irq};
use drivers::{e1000 as regs, pci};
use net::{MacAddr, NetError};

use crate::memory::{heap, vmm};
use crate::mmio::{map_mmio_window, mmio_read32, mmio_write32};
use crate::serial_println;

struct E1000Device {
    mmio_base: *mut u8,
    mac: MacAddr,

    // RX ring
    rx_descs: *mut regs::RxDesc,
    rx_descs_phys: u64,
    rx_buffers: [*mut u8; regs::NUM_RX_DESC],
    rx_buffers_phys: [u64; regs::NUM_RX_DESC],
    rx_cur: u32,

    // TX ring
    tx_descs: *mut regs::TxDesc,
    tx_descs_phys: u64,
    tx_buffers: [*mut u8; regs::NUM_TX_DESC],
    tx_buffers_phys: [u64; regs::NUM_TX_DESC],
    tx_cur: u32,

    // Link state
    link_up: bool,
    link_speed: u32,

    // IRQ
    irq: u8,

    initialized: bool,
}

static mut E1000_DEV: Option<E1000Device> = None;

// ---------------------------------------------------------------------------
// Register access helpers
// ---------------------------------------------------------------------------

fn read_reg(base: *mut u8, reg: usize) -> u32 {
    mmio_read32(base, reg)
}

fn write_reg(base: *mut u8, reg: usize, value: u32) {
    mmio_write32(base, reg, value);
    // Read back to ensure write completes
    let _ = mmio_read32(base, reg);
}

// ---------------------------------------------------------------------------
// EEPROM
// ---------------------------------------------------------------------------

fn eeprom_read(base: *mut u8, addr: u8) -> u16 {
    write_reg(
        base,
        regs::E1000_REG_EERD,
        regs::E1000_EERD_START | ((addr as u32) << regs::E1000_EERD_ADDR_SHIFT),
    );

    for _ in 0..regs::EEPROM_TIMEOUT {
        let val = read_reg(base, regs::E1000_REG_EERD);
        if (val & regs::E1000_EERD_DONE) != 0 {
            return (val >> regs::E1000_EERD_DATA_SHIFT) as u16;
        }
    }

    serial_println!("[E1000] EEPROM read timeout");
    0
}

fn read_mac_address(base: *mut u8) -> MacAddr {
    let w0 = eeprom_read(base, regs::E1000_EEPROM_MAC_ADDR);
    let w1 = eeprom_read(base, regs::E1000_EEPROM_MAC_ADDR + 1);
    let w2 = eeprom_read(base, regs::E1000_EEPROM_MAC_ADDR + 2);

    MacAddr::new([
        (w0 & 0xFF) as u8,
        ((w0 >> 8) & 0xFF) as u8,
        (w1 & 0xFF) as u8,
        ((w1 >> 8) & 0xFF) as u8,
        (w2 & 0xFF) as u8,
        ((w2 >> 8) & 0xFF) as u8,
    ])
}

// ---------------------------------------------------------------------------
// Device reset
// ---------------------------------------------------------------------------

fn device_reset(base: *mut u8) -> bool {
    let ctrl = read_reg(base, regs::E1000_REG_CTRL);
    write_reg(base, regs::E1000_REG_CTRL, ctrl | regs::E1000_CTRL_RST);

    for _ in 0..regs::RESET_TIMEOUT {
        if (read_reg(base, regs::E1000_REG_CTRL) & regs::E1000_CTRL_RST) == 0 {
            return true;
        }
    }

    serial_println!("[E1000] Reset timeout!");
    false
}

// ---------------------------------------------------------------------------
// Link status
// ---------------------------------------------------------------------------

fn update_link_status(dev: &mut E1000Device) {
    let status = read_reg(dev.mmio_base, regs::E1000_REG_STATUS);
    dev.link_up = (status & regs::E1000_STATUS_LU) != 0;
    dev.link_speed = match status & regs::E1000_STATUS_SPEED_MASK {
        regs::E1000_STATUS_SPEED_10 => 10,
        regs::E1000_STATUS_SPEED_100 => 100,
        regs::E1000_STATUS_SPEED_1000 => 1000,
        _ => 0,
    };

    if dev.link_up {
        serial_println!(
            "[E1000] Link: UP {} Mbps {}",
            dev.link_speed,
            if (status & regs::E1000_STATUS_FD) != 0 {
                "Full Duplex"
            } else {
                "Half Duplex"
            }
        );
    } else {
        serial_println!("[E1000] Link: DOWN");
    }
}

// ---------------------------------------------------------------------------
// DMA buffer allocation
// ---------------------------------------------------------------------------

fn alloc_buffer(size: usize, align: usize) -> Option<(*mut u8, u64)> {
    let ptr = heap::kmalloc_aligned(size, align);
    if ptr.is_null() {
        return None;
    }
    unsafe {
        ptr::write_bytes(ptr, 0, size);
    }
    let virt = arch_x86_64::addr::VirtAddr::new(ptr as u64);
    let phys = vmm::get_physical(virt)?;
    Some((ptr, phys.as_u64()))
}

// ---------------------------------------------------------------------------
// RX ring setup
// ---------------------------------------------------------------------------

fn init_rx(dev: &mut E1000Device) -> bool {
    let desc_ring_size = core::mem::size_of::<regs::RxDesc>() * regs::NUM_RX_DESC;

    let Some((desc_ptr, desc_phys)) = alloc_buffer(desc_ring_size, PAGE_SIZE as usize) else {
        serial_println!("[E1000] Failed to allocate RX descriptor ring");
        return false;
    };
    dev.rx_descs = desc_ptr as *mut regs::RxDesc;
    dev.rx_descs_phys = desc_phys;

    for i in 0..regs::NUM_RX_DESC {
        let Some((buf_ptr, buf_phys)) = alloc_buffer(regs::RX_BUFFER_SIZE, regs::BUFFER_ALIGN)
        else {
            serial_println!("[E1000] Failed to allocate RX buffer");
            return false;
        };
        dev.rx_buffers[i] = buf_ptr;
        dev.rx_buffers_phys[i] = buf_phys;

        unsafe {
            let desc = &mut *dev.rx_descs.add(i);
            desc.buffer_addr = buf_phys;
            desc.status = 0;
        }
    }

    // Program registers
    let base = dev.mmio_base;
    write_reg(
        base,
        regs::E1000_REG_RDBAL,
        (dev.rx_descs_phys & 0xFFFF_FFFF) as u32,
    );
    write_reg(
        base,
        regs::E1000_REG_RDBAH,
        (dev.rx_descs_phys >> 32) as u32,
    );
    write_reg(base, regs::E1000_REG_RDLEN, desc_ring_size as u32);
    write_reg(base, regs::E1000_REG_RDH, 0);
    write_reg(base, regs::E1000_REG_RDT, (regs::NUM_RX_DESC - 1) as u32);
    dev.rx_cur = 0;

    serial_println!(
        "[E1000] RX ring at 0x{:x}, {} descriptors",
        dev.rx_descs_phys,
        regs::NUM_RX_DESC
    );
    true
}

// ---------------------------------------------------------------------------
// TX ring setup
// ---------------------------------------------------------------------------

fn init_tx(dev: &mut E1000Device) -> bool {
    let desc_ring_size = core::mem::size_of::<regs::TxDesc>() * regs::NUM_TX_DESC;

    let Some((desc_ptr, desc_phys)) = alloc_buffer(desc_ring_size, PAGE_SIZE as usize) else {
        serial_println!("[E1000] Failed to allocate TX descriptor ring");
        return false;
    };
    dev.tx_descs = desc_ptr as *mut regs::TxDesc;
    dev.tx_descs_phys = desc_phys;

    for i in 0..regs::NUM_TX_DESC {
        let Some((buf_ptr, buf_phys)) = alloc_buffer(regs::TX_BUFFER_SIZE, regs::BUFFER_ALIGN)
        else {
            serial_println!("[E1000] Failed to allocate TX buffer");
            return false;
        };
        dev.tx_buffers[i] = buf_ptr;
        dev.tx_buffers_phys[i] = buf_phys;

        unsafe {
            let desc = &mut *dev.tx_descs.add(i);
            desc.buffer_addr = buf_phys;
            desc.status = regs::E1000_TXD_STAT_DD; // Mark as done (available)
            desc.cmd = 0;
        }
    }

    // Program registers
    let base = dev.mmio_base;
    write_reg(
        base,
        regs::E1000_REG_TDBAL,
        (dev.tx_descs_phys & 0xFFFF_FFFF) as u32,
    );
    write_reg(
        base,
        regs::E1000_REG_TDBAH,
        (dev.tx_descs_phys >> 32) as u32,
    );
    write_reg(base, regs::E1000_REG_TDLEN, desc_ring_size as u32);
    write_reg(base, regs::E1000_REG_TDH, 0);
    write_reg(base, regs::E1000_REG_TDT, 0);
    dev.tx_cur = 0;

    serial_println!(
        "[E1000] TX ring at 0x{:x}, {} descriptors",
        dev.tx_descs_phys,
        regs::NUM_TX_DESC
    );
    true
}

// ---------------------------------------------------------------------------
// RX / TX configuration
// ---------------------------------------------------------------------------

fn configure_rx(dev: &E1000Device) {
    let base = dev.mmio_base;

    // Set MAC address in receive address registers
    let ral = (dev.mac.0[0] as u32)
        | ((dev.mac.0[1] as u32) << 8)
        | ((dev.mac.0[2] as u32) << 16)
        | ((dev.mac.0[3] as u32) << 24);
    let rah = (dev.mac.0[4] as u32) | ((dev.mac.0[5] as u32) << 8) | regs::E1000_RAH_AV;

    write_reg(base, regs::E1000_REG_RAL, ral);
    write_reg(base, regs::E1000_REG_RAH, rah);

    // Clear multicast table
    for i in 0..128 {
        write_reg(base, regs::E1000_REG_MTA + (i * 4), 0);
    }

    // Enable receiver: accept broadcast, 2048-byte buffers, strip CRC
    let rctl = regs::E1000_RCTL_EN
        | regs::E1000_RCTL_BAM
        | regs::E1000_RCTL_BSIZE_2048
        | regs::E1000_RCTL_SECRC;
    write_reg(base, regs::E1000_REG_RCTL, rctl);
}

fn configure_tx(dev: &E1000Device) {
    let base = dev.mmio_base;

    // Set inter-packet gap
    let tipg = (regs::E1000_TIPG_IPGT_DEFAULT << regs::E1000_TIPG_IPGT_SHIFT)
        | (regs::E1000_TIPG_IPGR1_DEFAULT << regs::E1000_TIPG_IPGR1_SHIFT)
        | (regs::E1000_TIPG_IPGR2_DEFAULT << regs::E1000_TIPG_IPGR2_SHIFT);
    write_reg(base, regs::E1000_REG_TIPG, tipg);

    // Enable transmitter: pad short packets, collision threshold, full-duplex distance
    let tctl = regs::E1000_TCTL_EN
        | regs::E1000_TCTL_PSP
        | regs::E1000_TCTL_CT_DEFAULT
        | regs::E1000_TCTL_COLD_FD;
    write_reg(base, regs::E1000_REG_TCTL, tctl);
}

// ---------------------------------------------------------------------------
// Interrupts
// ---------------------------------------------------------------------------

fn configure_interrupts(dev: &E1000Device) {
    let base = dev.mmio_base;

    // Clear pending interrupts
    let _ = read_reg(base, regs::E1000_REG_ICR);

    // Enable RX timer, RX overrun, RX desc min threshold, link status change, TX done
    write_reg(
        base,
        regs::E1000_REG_IMS,
        regs::E1000_ICR_RXT0
            | regs::E1000_ICR_RXO
            | regs::E1000_ICR_RXDMT0
            | regs::E1000_ICR_LSC
            | regs::E1000_ICR_TXDW,
    );
}

extern "C" fn e1000_irq_handler(_frame: &mut idt::InterruptFrame) {
    let dev = unsafe {
        match E1000_DEV.as_mut() {
            Some(d) if d.initialized => d,
            _ => return,
        }
    };

    let icr = read_reg(dev.mmio_base, regs::E1000_REG_ICR);

    if (icr & regs::E1000_ICR_LSC) != 0 {
        update_link_status(dev);
    }

    // RXT0 / RXDMT0 — packets will be drained by poll()
    // RXO — overrun
    if (icr & regs::E1000_ICR_RXO) != 0 {
        serial_println!("[E1000] RX overrun!");
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub fn init(pci_devices: &[pci::DeviceInfo]) {
    // Scan for E1000 PCI devices
    let mut found: Option<&pci::DeviceInfo> = None;
    for dev in pci_devices {
        if dev.vendor_id == regs::E1000_VENDOR_ID
            && dev.class_code == regs::E1000_PCI_CLASS
            && dev.subclass == regs::E1000_PCI_SUBCLASS
            && (dev.device_id == regs::E1000_DEVICE_ID_82540EM
                || dev.device_id == regs::E1000_DEVICE_ID_82545EM)
        {
            found = Some(dev);
            break;
        }
    }

    let Some(pci_dev) = found else {
        serial_println!("[E1000] No device detected");
        return;
    };

    serial_println!(
        "[E1000] Detected {:02X}:{:02X}.{} vendor={:04X} device={:04X} irq={}",
        pci_dev.bus,
        pci_dev.device,
        pci_dev.function,
        pci_dev.vendor_id,
        pci_dev.device_id,
        pci_dev.irq_line
    );

    // Enable bus-mastering and memory-space in PCI command register
    let _ = pci::enable_mem_and_bus_master(pci_dev.bus, pci_dev.device, pci_dev.function);

    // Map BAR0 MMIO window
    let bar0 = &pci_dev.bars[0];
    if !matches!(
        bar0.kind,
        pci::PciBarKind::Memory32 | pci::PciBarKind::Memory64
    ) || bar0.addr == 0
    {
        serial_println!("[E1000] BAR0 not a valid memory BAR");
        return;
    }

    let Some(mmio_base) = map_mmio_window(bar0.addr, regs::MMIO_SIZE) else {
        serial_println!("[E1000] Failed to map MMIO region at 0x{:x}", bar0.addr);
        return;
    };

    serial_println!("[E1000] MMIO mapped at 0x{:x}", bar0.addr);

    // Reset device
    if !device_reset(mmio_base) {
        return;
    }
    serial_println!("[E1000] Reset complete");

    // Read MAC address from EEPROM
    let mac = read_mac_address(mmio_base);
    serial_println!(
        "[E1000] MAC address: {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        mac.0[0],
        mac.0[1],
        mac.0[2],
        mac.0[3],
        mac.0[4],
        mac.0[5]
    );

    // Initialize device state
    let dev = unsafe {
        E1000_DEV = Some(E1000Device {
            mmio_base,
            mac,
            rx_descs: ptr::null_mut(),
            rx_descs_phys: 0,
            rx_buffers: [ptr::null_mut(); regs::NUM_RX_DESC],
            rx_buffers_phys: [0; regs::NUM_RX_DESC],
            rx_cur: 0,
            tx_descs: ptr::null_mut(),
            tx_descs_phys: 0,
            tx_buffers: [ptr::null_mut(); regs::NUM_TX_DESC],
            tx_buffers_phys: [0; regs::NUM_TX_DESC],
            tx_cur: 0,
            link_up: false,
            link_speed: 0,
            irq: pci_dev.irq_line,
            initialized: false,
        });
        E1000_DEV.as_mut().unwrap()
    };

    // Initialize descriptor rings
    if !init_rx(dev) {
        return;
    }
    if !init_tx(dev) {
        return;
    }

    // Configure RX/TX
    configure_rx(dev);
    configure_tx(dev);

    // Set link up
    let ctrl = read_reg(mmio_base, regs::E1000_REG_CTRL);
    write_reg(mmio_base, regs::E1000_REG_CTRL, ctrl | regs::E1000_CTRL_SLU);

    // Check link status
    update_link_status(dev);

    // Configure and enable interrupts
    configure_interrupts(dev);

    // Register IRQ handler
    if dev.irq > 0 && dev.irq < 16 {
        idt::register_handler(irq::irq_to_vector(dev.irq), e1000_irq_handler);
        irq::unmask(dev.irq);
        serial_println!("[E1000] Registered IRQ handler for IRQ {}", dev.irq);
    }

    dev.initialized = true;
    serial_println!("[E1000] Device initialized successfully");
}

pub fn send_packet(data: &[u8]) -> Result<(), NetError> {
    let dev = unsafe {
        match E1000_DEV.as_mut() {
            Some(d) if d.initialized => d,
            _ => return Err(NetError::NotInitialized),
        }
    };

    if data.len() > regs::MAX_PACKET_SIZE {
        return Err(NetError::BufferTooLarge);
    }

    let cur = dev.tx_cur as usize;
    let desc = unsafe { &mut *dev.tx_descs.add(cur) };

    // Wait for descriptor to become available
    let mut timeout = regs::TX_TIMEOUT;
    while (desc.status & regs::E1000_TXD_STAT_DD) == 0 && timeout > 0 {
        timeout -= 1;
    }
    if timeout == 0 {
        return Err(NetError::TransmitFailed);
    }

    // Copy data to TX buffer
    unsafe {
        ptr::copy_nonoverlapping(data.as_ptr(), dev.tx_buffers[cur], data.len());
    }

    // Set up descriptor
    desc.length = data.len() as u16;
    desc.cmd = regs::E1000_TXD_CMD_EOP | regs::E1000_TXD_CMD_IFCS | regs::E1000_TXD_CMD_RS;
    desc.status = 0;

    // Advance tail
    dev.tx_cur = ((cur + 1) % regs::NUM_TX_DESC) as u32;
    write_reg(dev.mmio_base, regs::E1000_REG_TDT, dev.tx_cur);

    Ok(())
}

pub fn receive_packet(buf: &mut [u8]) -> Option<usize> {
    let dev = unsafe {
        match E1000_DEV.as_mut() {
            Some(d) if d.initialized => d,
            _ => return None,
        }
    };

    let cur = dev.rx_cur as usize;
    let desc = unsafe { &mut *dev.rx_descs.add(cur) };

    // Check if packet available
    if (desc.status & regs::E1000_RXD_STAT_DD) == 0 {
        return None;
    }

    let length = desc.length as usize;
    let copy_len = length.min(buf.len());

    // Copy data from RX buffer
    unsafe {
        ptr::copy_nonoverlapping(dev.rx_buffers[cur], buf.as_mut_ptr(), copy_len);
    }

    // Clear descriptor and return to hardware
    desc.status = 0;
    let old_cur = cur;
    dev.rx_cur = ((cur + 1) % regs::NUM_RX_DESC) as u32;
    write_reg(dev.mmio_base, regs::E1000_REG_RDT, old_cur as u32);

    Some(copy_len)
}

pub fn packet_available() -> bool {
    let dev = unsafe {
        match E1000_DEV.as_ref() {
            Some(d) if d.initialized => d,
            _ => return false,
        }
    };

    let desc = unsafe { &*dev.rx_descs.add(dev.rx_cur as usize) };
    (desc.status & regs::E1000_RXD_STAT_DD) != 0
}

pub fn get_mac() -> MacAddr {
    unsafe {
        match E1000_DEV.as_ref() {
            Some(d) if d.initialized => d.mac,
            _ => MacAddr::ZERO,
        }
    }
}

pub fn link_up() -> bool {
    unsafe {
        match E1000_DEV.as_ref() {
            Some(d) if d.initialized => d.link_up,
            _ => false,
        }
    }
}

pub fn is_initialized() -> bool {
    unsafe {
        match E1000_DEV.as_ref() {
            Some(d) => d.initialized,
            None => false,
        }
    }
}

/// Poll for and process incoming packets. Hands received frames to ethernet::receive().
pub fn poll() {
    static mut RX_BUF: [u8; regs::MAX_PACKET_SIZE + 4] = [0; regs::MAX_PACKET_SIZE + 4];

    while packet_available() {
        let len = unsafe { receive_packet(&mut RX_BUF) };
        if let Some(n) = len {
            unsafe {
                crate::ethernet::receive(&RX_BUF[..n]);
            }
        }
    }
}
