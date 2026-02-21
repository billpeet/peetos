use core::cmp;

use crate::block;
use crate::memory::heap;
use crate::serial_println;

const EXT2_MOUNT_POINT: &str = "/mnt/ext2";

const EXT2_SUPERBLOCK_OFFSET: usize = 1024;
const EXT2_SUPER_MAGIC_OFFSET: usize = EXT2_SUPERBLOCK_OFFSET + 56;
const EXT2_BLOCKS_COUNT_OFFSET: usize = EXT2_SUPERBLOCK_OFFSET + 4;
const EXT2_LOG_BLOCK_SIZE_OFFSET: usize = EXT2_SUPERBLOCK_OFFSET + 24;
const EXT2_SUPER_MAGIC: u16 = 0xEF53;

const BLOCK_SECTOR_SIZE: usize = 512;
const BLOCK_IO_CHUNK: usize = 4096;

#[derive(Clone, Copy)]
struct Ext2BlockBacking {
    active: bool,
    device_index: usize,
    image_len: usize,
}

impl Ext2BlockBacking {
    const EMPTY: Self = Self {
        active: false,
        device_index: 0,
        image_len: 0,
    };
}

static mut EXT2_BLOCK_BACKING: Ext2BlockBacking = Ext2BlockBacking::EMPTY;

pub fn init() {
    fs::init();
    serial_println!("[VFS] Initialized (tmpfs + devfs + procfs)");

    if !try_mount_ext2_from_block() {
        serial_println!("[VFS] No ext2 block-backed image detected");
    }
}

pub fn sync_ext2_to_block() -> bool {
    let backing = unsafe { EXT2_BLOCK_BACKING };
    if !backing.active || backing.image_len == 0 {
        return false;
    }

    if fs::ext2_has_journal() {
        let flushed = match fs::flush_ext2_dirty_blocks() {
            Ok(count) => count,
            Err(err) => {
                serial_println!("[VFS] WARN: ext2 journaled dirty flush failed ({})", err);
                return false;
            }
        };
        let checkpointed = match fs::checkpoint_ext2_journal() {
            Ok(count) => count,
            Err(err) => {
                serial_println!("[VFS] WARN: ext2 journal checkpoint failed ({})", err);
                return false;
            }
        };
        return flushed > 0 || checkpointed > 0;
    }

    let block_size = fs::ext2_block_size() as usize;
    if block_size == 0
        || (block_size % BLOCK_SECTOR_SIZE) != 0
        || block_size > BLOCK_IO_CHUNK
        || (BLOCK_IO_CHUNK % block_size) != 0
    {
        serial_println!("[VFS] WARN: ext2 sync invalid block size {}", block_size);
        return false;
    }

    let chunk_ptr = heap::kmalloc(BLOCK_IO_CHUNK);
    if chunk_ptr.is_null() {
        serial_println!("[VFS] WARN: ext2 sync alloc failed");
        return false;
    }

    let blocks_per_chunk = BLOCK_IO_CHUNK / block_size;
    let mut cursor = 0u32;
    let mut wrote_any = false;
    let mut ok = true;

    while ok {
        let run = match fs::ext2_next_dirty_run(cursor) {
            Ok(Some(run)) => run,
            Ok(None) => break,
            Err(err) => {
                serial_println!("[VFS] WARN: ext2 dirty-scan failed ({})", err);
                ok = false;
                break;
            }
        };

        let run_start = run.start_block as usize;
        let run_blocks = run.block_count as usize;
        let mut done_blocks = 0usize;
        while done_blocks < run_blocks {
            let chunk_blocks = cmp::min(run_blocks - done_blocks, blocks_per_chunk);
            let chunk_len = chunk_blocks * block_size;
            let start_block = run_start + done_blocks;
            let lba = (start_block * block_size / BLOCK_SECTOR_SIZE) as u64;

            let copied = unsafe {
                let chunk = core::slice::from_raw_parts_mut(chunk_ptr, chunk_len);
                fs::copy_ext2_blocks(start_block as u32, chunk)
            };

            let copied = match copied {
                Ok(v) => v,
                Err(err) => {
                    serial_println!("[VFS] WARN: ext2 block-copy failed ({})", err);
                    ok = false;
                    break;
                }
            };
            if copied != chunk_len {
                serial_println!("[VFS] WARN: ext2 block-copy short read");
                ok = false;
                break;
            }

            let write_res = unsafe {
                let chunk = core::slice::from_raw_parts(chunk_ptr, chunk_len);
                block::write_cached(backing.device_index, lba, chunk)
            };
            if write_res.is_err() {
                serial_println!(
                    "[VFS] WARN: ext2 sync cache-write failed dev{} lba{}",
                    backing.device_index,
                    lba
                );
                ok = false;
                break;
            }

            done_blocks += chunk_blocks;
            wrote_any = true;
        }

        cursor = run.start_block.saturating_add(run.block_count);
    }

    if ok && wrote_any {
        if let Err(err) = fs::mark_ext2_blocks_synced() {
            serial_println!("[VFS] WARN: ext2 sync checkpoint failed ({})", err);
            ok = false;
        }
    }

    heap::kfree(chunk_ptr);
    ok && wrote_any
}

