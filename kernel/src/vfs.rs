use crate::serial_println;

const EXT2_MOUNT_POINT: &str = "/mnt/ext2";
const EXT2_SAMPLE_HELLO: &str = "/mnt/ext2/hello.txt";

static SAMPLE_EXT2_IMG: &[u8] = include_bytes!("../../user/sample_ext2.img");

pub fn init() {
    fs::init();
    serial_println!("[VFS] Initialized (tmpfs + devfs + procfs)");

    match fs::mount_ext2_image(EXT2_MOUNT_POINT, SAMPLE_EXT2_IMG) {
        Ok(()) => {
            serial_println!("[VFS] Mounted ext2 sample at {}", EXT2_MOUNT_POINT);
            let mut buf = [0u8; 96];
            if let Ok(file) = fs::open(EXT2_SAMPLE_HELLO, fs::O_RDONLY, 0) {
                if let Ok(read_n) = fs::read(file.inode, 0, &mut buf) {
                    if read_n > 0 {
                        if let Ok(text) = core::str::from_utf8(&buf[..read_n]) {
                            serial_println!("[VFS] ext2 hello: {}", text.trim_end());
                        }
                    }
                }
            }
        }
        Err(err) => {
            serial_println!("[VFS] WARN: failed to mount ext2 sample ({})", err);
        }
    }
}
