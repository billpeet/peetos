use core::cell::UnsafeCell;

pub const MAX_BLOCK_DEVICES: usize = 16;
const BLOCK_CACHE_ENTRY_BYTES: usize = 4096;
const BLOCK_CACHE_ENTRIES: usize = 64;

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
/// Flushes the device's own volatile write cache (for example ATA FLUSH CACHE
/// EXT). After it returns `Ok`, every completed write is on stable media.
pub type BlockFlushOp = fn(BlockOpContext, &BlockDevice) -> Result<(), BlockIoError>;

#[derive(Clone, Copy)]
pub struct BlockOps {
    pub read: Option<BlockReadOp>,
    pub write: Option<BlockWriteOp>,
    /// `None` for devices without a volatile write cache to drain.
    pub flush: Option<BlockFlushOp>,
}

impl BlockOps {
    pub const NONE: Self = Self {
        read: None,
        write: None,
        flush: None,
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

#[derive(Clone, Copy)]
struct BlockCacheLine {
    valid: bool,
    dirty: bool,
    device_index: usize,
    lba: u64,
    len: usize,
    data: [u8; BLOCK_CACHE_ENTRY_BYTES],
    stamp: u64,
}

impl BlockCacheLine {
    const EMPTY: Self = Self {
        valid: false,
        dirty: false,
        device_index: 0,
        lba: 0,
        len: 0,
        data: [0; BLOCK_CACHE_ENTRY_BYTES],
        stamp: 0,
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub struct BlockCacheStats {
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    pub writebacks: u64,
    pub dirty_lines: usize,
}

impl BlockCacheStats {
    #[allow(dead_code)]
    pub const EMPTY: Self = Self {
        hits: 0,
        misses: 0,
        evictions: 0,
        writebacks: 0,
        dirty_lines: 0,
    };
}

struct BlockCache {
    lines: [BlockCacheLine; BLOCK_CACHE_ENTRIES],
    next_stamp: u64,
    hits: u64,
    misses: u64,
    evictions: u64,
    writebacks: u64,
}

impl BlockCache {
    const fn new() -> Self {
        Self {
            lines: [BlockCacheLine::EMPTY; BLOCK_CACHE_ENTRIES],
            next_stamp: 1,
            hits: 0,
            misses: 0,
            evictions: 0,
            writebacks: 0,
        }
    }

    fn clear(&mut self) {
        self.lines = [BlockCacheLine::EMPTY; BLOCK_CACHE_ENTRIES];
        self.next_stamp = 1;
        self.hits = 0;
        self.misses = 0;
        self.evictions = 0;
        self.writebacks = 0;
    }

    fn next_touch_stamp(&mut self) -> u64 {
        let stamp = self.next_stamp;
        self.next_stamp = self.next_stamp.wrapping_add(1);
        stamp
    }

    fn find_exact(&self, device_index: usize, lba: u64, len: usize) -> Option<usize> {
        let mut idx = 0usize;
        while idx < self.lines.len() {
            let line = &self.lines[idx];
            if line.valid && line.device_index == device_index && line.lba == lba && line.len == len
            {
                return Some(idx);
            }
            idx += 1;
        }
        None
    }

    fn pick_victim(&self) -> usize {
        let mut idx = 0usize;
        while idx < self.lines.len() {
            if !self.lines[idx].valid {
                return idx;
            }
            idx += 1;
        }

        let mut best = 0usize;
        let mut best_stamp = self.lines[0].stamp;
        idx = 1;
        while idx < self.lines.len() {
            let stamp = self.lines[idx].stamp;
            if stamp < best_stamp {
                best = idx;
                best_stamp = stamp;
            }
            idx += 1;
        }
        best
    }

    #[allow(dead_code)]
    fn dirty_line_count(&self) -> usize {
        let mut count = 0usize;
        let mut idx = 0usize;
        while idx < self.lines.len() {
            if self.lines[idx].valid && self.lines[idx].dirty {
                count += 1;
            }
            idx += 1;
        }
        count
    }
}

struct CacheCell(UnsafeCell<BlockCache>);

unsafe impl Sync for CacheCell {}

static BLOCK_CACHE: CacheCell = CacheCell(UnsafeCell::new(BlockCache::new()));

fn with_registry_mut<R>(f: impl FnOnce(&mut BlockRegistry) -> R) -> R {
    unsafe { f(&mut *BLOCK_REGISTRY.0.get()) }
}

fn with_registry<R>(f: impl FnOnce(&BlockRegistry) -> R) -> R {
    unsafe { f(&*BLOCK_REGISTRY.0.get()) }
}

fn with_cache_mut<R>(f: impl FnOnce(&mut BlockCache) -> R) -> R {
    unsafe { f(&mut *BLOCK_CACHE.0.get()) }
}

#[allow(dead_code)]
fn with_cache<R>(f: impl FnOnce(&BlockCache) -> R) -> R {
    unsafe { f(&*BLOCK_CACHE.0.get()) }
}

pub const fn transport_name(transport: BlockTransport) -> &'static str {
    match transport {
        BlockTransport::AtaPio => "ata-pio",
        BlockTransport::AhciDma => "ahci-dma",
    }
}

pub fn reset() {
    with_registry_mut(|registry| registry.clear());
    with_cache_mut(|cache| cache.clear());
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

#[allow(dead_code)]
pub fn cache_stats() -> BlockCacheStats {
    with_cache(|cache| BlockCacheStats {
        hits: cache.hits,
        misses: cache.misses,
        evictions: cache.evictions,
        writebacks: cache.writebacks,
        dirty_lines: cache.dirty_line_count(),
    })
}

fn lookup_entry(index: usize) -> Result<BlockEntry, BlockIoError> {
    with_registry(|registry| registry.get(index).ok_or(BlockIoError::InvalidDevice))
}

fn dispatch_read(entry: BlockEntry, lba: u64, out: &mut [u8]) -> Result<(), BlockIoError> {
    let Some(read_op) = entry.ops.read else {
        return Err(BlockIoError::Unsupported);
    };
    read_op(entry.ctx, &entry.device, lba, out)
}

fn dispatch_write(entry: BlockEntry, lba: u64, data: &[u8]) -> Result<(), BlockIoError> {
    let Some(write_op) = entry.ops.write else {
        return Err(BlockIoError::Unsupported);
    };
    write_op(entry.ctx, &entry.device, lba, data)
}

fn validate_cached_io(entry: BlockEntry, lba: u64, len: usize) -> Result<usize, BlockIoError> {
    if len == 0 {
        return Ok(0);
    }

    let logical = entry.device.logical_block_size as usize;
    if logical == 0 {
        return Err(BlockIoError::InvalidRequest);
    }
    if (len % logical) != 0 {
        return Err(BlockIoError::UnalignedBuffer);
    }

    let sectors = len / logical;
    let end = lba
        .checked_add(sectors as u64)
        .ok_or(BlockIoError::OutOfRange)?;
    if entry.device.sectors != 0 && end > entry.device.sectors {
        return Err(BlockIoError::OutOfRange);
    }
    Ok(logical)
}

fn flush_line(cache: &mut BlockCache, line_index: usize) -> Result<(), BlockIoError> {
    let (device_index, lba, len) = {
        let line = cache.lines[line_index];
        if !line.valid || !line.dirty {
            return Ok(());
        }
        (line.device_index, line.lba, line.len)
    };

    let entry = lookup_entry(device_index)?;
    {
        let line = &cache.lines[line_index];
        dispatch_write(entry, lba, &line.data[..len])?;
    }

    cache.lines[line_index].dirty = false;
    cache.writebacks = cache.writebacks.saturating_add(1);
    Ok(())
}

/// Keep uncached I/O and differently-sized cached I/O coherent with the
/// write-back cache: write back every dirty line of `device_index` that
/// overlaps `[lba, lba + len)` (bytes `len`, `logical`-byte sectors). With `invalidate`, the overlapping lines are also dropped
/// (used before a write that makes their contents stale).
///
/// Without this, a block written through `write_cached` and later read with
/// the raw `read` (or with a different chunk size) came back from the device
/// with its pre-write contents for as long as the dirty line stayed cached.
fn sync_overlapping_lines(
    cache: &mut BlockCache,
    device_index: usize,
    lba: u64,
    len: usize,
    logical: usize,
    invalidate: bool,
) -> Result<(), BlockIoError> {
    if len == 0 || logical == 0 {
        return Ok(());
    }
    let start = lba;
    let end = lba.saturating_add(len.div_ceil(logical) as u64);
    let mut i = 0usize;
    while i < cache.lines.len() {
        let line = &cache.lines[i];
        if line.valid && line.device_index == device_index {
            let line_end = line.lba.saturating_add(line.len.div_ceil(logical) as u64);
            if line.lba < end && start < line_end {
                flush_line(cache, i)?;
                if invalidate {
                    cache.lines[i].valid = false;
                }
            }
        }
        i += 1;
    }
    Ok(())
}

fn cache_store_chunk(
    cache: &mut BlockCache,
    device_index: usize,
    lba: u64,
    src: &[u8],
    dirty: bool,
) -> Result<(), BlockIoError> {
    let len = src.len();
    if len == 0 || len > BLOCK_CACHE_ENTRY_BYTES {
        return Err(BlockIoError::InvalidRequest);
    }

    if let Some(hit_idx) = cache.find_exact(device_index, lba, len) {
        let stamp = cache.next_touch_stamp();
        let line = &mut cache.lines[hit_idx];
        line.data[..len].copy_from_slice(src);
        if dirty {
            line.dirty = true;
        }
        line.stamp = stamp;
        cache.hits = cache.hits.saturating_add(1);
        return Ok(());
    }

    cache.misses = cache.misses.saturating_add(1);
    // A line of a different size may cover part of this range: write it back
    // and drop it so the device and the cache never disagree about the order
    // of writes, and later lookups cannot return its stale bytes.
    let logical = lookup_entry(device_index)?.device.logical_block_size as usize;
    sync_overlapping_lines(cache, device_index, lba, len, logical, true)?;
    let victim = cache.pick_victim();
    if cache.lines[victim].valid {
        cache.evictions = cache.evictions.saturating_add(1);
        flush_line(cache, victim)?;
    }

    let stamp = cache.next_touch_stamp();
    let line = &mut cache.lines[victim];
    line.valid = true;
    line.dirty = dirty;
    line.device_index = device_index;
    line.lba = lba;
    line.len = len;
    line.data[..len].copy_from_slice(src);
    if len < BLOCK_CACHE_ENTRY_BYTES {
        line.data[len..].fill(0);
    }
    line.stamp = stamp;
    Ok(())
}

pub fn flush_cached_device(index: usize) -> Result<(), BlockIoError> {
    with_cache_mut(|cache| {
        let mut i = 0usize;
        while i < cache.lines.len() {
            if cache.lines[i].valid && cache.lines[i].dirty && cache.lines[i].device_index == index
            {
                flush_line(cache, i)?;
            }
            i += 1;
        }
        Ok(())
    })
}

/// Durability barrier for one device: write back its dirty block-cache lines,
/// then flush the device's own write cache. Everything written to the device
/// before this call is on stable media once it returns `Ok`, so it orders
/// earlier writes before later ones (used by the ext3 journal).
pub fn flush_device(index: usize) -> Result<(), BlockIoError> {
    flush_cached_device(index)?;
    let entry = lookup_entry(index)?;
    match entry.ops.flush {
        Some(flush_op) => flush_op(entry.ctx, &entry.device),
        None => Ok(()),
    }
}

#[allow(dead_code)]
pub fn flush_cached_all() -> Result<(), BlockIoError> {
    with_cache_mut(|cache| {
        let mut i = 0usize;
        while i < cache.lines.len() {
            if cache.lines[i].valid && cache.lines[i].dirty {
                flush_line(cache, i)?;
            }
            i += 1;
        }
        Ok(())
    })
}

pub fn read(index: usize, lba: u64, out: &mut [u8]) -> Result<(), BlockIoError> {
    if out.is_empty() {
        return Ok(());
    }

    let entry = lookup_entry(index)?;
    // Dirty write-back lines are newer than the media: write them back first.
    let logical = entry.device.logical_block_size as usize;
    with_cache_mut(|cache| {
        sync_overlapping_lines(cache, index, lba, out.len(), logical, false)
    })?;
    dispatch_read(entry, lba, out)
}

pub fn write(index: usize, lba: u64, data: &[u8]) -> Result<(), BlockIoError> {
    if data.is_empty() {
        return Ok(());
    }

    let entry = lookup_entry(index)?;
    // Write back (to keep the write order) and drop overlapping cached lines,
    // which this write makes stale.
    let logical = entry.device.logical_block_size as usize;
    with_cache_mut(|cache| {
        sync_overlapping_lines(cache, index, lba, data.len(), logical, true)
    })?;
    dispatch_write(entry, lba, data)
}

#[allow(dead_code)]
pub fn read_cached(index: usize, lba: u64, out: &mut [u8]) -> Result<(), BlockIoError> {
    if out.is_empty() {
        return Ok(());
    }

    let entry = lookup_entry(index)?;
    let logical = validate_cached_io(entry, lba, out.len())?;

    let mut bytes_done = 0usize;
    let mut chunk_lba = lba;
    while bytes_done < out.len() {
        let remaining = out.len() - bytes_done;
        let mut chunk_len = core::cmp::min(remaining, BLOCK_CACHE_ENTRY_BYTES);
        chunk_len -= chunk_len % logical;
        if chunk_len == 0 {
            return Err(BlockIoError::InvalidRequest);
        }

        let mut hit = false;
        with_cache_mut(|cache| {
            if let Some(hit_idx) = cache.find_exact(index, chunk_lba, chunk_len) {
                let stamp = cache.next_touch_stamp();
                let line = &mut cache.lines[hit_idx];
                out[bytes_done..bytes_done + chunk_len].copy_from_slice(&line.data[..chunk_len]);
                line.stamp = stamp;
                cache.hits = cache.hits.saturating_add(1);
                hit = true;
            } else {
                cache.misses = cache.misses.saturating_add(1);
            }
        });

        if !hit {
            let dst = &mut out[bytes_done..bytes_done + chunk_len];
            with_cache_mut(|cache| {
                sync_overlapping_lines(cache, index, chunk_lba, chunk_len, logical, false)
            })?;
            dispatch_read(entry, chunk_lba, dst)?;
            with_cache_mut(|cache| {
                let _ = cache_store_chunk(cache, index, chunk_lba, dst, false);
            });
        }

        bytes_done += chunk_len;
        chunk_lba = chunk_lba.saturating_add((chunk_len / logical) as u64);
    }

    Ok(())
}

pub fn write_cached(index: usize, lba: u64, data: &[u8]) -> Result<(), BlockIoError> {
    if data.is_empty() {
        return Ok(());
    }

    let entry = lookup_entry(index)?;
    let logical = validate_cached_io(entry, lba, data.len())?;

    let mut bytes_done = 0usize;
    let mut chunk_lba = lba;
    while bytes_done < data.len() {
        let remaining = data.len() - bytes_done;
        let mut chunk_len = core::cmp::min(remaining, BLOCK_CACHE_ENTRY_BYTES);
        chunk_len -= chunk_len % logical;
        if chunk_len == 0 {
            return Err(BlockIoError::InvalidRequest);
        }

        let src = &data[bytes_done..bytes_done + chunk_len];
        with_cache_mut(|cache| cache_store_chunk(cache, index, chunk_lba, src, true))?;

        bytes_done += chunk_len;
        chunk_lba = chunk_lba.saturating_add((chunk_len / logical) as u64);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Mutex, OnceLock};

    static LAST_READ_CTX: AtomicU64 = AtomicU64::new(0);
    static LAST_WRITE_SUM: AtomicU64 = AtomicU64::new(0);
    static CACHE_READ_CALLS: AtomicU64 = AtomicU64::new(0);
    static CACHE_WRITE_CALLS: AtomicU64 = AtomicU64::new(0);
    static CACHE_LAST_WRITE_LBA: AtomicU64 = AtomicU64::new(0);

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

    fn cache_test_read(
        _ctx: BlockOpContext,
        _dev: &BlockDevice,
        lba: u64,
        out: &mut [u8],
    ) -> Result<(), BlockIoError> {
        CACHE_READ_CALLS.fetch_add(1, Ordering::Relaxed);
        for byte in out.iter_mut() {
            *byte = (lba as u8) ^ 0x5A;
        }
        Ok(())
    }

    fn cache_test_write(
        _ctx: BlockOpContext,
        _dev: &BlockDevice,
        lba: u64,
        data: &[u8],
    ) -> Result<(), BlockIoError> {
        CACHE_WRITE_CALLS.fetch_add(1, Ordering::Relaxed);
        CACHE_LAST_WRITE_LBA.store(lba, Ordering::Relaxed);
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
                flush: None,
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

    #[test]
    fn cached_write_coalesces_until_flush() {
        let _guard = global_registry_lock().lock().unwrap();
        reset();
        CACHE_WRITE_CALLS.store(0, Ordering::Relaxed);
        LAST_WRITE_SUM.store(0, Ordering::Relaxed);
        CACHE_LAST_WRITE_LBA.store(0, Ordering::Relaxed);

        let index = register_with_ops(
            BlockDevice {
                sectors: 4096,
                ..BlockDevice::EMPTY
            },
            BlockOps {
                read: Some(cache_test_read),
                write: Some(cache_test_write),
                flush: None,
            },
            BlockOpContext::EMPTY,
        )
        .unwrap();

        let ones = [1u8; 1024];
        let twos = [2u8; 1024];

        write_cached(index, 8, &ones).expect("cache ones");
        write_cached(index, 8, &twos).expect("overwrite cached line");
        assert_eq!(CACHE_WRITE_CALLS.load(Ordering::Relaxed), 0);

        flush_cached_device(index).expect("flush cached writes");
        assert_eq!(CACHE_WRITE_CALLS.load(Ordering::Relaxed), 1);
        assert_eq!(CACHE_LAST_WRITE_LBA.load(Ordering::Relaxed), 8);
        assert_eq!(LAST_WRITE_SUM.load(Ordering::Relaxed), (2u64 * 1024));
        assert_eq!(cache_stats().dirty_lines, 0);
    }

    #[test]
    fn cached_write_evicts_dirty_lines_with_writeback() {
        let _guard = global_registry_lock().lock().unwrap();
        reset();
        CACHE_WRITE_CALLS.store(0, Ordering::Relaxed);
        LAST_WRITE_SUM.store(0, Ordering::Relaxed);

        let index = register_with_ops(
            BlockDevice {
                sectors: 8192,
                ..BlockDevice::EMPTY
            },
            BlockOps {
                read: Some(cache_test_read),
                write: Some(cache_test_write),
                flush: None,
            },
            BlockOpContext::EMPTY,
        )
        .unwrap();

        let sector = [0xABu8; 512];
        for lba in 0..BLOCK_CACHE_ENTRIES {
            write_cached(index, lba as u64, &sector).expect("fill cache line");
        }
        assert_eq!(CACHE_WRITE_CALLS.load(Ordering::Relaxed), 0);

        write_cached(index, BLOCK_CACHE_ENTRIES as u64, &sector).expect("evict oldest line");
        assert_eq!(CACHE_WRITE_CALLS.load(Ordering::Relaxed), 1);

        flush_cached_device(index).expect("flush remaining dirty lines");
        assert_eq!(
            CACHE_WRITE_CALLS.load(Ordering::Relaxed),
            (BLOCK_CACHE_ENTRIES + 1) as u64
        );
        assert_eq!(cache_stats().dirty_lines, 0);
    }

    #[test]
    fn cached_read_hits_after_first_miss() {
        let _guard = global_registry_lock().lock().unwrap();
        reset();
        CACHE_READ_CALLS.store(0, Ordering::Relaxed);

        let index = register_with_ops(
            BlockDevice {
                sectors: 1024,
                ..BlockDevice::EMPTY
            },
            BlockOps {
                read: Some(cache_test_read),
                write: Some(cache_test_write),
                flush: None,
            },
            BlockOpContext::EMPTY,
        )
        .unwrap();

        let mut first = [0u8; 512];
        let mut second = [0u8; 512];
        read_cached(index, 3, &mut first).expect("first cached read");
        read_cached(index, 3, &mut second).expect("second cached read");

        assert_eq!(CACHE_READ_CALLS.load(Ordering::Relaxed), 1);
        assert_eq!(first, second);
        let stats = cache_stats();
        assert!(stats.hits >= 1);
        assert!(stats.misses >= 1);
    }

    static FLUSH_CALLS: AtomicU64 = AtomicU64::new(0);
    static WRITES_SEEN_AT_FLUSH: AtomicU64 = AtomicU64::new(u64::MAX);

    fn flush_test_flush(_ctx: BlockOpContext, _dev: &BlockDevice) -> Result<(), BlockIoError> {
        FLUSH_CALLS.fetch_add(1, Ordering::Relaxed);
        WRITES_SEEN_AT_FLUSH.store(CACHE_WRITE_CALLS.load(Ordering::Relaxed), Ordering::Relaxed);
        Ok(())
    }

    #[test]
    fn flush_device_writes_back_cache_before_device_flush() {
        let _guard = global_registry_lock().lock().unwrap();
        reset();
        CACHE_WRITE_CALLS.store(0, Ordering::Relaxed);
        FLUSH_CALLS.store(0, Ordering::Relaxed);
        WRITES_SEEN_AT_FLUSH.store(u64::MAX, Ordering::Relaxed);

        let index = register_with_ops(
            BlockDevice {
                sectors: 4096,
                ..BlockDevice::EMPTY
            },
            BlockOps {
                read: Some(cache_test_read),
                write: Some(cache_test_write),
                flush: Some(flush_test_flush),
            },
            BlockOpContext::EMPTY,
        )
        .unwrap();

        write_cached(index, 16, &[3u8; 1024]).expect("cache line a");
        write_cached(index, 32, &[4u8; 1024]).expect("cache line b");
        assert_eq!(CACHE_WRITE_CALLS.load(Ordering::Relaxed), 0);

        flush_device(index).expect("barrier");
        assert_eq!(FLUSH_CALLS.load(Ordering::Relaxed), 1);
        // Both dirty lines reached the device before the device flush ran.
        assert_eq!(WRITES_SEEN_AT_FLUSH.load(Ordering::Relaxed), 2);
        assert_eq!(cache_stats().dirty_lines, 0);
    }

    #[test]
    fn flush_device_without_flush_op_only_drains_cache() {
        let _guard = global_registry_lock().lock().unwrap();
        reset();
        CACHE_WRITE_CALLS.store(0, Ordering::Relaxed);
        let index = register_with_ops(
            BlockDevice {
                sectors: 4096,
                ..BlockDevice::EMPTY
            },
            BlockOps {
                read: Some(cache_test_read),
                write: Some(cache_test_write),
                flush: None,
            },
            BlockOpContext::EMPTY,
        )
        .unwrap();
        write_cached(index, 8, &[5u8; 512]).expect("cache line");
        flush_device(index).expect("barrier without device flush");
        assert_eq!(CACHE_WRITE_CALLS.load(Ordering::Relaxed), 1);
        assert_eq!(flush_device(99), Err(BlockIoError::InvalidDevice));
    }

    // A RAM-backed device, so tests can check what actually reached "media".
    const RAM_DEV_BYTES: usize = 64 * 1024;

    fn ram_dev() -> &'static Mutex<Vec<u8>> {
        static RAM: OnceLock<Mutex<Vec<u8>>> = OnceLock::new();
        RAM.get_or_init(|| Mutex::new(vec![0u8; RAM_DEV_BYTES]))
    }

    fn ram_read(
        _ctx: BlockOpContext,
        _dev: &BlockDevice,
        lba: u64,
        out: &mut [u8],
    ) -> Result<(), BlockIoError> {
        let ram = ram_dev().lock().unwrap();
        let off = lba as usize * 512;
        out.copy_from_slice(&ram[off..off + out.len()]);
        Ok(())
    }

    fn ram_write(
        _ctx: BlockOpContext,
        _dev: &BlockDevice,
        lba: u64,
        data: &[u8],
    ) -> Result<(), BlockIoError> {
        let mut ram = ram_dev().lock().unwrap();
        let off = lba as usize * 512;
        ram[off..off + data.len()].copy_from_slice(data);
        Ok(())
    }

    fn register_ram_dev() -> usize {
        ram_dev().lock().unwrap().fill(0);
        register_with_ops(
            BlockDevice {
                sectors: (RAM_DEV_BYTES / 512) as u64,
                ..BlockDevice::EMPTY
            },
            BlockOps {
                read: Some(ram_read),
                write: Some(ram_write),
                flush: None,
            },
            BlockOpContext::EMPTY,
        )
        .unwrap()
    }

    /// Regression: the ext2 backend writes through `write_cached` and reads
    /// with the raw `read`. A dirty line must be visible to that read, or a
    /// metadata block the fs crate evicted and reloaded came back with its
    /// pre-write contents (runtime-created inodes reverted to mode 0).
    #[test]
    fn raw_read_sees_dirty_cached_write() {
        let _guard = global_registry_lock().lock().unwrap();
        reset();
        let index = register_ram_dev();

        write_cached(index, 4, &[0x11u8; 1024]).expect("cached write");
        let mut out = [0u8; 1024];
        read(index, 4, &mut out).expect("raw read");
        assert!(out.iter().all(|b| *b == 0x11));

        // A read that only partly overlaps the dirty line sees it too.
        let mut wide = [0u8; 2048];
        read(index, 3, &mut wide).expect("overlapping raw read");
        assert!(wide[..512].iter().all(|b| *b == 0));
        assert!(wide[512..1536].iter().all(|b| *b == 0x11));
        assert!(wide[1536..].iter().all(|b| *b == 0));
    }

    #[test]
    fn raw_write_replaces_cached_line() {
        let _guard = global_registry_lock().lock().unwrap();
        reset();
        let index = register_ram_dev();

        write_cached(index, 8, &[0x22u8; 1024]).expect("cached write");
        write(index, 8, &[0x33u8; 1024]).expect("raw write");
        // The stale dirty line must not be written back over the raw write
        // or served by a later cached read.
        flush_cached_device(index).expect("flush");
        let mut out = [0u8; 1024];
        read_cached(index, 8, &mut out).expect("cached read");
        assert!(out.iter().all(|b| *b == 0x33));
        read(index, 8, &mut out).expect("raw read");
        assert!(out.iter().all(|b| *b == 0x33));
    }

    #[test]
    fn cached_io_of_different_sizes_stays_coherent() {
        let _guard = global_registry_lock().lock().unwrap();
        reset();
        let index = register_ram_dev();

        // An older 4 KiB dirty line, then a newer 1 KiB write inside it.
        write_cached(index, 16, &[0x44u8; 4096]).expect("wide cached write");
        write_cached(index, 18, &[0x55u8; 1024]).expect("narrow cached write");
        flush_cached_device(index).expect("flush");

        let mut out = [0u8; 4096];
        read_cached(index, 16, &mut out).expect("cached read");
        assert!(out[..1024].iter().all(|b| *b == 0x44));
        assert!(out[1024..2048].iter().all(|b| *b == 0x55));
        assert!(out[2048..].iter().all(|b| *b == 0x44));
    }
}
