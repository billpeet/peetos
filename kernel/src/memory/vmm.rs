use core::ptr;

use arch_x86_64::addr::{PhysAddr, VirtAddr, PAGE_SIZE};

use crate::memory::pmm;
use crate::serial_println;
use crate::syscall;

pub const VMM_ENTRIES_PER_TABLE: usize = 512;

pub const PTE_PRESENT: u64 = 1 << 0;
pub const PTE_WRITE: u64 = 1 << 1;
pub const PTE_USER: u64 = 1 << 2;
pub const PTE_WRITE_THROUGH: u64 = 1 << 3;
pub const PTE_CACHE_DISABLE: u64 = 1 << 4;
pub const PTE_ACCESSED: u64 = 1 << 5;
pub const PTE_DIRTY: u64 = 1 << 6;
pub const PTE_LARGE: u64 = 1 << 7;
pub const PTE_GLOBAL: u64 = 1 << 8;
/// Software-defined (AVL) bit: page is shared copy-on-write and the hardware
/// write bit has been cleared. A write fault on such a page copies it, or
/// re-enables write when the faulting mapping is the last owner.
pub const PTE_COW: u64 = 1 << 9;
pub const PTE_NO_EXECUTE: u64 = 1 << 63;

pub const PTE_ADDR_MASK: u64 = 0x000F_FFFF_FFFF_F000;
pub const PTE_FLAGS_MASK: u64 = 0xFFF0_0000_0000_0FFF;

pub const VMM_FLAG_KERNEL_RO: u64 = PTE_PRESENT;
pub const VMM_FLAG_KERNEL_RW: u64 = PTE_PRESENT | PTE_WRITE;
pub const VMM_FLAG_USER_RO: u64 = PTE_PRESENT | PTE_USER;
pub const VMM_FLAG_USER_RW: u64 = PTE_PRESENT | PTE_WRITE | PTE_USER;
pub const VMM_FLAG_KERNEL_CODE: u64 = PTE_PRESENT;
pub const VMM_FLAG_KERNEL_DATA: u64 = PTE_PRESENT | PTE_WRITE | PTE_NO_EXECUTE;

static mut PML4_PHYS: PhysAddr = PhysAddr::new(0);
static mut MAPPED_PAGES: u64 = 1;
static mut TABLES_ALLOCATED: u64 = 1;
static mut VMM_INITIALIZED: bool = false;
static mut INIT_PML4_PHYS: PhysAddr = PhysAddr::new(0);

#[inline]
pub const fn pml4_index(addr: u64) -> usize {
    ((addr >> 39) & 0x1FF) as usize
}

#[inline]
pub const fn pdpt_index(addr: u64) -> usize {
    ((addr >> 30) & 0x1FF) as usize
}

#[inline]
pub const fn pd_index(addr: u64) -> usize {
    ((addr >> 21) & 0x1FF) as usize
}

#[inline]
pub const fn pt_index(addr: u64) -> usize {
    ((addr >> 12) & 0x1FF) as usize
}

#[inline]
pub const fn page_offset(addr: u64) -> u64 {
    addr & 0xFFF
}

#[inline]
fn pte_addr(pte: u64) -> PhysAddr {
    PhysAddr::new(pte & PTE_ADDR_MASK)
}

#[inline]
fn pte_present(pte: u64) -> bool {
    (pte & PTE_PRESENT) != 0
}

#[inline]
fn table_ptr(table_phys: PhysAddr) -> *mut u64 {
    table_phys.to_virt().as_u64() as *mut u64
}

#[inline]
fn read_pte(table_phys: PhysAddr, index: usize) -> u64 {
    unsafe { ptr::read(table_ptr(table_phys).add(index)) }
}

#[inline]
fn write_pte(table_phys: PhysAddr, index: usize, value: u64) {
    unsafe {
        ptr::write(table_ptr(table_phys).add(index), value);
    }
}

fn alloc_page_table() -> PhysAddr {
    let table = pmm::alloc_page();
    if table.as_u64() == 0 {
        serial_println!("[VMM] ERROR: Failed to allocate page table!");
        return PhysAddr::new(0);
    }

    unsafe {
        ptr::write_bytes(table_ptr(table), 0, VMM_ENTRIES_PER_TABLE);
    }

    unsafe {
        TABLES_ALLOCATED = TABLES_ALLOCATED.saturating_add(1);
    }

    table
}

fn get_or_create_table(table_phys: PhysAddr, index: usize, create: bool) -> PhysAddr {
    let pte = read_pte(table_phys, index);

    if pte_present(pte) {
        return pte_addr(pte);
    }

    if !create {
        return PhysAddr::new(0);
    }

    let new_table = alloc_page_table();
    if new_table.as_u64() == 0 {
        return PhysAddr::new(0);
    }

    let new_pte = new_table.as_u64() | PTE_PRESENT | PTE_WRITE | PTE_USER;
    write_pte(table_phys, index, new_pte);

    new_table
}