pub fn flush_ext2_block_cache() -> bool {
    let backing = unsafe { EXT2_BLOCK_BACKING };
    if !backing.active || backing.image_len == 0 {
        return false;
    }

    match block::flush_cached_device(backing.device_index) {
        Ok(()) => true,
        Err(err) => {
            serial_println!(
                "[VFS] WARN: ext2 cache flush failed dev{} ({})",
                backing.device_index,
                block::io_error_name(err)
            );
            false
        }
    }
}

fn try_mount_ext2_from_block() -> bool {
    let mut devices = [block::BlockDevice::EMPTY; block::MAX_BLOCK_DEVICES];
    let count = block::snapshot(&mut devices);

    // Probe each device with a small read to find the ext2 superblock.
    // If found, mount via fs backend callbacks and lazy block residency.
    const PROBE_SIZE: usize = 4096;

    for (index, dev) in devices.iter().take(count).enumerate() {
        if dev.logical_block_size as usize != BLOCK_SECTOR_SIZE {
            continue;
        }
        let dev_bytes = dev.sectors.saturating_mul(BLOCK_SECTOR_SIZE as u64) as usize;
        if dev_bytes < PROBE_SIZE {
            continue;
        }

        let probe_ptr = heap::kmalloc(PROBE_SIZE);
        if probe_ptr.is_null() {
            continue;
        }

        let mut ext2_len: Option<usize> = None;
        unsafe {
            let probe = core::slice::from_raw_parts_mut(probe_ptr, PROBE_SIZE);
            if read_block_image(index, probe).is_ok() {
                ext2_len = detect_ext2_image_len(probe);
            }
        }
        let image_len = match ext2_len {
            Some(len) if len <= dev_bytes => len,
            _ => {
                heap::kfree(probe_ptr);
                continue;
            }
        };

        let mut mounted = false;
        unsafe {
            let backend = fs::Ext2BlockBackend {
                ctx: index as u64,
                read_block: ext2_block_backend_read,
                write_block: ext2_block_backend_write,
            };
            if fs::set_ext2_block_backend(backend).is_ok() {
                let probe = core::slice::from_raw_parts(probe_ptr, PROBE_SIZE);
                if fs::mount_ext2_block_backend(EXT2_MOUNT_POINT, image_len, probe).is_ok() {
                    EXT2_BLOCK_BACKING = Ext2BlockBacking {
                        active: true,
                        device_index: index,
                        image_len,
                    };
                    serial_println!(
                        "[VFS] Mounted ext2 from block dev{} at {} ({} KiB, lazy cache)",
                        index,
                        EXT2_MOUNT_POINT,
                        image_len / 1024
                    );
                    mounted = true;
                } else {
                    let _ = fs::clear_ext2_block_backend();
                }
            }
        }

        heap::kfree(probe_ptr);
        if mounted {
            return true;
        }
    }

    false
}

fn ext2_block_backend_read(ctx: u64, block_index: u32, out: &mut [u8]) -> Result<(), i64> {
    if out.is_empty() || (out.len() % BLOCK_SECTOR_SIZE) != 0 {
        return Err(fs::EINVAL);
    }

    let block_bytes = out.len();
    let byte_offset = (block_index as u64)
        .checked_mul(block_bytes as u64)
        .ok_or(fs::EIO)?;
    if (byte_offset % BLOCK_SECTOR_SIZE as u64) != 0 {
        return Err(fs::EIO);
    }
    let lba = byte_offset / BLOCK_SECTOR_SIZE as u64;
    let device_index = ctx as usize;

    block::read(device_index, lba, out).map_err(map_block_io_to_errno)
}

fn ext2_block_backend_write(ctx: u64, block_index: u32, data: &[u8]) -> Result<(), i64> {
    if data.is_empty() || (data.len() % BLOCK_SECTOR_SIZE) != 0 {
        return Err(fs::EINVAL);
    }

    let block_bytes = data.len();
    let byte_offset = (block_index as u64)
        .checked_mul(block_bytes as u64)
        .ok_or(fs::EIO)?;
    if (byte_offset % BLOCK_SECTOR_SIZE as u64) != 0 {
        return Err(fs::EIO);
    }
    let lba = byte_offset / BLOCK_SECTOR_SIZE as u64;
    let device_index = ctx as usize;

    block::write_cached(device_index, lba, data).map_err(map_block_io_to_errno)
}

