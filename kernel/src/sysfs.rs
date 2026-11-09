//! A minimal, static `/sys` for device discovery (libudev-zero, libinput,
//! libdrm): the DRM primary node and the evdev devices, laid out as Linux
//! does under `/sys/devices`, with `/sys/dev/char/<major>:<minor>` and
//! `/sys/class/<subsystem>` symlinks.
//!
//! The tree is built once at boot as ordinary tmpfs directories, files and
//! symlinks (`/sys` is a root pseudo mount, so it survives the ext2 root
//! mount). Nothing in it changes at run time: there is no hotplug.

use alloc::format;
use alloc::string::String;

use crate::input;

/// The operations the tree is built with (the VFS at boot, a recorder in
/// tests).
pub trait SysfsOps {
    fn mkdir(&mut self, path: &str);
    fn file(&mut self, path: &str, content: &str);
    fn symlink(&mut self, target: &str, path: &str);
}

const DRM_MAJOR: u32 = 226;
const INPUT_MAJOR: u32 = 13;
const EVDEV_MINOR_BASE: u32 = 64;

fn bitmap_hex(device_id: u16, ev_type: u16) -> String {
    let size = input::evdev_bitmap_bytes(ev_type).unwrap_or(8);
    let mut bits = [0u8; input::EVDEV_KEY_BITMAP_BYTES];
    input::evdev_capabilities(device_id, ev_type, &mut bits[..size]);
    let mut out = [0u8; 256];
    let n = input::format_bitmap_hex(&bits[..size], &mut out);
    String::from(core::str::from_utf8(&out[..n]).unwrap_or("0"))
}