fn get_or_create_table_in(
    table_phys: PhysAddr,
    index: usize,
    create: bool,
    user_accessible: bool,
) -> PhysAddr {
    let mut pte = read_pte(table_phys, index);

    if pte_present(pte) {
        if (pte & PTE_LARGE) != 0 && user_accessible && create {
            let new_table = alloc_page_table();
            if new_table.as_u64() == 0 {
                return PhysAddr::new(0);
            }

            let mut new_pte = new_table.as_u64() | PTE_PRESENT | PTE_WRITE | PTE_USER;
            if user_accessible {
                new_pte |= PTE_USER;
            }
            write_pte(table_phys, index, new_pte);
            return new_table;
        }

        if user_accessible && (pte & PTE_USER) == 0 {
            pte |= PTE_USER;
            write_pte(table_phys, index, pte);
        }

        return pte_addr(pte);
    }

    if !create {
        return PhysAddr::new(0);
    }

    let new_table = alloc_page_table();
    if new_table.as_u64() == 0 {
        return PhysAddr::new(0);
    }

    let mut new_pte = new_table.as_u64() | PTE_PRESENT | PTE_WRITE;
    if user_accessible {
        new_pte |= PTE_USER;
    }
    write_pte(table_phys, index, new_pte);

    new_table
}

#[cfg(target_os = "none")]
fn read_cr3() -> u64 {
    let value: u64;
    unsafe {
        core::arch::asm!("mov {}, cr3", out(reg) value, options(nomem, nostack, preserves_flags));
    }
    value
}

#[cfg(not(target_os = "none"))]
fn read_cr3() -> u64 {
    0
}

#[cfg(target_os = "none")]
fn write_cr3(value: u64) {
    unsafe {
        core::arch::asm!("mov cr3, {}", in(reg) value, options(nomem, nostack, preserves_flags));
    }
}

#[cfg(not(target_os = "none"))]
fn write_cr3(_value: u64) {}

#[cfg(target_os = "none")]
pub fn invalidate_page(virt: VirtAddr) {
    let addr = virt.as_u64();
    unsafe {
        core::arch::asm!("invlpg [{}]", in(reg) addr, options(nostack, preserves_flags));
    }
}

#[cfg(not(target_os = "none"))]
pub fn invalidate_page(_virt: VirtAddr) {}

#[cfg(target_os = "none")]
pub fn flush_tlb() {
    let cr3 = read_cr3();
    write_cr3(cr3);
}

#[cfg(not(target_os = "none"))]
pub fn flush_tlb() {}

pub fn init() {
    serial_println!("[VMM] Initializing virtual memory manager...");

    let cr3 = read_cr3();
    let pml4_phys = PhysAddr::new(cr3 & PTE_ADDR_MASK);

    unsafe {
        PML4_PHYS = pml4_phys;
        MAPPED_PAGES = 512 + (63 * 512);
        TABLES_ALLOCATED = 4;
        VMM_INITIALIZED = true;
    }

    serial_println!("[VMM] PML4 phys = 0x{:x}", pml4_phys.as_u64());
    serial_println!("[VMM] Initial mappings: {} pages", unsafe { MAPPED_PAGES });
}

pub fn map_page(virt: VirtAddr, phys: PhysAddr, flags: u64) -> i32 {
    unsafe {
        if !VMM_INITIALIZED {
            serial_println!("[VMM] ERROR: VMM not initialized!");
            return -1;
        }
    }

    let virt = virt.align_down();
    let phys = phys.align_down();

    let pml4_idx = pml4_index(virt.as_u64());
    let pdpt_idx = pdpt_index(virt.as_u64());
    let pd_idx = pd_index(virt.as_u64());
    let pt_idx = pt_index(virt.as_u64());

    let pml4 = unsafe { PML4_PHYS };

    let pdpt = get_or_create_table(pml4, pml4_idx, true);
    if pdpt.as_u64() == 0 {
        return -1;
    }

    let pd = get_or_create_table(pdpt, pdpt_idx, true);
    if pd.as_u64() == 0 {
        return -1;
    }

    let pd_entry = read_pte(pd, pd_idx);
    if pte_present(pd_entry) && (pd_entry & PTE_LARGE) != 0 {
        let large_page_phys = pte_addr(pd_entry).as_u64();
        let large_page_virt =
            ((pml4_idx as u64) << 39) | ((pdpt_idx as u64) << 30) | ((pd_idx as u64) << 21);
        let offset = virt.as_u64() - large_page_virt;
        if phys.as_u64() == large_page_phys + offset {
            return 0;
        }
        serial_println!("[VMM] ERROR: Cannot remap within 2MB page!");
        return -1;
    }

    let pt = get_or_create_table(pd, pd_idx, true);
    if pt.as_u64() == 0 {
        return -1;
    }

    let existing = read_pte(pt, pt_idx);
    if pte_present(existing) {
        write_pte(
            pt,
            pt_idx,
            phys.as_u64() | (flags & PTE_FLAGS_MASK) | PTE_PRESENT,
        );
    } else {
        write_pte(
            pt,
            pt_idx,
            phys.as_u64() | (flags & PTE_FLAGS_MASK) | PTE_PRESENT,
        );
        unsafe {
            MAPPED_PAGES = MAPPED_PAGES.saturating_add(1);
        }
    }

    invalidate_page(virt);
    0
}

