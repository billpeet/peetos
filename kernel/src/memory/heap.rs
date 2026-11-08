use core::mem;
use core::ptr;

use arch_x86_64::addr::{VirtAddr, PAGE_SIZE};

use crate::memory::pmm;
use crate::memory::vmm;
use crate::serial_println;
use crate::sync::IrqSpinLock;

const HEAP_START: u64 = 0xFFFF_FFFF_9000_0000;
const HEAP_MAX_SIZE: u64 = 64 * 1024 * 1024;
const HEAP_END: u64 = HEAP_START + HEAP_MAX_SIZE;

const KMALLOC_MIN_SIZE: usize = 32;
const KMALLOC_ALIGN: usize = 16;
const KMALLOC_MAGIC: u32 = 0xDEAD_C0DE;

const BLOCK_FREE: u32 = 0;
const BLOCK_USED: u32 = 1;

#[repr(C)]
struct BlockHeader {
    magic: u32,
    flags: u32,
    size: u64,
}

#[repr(C)]
struct FreeBlock {
    header: BlockHeader,
    next: *mut FreeBlock,
    prev: *mut FreeBlock,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct HeapStats {
    pub total_size: u64,
    pub used_size: u64,
    pub free_size: u64,
    pub alloc_count: u64,
    pub total_allocs: u64,
    pub total_frees: u64,
}

static mut FREE_LIST: *mut FreeBlock = ptr::null_mut();
static mut HEAP_CURRENT: u64 = HEAP_START;
static mut STAT_TOTAL_SIZE: u64 = 0;
static mut STAT_USED_SIZE: u64 = 0;
static mut STAT_ALLOC_COUNT: u64 = 0;
static mut STAT_TOTAL_ALLOCS: u64 = 0;
static mut STAT_TOTAL_FREES: u64 = 0;
static mut HEAP_INITIALIZED: bool = false;
/// Serialises the free list, expansion and statistics. Interrupt-safe and
/// independent of the big kernel lock. Lock order: heap -> PMM. Expansion
/// maps fresh kernel pages through `vmm::map_page` while holding it.
static HEAP_LOCK: IrqSpinLock<()> = IrqSpinLock::new(());

#[inline]
fn align_up(value: usize, align: usize) -> usize {
    (value + align - 1) & !(align - 1)
}

#[inline]
fn min_block_size() -> usize {
    mem::size_of::<BlockHeader>() + KMALLOC_MIN_SIZE
}

#[inline]
fn block_to_data(block: *mut BlockHeader) -> *mut u8 {
    unsafe { (block as *mut u8).add(mem::size_of::<BlockHeader>()) }
}

#[inline]
fn data_to_block(ptr: *mut u8) -> *mut BlockHeader {
    unsafe { ptr.sub(mem::size_of::<BlockHeader>()) as *mut BlockHeader }
}

fn heap_expand(min_size: usize) -> *mut FreeBlock {
    let mut pages_needed = (min_size + PAGE_SIZE as usize - 1) / PAGE_SIZE as usize;
    if pages_needed < 4 {
        pages_needed = 4;
    }

    let expand_size = pages_needed as u64 * PAGE_SIZE;

    unsafe {
        if HEAP_CURRENT + expand_size > HEAP_END {
            serial_println!("[KMALLOC] ERROR: Heap limit reached!");
            return ptr::null_mut();
        }
    }

    serial_println!("[KMALLOC] Expanding heap by {} KB", expand_size / 1024);

    for i in 0..pages_needed {
        let virt_addr = unsafe { HEAP_CURRENT + (i as u64 * PAGE_SIZE) };
        let phys = pmm::alloc_page();
        if phys.as_u64() == 0 {
            serial_println!("[KMALLOC] ERROR: PMM out of memory!");
            return ptr::null_mut();
        }

        if vmm::map_page(VirtAddr::new(virt_addr), phys, vmm::VMM_FLAG_KERNEL_RW) != 0 {
            serial_println!("[KMALLOC] ERROR: Failed to map heap page!");
            pmm::free_page(phys);
            return ptr::null_mut();
        }
    }

    let new_block = unsafe { HEAP_CURRENT as *mut FreeBlock };
    unsafe {
        (*new_block).header.magic = KMALLOC_MAGIC;
        (*new_block).header.flags = BLOCK_FREE;
        (*new_block).header.size = expand_size;
        (*new_block).next = ptr::null_mut();
        (*new_block).prev = ptr::null_mut();

        HEAP_CURRENT += expand_size;
        STAT_TOTAL_SIZE = STAT_TOTAL_SIZE.saturating_add(expand_size);
    }

    new_block
}

fn free_list_insert(block: *mut FreeBlock) {
    unsafe {
        (*block).header.flags = BLOCK_FREE;

        if FREE_LIST.is_null() {
            (*block).next = ptr::null_mut();
            (*block).prev = ptr::null_mut();
            FREE_LIST = block;
            return;
        }

        let mut current = FREE_LIST;
        let mut prev = ptr::null_mut();

        while !current.is_null() && current < block {
            prev = current;
            current = (*current).next;
        }

        (*block).next = current;
        (*block).prev = prev;

        if !prev.is_null() {
            (*prev).next = block;
        } else {
            FREE_LIST = block;
        }

        if !current.is_null() {
            (*current).prev = block;
        }
    }
}

fn free_list_remove(block: *mut FreeBlock) {
    unsafe {
        if !(*block).prev.is_null() {
            (*(*block).prev).next = (*block).next;
        } else {
            FREE_LIST = (*block).next;
        }

        if !(*block).next.is_null() {
            (*(*block).next).prev = (*block).prev;
        }

        (*block).next = ptr::null_mut();
        (*block).prev = ptr::null_mut();
    }
}

fn coalesce(block: *mut FreeBlock) {
    unsafe {
        if !(*block).next.is_null() {
            let block_end = (block as *mut u8).add((*block).header.size as usize);
            if block_end == (*block).next as *mut u8 {
                (*block).header.size += (*(*block).next).header.size;
                free_list_remove((*block).next);
            }
        }

        if !(*block).prev.is_null() {
            let prev_end = ((*block).prev as *mut u8).add((*(*block).prev).header.size as usize);
            if prev_end == block as *mut u8 {
                (*(*block).prev).header.size += (*block).header.size;
                free_list_remove(block);
            }
        }
    }
}

fn split_block(block: *mut FreeBlock, needed_size: usize) {
    unsafe {
        let remaining = (*block).header.size as usize - needed_size;
        if remaining < min_block_size() {
            return;
        }

        let new_block = (block as *mut u8).add(needed_size) as *mut FreeBlock;
        (*new_block).header.magic = KMALLOC_MAGIC;
        (*new_block).header.flags = BLOCK_FREE;
        (*new_block).header.size = remaining as u64;

        (*block).header.size = needed_size as u64;

        free_list_insert(new_block);
    }
}

fn find_free_block(size: usize) -> *mut FreeBlock {
    unsafe {
        let mut current = FREE_LIST;
        while !current.is_null() {
            if (*current).header.size as usize >= size {
                return current;
            }
            current = (*current).next;
        }
    }
    ptr::null_mut()
}

pub fn init() {
    unsafe {
        if HEAP_INITIALIZED {
            return;
        }
    }

    let block = heap_expand(4 * PAGE_SIZE as usize);
    if block.is_null() {
        return;
    }

    free_list_insert(block);

    unsafe {
        HEAP_INITIALIZED = true;
    }

    serial_println!("[KMALLOC] Heap initialized at 0x{:x}", HEAP_START);
}

pub fn kmalloc(size: usize) -> *mut u8 {
    let _guard = HEAP_LOCK.lock();
    kmalloc_locked(size)
}

fn kmalloc_locked(size: usize) -> *mut u8 {
    if size == 0 {
        return ptr::null_mut();
    }

    unsafe {
        if !HEAP_INITIALIZED {
            serial_println!("[KMALLOC] ERROR: kmalloc called before init!");
            return ptr::null_mut();
        }
    }

    let mut total_size = mem::size_of::<BlockHeader>() + size;
    total_size = align_up(total_size, KMALLOC_ALIGN);

    let mut block = find_free_block(total_size);
    if block.is_null() {
        block = heap_expand(total_size);
        if block.is_null() {
            return ptr::null_mut();
        }
        free_list_insert(block);
    }

    free_list_remove(block);
    split_block(block, total_size);

    unsafe {
        (*block).header.flags = BLOCK_USED;
        STAT_USED_SIZE = STAT_USED_SIZE.saturating_add((*block).header.size);
        STAT_ALLOC_COUNT = STAT_ALLOC_COUNT.saturating_add(1);
        STAT_TOTAL_ALLOCS = STAT_TOTAL_ALLOCS.saturating_add(1);
    }

    block_to_data(block as *mut BlockHeader)
}

pub fn kzalloc(size: usize) -> *mut u8 {
    let ptr = kmalloc(size);
    if !ptr.is_null() {
        unsafe {
            ptr::write_bytes(ptr, 0, size);
        }
    }
    ptr
}

pub fn kmalloc_aligned(size: usize, align: usize) -> *mut u8 {
    if align <= KMALLOC_ALIGN {
        return kmalloc(size);
    }

    let total = size
        .saturating_add(align)
        .saturating_add(mem::size_of::<*mut u8>());
    let ptr = kmalloc(total);
    if ptr.is_null() {
        return ptr::null_mut();
    }

    let mut addr = ptr as usize + mem::size_of::<*mut u8>();
    addr = align_up(addr, align);

    unsafe {
        let orig_ptr = (addr - mem::size_of::<*mut u8>()) as *mut *mut u8;
        *orig_ptr = ptr;
    }

    addr as *mut u8
}

fn resolve_header(ptr: *mut u8) -> Option<(*mut BlockHeader, usize)> {
    if ptr.is_null() {
        return None;
    }

    let header = data_to_block(ptr);
    unsafe {
        if (*header).magic == KMALLOC_MAGIC {
            return Some((header, 0));
        }
    }

    unsafe {
        let orig_slot = (ptr as *mut *mut u8).offset(-1);
        let orig_ptr = *orig_slot;
        if orig_ptr.is_null() {
            return None;
        }
        let header = data_to_block(orig_ptr);
        if (*header).magic == KMALLOC_MAGIC {
            let offset = ptr as usize - orig_ptr as usize;
            return Some((header, offset));
        }
    }

    None
}

pub fn krealloc(ptr: *mut u8, size: usize) -> *mut u8 {
    if ptr.is_null() {
        return kmalloc(size);
    }

    if size == 0 {
        kfree(ptr);
        return ptr::null_mut();
    }

    let Some((header, offset)) = resolve_header(ptr) else {
        serial_println!("[KMALLOC] ERROR: Invalid realloc pointer");
        return ptr::null_mut();
    };

    let header_size = mem::size_of::<BlockHeader>();
    let total_payload = unsafe { (*header).size as usize };
    let mut old_size = total_payload.saturating_sub(header_size + offset);

    let mut needed = header_size + size + offset;
    needed = align_up(needed, KMALLOC_ALIGN);

    if needed <= total_payload {
        return ptr;
    }

    let new_ptr = kmalloc(size);
    if new_ptr.is_null() {
        return ptr::null_mut();
    }

    if old_size > size {
        old_size = size;
    }

    unsafe {
        ptr::copy_nonoverlapping(ptr, new_ptr, old_size);
    }

    kfree(ptr);
    new_ptr
}

pub fn kfree(ptr: *mut u8) {
    let _guard = HEAP_LOCK.lock();
    kfree_locked(ptr)
}

fn kfree_locked(ptr: *mut u8) {
    if ptr.is_null() {
        return;
    }

    unsafe {
        if !HEAP_INITIALIZED {
            serial_println!("[KMALLOC] ERROR: kfree called before init!");
            return;
        }
    }

    let Some((header, _offset)) = resolve_header(ptr) else {
        serial_println!("[KMALLOC] ERROR: Invalid free - bad magic!");
        return;
    };

    unsafe {
        if (*header).flags == BLOCK_FREE {
            serial_println!("[KMALLOC] ERROR: Double free detected!");
            return;
        }

        STAT_USED_SIZE = STAT_USED_SIZE.saturating_sub((*header).size);
        STAT_ALLOC_COUNT = STAT_ALLOC_COUNT.saturating_sub(1);
        STAT_TOTAL_FREES = STAT_TOTAL_FREES.saturating_add(1);

        let block = header as *mut FreeBlock;
        free_list_insert(block);
        coalesce(block);
    }
}

pub fn get_stats() -> HeapStats {
    let _guard = HEAP_LOCK.lock();
    unsafe {
        let total = STAT_TOTAL_SIZE;
        let used = STAT_USED_SIZE;
        HeapStats {
            total_size: total,
            used_size: used,
            free_size: total.saturating_sub(used),
            alloc_count: STAT_ALLOC_COUNT,
            total_allocs: STAT_TOTAL_ALLOCS,
            total_frees: STAT_TOTAL_FREES,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn align_up_rounds() {
        assert_eq!(align_up(1, 16), 16);
        assert_eq!(align_up(16, 16), 16);
        assert_eq!(align_up(31, 16), 32);
    }

    #[test]
    fn resolve_header_accepts_regular_ptr() {
        let header = BlockHeader {
            magic: KMALLOC_MAGIC,
            flags: BLOCK_USED,
            size: 64,
        };

        let mut buffer = [0u8; 128];
        let header_ptr = buffer.as_mut_ptr() as *mut BlockHeader;
        unsafe {
            *header_ptr = header;
        }

        let data_ptr = unsafe { (header_ptr as *mut u8).add(mem::size_of::<BlockHeader>()) };
        let resolved = resolve_header(data_ptr);
        assert!(resolved.is_some());
    }
}
