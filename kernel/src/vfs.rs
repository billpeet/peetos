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
const EXT2_MAX_IMAGE_SIZE: usize = 512 * 1024;

const BLOCK_SECTOR_SIZE: usize = 512;
const BLOCK_IO_CHUNK: usize = 4096;

static SAMPLE_EXT2_IMG: &[u8] = include_bytes!("../../user/sample_ext2.img");

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

    if try_mount_ext2_from_block() {
        return;
    }

    match fs::mount_ext2_image(EXT2_MOUNT_POINT, SAMPLE_EXT2_IMG) {
        Ok(()) => {
            serial_println!("[VFS] Mounted ext2 sample at {}", EXT2_MOUNT_POINT);
        }
        Err(err) => {
            serial_println!("[VFS] WARN: failed to mount ext2 sample ({})", err);
        }
    }
}

pub fn sync_ext2_to_block() -> bool {
    let backing = unsafe { EXT2_BLOCK_BACKING };
    if !backing.active || backing.image_len == 0 {
        return false;
    }

    let image_ptr = heap::kmalloc(backing.image_len);
    if image_ptr.is_null() {
        serial_println!("[VFS] WARN: ext2 sync alloc failed");
        return false;
    }

    let mut ok = false;
    unsafe {
        let image = core::slice::from_raw_parts_mut(image_ptr, backing.image_len);
        match fs::export_ext2_image(image) {
            Ok(exported_len) => {
                ok = write_ext2_image_to_block(backing.device_index, &image[..exported_len]);
                if !ok {
                    serial_println!(
                        "[VFS] WARN: ext2 sync to block dev{} failed",
                        backing.device_index
                    );
                }
            }
            Err(err) => {
                serial_println!("[VFS] WARN: ext2 export for sync failed ({})", err);
            }
        }
    }

    heap::kfree(image_ptr);
    ok
}

fn try_mount_ext2_from_block() -> bool {
    let mut devices = [block::BlockDevice::EMPTY; block::MAX_BLOCK_DEVICES];
    let count = block::snapshot(&mut devices);

    for (index, dev) in devices.iter().take(count).enumerate() {
        if dev.logical_block_size as usize != BLOCK_SECTOR_SIZE {
            continue;
        }

        let max_len = cmp::min(
            EXT2_MAX_IMAGE_SIZE,
            dev.sectors.saturating_mul(BLOCK_SECTOR_SIZE as u64) as usize,
        );
        let read_len = max_len - (max_len % BLOCK_SECTOR_SIZE);
        if read_len < EXT2_SUPER_MAGIC_OFFSET + 2 {
            continue;
        }

        let image_ptr = heap::kmalloc(read_len);
        if image_ptr.is_null() {
            continue;
        }

        let mut mounted = false;
        unsafe {
            let image = core::slice::from_raw_parts_mut(image_ptr, read_len);
            if read_block_image(index, image).is_ok() {
                if let Some(image_len) = detect_ext2_image_len(image) {
                    match fs::mount_ext2_image(EXT2_MOUNT_POINT, &image[..image_len]) {
                        Ok(()) => {
                            EXT2_BLOCK_BACKING = Ext2BlockBacking {
                                active: true,
                                device_index: index,
                                image_len,
                            };
                            serial_println!(
                                "[VFS] Mounted ext2 from block dev{} at {} ({} KiB)",
                                index,
                                EXT2_MOUNT_POINT,
                                image_len / 1024
                            );
                            mounted = true;
                        }
                        Err(_) => {}
                    }
                }
            }
        }

        heap::kfree(image_ptr);
        if mounted {
            return true;
        }
    }

    false
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

fn write_ext2_image_to_block(device_index: usize, image: &[u8]) -> bool {
    let mut offset = 0usize;
    let mut lba = 0u64;

    while offset + BLOCK_SECTOR_SIZE <= image.len() {
        let remaining = image.len() - offset;
        let chunk_len = cmp::min(BLOCK_IO_CHUNK, remaining - (remaining % BLOCK_SECTOR_SIZE));
        if chunk_len == 0 {
            break;
        }

        if block::write(device_index, lba, &image[offset..offset + chunk_len]).is_err() {
            return false;
        }

        offset += chunk_len;
        lba += (chunk_len / BLOCK_SECTOR_SIZE) as u64;
    }

    if offset < image.len() {
        let mut sector = [0u8; BLOCK_SECTOR_SIZE];
        if block::read(device_index, lba, &mut sector).is_err() {
            return false;
        }

        let tail = image.len() - offset;
        sector[..tail].copy_from_slice(&image[offset..]);

        if block::write(device_index, lba, &sector).is_err() {
            return false;
        }
    }

    true
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
    if total_len == 0 || total_len > image.len() {
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
    fn ext2_length_rejects_oversized_claim() {
        let mut image = [0u8; 4096];
        write_u16_le(&mut image, EXT2_SUPER_MAGIC_OFFSET, EXT2_SUPER_MAGIC);
        write_u32_le(&mut image, EXT2_BLOCKS_COUNT_OFFSET, 1024);
        write_u32_le(&mut image, EXT2_LOG_BLOCK_SIZE_OFFSET, 0);

        assert_eq!(detect_ext2_image_len(&image), None);
    }
}