pub fn unmap_page(virt: VirtAddr) -> i32 {
    unsafe {
        if !VMM_INITIALIZED {
            return -1;
        }
    }

    let virt = virt.align_down();

    let pml4_idx = pml4_index(virt.as_u64());
    let pdpt_idx = pdpt_index(virt.as_u64());
    let pd_idx = pd_index(virt.as_u64());
    let pt_idx = pt_index(virt.as_u64());

    let pml4 = unsafe { PML4_PHYS };

    let pml4_entry = read_pte(pml4, pml4_idx);
    if !pte_present(pml4_entry) {
        return -1;
    }

    let pdpt = pte_addr(pml4_entry);
    let pdpt_entry = read_pte(pdpt, pdpt_idx);
    if !pte_present(pdpt_entry) {
        return -1;
    }

    if (pdpt_entry & PTE_LARGE) != 0 {
        write_pte(pdpt, pdpt_idx, 0);
        invalidate_page(virt);
        return 0;
    }

    let pd = pte_addr(pdpt_entry);
    let pd_entry = read_pte(pd, pd_idx);
    if !pte_present(pd_entry) {
        return -1;
    }

    if (pd_entry & PTE_LARGE) != 0 {
        write_pte(pd, pd_idx, 0);
        invalidate_page(virt);
        return 0;
    }

    let pt = pte_addr(pd_entry);
    let pt_entry = read_pte(pt, pt_idx);
    if !pte_present(pt_entry) {
        return -1;
    }

    write_pte(pt, pt_idx, 0);
    unsafe {
        MAPPED_PAGES = MAPPED_PAGES.saturating_sub(1);
    }
    invalidate_page(virt);
    0
}

pub fn map_range(virt_start: VirtAddr, phys_start: PhysAddr, count: usize, flags: u64) -> i32 {
    for i in 0..count {
        let virt = virt_start + (i as u64 * PAGE_SIZE);
        let phys = phys_start + (i as u64 * PAGE_SIZE);
        if map_page(virt, phys, flags) != 0 {
            unmap_range(virt_start, i);
            return -1;
        }
    }
    0
}

pub fn unmap_range(virt_start: VirtAddr, count: usize) -> usize {
    let mut unmapped = 0;
    for i in 0..count {
        let virt = virt_start + (i as u64 * PAGE_SIZE);
        if unmap_page(virt) == 0 {
            unmapped += 1;
        }
    }
    unmapped
}

pub fn get_physical(virt: VirtAddr) -> Option<PhysAddr> {
    get_physical_in(PhysAddr::new(0), virt)
}

pub fn get_physical_in(target_pml4: PhysAddr, virt: VirtAddr) -> Option<PhysAddr> {
    let mut pml4 = target_pml4;
    if pml4.as_u64() == 0 {
        pml4 = unsafe { PML4_PHYS };
    }

    let pml4_idx = pml4_index(virt.as_u64());
    let pdpt_idx = pdpt_index(virt.as_u64());
    let pd_idx = pd_index(virt.as_u64());
    let pt_idx = pt_index(virt.as_u64());

    let pml4_entry = read_pte(pml4, pml4_idx);
    if !pte_present(pml4_entry) {
        return None;
    }

    let pdpt = pte_addr(pml4_entry);
    let pdpt_entry = read_pte(pdpt, pdpt_idx);
    if !pte_present(pdpt_entry) {
        return None;
    }

    if (pdpt_entry & PTE_LARGE) != 0 {
        let phys = (pdpt_entry & 0x000F_FFFF_C000_0000) | (virt.as_u64() & 0x3FFF_FFFF);
        return Some(PhysAddr::new(phys));
    }

    let pd = pte_addr(pdpt_entry);
    let pd_entry = read_pte(pd, pd_idx);
    if !pte_present(pd_entry) {
        return None;
    }

    if (pd_entry & PTE_LARGE) != 0 {
        let phys = (pd_entry & 0x000F_FFFF_FFE0_0000) | (virt.as_u64() & 0x1F_FFFF);
        return Some(PhysAddr::new(phys));
    }

    let pt = pte_addr(pd_entry);
    let pt_entry = read_pte(pt, pt_idx);
    if !pte_present(pt_entry) {
        return None;
    }

    Some(PhysAddr::new(
        pte_addr(pt_entry).as_u64() | page_offset(virt.as_u64()),
    ))
}

