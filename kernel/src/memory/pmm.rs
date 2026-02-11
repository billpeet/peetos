#[cfg(target_os = "none")]
use core::ptr;

#[cfg(target_os = "none")]
use arch_x86_64::addr::VirtAddr;
use arch_x86_64::addr::{PhysAddr, PAGE_OFFSET_MASK, PAGE_SHIFT, PAGE_SIZE};

#[cfg(target_os = "none")]
use crate::serial_println;

#[cfg(target_os = "none")]
const MMAP_ADDR: u64 = 0xFFFF_FFFF_8000_0500;
#[cfg(target_os = "none")]
const MMAP_COUNT_ADDR: u64 = 0xFFFF_FFFF_8000_07F0;
#[cfg(target_os = "none")]
const MMAP_MAX_ENTRIES: usize = 32;

#[cfg(target_os = "none")]
const MMAP_TYPE_USABLE: u32 = 1;

const PMM_MAX_MEMORY: u64 = 128 * 1024 * 1024;
const PMM_MAX_PAGES: u64 = PMM_MAX_MEMORY / PAGE_SIZE;
const PMM_BITMAP_SIZE: usize = (PMM_MAX_PAGES / 8) as usize;

#[cfg(any(target_os = "none", test))]
#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
struct MmapEntry {
    base: u64,
    length: u64,
    kind: u32,
    acpi_attr: u32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct PmmStats {
    pub total_pages: u64,
    pub used_pages: u64,
    pub free_pages: u64,
    pub reserved_pages: u64,
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub free_bytes: u64,
}

static mut PMM_BITMAP: [u8; PMM_BITMAP_SIZE] = [0; PMM_BITMAP_SIZE];
static mut TOTAL_PAGE_COUNT: u64 = 0;
static mut FREE_PAGE_COUNT: u64 = 0;
static mut USED_PAGE_COUNT: u64 = 0;
static mut FIRST_USABLE_PFN: u64 = 0;
static mut LAST_USABLE_PFN: u64 = 0;
static mut SEARCH_HINT: u64 = 0;
static mut PMM_INITIALIZED: bool = false;

extern "C" {
    static __kernel_start: u8;
    static __kernel_end: u8;
}

#[cfg(target_os = "none")]
pub fn init() {
    serial_println!("[PMM] Initializing physical memory manager...");

    unsafe {
        let entry_count = read_mmap_count();
        let entries = MMAP_ADDR as *const MmapEntry;

        serial_println!("[PMM] Found {} memory regions from E820", entry_count);

        let bitmap_ptr = core::ptr::addr_of_mut!(PMM_BITMAP) as *mut u8;
        core::ptr::write_bytes(bitmap_ptr, 0xFF, PMM_BITMAP_SIZE);
        FREE_PAGE_COUNT = 0;
        USED_PAGE_COUNT = PMM_MAX_PAGES;
        TOTAL_PAGE_COUNT = 0;
        FIRST_USABLE_PFN = 0;
        LAST_USABLE_PFN = 0;

        for i in 0..entry_count {
            let entry = read_mmap_entry(entries, i);

            if entry.kind != MMAP_TYPE_USABLE {
                continue;
            }

            let mut region_start = entry.base;
            let mut region_end = entry.base.saturating_add(entry.length);

            if region_end <= 0x1_00000 {
                continue;
            }

            if region_start < 0x1_00000 {
                region_start = 0x1_00000;
            }

            if region_end > PMM_MAX_MEMORY {
                region_end = PMM_MAX_MEMORY;
            }

            if region_start >= region_end {
                continue;
            }

            mark_range_free(region_start, region_end);

            let start_pfn = addr_to_pfn(region_start);
            let end_pfn = addr_to_pfn(region_end);

            if FIRST_USABLE_PFN == 0 || start_pfn < FIRST_USABLE_PFN {
                FIRST_USABLE_PFN = start_pfn;
            }
            if end_pfn > LAST_USABLE_PFN {
                LAST_USABLE_PFN = end_pfn;
            }

            TOTAL_PAGE_COUNT = TOTAL_PAGE_COUNT.saturating_add(end_pfn - start_pfn);
        }

        serial_println!("[PMM] Reserving kernel and system regions...");

        mark_range_used(0, 0x1_00000);

        let kernel_start = VirtAddr::new(ptr::addr_of!(__kernel_start) as u64);
        let kernel_end = VirtAddr::new(ptr::addr_of!(__kernel_end) as u64);
        let kernel_start_phys = kernel_start.to_phys().as_u64();
        let kernel_end_phys = kernel_end.to_phys().as_u64();
        mark_range_used(kernel_start_phys, kernel_end_phys);

        SEARCH_HINT = FIRST_USABLE_PFN;
        PMM_INITIALIZED = true;

        let first = FIRST_USABLE_PFN;
        let last = LAST_USABLE_PFN;
        let free = FREE_PAGE_COUNT;
        serial_println!("[PMM] First usable PFN: 0x{:x}", first);
        serial_println!("[PMM] Last usable PFN: 0x{:x}", last);
        serial_println!("[PMM] Free pages: {}", free);
    }
}

#[cfg(not(target_os = "none"))]
pub fn init() {}

#[cfg(target_os = "none")]
pub fn alloc_page() -> PhysAddr {
    unsafe {
        if !PMM_INITIALIZED {
            serial_println!("[PMM] ERROR: PMM not initialized!");
            return PhysAddr::new(0);
        }

        if FREE_PAGE_COUNT == 0 {
            serial_println!("[PMM] ERROR: Out of memory!");
            return PhysAddr::new(0);
        }

        let start = SEARCH_HINT;
        let mut pfn = start;

        loop {
            if !bitmap_test(pfn) {
                bitmap_set(pfn);
                FREE_PAGE_COUNT = FREE_PAGE_COUNT.saturating_sub(1);
                USED_PAGE_COUNT = USED_PAGE_COUNT.saturating_add(1);

                SEARCH_HINT = pfn + 1;
                if SEARCH_HINT >= LAST_USABLE_PFN {
                    SEARCH_HINT = FIRST_USABLE_PFN;
                }

                return PhysAddr::new(pfn_to_addr(pfn));
            }

            pfn += 1;
            if pfn >= LAST_USABLE_PFN {
                pfn = FIRST_USABLE_PFN;
            }

            if pfn == start {
                break;
            }
        }

        serial_println!("[PMM] ERROR: No free page found (bitmap inconsistency)!");
        PhysAddr::new(0)
    }
}

#[cfg(not(target_os = "none"))]
pub fn alloc_page() -> PhysAddr {
    PhysAddr::new(0)
}

#[cfg(target_os = "none")]
pub fn free_page(addr: PhysAddr) {
    unsafe {
        if !PMM_INITIALIZED {
            serial_println!("[PMM] ERROR: PMM not initialized!");
            return;
        }

        if addr.as_u64() & PAGE_OFFSET_MASK != 0 {
            serial_println!("[PMM] ERROR: Unaligned address: 0x{:x}", addr.as_u64());
            return;
        }

        let pfn = addr.to_pfn();

        if pfn < FIRST_USABLE_PFN || pfn >= LAST_USABLE_PFN {
            serial_println!("[PMM] ERROR: Address out of range: 0x{:x}", addr.as_u64());
            return;
        }

        if !bitmap_test(pfn) {
            serial_println!(
                "[PMM] WARNING: Double-free detected at: 0x{:x}",
                addr.as_u64()
            );
            return;
        }

        bitmap_clear(pfn);
        FREE_PAGE_COUNT = FREE_PAGE_COUNT.saturating_add(1);
        USED_PAGE_COUNT = USED_PAGE_COUNT.saturating_sub(1);

        if pfn < SEARCH_HINT {
            SEARCH_HINT = pfn;
        }
    }
}

#[cfg(not(target_os = "none"))]
pub fn free_page(_addr: PhysAddr) {}

#[cfg(target_os = "none")]
pub fn alloc_pages(count: usize) -> PhysAddr {
    unsafe {
        if !PMM_INITIALIZED || count == 0 {
            return PhysAddr::new(0);
        }

        if count == 1 {
            return alloc_page();
        }

        if FREE_PAGE_COUNT < count as u64 {
            serial_println!("[PMM] ERROR: Not enough free pages for contiguous allocation!");
            return PhysAddr::new(0);
        }

        let count = count as u64;
        let mut start = FIRST_USABLE_PFN;
        while start + count <= LAST_USABLE_PFN {
            let mut found = true;
            for offset in 0..count {
                if bitmap_test(start + offset) {
                    found = false;
                    start += offset + 1;
                    break;
                }
            }

            if found {
                for offset in 0..count {
                    bitmap_set(start + offset);
                }
                FREE_PAGE_COUNT = FREE_PAGE_COUNT.saturating_sub(count);
                USED_PAGE_COUNT = USED_PAGE_COUNT.saturating_add(count);
                return PhysAddr::new(pfn_to_addr(start));
            }
        }

        serial_println!("[PMM] ERROR: No contiguous block found!");
        PhysAddr::new(0)
    }
}

#[cfg(not(target_os = "none"))]
pub fn alloc_pages(_count: usize) -> PhysAddr {
    PhysAddr::new(0)
}

#[cfg(target_os = "none")]
pub fn free_pages(addr: PhysAddr, count: usize) {
    for i in 0..count {
        free_page(addr + (i as u64 * PAGE_SIZE));
    }
}

#[cfg(not(target_os = "none"))]
pub fn free_pages(_addr: PhysAddr, _count: usize) {}

#[cfg(target_os = "none")]
pub fn mark_used(addr: PhysAddr) {
    unsafe {
        if !PMM_INITIALIZED {
            return;
        }

        let addr = addr.align_down();
        let pfn = addr.to_pfn();

        if pfn < FIRST_USABLE_PFN || pfn >= LAST_USABLE_PFN {
            return;
        }

        if bitmap_test(pfn) {
            return;
        }

        bitmap_set(pfn);
        USED_PAGE_COUNT = USED_PAGE_COUNT.saturating_add(1);
        FREE_PAGE_COUNT = FREE_PAGE_COUNT.saturating_sub(1);
    }
}

#[cfg(not(target_os = "none"))]
pub fn mark_used(_addr: PhysAddr) {}

pub fn get_stats() -> PmmStats {
    unsafe {
        let total_pages = TOTAL_PAGE_COUNT;
        let used_pages = USED_PAGE_COUNT;
        let free_pages = FREE_PAGE_COUNT;
        let reserved_pages = PMM_MAX_PAGES.saturating_sub(total_pages);

        PmmStats {
            total_pages,
            used_pages,
            free_pages,
            reserved_pages,
            total_bytes: total_pages * PAGE_SIZE,
            used_bytes: used_pages * PAGE_SIZE,
            free_bytes: free_pages * PAGE_SIZE,
        }
    }
}

pub fn is_managed(addr: PhysAddr) -> bool {
    unsafe {
        let pfn = addr.to_pfn();
        pfn >= FIRST_USABLE_PFN && pfn < LAST_USABLE_PFN
    }
}

fn bitmap_set(pfn: u64) {
    if pfn >= PMM_MAX_PAGES {
        return;
    }
    unsafe {
        let index = (pfn / 8) as usize;
        let bit = (pfn % 8) as u8;
        PMM_BITMAP[index] |= 1 << bit;
    }
}

fn bitmap_clear(pfn: u64) {
    if pfn >= PMM_MAX_PAGES {
        return;
    }
    unsafe {
        let index = (pfn / 8) as usize;
        let bit = (pfn % 8) as u8;
        PMM_BITMAP[index] &= !(1 << bit);
    }
}

fn bitmap_test(pfn: u64) -> bool {
    if pfn >= PMM_MAX_PAGES {
        return true;
    }
    unsafe {
        let index = (pfn / 8) as usize;
        let bit = (pfn % 8) as u8;
        (PMM_BITMAP[index] & (1 << bit)) != 0
    }
}

fn mark_range_used(start: u64, end: u64) {
    let start_pfn = addr_to_pfn(align_down(start));
    let end_pfn = addr_to_pfn(align_up(end));

    for pfn in start_pfn..end_pfn {
        if !bitmap_test(pfn) {
            bitmap_set(pfn);
            unsafe {
                if FREE_PAGE_COUNT > 0 {
                    FREE_PAGE_COUNT = FREE_PAGE_COUNT.saturating_sub(1);
                }
                USED_PAGE_COUNT = USED_PAGE_COUNT.saturating_add(1);
            }
        }
    }
}

fn mark_range_free(start: u64, end: u64) {
    let start_pfn = addr_to_pfn(align_up(start));
    let end_pfn = addr_to_pfn(align_down(end));

    for pfn in start_pfn..end_pfn {
        if bitmap_test(pfn) {
            bitmap_clear(pfn);
            unsafe {
                FREE_PAGE_COUNT = FREE_PAGE_COUNT.saturating_add(1);
                if USED_PAGE_COUNT > 0 {
                    USED_PAGE_COUNT = USED_PAGE_COUNT.saturating_sub(1);
                }
            }
        }
    }
}

#[cfg(target_os = "none")]
fn read_mmap_count() -> usize {
    unsafe {
        let count_ptr = MMAP_COUNT_ADDR as *const u16;
        let count = ptr::read_volatile(count_ptr) as usize;
        count.min(MMAP_MAX_ENTRIES)
    }
}

#[cfg(not(target_os = "none"))]
fn read_mmap_count() -> usize {
    0
}

#[cfg(target_os = "none")]
fn read_mmap_entry(entries: *const MmapEntry, index: usize) -> MmapEntry {
    unsafe { ptr::read_unaligned(entries.add(index)) }
}

#[cfg(not(target_os = "none"))]
fn read_mmap_entry(_entries: *const MmapEntry, _index: usize) -> MmapEntry {
    MmapEntry {
        base: 0,
        length: 0,
        kind: 0,
        acpi_attr: 0,
    }
}

#[inline]
const fn align_down(addr: u64) -> u64 {
    addr & !PAGE_OFFSET_MASK
}

#[inline]
const fn align_up(addr: u64) -> u64 {
    if addr & PAGE_OFFSET_MASK == 0 {
        addr
    } else {
        (addr + PAGE_OFFSET_MASK) & !PAGE_OFFSET_MASK
    }
}

#[inline]
const fn addr_to_pfn(addr: u64) -> u64 {
    addr >> PAGE_SHIFT
}

#[inline]
const fn pfn_to_addr(pfn: u64) -> u64 {
    pfn << PAGE_SHIFT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bitmap_set_clear_roundtrip() {
        unsafe {
            PMM_BITMAP = [0u8; PMM_BITMAP_SIZE];
        }

        bitmap_set(3);
        bitmap_set(15);
        assert!(bitmap_test(3));
        assert!(bitmap_test(15));

        bitmap_clear(3);
        assert!(!bitmap_test(3));
        assert!(bitmap_test(15));
    }

    #[test]
    fn range_markers_align_bounds() {
        unsafe {
            PMM_BITMAP = [0xFFu8; PMM_BITMAP_SIZE];
            FREE_PAGE_COUNT = 0;
            USED_PAGE_COUNT = PMM_MAX_PAGES;
        }

        mark_range_free(0x2000, 0x5000);
        assert!(!bitmap_test(2));
        assert!(!bitmap_test(3));
        assert!(bitmap_test(1));

        mark_range_used(0x3000, 0x4001);
        assert!(bitmap_test(3));
    }

    #[test]
    fn stats_compute_bytes() {
        unsafe {
            TOTAL_PAGE_COUNT = 10;
            USED_PAGE_COUNT = 4;
            FREE_PAGE_COUNT = 6;
        }

        let stats = get_stats();
        assert_eq!(stats.total_bytes, 10 * PAGE_SIZE);
        assert_eq!(stats.used_bytes, 4 * PAGE_SIZE);
        assert_eq!(stats.free_bytes, 6 * PAGE_SIZE);
    }

    #[test]
    fn phys_virt_constants_match() {
        let phys = PhysAddr::new(0x2000);
        let virt = phys.to_virt();
        assert_eq!(virt.as_u64(), 0xFFFF_FFFF_8000_2000);
        let round = virt.to_phys();
        assert_eq!(round, phys);
    }
}
