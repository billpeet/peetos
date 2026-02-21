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

pub fn clone_user_space(src_pml4: PhysAddr, dst_pml4: PhysAddr) -> i32 {
    if src_pml4.as_u64() == 0 || dst_pml4.as_u64() == 0 {
        serial_println!("[VMM] ERROR: Invalid PML4 addresses");
        return -1;
    }

    let src_table = table_ptr(src_pml4);
    let mut pages_copied = 0u64;

    for pml4_idx in 0..256usize {
        let pml4e = unsafe { ptr::read(src_table.add(pml4_idx)) };
        if !pte_present(pml4e) {
            continue;
        }

        let src_pdpt = pte_addr(pml4e);
        let pdpt_table = table_ptr(src_pdpt);

        for pdpt_idx in 0..VMM_ENTRIES_PER_TABLE {
            let pdpte = unsafe { ptr::read(pdpt_table.add(pdpt_idx)) };
            if !pte_present(pdpte) {
                continue;
            }

            let src_pd = pte_addr(pdpte);
            let pd_table = table_ptr(src_pd);

            for pd_idx in 0..VMM_ENTRIES_PER_TABLE {
                let pde = unsafe { ptr::read(pd_table.add(pd_idx)) };
                if !pte_present(pde) {
                    continue;
                }

                if (pde & PTE_LARGE) != 0 {
                    continue;
                }

                let src_pt = pte_addr(pde);
                let pt_table = table_ptr(src_pt);

                for pt_idx in 0..VMM_ENTRIES_PER_TABLE {
                    let pte = unsafe { ptr::read(pt_table.add(pt_idx)) };
                    if !pte_present(pte) {
                        continue;
                    }

                    let src_phys = pte_addr(pte);
                    let flags = pte & PTE_FLAGS_MASK;

                    let dst_phys = pmm::alloc_page();
                    if dst_phys.as_u64() == 0 {
                        serial_println!("[VMM] ERROR: Failed to allocate page for clone");
                        return -1;
                    }

                    copy_bytes(
                        dst_phys.to_virt().as_u64() as *mut u8,
                        src_phys.to_virt().as_u64() as *const u8,
                        PAGE_SIZE as usize,
                    );

                    let virt = ((pml4_idx as u64) << 39)
                        | ((pdpt_idx as u64) << 30)
                        | ((pd_idx as u64) << 21)
                        | ((pt_idx as u64) << 12);

                    if map_page_in(dst_pml4, VirtAddr::new(virt), dst_phys, flags) != 0 {
                        serial_println!("[VMM] ERROR: Failed to map cloned page");
                        pmm::free_page(dst_phys);
                        return -1;
                    }

                    pages_copied += 1;
                }
            }
        }
    }

    serial_println!("[VMM] Cloned {} user pages", pages_copied);
    0
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
}