/// Raw 4 KiB leaf page-table entry for `virt` in `target_pml4`, or `None` if
/// the page is not present or is covered by a large page.
pub fn get_pte_in(target_pml4: PhysAddr, virt: VirtAddr) -> Option<u64> {
    let mut pml4 = target_pml4;
    if pml4.as_u64() == 0 {
        pml4 = unsafe { PML4_PHYS };
    }
    let v = virt.as_u64();
    let pml4_entry = read_pte(pml4, pml4_index(v));
    if !pte_present(pml4_entry) {
        return None;
    }
    let pdpt_entry = read_pte(pte_addr(pml4_entry), pdpt_index(v));
    if !pte_present(pdpt_entry) || (pdpt_entry & PTE_LARGE) != 0 {
        return None;
    }
    let pd_entry = read_pte(pte_addr(pdpt_entry), pd_index(v));
    if !pte_present(pd_entry) || (pd_entry & PTE_LARGE) != 0 {
        return None;
    }
    let pt_entry = read_pte(pte_addr(pd_entry), pt_index(v));
    if !pte_present(pt_entry) {
        return None;
    }
    Some(pt_entry)
}

pub fn is_mapped(virt: VirtAddr) -> bool {
    get_physical(virt).is_some()
}

pub fn set_flags(virt: VirtAddr, flags: u64) -> i32 {
    unsafe {
        if !VMM_INITIALIZED {
            return -1;
        }
    }

    let virt = virt.align_down();
    let pml4_idx = pml4_index(virt.as_u64());
    let pdpt_idx = pdpt_index(virt.as_u64());
    let pd_idx = pd_index(virt.as_u64());
    let pt_idx = pt_index(virt.as_u64());

    let pml4 = unsafe { PML4_PHYS };
    let pml4_entry = read_pte(pml4, pml4_idx);
    if !pte_present(pml4_entry) {
        return -1;
    }

    let pdpt = pte_addr(pml4_entry);
    let pdpt_entry = read_pte(pdpt, pdpt_idx);
    if !pte_present(pdpt_entry) {
        return -1;
    }

    let pd = pte_addr(pdpt_entry);
    let pd_entry = read_pte(pd, pd_idx);
    if !pte_present(pd_entry) {
        return -1;
    }

    let pt = pte_addr(pd_entry);
    let pt_entry = read_pte(pt, pt_idx);
    if !pte_present(pt_entry) {
        return -1;
    }

    let phys = pte_addr(pt_entry).as_u64();
    write_pte(pt, pt_idx, phys | (flags & PTE_FLAGS_MASK) | PTE_PRESENT);

    invalidate_page(virt);
    0
}

pub fn get_pml4() -> PhysAddr {
    unsafe { PML4_PHYS }
}

pub fn get_kernel_pml4() -> PhysAddr {
    unsafe { PML4_PHYS }
}

pub fn switch(target_pml4: PhysAddr) {
    let mut target = target_pml4;
    if target.as_u64() == 0 {
        target = unsafe { PML4_PHYS };
    }

    let current = read_cr3() & PTE_ADDR_MASK;
    if current != target.as_u64() {
        write_cr3(target.as_u64());
    }
}

pub fn create_address_space() -> PhysAddr {
    let new_pml4 = alloc_page_table();
    if new_pml4.as_u64() == 0 {
        serial_println!("[VMM] ERROR: Failed to allocate PML4 for new address space");
        return PhysAddr::new(0);
    }

    let new_table = table_ptr(new_pml4);
    let kernel_table = table_ptr(unsafe { PML4_PHYS });

    unsafe {
        for i in 0..VMM_ENTRIES_PER_TABLE {
            ptr::write(new_table.add(i), 0);
        }
    }

    let kernel_entry0 = unsafe { ptr::read(kernel_table) };
    let mut new_pdpt = PhysAddr::new(0);

    if pte_present(kernel_entry0) {
        new_pdpt = alloc_page_table();
        if new_pdpt.as_u64() == 0 {
            pmm::free_page(new_pml4);
            return PhysAddr::new(0);
        }

        let new_pdpt_table = table_ptr(new_pdpt);
        let kernel_pdpt = pte_addr(kernel_entry0);
        let kernel_pdpt_table = table_ptr(kernel_pdpt);

        for i in 0..VMM_ENTRIES_PER_TABLE {
            let pdpte = unsafe { ptr::read(kernel_pdpt_table.add(i)) };
            if !pte_present(pdpte) {
                unsafe { ptr::write(new_pdpt_table.add(i), 0) };
                continue;
            }

            let kernel_pd = pte_addr(pdpte);
            let kernel_pd_table = table_ptr(kernel_pd);

            let mut has_only_large = true;
            for j in 0..VMM_ENTRIES_PER_TABLE {
                let pde = unsafe { ptr::read(kernel_pd_table.add(j)) };
                if pte_present(pde) && (pde & PTE_LARGE) == 0 {
                    has_only_large = false;
                    break;
                }
            }

            if has_only_large {
                unsafe { ptr::write(new_pdpt_table.add(i), pdpte) };
            } else {
                let new_pd = alloc_page_table();
                if new_pd.as_u64() == 0 {
                    serial_println!("[VMM] ERROR: Failed to allocate PD");
                    continue;
                }
                let new_pd_table = table_ptr(new_pd);

                for j in 0..VMM_ENTRIES_PER_TABLE {
                    let pde = unsafe { ptr::read(kernel_pd_table.add(j)) };
                    if pte_present(pde) && (pde & PTE_LARGE) != 0 {
                        unsafe { ptr::write(new_pd_table.add(j), pde) };
                    } else {
                        unsafe { ptr::write(new_pd_table.add(j), 0) };
                    }
                }

                let new_pdpte = new_pd.as_u64() | (pdpte & PTE_FLAGS_MASK);
                unsafe { ptr::write(new_pdpt_table.add(i), new_pdpte) };
            }
        }
    }

    unsafe {
        if pte_present(kernel_entry0) {
            ptr::write(
                new_table,
                new_pdpt.as_u64() | (kernel_entry0 & PTE_FLAGS_MASK),
            );
        }

        for i in 256..VMM_ENTRIES_PER_TABLE {
            ptr::write(new_table.add(i), ptr::read(kernel_table.add(i)));
        }
    }

    unsafe {
        if INIT_PML4_PHYS.as_u64() == 0 {
            INIT_PML4_PHYS = new_pml4;
        }
    }

    new_pml4
}

