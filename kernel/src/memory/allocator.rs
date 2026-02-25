use core::alloc::{GlobalAlloc, Layout};
use core::cmp::min;
use core::ptr;

use super::heap;

struct KernelAllocator;

#[global_allocator]
static GLOBAL_ALLOCATOR: KernelAllocator = KernelAllocator;

unsafe impl GlobalAlloc for KernelAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let size = layout.size().max(1);
        heap::kmalloc_aligned(size, layout.align())
    }

    unsafe fn dealloc(&self, ptr: *mut u8, _layout: Layout) {
        heap::kfree(ptr);
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = self.alloc(layout);
        if !ptr.is_null() {
            ptr::write_bytes(ptr, 0, layout.size());
        }
        ptr
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if ptr.is_null() {
            return match Layout::from_size_align(new_size, layout.align()) {
                Ok(new_layout) => self.alloc(new_layout),
                Err(_) => ptr::null_mut(),
            };
        }

        if new_size == 0 {
            self.dealloc(ptr, layout);
            return ptr::null_mut();
        }

        let new_layout = match Layout::from_size_align(new_size, layout.align()) {
            Ok(layout) => layout,
            Err(_) => return ptr::null_mut(),
        };

        let new_ptr = self.alloc(new_layout);
        if new_ptr.is_null() {
            return ptr::null_mut();
        }

        ptr::copy_nonoverlapping(ptr, new_ptr, min(layout.size(), new_size));
        self.dealloc(ptr, layout);
        new_ptr
    }
}