pub fn build(ops: &mut impl SysfsOps) {
    for dir in [
        "/sys/devices",
        "/sys/devices/platform",
        "/sys/devices/virtual",
        "/sys/devices/virtual/input",
        "/sys/dev",
        "/sys/dev/char",
        "/sys/dev/block",
        "/sys/bus",
        "/sys/bus/platform",
        "/sys/class",
        "/sys/class/drm",
        "/sys/class/input",
    ] {
        ops.mkdir(dir);
    }

    // DRM primary node /dev/dri/card0 (226:0), the child of a platform
    // device as with Linux's simpledrm. libdrm's drmNodeIsDRM() checks for
    // /sys/dev/char/226:0/device/drm.
    let parent = "/sys/devices/platform/simple-framebuffer.0";
    ops.mkdir(parent);
    ops.file(
        &format!("{}/uevent", parent),
        "DRIVER=simple-framebuffer\nMODALIAS=platform:simple-framebuffer\n",
    );
    ops.symlink("../../../bus/platform", &format!("{}/subsystem", parent));
    ops.mkdir(&format!("{}/drm", parent));
    let card = "/sys/devices/platform/simple-framebuffer.0/drm/card0";
    ops.mkdir(card);
    ops.file(
        &format!("{}/uevent", card),
        &format!("MAJOR={}\nMINOR=0\nDEVNAME=dri/card0\nDEVTYPE=drm_minor\n", DRM_MAJOR),
    );
    ops.file(&format!("{}/dev", card), &format!("{}:0\n", DRM_MAJOR));
    ops.symlink("../../../../../class/drm", &format!("{}/subsystem", card));
    ops.symlink("../../../simple-framebuffer.0", &format!("{}/device", card));
    ops.symlink(
        "../../devices/platform/simple-framebuffer.0/drm/card0",
        "/sys/class/drm/card0",
    );
    ops.symlink(
        "../../devices/platform/simple-framebuffer.0/drm/card0",
        &format!("/sys/dev/char/{}:0", DRM_MAJOR),
    );

    // evdev devices: inputN (capabilities) with child eventN (the node).
    for device_id in input::SYSFS_DEVICES {
        let Some(ident) = input::evdev_identity(device_id) else {
            continue;
        };
        let input_dir = format!("/sys/devices/virtual/input/input{}", device_id);
        let minor = EVDEV_MINOR_BASE + device_id as u32;
        ops.mkdir(&input_dir);
        ops.file(
            &format!("{}/uevent", input_dir),
            &format!(
                "PRODUCT={:x}/{:x}/{:x}/{:x}\nNAME=\"{}\"\nPHYS=\"{}\"\nPROP=0\nEV={}\nKEY={}\nREL={}\nABS={}\n",
                ident.bustype,
                ident.vendor,
                ident.product,
                ident.version,
                ident.name,
                ident.phys,
                bitmap_hex(device_id, 0),
                bitmap_hex(device_id, input::EV_KEY),
                bitmap_hex(device_id, input::EV_REL),
                bitmap_hex(device_id, input::EV_ABS),
            ),
        );
        ops.file(&format!("{}/name", input_dir), &format!("{}\n", ident.name));
        ops.file(&format!("{}/phys", input_dir), &format!("{}\n", ident.phys));
        ops.file(&format!("{}/properties", input_dir), "0\n");
        ops.symlink("../../../../class/input", &format!("{}/subsystem", input_dir));
        ops.mkdir(&format!("{}/id", input_dir));
        for (name, value) in [
            ("bustype", ident.bustype),
            ("vendor", ident.vendor),
            ("product", ident.product),
            ("version", ident.version),
        ] {
            ops.file(&format!("{}/id/{}", input_dir, name), &format!("{:04x}\n", value));
        }
        ops.mkdir(&format!("{}/capabilities", input_dir));
        for (name, ev_type) in [
            ("ev", 0),
            ("key", input::EV_KEY),
            ("rel", input::EV_REL),
            ("abs", input::EV_ABS),
            ("msc", input::EV_MSC),
            ("sw", input::EV_SW),
            ("led", input::EV_LED),
            ("snd", input::EV_SND),
            ("ff", input::EV_FF),
        ] {
            ops.file(
                &format!("{}/capabilities/{}", input_dir, name),
                &format!("{}\n", bitmap_hex(device_id, ev_type)),
            );
        }
        ops.symlink(
            &format!("../../devices/virtual/input/input{}", device_id),
            &format!("/sys/class/input/input{}", device_id),
        );

        let event_dir = format!("{}/event{}", input_dir, device_id);
        ops.mkdir(&event_dir);
        ops.file(
            &format!("{}/uevent", event_dir),
            &format!("MAJOR={}\nMINOR={}\nDEVNAME=input/event{}\n", INPUT_MAJOR, minor, device_id),
        );
        ops.file(&format!("{}/dev", event_dir), &format!("{}:{}\n", INPUT_MAJOR, minor));
        ops.symlink("../../../../../class/input", &format!("{}/subsystem", event_dir));
        ops.symlink(
            &format!("../../devices/virtual/input/input{}/event{}", device_id, device_id),
            &format!("/sys/class/input/event{}", device_id),
        );
        ops.symlink(
            &format!("../../devices/virtual/input/input{}/event{}", device_id, device_id),
            &format!("/sys/dev/char/{}:{}", INPUT_MAJOR, minor),
        );
    }
}