pub fn destroy_address_space(target_pml4: PhysAddr) {
    if target_pml4.as_u64() == 0 || target_pml4 == unsafe { PML4_PHYS } {
        return;
    }

    pmm::free_page(target_pml4);
}

pub fn map_page_in(target_pml4: PhysAddr, virt: VirtAddr, phys: PhysAddr, flags: u64) -> i32 {
    let mut pml4 = target_pml4;
    if pml4.as_u64() == 0 {
        pml4 = unsafe { PML4_PHYS };
    }

    let virt = virt.align_down();
    let phys = phys.align_down();

    let user_accessible = (flags & PTE_USER) != 0;

    let pml4_idx = pml4_index(virt.as_u64());
    let pdpt_idx = pdpt_index(virt.as_u64());
    let pd_idx = pd_index(virt.as_u64());
    let pt_idx = pt_index(virt.as_u64());

    let pdpt = get_or_create_table_in(pml4, pml4_idx, true, user_accessible);
    if pdpt.as_u64() == 0 {
        return -1;
    }

    let pd = get_or_create_table_in(pdpt, pdpt_idx, true, user_accessible);
    if pd.as_u64() == 0 {
        return -1;
    }

    let pt = get_or_create_table_in(pd, pd_idx, true, user_accessible);
    if pt.as_u64() == 0 {
        return -1;
    }

    write_pte(
        pt,
        pt_idx,
        phys.as_u64() | (flags & PTE_FLAGS_MASK) | PTE_PRESENT,
    );

    let current = read_cr3() & PTE_ADDR_MASK;
    if current == pml4.as_u64() {
        invalidate_page(virt);
    }

    0
}

pub fn unmap_page_in(target_pml4: PhysAddr, virt: VirtAddr) -> i32 {
    unsafe {
        if !VMM_INITIALIZED {
            return -1;
        }
    }

    let mut pml4 = target_pml4;
    if pml4.as_u64() == 0 {
        pml4 = unsafe { PML4_PHYS };
    }

    let virt = virt.align_down();

    let pml4_idx = pml4_index(virt.as_u64());
    let pdpt_idx = pdpt_index(virt.as_u64());
    let pd_idx = pd_index(virt.as_u64());
    let pt_idx = pt_index(virt.as_u64());

    let pml4_entry = read_pte(pml4, pml4_idx);
    if !pte_present(pml4_entry) {
        return -1;
    }

    let pdpt = pte_addr(pml4_entry);
    let pdpt_entry = read_pte(pdpt, pdpt_idx);
    if !pte_present(pdpt_entry) {
        return -1;
    }

    if (pdpt_entry & PTE_LARGE) != 0 {
        write_pte(pdpt, pdpt_idx, 0);
        invalidate_page(virt);
        return 0;
    }

    let pd = pte_addr(pdpt_entry);
    let pd_entry = read_pte(pd, pd_idx);
    if !pte_present(pd_entry) {
        return -1;
    }

    if (pd_entry & PTE_LARGE) != 0 {
        write_pte(pd, pd_idx, 0);
        invalidate_page(virt);
        return 0;
    }

    let pt = pte_addr(pd_entry);
    let pt_entry = read_pte(pt, pt_idx);
    if !pte_present(pt_entry) {
        return -1;
    }

    write_pte(pt, pt_idx, 0);

    let current = read_cr3() & PTE_ADDR_MASK;
    if current == pml4.as_u64() {
        invalidate_page(virt);
    }

    0
}

