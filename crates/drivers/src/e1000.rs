#![allow(dead_code)]

// PCI identification
pub const E1000_VENDOR_ID: u16 = 0x8086;
pub const E1000_DEVICE_ID_82540EM: u16 = 0x100E;
pub const E1000_DEVICE_ID_82545EM: u16 = 0x100F;
pub const E1000_PCI_CLASS: u8 = 0x02;
pub const E1000_PCI_SUBCLASS: u8 = 0x00;

// Register offsets (all 32-bit, accessed via MMIO at BAR0 + offset)
pub const E1000_REG_CTRL: usize = 0x0000;
pub const E1000_REG_STATUS: usize = 0x0008;
pub const E1000_REG_EERD: usize = 0x0014;
pub const E1000_REG_ICR: usize = 0x00C0;
pub const E1000_REG_ITR: usize = 0x00C4;
pub const E1000_REG_IMS: usize = 0x00D0;
pub const E1000_REG_IMC: usize = 0x00D8;
pub const E1000_REG_RCTL: usize = 0x0100;
pub const E1000_REG_TCTL: usize = 0x0400;
pub const E1000_REG_TIPG: usize = 0x0410;

// Receive descriptor ring registers
pub const E1000_REG_RDBAL: usize = 0x2800;
pub const E1000_REG_RDBAH: usize = 0x2804;
pub const E1000_REG_RDLEN: usize = 0x2808;
pub const E1000_REG_RDH: usize = 0x2810;
pub const E1000_REG_RDT: usize = 0x2818;

// Transmit descriptor ring registers
pub const E1000_REG_TDBAL: usize = 0x3800;
pub const E1000_REG_TDBAH: usize = 0x3804;
pub const E1000_REG_TDLEN: usize = 0x3808;
pub const E1000_REG_TDH: usize = 0x3810;
pub const E1000_REG_TDT: usize = 0x3818;

// Receive address (MAC) registers
pub const E1000_REG_RAL: usize = 0x5400;
pub const E1000_REG_RAH: usize = 0x5404;

// Multicast table array
pub const E1000_REG_MTA: usize = 0x5200;

// CTRL register bits
pub const E1000_CTRL_FD: u32 = 1 << 0;
pub const E1000_CTRL_ASDE: u32 = 1 << 5;
pub const E1000_CTRL_SLU: u32 = 1 << 6;
pub const E1000_CTRL_RST: u32 = 1 << 26;

// STATUS register bits
pub const E1000_STATUS_FD: u32 = 1 << 0;
pub const E1000_STATUS_LU: u32 = 1 << 1;
pub const E1000_STATUS_SPEED_MASK: u32 = 3 << 6;
pub const E1000_STATUS_SPEED_10: u32 = 0 << 6;
pub const E1000_STATUS_SPEED_100: u32 = 1 << 6;
pub const E1000_STATUS_SPEED_1000: u32 = 2 << 6;

// EEPROM read register bits
pub const E1000_EERD_START: u32 = 1 << 0;
pub const E1000_EERD_DONE: u32 = 1 << 4;
pub const E1000_EERD_ADDR_SHIFT: u32 = 8;
pub const E1000_EERD_DATA_SHIFT: u32 = 16;
pub const E1000_EEPROM_MAC_ADDR: u8 = 0x00;

// Interrupt cause register bits (ICR/IMS/IMC)
pub const E1000_ICR_TXDW: u32 = 1 << 0;
pub const E1000_ICR_LSC: u32 = 1 << 2;
pub const E1000_ICR_RXDMT0: u32 = 1 << 4;
pub const E1000_ICR_RXO: u32 = 1 << 6;
pub const E1000_ICR_RXT0: u32 = 1 << 7;

// Receive control register bits (RCTL)
pub const E1000_RCTL_EN: u32 = 1 << 1;
pub const E1000_RCTL_BAM: u32 = 1 << 15;
pub const E1000_RCTL_BSIZE_2048: u32 = 0 << 16;
pub const E1000_RCTL_SECRC: u32 = 1 << 26;

// Transmit control register bits (TCTL)
pub const E1000_TCTL_EN: u32 = 1 << 1;
pub const E1000_TCTL_PSP: u32 = 1 << 3;
pub const E1000_TCTL_CT_DEFAULT: u32 = 0x0F << 4;
pub const E1000_TCTL_COLD_FD: u32 = 0x40 << 12;

// Transmit IPG defaults (IEEE 802.3 standard for 1000BASE-T)
pub const E1000_TIPG_IPGT_DEFAULT: u32 = 10;
pub const E1000_TIPG_IPGR1_DEFAULT: u32 = 8;
pub const E1000_TIPG_IPGR2_DEFAULT: u32 = 6;
pub const E1000_TIPG_IPGT_SHIFT: u32 = 0;
pub const E1000_TIPG_IPGR1_SHIFT: u32 = 10;
pub const E1000_TIPG_IPGR2_SHIFT: u32 = 20;

// RAH register bits
pub const E1000_RAH_AV: u32 = 1 << 31;

// Receive descriptor status bits
pub const E1000_RXD_STAT_DD: u8 = 1 << 0;
pub const E1000_RXD_STAT_EOP: u8 = 1 << 1;

// Transmit descriptor command bits
pub const E1000_TXD_CMD_EOP: u8 = 1 << 0;
pub const E1000_TXD_CMD_IFCS: u8 = 1 << 1;
pub const E1000_TXD_CMD_RS: u8 = 1 << 3;

// Transmit descriptor status bits
pub const E1000_TXD_STAT_DD: u8 = 1 << 0;

// Ring sizes and buffer sizes
pub const NUM_RX_DESC: usize = 256;
pub const NUM_TX_DESC: usize = 256;
pub const RX_BUFFER_SIZE: usize = 2048;
pub const TX_BUFFER_SIZE: usize = 2048;
pub const MAX_PACKET_SIZE: usize = 1518;

// Alignment requirements
pub const DESC_ALIGN: usize = 16;
pub const BUFFER_ALIGN: usize = 16;

// Timeouts
pub const RESET_TIMEOUT: usize = 10_000;
pub const EEPROM_TIMEOUT: usize = 10_000;
pub const TX_TIMEOUT: usize = 10_000;

// MMIO window size needed for E1000 registers
pub const MMIO_SIZE: usize = 0x20000;

/// Receive descriptor (legacy format, 16 bytes).
#[derive(Clone, Copy)]
#[repr(C, packed)]
pub struct RxDesc {
    pub buffer_addr: u64,
    pub length: u16,
    pub checksum: u16,
    pub status: u8,
    pub errors: u8,
    pub special: u16,
}

impl RxDesc {
    pub const EMPTY: Self = Self {
        buffer_addr: 0,
        length: 0,
        checksum: 0,
        status: 0,
        errors: 0,
        special: 0,
    };
}

/// Transmit descriptor (legacy format, 16 bytes).
#[derive(Clone, Copy)]
#[repr(C, packed)]
pub struct TxDesc {
    pub buffer_addr: u64,
    pub length: u16,
    pub cso: u8,
    pub cmd: u8,
    pub status: u8,
    pub css: u8,
    pub special: u16,
}

impl TxDesc {
    pub const EMPTY: Self = Self {
        buffer_addr: 0,
        length: 0,
        cso: 0,
        cmd: 0,
        status: 0,
        css: 0,
        special: 0,
    };
}