/// Build `/sys` in the VFS. Must run before the root filesystem is mounted
/// over `/` (the tree lives in the boot tmpfs).
pub fn populate() {
    struct Vfs {
        failures: usize,
    }
    impl SysfsOps for Vfs {
        fn mkdir(&mut self, path: &str) {
            if fs::mkdir(path).is_err() {
                self.failures += 1;
            }
        }
        fn file(&mut self, path: &str, content: &str) {
            match fs::open(path, fs::O_CREAT | fs::O_TRUNC | fs::O_WRONLY, 0o444) {
                Ok(f) => {
                    if fs::write(f.inode, 0, content.as_bytes(), false).is_err() {
                        self.failures += 1;
                    }
                }
                Err(_) => self.failures += 1,
            }
        }
        fn symlink(&mut self, target: &str, path: &str) {
            if fs::symlinkat(fs::root_inode(), target, path).is_err() {
                self.failures += 1;
            }
        }
    }
    let _ = fs::mkdir("/sys");
    let mut vfs = Vfs { failures: 0 };
    build(&mut vfs);
    if vfs.failures > 0 {
        crate::serial_println!("[SYSFS] {} entries could not be created", vfs.failures);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    #[derive(Default)]
    struct Recorder {
        dirs: Vec<String>,
        files: Vec<(String, String)>,
        links: Vec<(String, String)>,
    }

    impl SysfsOps for Recorder {
        fn mkdir(&mut self, path: &str) {
            self.dirs.push(path.into());
        }
        fn file(&mut self, path: &str, content: &str) {
            self.files.push((path.into(), content.into()));
        }
        fn symlink(&mut self, target: &str, path: &str) {
            self.links.push((path.into(), target.into()));
        }
    }

    fn file<'a>(r: &'a Recorder, path: &str) -> &'a str {
        &r.files.iter().find(|(p, _)| p == path).unwrap_or_else(|| panic!("no {}", path)).1
    }

    /// Resolve a relative symlink target against the link's directory.
    fn resolve(link: &str, target: &str) -> String {
        let mut parts: Vec<&str> = link.split('/').filter(|p| !p.is_empty()).collect();
        parts.pop();
        for t in target.split('/') {
            match t {
                ".." => {
                    parts.pop();
                }
                "" | "." => {}
                other => parts.push(other),
            }
        }
        format!("/{}", parts.join("/"))
    }

    #[test]
    fn tree_describes_drm_and_evdev_devices_for_libudev() {
        let mut r = Recorder::default();
        build(&mut r);

        // Every directory a file or link lives in is created first.
        for (path, _) in r.files.iter().chain(r.links.iter()) {
            let parent = &path[..path.rfind('/').unwrap()];
            assert!(r.dirs.iter().any(|d| d == parent), "missing dir {}", parent);
        }

        let card = file(&r, "/sys/devices/platform/simple-framebuffer.0/drm/card0/uevent");
        assert!(card.contains("MAJOR=226\nMINOR=0\nDEVNAME=dri/card0\n"));
        // libdrm: /sys/dev/char/226:0/device/drm must exist.
        let (link, target) = r.links.iter().find(|(l, _)| l.ends_with("card0/device")).expect("device link");
        let device = resolve(link, target);
        assert!(r.dirs.contains(&format!("{}/drm", device)), "{}", device);

        // /sys/dev/char links resolve to the device directories, and the
        // subsystem links end in the subsystem name.
        for (link, target) in &r.links {
            let resolved = resolve(link, target);
            if link.starts_with("/sys/dev/char/") || link.starts_with("/sys/class/") {
                assert!(r.dirs.contains(&resolved), "{} -> {} ({})", link, target, resolved);
            }
            if link.ends_with("/subsystem") {
                assert!(
                    ["/sys/class/drm", "/sys/class/input", "/sys/bus/platform"].contains(&resolved.as_str()),
                    "{}",
                    resolved
                );
            }
        }
        let char_links: Vec<_> = r.links.iter().filter(|(l, _)| l.starts_with("/sys/dev/char/")).collect();
        // card0 plus keyboard, mouse and tablet: not the aggregate event1.
        assert_eq!(char_links.len(), 4);
        assert!(!r.links.iter().any(|(l, _)| l.ends_with("13:65")));

        let kbd = file(&r, "/sys/devices/virtual/input/input0/uevent");
        assert!(kbd.contains("NAME=\"AT Translated Set 2 keyboard\""));
        assert!(kbd.contains("EV=3\n"));
        let tablet = file(&r, "/sys/devices/virtual/input/input3/uevent");
        assert!(tablet.contains("EV=b\n"), "{}", tablet);
        assert!(tablet.contains("ABS=3\n"));
        assert!(tablet.contains("KEY=70000 0 0 0 0\n"));
        let ev3 = file(&r, "/sys/devices/virtual/input/input3/event3/uevent");
        assert_eq!(ev3, "MAJOR=13\nMINOR=67\nDEVNAME=input/event3\n");
    }
}