fn copy_bytes(dest: *mut u8, src: *const u8, size: usize) {
    unsafe {
        ptr::copy_nonoverlapping(src, dest, size);
    }
}

/// PTE value both parent and child use after a copy-on-write fork: writable
/// pages lose their hardware write bit and gain `PTE_COW`; read-only pages are
/// shared unchanged.
pub const fn cow_share_pte(pte: u64) -> u64 {
    if (pte & PTE_WRITE) != 0 {
        (pte & !PTE_WRITE) | PTE_COW
    } else {
        pte
    }
}

/// Adjust the PTE flags `mprotect` wants for one page. A page still shared
/// with another address space must not become hardware-writable, so it
/// becomes COW instead. Read-only protections drop the COW marker so that
/// writes fault as protection violations.
pub const fn cow_adjust_flags(flags: u64, page_shared: bool) -> u64 {
    if (flags & PTE_WRITE) == 0 {
        return flags & !PTE_COW;
    }
    if page_shared {
        (flags & !PTE_WRITE) | PTE_COW
    } else {
        flags & !PTE_COW
    }
}

/// Result of a COW write-fault resolution attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CowResolve {
    /// The page is not mapped or not marked COW; nothing to do.
    NotCow,
    /// The page is now privately writable.
    Resolved,
    /// A private copy was needed but no physical page was available.
    NoMemory,
}

/// What resolving one PTE requires, given whether its frame is still shared.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CowAction {
    None,
    /// Last owner: make the existing frame writable again, with these flags.
    Reclaim(u64),
    /// Still shared: copy into a fresh frame mapped with these flags.
    Copy(u64),
}

pub const fn cow_action(pte: u64, page_shared: bool) -> CowAction {
    if (pte & PTE_PRESENT) == 0 || (pte & PTE_COW) == 0 {
        return CowAction::None;
    }
    let flags = ((pte & PTE_FLAGS_MASK) | PTE_WRITE) & !PTE_COW;
    if page_shared {
        CowAction::Copy(flags)
    } else {
        CowAction::Reclaim(flags)
    }
}

/// Locate the leaf page-table slot of a 4 KiB mapping.
fn leaf_pte_slot(pml4: PhysAddr, virt: u64) -> Option<(PhysAddr, usize)> {
    let pml4e = read_pte(pml4, pml4_index(virt));
    if !pte_present(pml4e) {
        return None;
    }
    let pdpte = read_pte(pte_addr(pml4e), pdpt_index(virt));
    if !pte_present(pdpte) || (pdpte & PTE_LARGE) != 0 {
        return None;
    }
    let pde = read_pte(pte_addr(pdpte), pd_index(virt));
    if !pte_present(pde) || (pde & PTE_LARGE) != 0 {
        return None;
    }
    Some((pte_addr(pde), pt_index(virt)))
}

/// Break copy-on-write sharing for the page containing `virt`.
pub fn resolve_cow_in(target_pml4: PhysAddr, virt: VirtAddr) -> CowResolve {
    let mut pml4 = target_pml4;
    if pml4.as_u64() == 0 {
        pml4 = unsafe { PML4_PHYS };
    }
    let virt = virt.align_down();
    let (pt, idx) = match leaf_pte_slot(pml4, virt.as_u64()) {
        Some(slot) => slot,
        None => return CowResolve::NotCow,
    };
    let pte = read_pte(pt, idx);
    let old_phys = pte_addr(pte);
    let new_pte = match cow_action(pte, pmm::page_is_shared(old_phys)) {
        CowAction::None => return CowResolve::NotCow,
        CowAction::Reclaim(flags) => old_phys.as_u64() | flags,
        CowAction::Copy(flags) => {
            let new_phys = pmm::alloc_page();
            if new_phys.as_u64() == 0 {
                return CowResolve::NoMemory;
            }
            copy_bytes(
                new_phys.to_virt().as_u64() as *mut u8,
                old_phys.to_virt().as_u64() as *const u8,
                PAGE_SIZE as usize,
            );
            // Drops this mapping's share of the old frame.
            pmm::free_page(old_phys);
            new_phys.as_u64() | flags
        }
    };
    write_pte(pt, idx, new_pte);
    if (read_cr3() & PTE_ADDR_MASK) == pml4.as_u64() {
        invalidate_page(virt);
    }
    CowResolve::Resolved
}