fn map_block_io_to_errno(err: block::BlockIoError) -> i64 {
    match err {
        block::BlockIoError::InvalidDevice => fs::ENOENT,
        block::BlockIoError::Unsupported => fs::EINVAL,
        block::BlockIoError::UnalignedBuffer => fs::EINVAL,
        block::BlockIoError::OutOfRange => fs::EIO,
        block::BlockIoError::InvalidRequest => fs::EINVAL,
        block::BlockIoError::IoFault => fs::EIO,
    }
}

fn read_block_image(device_index: usize, image: &mut [u8]) -> Result<(), block::BlockIoError> {
    let mut offset = 0usize;
    let mut lba = 0u64;

    while offset < image.len() {
        let remaining = image.len() - offset;
        let mut chunk_len = cmp::min(BLOCK_IO_CHUNK, remaining);
        chunk_len -= chunk_len % BLOCK_SECTOR_SIZE;
        if chunk_len == 0 {
            break;
        }

        block::read(device_index, lba, &mut image[offset..offset + chunk_len])?;
        offset += chunk_len;
        lba += (chunk_len / BLOCK_SECTOR_SIZE) as u64;
    }

    Ok(())
}

fn detect_ext2_image_len(image: &[u8]) -> Option<usize> {
    if image.len() < EXT2_SUPER_MAGIC_OFFSET + 2 {
        return None;
    }

    let magic = u16::from_le_bytes([
        image[EXT2_SUPER_MAGIC_OFFSET],
        image[EXT2_SUPER_MAGIC_OFFSET + 1],
    ]);
    if magic != EXT2_SUPER_MAGIC {
        return None;
    }

    let blocks_count = u32::from_le_bytes([
        image[EXT2_BLOCKS_COUNT_OFFSET],
        image[EXT2_BLOCKS_COUNT_OFFSET + 1],
        image[EXT2_BLOCKS_COUNT_OFFSET + 2],
        image[EXT2_BLOCKS_COUNT_OFFSET + 3],
    ]) as usize;

    let log_block_size = u32::from_le_bytes([
        image[EXT2_LOG_BLOCK_SIZE_OFFSET],
        image[EXT2_LOG_BLOCK_SIZE_OFFSET + 1],
        image[EXT2_LOG_BLOCK_SIZE_OFFSET + 2],
        image[EXT2_LOG_BLOCK_SIZE_OFFSET + 3],
    ]);

    if blocks_count == 0 || log_block_size > 4 {
        return None;
    }

    let block_size = 1024usize.checked_shl(log_block_size)?;
    let total_len = blocks_count.checked_mul(block_size)?;
    if total_len == 0 {
        return None;
    }

    Some(total_len)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_u16_le(buf: &mut [u8], offset: usize, value: u16) {
        let bytes = value.to_le_bytes();
        buf[offset] = bytes[0];
        buf[offset + 1] = bytes[1];
    }

    fn write_u32_le(buf: &mut [u8], offset: usize, value: u32) {
        let bytes = value.to_le_bytes();
        buf[offset] = bytes[0];
        buf[offset + 1] = bytes[1];
        buf[offset + 2] = bytes[2];
        buf[offset + 3] = bytes[3];
    }

    #[test]
    fn ext2_length_detects_valid_superblock() {
        let mut image = [0u8; 16 * 1024];
        write_u16_le(&mut image, EXT2_SUPER_MAGIC_OFFSET, EXT2_SUPER_MAGIC);
        write_u32_le(&mut image, EXT2_BLOCKS_COUNT_OFFSET, 8);
        write_u32_le(&mut image, EXT2_LOG_BLOCK_SIZE_OFFSET, 0);

        assert_eq!(detect_ext2_image_len(&image), Some(8 * 1024));
    }

    #[test]
    fn ext2_length_rejects_bad_magic() {
        let mut image = [0u8; 4096];
        write_u16_le(&mut image, EXT2_SUPER_MAGIC_OFFSET, 0x1234);
        write_u32_le(&mut image, EXT2_BLOCKS_COUNT_OFFSET, 4);
        write_u32_le(&mut image, EXT2_LOG_BLOCK_SIZE_OFFSET, 0);

        assert_eq!(detect_ext2_image_len(&image), None);
    }

    #[test]
    fn ext2_length_reports_size_from_superblock() {
        let mut image = [0u8; 4096];
        write_u16_le(&mut image, EXT2_SUPER_MAGIC_OFFSET, EXT2_SUPER_MAGIC);
        write_u32_le(&mut image, EXT2_BLOCKS_COUNT_OFFSET, 1024);
        write_u32_le(&mut image, EXT2_LOG_BLOCK_SIZE_OFFSET, 0);

        // Reports 1024 * 1024 = 1 MiB even though the buffer is only 4 KiB.
        assert_eq!(detect_ext2_image_len(&image), Some(1024 * 1024));
    }
}
