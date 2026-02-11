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

pub struct BlockRegistry {
    devices: [BlockDevice; MAX_BLOCK_DEVICES],
    summary: BlockInventory,
}

impl BlockRegistry {
    pub const fn new() -> Self {
        Self {
            devices: [BlockDevice::EMPTY; MAX_BLOCK_DEVICES],
            summary: BlockInventory::EMPTY,
        }
    }

    pub fn clear(&mut self) {
        self.devices = [BlockDevice::EMPTY; MAX_BLOCK_DEVICES];
        self.summary = BlockInventory::EMPTY;
    }

    pub fn register(&mut self, device: BlockDevice) -> Option<usize> {
        self.summary.total += 1;

        if self.summary.stored < self.devices.len() {
            let index = self.summary.stored;
            self.devices[index] = device;
            self.summary.stored += 1;
            Some(index)
        } else {
            self.summary.overflow = true;
            None
        }
    }

    pub fn snapshot(&self, out: &mut [BlockDevice]) -> usize {
        let count = core::cmp::min(out.len(), self.summary.stored);
        out[..count].copy_from_slice(&self.devices[..count]);
        count
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
    with_registry_mut(|registry| registry.register(device))
}

pub fn snapshot(out: &mut [BlockDevice]) -> usize {
    with_registry(|registry| registry.snapshot(out))
}

pub fn inventory() -> BlockInventory {
    with_registry(|registry| registry.summary())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_tracks_counts_and_overflow() {
        let mut reg = BlockRegistry::new();

        for i in 0..MAX_BLOCK_DEVICES {
            let result = reg.register(BlockDevice {
                target: i as u8,
                ..BlockDevice::EMPTY
            });
            assert_eq!(result, Some(i));
        }

        assert_eq!(reg.summary().stored, MAX_BLOCK_DEVICES);
        assert_eq!(reg.summary().total, MAX_BLOCK_DEVICES);
        assert!(!reg.summary().overflow);

        let overflow = reg.register(BlockDevice {
            target: 99,
            ..BlockDevice::EMPTY
        });
        assert_eq!(overflow, None);
        assert_eq!(reg.summary().total, MAX_BLOCK_DEVICES + 1);
        assert!(reg.summary().overflow);
    }

    #[test]
    fn snapshot_returns_dense_list() {
        let mut reg = BlockRegistry::new();
        reg.register(BlockDevice {
            target: 7,
            ..BlockDevice::EMPTY
        });
        reg.register(BlockDevice {
            target: 8,
            ..BlockDevice::EMPTY
        });

        let mut out = [BlockDevice::EMPTY; 1];
        let copied = reg.snapshot(&mut out);

        assert_eq!(copied, 1);
        assert_eq!(out[0].target, 7);
    }
}