/// Duplicate the user half of `src_pml4` into `dst_pml4` for `fork`.
///
/// Private pages are shared copy-on-write: both sides map the same frame with
/// the write bit cleared and `PTE_COW` set, and the frame's PMM share count is
/// bumped. Pages for which `skip(virt)` returns true are left unmapped in the
/// child; the caller uses this for shared file mappings, device mappings and
/// SysV shm, which are refaulted or re-attached instead. Frames the PMM cannot
/// share (unmanaged memory, share-count overflow) are copied eagerly.
pub fn clone_user_space(
    src_pml4: PhysAddr,
    dst_pml4: PhysAddr,
    skip: &dyn Fn(u64) -> bool,
) -> i32 {
    if src_pml4.as_u64() == 0 || dst_pml4.as_u64() == 0 {
        serial_println!("[VMM] ERROR: Invalid PML4 addresses");
        return -1;
    }

    let src_table = table_ptr(src_pml4);
    let mut pages_shared = 0u64;
    let mut pages_copied = 0u64;
    let mut result = 0;

    'outer: for pml4_idx in 0..256usize {
        let pml4e = unsafe { ptr::read(src_table.add(pml4_idx)) };
        if !pte_present(pml4e) {
            continue;
        }

        let pdpt_table = table_ptr(pte_addr(pml4e));

        for pdpt_idx in 0..VMM_ENTRIES_PER_TABLE {
            let pdpte = unsafe { ptr::read(pdpt_table.add(pdpt_idx)) };
            if !pte_present(pdpte) || (pdpte & PTE_LARGE) != 0 {
                continue;
            }

            let pd_table = table_ptr(pte_addr(pdpte));

            for pd_idx in 0..VMM_ENTRIES_PER_TABLE {
                let pde = unsafe { ptr::read(pd_table.add(pd_idx)) };
                if !pte_present(pde) || (pde & PTE_LARGE) != 0 {
                    continue;
                }

                let pt_table = table_ptr(pte_addr(pde));

                for pt_idx in 0..VMM_ENTRIES_PER_TABLE {
                    let pte = unsafe { ptr::read(pt_table.add(pt_idx)) };
                    if !pte_present(pte) {
                        continue;
                    }

                    let virt = ((pml4_idx as u64) << 39)
                        | ((pdpt_idx as u64) << 30)
                        | ((pd_idx as u64) << 21)
                        | ((pt_idx as u64) << 12);
                    if skip(virt) {
                        continue;
                    }

                    let src_phys = pte_addr(pte);
                    if pmm::page_share(src_phys) {
                        let shared = cow_share_pte(pte);
                        if shared != pte {
                            unsafe { ptr::write(pt_table.add(pt_idx), shared) };
                        }
                        if map_page_in(
                            dst_pml4,
                            VirtAddr::new(virt),
                            src_phys,
                            shared & PTE_FLAGS_MASK,
                        ) != 0
                        {
                            serial_println!("[VMM] ERROR: Failed to map COW page");
                            pmm::free_page(src_phys);
                            result = -1;
                            break 'outer;
                        }
                        pages_shared += 1;
                        continue;
                    }

                    let dst_phys = pmm::alloc_page();
                    if dst_phys.as_u64() == 0 {
                        serial_println!("[VMM] ERROR: Failed to allocate page for clone");
                        result = -1;
                        break 'outer;
                    }
                    copy_bytes(
                        dst_phys.to_virt().as_u64() as *mut u8,
                        src_phys.to_virt().as_u64() as *const u8,
                        PAGE_SIZE as usize,
                    );
                    if map_page_in(dst_pml4, VirtAddr::new(virt), dst_phys, pte & PTE_FLAGS_MASK)
                        != 0
                    {
                        serial_println!("[VMM] ERROR: Failed to map cloned page");
                        pmm::free_page(dst_phys);
                        result = -1;
                        break 'outer;
                    }
                    pages_copied += 1;
                }
            }
        }
    }

    // Parent PTEs may have lost their write bit; drop stale TLB entries.
    if (read_cr3() & PTE_ADDR_MASK) == src_pml4.as_u64() {
        flush_tlb();
    }

    serial_println!(
        "[VMM] Fork: {} pages shared copy-on-write, {} copied",
        pages_shared,
        pages_copied
    );
    result
}

