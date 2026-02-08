use crate::serial_println;

pub fn init() {
    fs::init();
    serial_println!("[VFS] Initialized (tmpfs root)");
}
