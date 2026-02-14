use arch_x86_64::addr::{PhysAddr, VirtAddr, PAGE_SIZE};

use crate::memory::vmm;

const MMIO_WINDOW_BASE: u64 = 0xFFFF_FFFF_D000_0000;
const MMIO_WINDOW_STRIDE: u64 = 0x20_000;
const MMIO_WINDOW_LIMIT: usize = 16;

static mut MMIO_WINDOW_NEXT: usize = 0;

pub fn map_mmio_window(phys_addr: u64, size: usize) -> Option<*mut u8> {
    let window_index = unsafe {
        if MMIO_WINDOW_NEXT >= MMIO_WINDOW_LIMIT {
            return None;
        }
        let current = MMIO_WINDOW_NEXT;
        MMIO_WINDOW_NEXT += 1;
        current
    };

    let virt_base = MMIO_WINDOW_BASE + (window_index as u64) * MMIO_WINDOW_STRIDE;
    let phys_base = phys_addr & !(PAGE_SIZE - 1);
    let page_offset = (phys_addr - phys_base) as usize;
    let map_size = page_offset.saturating_add(size);
    let page_count = (map_size as u64 + PAGE_SIZE - 1) / PAGE_SIZE;

    for page in 0..page_count {
        let virt = VirtAddr::new(virt_base + page * PAGE_SIZE);
        let phys = PhysAddr::new(phys_base + page * PAGE_SIZE);
        if vmm::map_page(virt, phys, vmm::VMM_FLAG_KERNEL_DATA) != 0 {
            return None;
        }
    }

    Some((virt_base as usize + page_offset) as *mut u8)
}

pub fn mmio_read32(base: *mut u8, offset: usize) -> u32 {
    unsafe { core::ptr::read_volatile(base.add(offset) as *const u32) }
}

pub fn mmio_write32(base: *mut u8, offset: usize, value: u32) {
    unsafe {
        core::ptr::write_volatile(base.add(offset) as *mut u32, value);
    }
}