pub fn clear_user_space(target_pml4: PhysAddr) {
    if target_pml4.as_u64() == 0 || target_pml4 == unsafe { PML4_PHYS } {
        serial_println!("[VMM] ERROR: Cannot clear kernel address space");
        return;
    }

    let pml4_table = table_ptr(target_pml4);

    for pml4_idx in 0..256usize {
        let pml4e = unsafe { ptr::read(pml4_table.add(pml4_idx)) };
        if !pte_present(pml4e) {
            continue;
        }

        let shared_with_kernel = pml4_idx == 0;

        let pdpt_phys = pte_addr(pml4e);
        let pdpt_table = table_ptr(pdpt_phys);

        for pdpt_idx in 0..VMM_ENTRIES_PER_TABLE {
            let pdpte = unsafe { ptr::read(pdpt_table.add(pdpt_idx)) };
            if !pte_present(pdpte) {
                continue;
            }

            let pd_phys = pte_addr(pdpte);
            let pd_table = table_ptr(pd_phys);

            for pd_idx in 0..VMM_ENTRIES_PER_TABLE {
                let pde = unsafe { ptr::read(pd_table.add(pd_idx)) };
                if !pte_present(pde) {
                    continue;
                }

                if (pde & PTE_LARGE) != 0 {
                    continue;
                }

                let pt_phys = pte_addr(pde);
                let pt_table = table_ptr(pt_phys);

                for pt_idx in 0..VMM_ENTRIES_PER_TABLE {
                    let pte = unsafe { ptr::read(pt_table.add(pt_idx)) };
                    if !pte_present(pte) {
                        continue;
                    }

                    let page_phys = pte_addr(pte);
                    if !syscall::vm_page_cache_release_mapped_phys(page_phys) {
                        pmm::free_page(page_phys);
                    }
                    unsafe { ptr::write(pt_table.add(pt_idx), 0) };
                }

                if !shared_with_kernel {
                    pmm::free_page(pt_phys);
                    unsafe { ptr::write(pd_table.add(pd_idx), 0) };
                }
            }

            if !shared_with_kernel {
                pmm::free_page(pd_phys);
                unsafe { ptr::write(pdpt_table.add(pdpt_idx), 0) };
            }
        }

        if !shared_with_kernel {
            pmm::free_page(pdpt_phys);
            unsafe { ptr::write(pml4_table.add(pml4_idx), 0) };
        }
    }
}

pub fn print_stats() {
    serial_println!("[VMM] Virtual Memory Statistics:");
    serial_println!("[VMM]   PML4 address: 0x{:x}", unsafe {
        PML4_PHYS.as_u64()
    });
    serial_println!("[VMM]   Mapped pages: {}", unsafe { MAPPED_PAGES });
    serial_println!("[VMM]   Page tables allocated: {}", unsafe {
        TABLES_ALLOCATED
    });
}

pub struct MappingGuard {
    virt: VirtAddr,
    count: usize,
}

impl MappingGuard {
    pub fn new(virt: VirtAddr, count: usize) -> Self {
        Self { virt, count }
    }
}

impl Drop for MappingGuard {
    fn drop(&mut self) {
        let _ = unmap_range(self.virt, self.count);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_helpers() {
        let addr = 0xFFFF_FFFF_8123_4567u64;
        assert_eq!(pml4_index(addr), ((addr >> 39) & 0x1FF) as usize);
        assert_eq!(pdpt_index(addr), ((addr >> 30) & 0x1FF) as usize);
        assert_eq!(pd_index(addr), ((addr >> 21) & 0x1FF) as usize);
        assert_eq!(pt_index(addr), ((addr >> 12) & 0x1FF) as usize);
        assert_eq!(page_offset(addr), addr & 0xFFF);
    }

    const PHYS: u64 = 0x0012_3000;

    #[test]
    fn cow_share_strips_write_and_marks_cow() {
        let rw = PHYS | PTE_PRESENT | PTE_WRITE | PTE_USER | PTE_NO_EXECUTE;
        let shared = cow_share_pte(rw);
        assert_eq!(shared & PTE_WRITE, 0);
        assert_ne!(shared & PTE_COW, 0);
        assert_eq!(pte_addr(shared).as_u64(), PHYS);
        assert_ne!(shared & PTE_NO_EXECUTE, 0);

        let ro = PHYS | PTE_PRESENT | PTE_USER;
        assert_eq!(cow_share_pte(ro), ro);
    }

    #[test]
    fn cow_action_reclaims_last_owner_and_copies_shared() {
        let cow = cow_share_pte(PHYS | PTE_PRESENT | PTE_WRITE | PTE_USER);
        match cow_action(cow, false) {
            CowAction::Reclaim(flags) => {
                assert_ne!(flags & PTE_WRITE, 0);
                assert_eq!(flags & PTE_COW, 0);
                assert_eq!(flags & PTE_ADDR_MASK, 0);
            }
            other => panic!("unexpected {:?}", other),
        }
        match cow_action(cow, true) {
            CowAction::Copy(flags) => {
                assert_ne!(flags & PTE_WRITE, 0);
                assert_ne!(flags & PTE_USER, 0);
                assert_eq!(flags & PTE_COW, 0);
            }
            other => panic!("unexpected {:?}", other),
        }
        // Plain read-only and non-present entries are not COW.
        assert_eq!(
            cow_action(PHYS | PTE_PRESENT | PTE_USER, true),
            CowAction::None
        );
        assert_eq!(cow_action(PHYS | PTE_COW, true), CowAction::None);
    }

    #[test]
    fn cow_adjust_flags_keeps_shared_pages_read_only() {
        let rw = PTE_PRESENT | PTE_WRITE | PTE_USER;
        let ro = PTE_PRESENT | PTE_USER;
        assert_eq!(cow_adjust_flags(rw, true), ro | PTE_COW);
        assert_eq!(cow_adjust_flags(rw, false), rw);
        assert_eq!(cow_adjust_flags(ro | PTE_COW, true), ro);
        assert_eq!(cow_adjust_flags(ro, false), ro);
    }
}
