use crate::serial_println;

const EXT2_MOUNT_POINT: &str = "/mnt/ext2";

static SAMPLE_EXT2_IMG: &[u8] = include_bytes!("../../user/sample_ext2.img");

pub fn init() {
    fs::init();
    serial_println!("[VFS] Initialized (tmpfs + devfs + procfs)");

    match fs::mount_ext2_image(EXT2_MOUNT_POINT, SAMPLE_EXT2_IMG) {
        Ok(()) => {
            serial_println!("[VFS] Mounted ext2 sample at {}", EXT2_MOUNT_POINT);
        }
        Err(err) => {
            serial_println!("[VFS] WARN: failed to mount ext2 sample ({})", err);
        }
    }
}
