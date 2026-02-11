use core::cell::UnsafeCell;

pub const MAX_BLOCK_DEVICES: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum BlockTransport {
    AtaPio = 0,
    AhciDma = 1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockDevice {
    pub transport: BlockTransport,
    pub controller_bus: u8,
    pub controller_device: u8,
    pub controller_function: u8,
    pub target: u8,
    pub logical_block_size: u32,
    pub sectors: u64,
    pub lba48: bool,
}

impl BlockDevice {
    pub const EMPTY: Self = Self {
        transport: BlockTransport::AtaPio,
        controller_bus: 0,
        controller_device: 0,
        controller_function: 0,
        target: 0,
        logical_block_size: 512,
        sectors: 0,
        lba48: false,
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockInventory {
    pub total: usize,
    pub stored: usize,
    pub overflow: bool,
}

impl BlockInventory {
    pub const EMPTY: Self = Self {
        total: 0,
        stored: 0,
        overflow: false,
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum BlockIoError {
    InvalidDevice = 0,
    Unsupported = 1,
    UnalignedBuffer = 2,
    OutOfRange = 3,
    InvalidRequest = 4,
    IoFault = 5,
}

pub const fn io_error_name(error: BlockIoError) -> &'static str {
    match error {
        BlockIoError::InvalidDevice => "invalid-device",
        BlockIoError::Unsupported => "unsupported",
        BlockIoError::UnalignedBuffer => "unaligned-buffer",
        BlockIoError::OutOfRange => "out-of-range",
        BlockIoError::InvalidRequest => "invalid-request",
        BlockIoError::IoFault => "io-fault",
    }
}

pub type BlockReadOp =
    fn(BlockOpContext, &BlockDevice, lba: u64, out: &mut [u8]) -> Result<(), BlockIoError>;
pub type BlockWriteOp =
    fn(BlockOpContext, &BlockDevice, lba: u64, data: &[u8]) -> Result<(), BlockIoError>;

#[derive(Clone, Copy)]
pub struct BlockOps {
    pub read: Option<BlockReadOp>,
    pub write: Option<BlockWriteOp>,
}

impl BlockOps {
    pub const NONE: Self = Self {
        read: None,
        write: None,
    };
}

#[derive(Clone, Copy, Default)]
pub struct BlockOpContext {
    pub opaque0: u64,
    pub opaque1: u64,
    pub opaque2: u64,
}

impl BlockOpContext {
    pub const EMPTY: Self = Self {
        opaque0: 0,
        opaque1: 0,
        opaque2: 0,
    };
}

#[derive(Clone, Copy)]
struct BlockEntry {
    device: BlockDevice,
    ops: BlockOps,
    ctx: BlockOpContext,
}

impl BlockEntry {
    const EMPTY: Self = Self {
        device: BlockDevice::EMPTY,
        ops: BlockOps::NONE,
        ctx: BlockOpContext::EMPTY,
    };
}

pub struct BlockRegistry {
    entries: [BlockEntry; MAX_BLOCK_DEVICES],
    summary: BlockInventory,
}

impl BlockRegistry {
    pub const fn new() -> Self {
        Self {
            entries: [BlockEntry::EMPTY; MAX_BLOCK_DEVICES],
            summary: BlockInventory::EMPTY,
        }
    }

    pub fn clear(&mut self) {
        self.entries = [BlockEntry::EMPTY; MAX_BLOCK_DEVICES];
        self.summary = BlockInventory::EMPTY;
    }

    pub fn register(
        &mut self,
        device: BlockDevice,
        ops: BlockOps,
        ctx: BlockOpContext,
    ) -> Option<usize> {
        self.summary.total += 1;

        if self.summary.stored < self.entries.len() {
            let index = self.summary.stored;
            self.entries[index] = BlockEntry { device, ops, ctx };
            self.summary.stored += 1;
            Some(index)
        } else {
            self.summary.overflow = true;
            None
        }
    }

    pub fn snapshot(&self, out: &mut [BlockDevice]) -> usize {
        let count = core::cmp::min(out.len(), self.summary.stored);
        for (dst, entry) in out[..count].iter_mut().zip(self.entries[..count].iter()) {
            *dst = entry.device;
        }
        count
    }

    fn get(&self, index: usize) -> Option<BlockEntry> {
        if index >= self.summary.stored {
            return None;
        }
        Some(self.entries[index])
    }

    pub const fn summary(&self) -> BlockInventory {
        self.summary
    }
}

struct RegistryCell(UnsafeCell<BlockRegistry>);

unsafe impl Sync for RegistryCell {}

static BLOCK_REGISTRY: RegistryCell = RegistryCell(UnsafeCell::new(BlockRegistry::new()));

fn with_registry_mut<R>(f: impl FnOnce(&mut BlockRegistry) -> R) -> R {
    unsafe { f(&mut *BLOCK_REGISTRY.0.get()) }
}

fn with_registry<R>(f: impl FnOnce(&BlockRegistry) -> R) -> R {
    unsafe { f(&*BLOCK_REGISTRY.0.get()) }
}

pub const fn transport_name(transport: BlockTransport) -> &'static str {
    match transport {
        BlockTransport::AtaPio => "ata-pio",
        BlockTransport::AhciDma => "ahci-dma",
    }
}

pub fn reset() {
    with_registry_mut(|registry| registry.clear());
}

pub fn register(device: BlockDevice) -> Option<usize> {
    register_with_ops(device, BlockOps::NONE, BlockOpContext::EMPTY)
}

pub fn register_with_ops(device: BlockDevice, ops: BlockOps, ctx: BlockOpContext) -> Option<usize> {
    with_registry_mut(|registry| registry.register(device, ops, ctx))
}

pub fn snapshot(out: &mut [BlockDevice]) -> usize {
    with_registry(|registry| registry.snapshot(out))
}

pub fn inventory() -> BlockInventory {
    with_registry(|registry| registry.summary())
}

pub fn read(index: usize, lba: u64, out: &mut [u8]) -> Result<(), BlockIoError> {
    if out.is_empty() {
        return Ok(());
    }

    with_registry(|registry| {
        let Some(entry) = registry.get(index) else {
            return Err(BlockIoError::InvalidDevice);
        };
        let Some(read_op) = entry.ops.read else {
            return Err(BlockIoError::Unsupported);
        };
        read_op(entry.ctx, &entry.device, lba, out)
    })
}

pub fn write(index: usize, lba: u64, data: &[u8]) -> Result<(), BlockIoError> {
    if data.is_empty() {
        return Ok(());
    }

    with_registry(|registry| {
        let Some(entry) = registry.get(index) else {
            return Err(BlockIoError::InvalidDevice);
        };
        let Some(write_op) = entry.ops.write else {
            return Err(BlockIoError::Unsupported);
        };
        write_op(entry.ctx, &entry.device, lba, data)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};
    use std::sync::atomic::{AtomicU64, Ordering};

    static LAST_READ_CTX: AtomicU64 = AtomicU64::new(0);
    static LAST_WRITE_SUM: AtomicU64 = AtomicU64::new(0);

    fn global_registry_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn test_read(
        ctx: BlockOpContext,
        _dev: &BlockDevice,
        lba: u64,
        out: &mut [u8],
    ) -> Result<(), BlockIoError> {
        LAST_READ_CTX.store(ctx.opaque0 ^ lba, Ordering::Relaxed);
        for byte in out.iter_mut() {
            *byte = 0xA5;
        }
        Ok(())
    }

    fn test_write(
        _ctx: BlockOpContext,
        _dev: &BlockDevice,
        _lba: u64,
        data: &[u8],
    ) -> Result<(), BlockIoError> {
        let mut sum = 0u64;
        for byte in data {
            sum = sum.wrapping_add(*byte as u64);
        }
        LAST_WRITE_SUM.store(sum, Ordering::Relaxed);
        Ok(())
    }

    #[test]
    fn registry_tracks_counts_and_overflow() {
        let mut reg = BlockRegistry::new();

        for i in 0..MAX_BLOCK_DEVICES {
            let result = reg.register(
                BlockDevice {
                    target: i as u8,
                    ..BlockDevice::EMPTY
                },
                BlockOps::NONE,
                BlockOpContext::EMPTY,
            );
            assert_eq!(result, Some(i));
        }

        assert_eq!(reg.summary().stored, MAX_BLOCK_DEVICES);
        assert_eq!(reg.summary().total, MAX_BLOCK_DEVICES);
        assert!(!reg.summary().overflow);

        let overflow = reg.register(
            BlockDevice {
                target: 99,
                ..BlockDevice::EMPTY
            },
            BlockOps::NONE,
            BlockOpContext::EMPTY,
        );
        assert_eq!(overflow, None);
        assert_eq!(reg.summary().total, MAX_BLOCK_DEVICES + 1);
        assert!(reg.summary().overflow);
    }

    #[test]
    fn snapshot_returns_dense_list() {
        let mut reg = BlockRegistry::new();
        reg.register(
            BlockDevice {
                target: 7,
                ..BlockDevice::EMPTY
            },
            BlockOps::NONE,
            BlockOpContext::EMPTY,
        );
        reg.register(
            BlockDevice {
                target: 8,
                ..BlockDevice::EMPTY
            },
            BlockOps::NONE,
            BlockOpContext::EMPTY,
        );

        let mut out = [BlockDevice::EMPTY; 1];
        let copied = reg.snapshot(&mut out);

        assert_eq!(copied, 1);
        assert_eq!(out[0].target, 7);
    }

    #[test]
    fn registry_dispatches_read_and_write_ops() {
        let _guard = global_registry_lock().lock().unwrap();
        reset();
        let index = register_with_ops(
            BlockDevice {
                sectors: 1024,
                ..BlockDevice::EMPTY
            },
            BlockOps {
                read: Some(test_read),
                write: Some(test_write),
            },
            BlockOpContext {
                opaque0: 0x55AA,
                ..BlockOpContext::EMPTY
            },
        )
        .unwrap();

        let mut read_buf = [0u8; 8];
        read(index, 2, &mut read_buf).unwrap();
        assert_eq!(read_buf, [0xA5; 8]);
        assert_eq!(LAST_READ_CTX.load(Ordering::Relaxed), 0x55AA ^ 2);

        write(index, 0, &[1, 2, 3, 4]).unwrap();
        assert_eq!(LAST_WRITE_SUM.load(Ordering::Relaxed), 10);
    }

    #[test]
    fn missing_ops_return_unsupported() {
        let _guard = global_registry_lock().lock().unwrap();
        reset();
        let index = register(BlockDevice::EMPTY).unwrap();

        let mut out = [0u8; 4];
        assert_eq!(read(index, 0, &mut out), Err(BlockIoError::Unsupported));
        assert_eq!(write(index, 0, &[1]), Err(BlockIoError::Unsupported));
        assert_eq!(read(99, 0, &mut out), Err(BlockIoError::InvalidDevice));
    }
}
