//! Page-backed storage for `memfd_create` objects.
//!
//! A memfd is still a tmpfs inode (so it has a path, an inode number, and
//! works with `fstat`/`fcntl`), but its contents live here instead of in the
//! tmpfs inode's small inline buffer. That lets shared-memory buffers grow to
//! [`SHMEM_MAX_BYTES`] (Wayland pools routinely need megabytes).
//!
//! Storage is sparse: an object owns up to [`MAX_INDEX_PAGES`] index pages,
//! each holding 512 physical page addresses; data pages are allocated
//! zero-filled on first touch.
//!
//! Lifetimes:
//! - The object holds one PMM reference per data page. Every user mapping of
//!   a page takes another (`pmm::page_share`), dropped by the normal unmap,
//!   exit, and exec paths (`pmm::free_page`). Mapped pages therefore outlive
//!   the object.
//! - The object lives exactly as long as its (unlinked) tmpfs inode: fds and
//!   mappings hold VFS open references on the inode, and when the last one
//!   goes `vfs_inode_freed` calls [`destroy`]. The same storage backs the
//!   hidden files used for `MAP_SHARED | MAP_ANONYMOUS`.

use crate::sync::SpinLock;

const EFBIG: i64 = 27;
const ENOMEM: i64 = 12;
const ENFILE: i64 = 23;

pub const PAGE_SIZE: u64 = 4096;
pub const SHMEM_MAX_OBJECTS: usize = 64;
/// Largest memfd / shared-anonymous object (1 GiB). Objects are sparse:
/// pages are allocated on first touch, so this only sizes each object's
/// index (512 slots, 4 KiB). foot sizes its scrollback pool at 512 MiB.
pub const SHMEM_MAX_BYTES: u64 = 1024 * 1024 * 1024;
const ENTRIES_PER_INDEX: usize = (PAGE_SIZE as usize) / 8;
pub const MAX_INDEX_PAGES: usize =
    (SHMEM_MAX_BYTES / PAGE_SIZE) as usize / ENTRIES_PER_INDEX;

// ---------------------------------------------------------------------------
// Page allocation shim (real PMM in the kernel, std allocator in host tests)
// ---------------------------------------------------------------------------

#[cfg(not(test))]
mod pages {
    use super::PAGE_SIZE;
    use crate::memory::pmm;
    use arch_x86_64::addr::PhysAddr;

    pub fn alloc_zeroed() -> u64 {
        let phys = pmm::alloc_page();
        if phys.as_u64() == 0 {
            return 0;
        }
        unsafe {
            core::ptr::write_bytes(ptr(phys.as_u64()), 0, PAGE_SIZE as usize);
        }
        phys.as_u64()
    }

    pub fn free(phys: u64) {
        pmm::free_page(PhysAddr::new(phys));
    }

    pub fn ptr(phys: u64) -> *mut u8 {
        PhysAddr::new(phys).to_virt().as_u64() as *mut u8
    }
}

#[cfg(test)]
pub(crate) mod pages {
    use super::PAGE_SIZE;
    use std::alloc::{alloc_zeroed as std_alloc_zeroed, dealloc, Layout};
    use std::sync::atomic::{AtomicIsize, Ordering};

    /// Live test allocations, so tests can assert nothing leaks.
    pub static LIVE: AtomicIsize = AtomicIsize::new(0);

    fn layout() -> Layout {
        Layout::from_size_align(PAGE_SIZE as usize, PAGE_SIZE as usize).unwrap()
    }

    pub fn alloc_zeroed() -> u64 {
        LIVE.fetch_add(1, Ordering::SeqCst);
        unsafe { std_alloc_zeroed(layout()) as u64 }
    }

    pub fn free(phys: u64) {
        LIVE.fetch_sub(1, Ordering::SeqCst);
        unsafe { dealloc(phys as *mut u8, layout()) }
    }

    pub fn ptr(phys: u64) -> *mut u8 {
        phys as *mut u8
    }
}

// ---------------------------------------------------------------------------
// Objects
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Object {
    used: bool,
    inode: u16,
    size: u64,
    index: [u64; MAX_INDEX_PAGES],
}

impl Object {
    const EMPTY: Self = Self {
        used: false,
        inode: 0,
        size: 0,
        index: [0; MAX_INDEX_PAGES],
    };

    fn entry_slot(&self, page: usize) -> Option<*mut u64> {
        let index_phys = self.index[page / ENTRIES_PER_INDEX];
        if index_phys == 0 {
            return None;
        }
        let base = pages::ptr(index_phys) as *mut u64;
        Some(unsafe { base.add(page % ENTRIES_PER_INDEX) })
    }

    /// Physical address of a data page, or 0 when it was never touched.
    fn page(&self, page: usize) -> u64 {
        match self.entry_slot(page) {
            Some(slot) => unsafe { *slot },
            None => 0,
        }
    }

    /// Physical address of a data page, allocating it (and its index page)
    /// zero-filled if needed.
    fn page_or_alloc(&mut self, page: usize) -> Result<u64, i64> {
        if page >= MAX_INDEX_PAGES * ENTRIES_PER_INDEX {
            return Err(-EFBIG);
        }
        let idx = page / ENTRIES_PER_INDEX;
        if self.index[idx] == 0 {
            let phys = pages::alloc_zeroed();
            if phys == 0 {
                return Err(-ENOMEM);
            }
            self.index[idx] = phys;
        }
        let slot = self.entry_slot(page).ok_or(-ENOMEM)?;
        unsafe {
            if *slot == 0 {
                let phys = pages::alloc_zeroed();
                if phys == 0 {
                    return Err(-ENOMEM);
                }
                *slot = phys;
            }
            Ok(*slot)
        }
    }

    /// Drop data pages at or beyond `first_page` (the object's references;
    /// pages still mapped somewhere stay alive through their share counts).
    fn release_pages_from(&mut self, first_page: usize) {
        for idx in 0..MAX_INDEX_PAGES {
            let index_phys = self.index[idx];
            if index_phys == 0 {
                continue;
            }
            let base = pages::ptr(index_phys) as *mut u64;
            let mut any_left = false;
            for e in 0..ENTRIES_PER_INDEX {
                let page = idx * ENTRIES_PER_INDEX + e;
                unsafe {
                    let slot = base.add(e);
                    if *slot == 0 {
                        continue;
                    }
                    if page >= first_page {
                        pages::free(*slot);
                        *slot = 0;
                    } else {
                        any_left = true;
                    }
                }
            }
            if !any_left && idx * ENTRIES_PER_INDEX >= first_page {
                pages::free(index_phys);
                self.index[idx] = 0;
            }
        }
    }
}

struct ShmemState {
    objects: [Object; SHMEM_MAX_OBJECTS],
}

static STATE: SpinLock<ShmemState> = SpinLock::new(ShmemState {
    objects: [Object::EMPTY; SHMEM_MAX_OBJECTS],
});

fn find(state: &mut ShmemState, inode: u16) -> Option<&mut Object> {
    state.objects.iter_mut().find(|o| o.used && o.inode == inode)
}

/// Register a new, empty object for a freshly created memfd (or shared
/// anonymous mapping) inode.
pub fn create(inode: u16) -> Result<(), i64> {
    let mut state = STATE.lock();
    if let Some(stale) = find(&mut state, inode) {
        stale.release_pages_from(0);
        *stale = Object::EMPTY;
    }
    let obj = state
        .objects
        .iter_mut()
        .find(|o| !o.used)
        .ok_or(-ENFILE)?;
    *obj = Object::EMPTY;
    obj.used = true;
    obj.inode = inode;
    Ok(())
}

/// Free the object for `inode`, if any (called when the inode is freed).
pub fn destroy(inode: u16) {
    let mut state = STATE.lock();
    if let Some(obj) = find(&mut state, inode) {
        obj.release_pages_from(0);
        *obj = Object::EMPTY;
    }
}

pub fn exists(inode: u16) -> bool {
    let mut state = STATE.lock();
    find(&mut state, inode).is_some()
}

pub fn size(inode: u16) -> Option<u64> {
    let mut state = STATE.lock();
    find(&mut state, inode).map(|o| o.size)
}

/// Read up to `dst.len()` bytes at `offset`. `None` if `inode` is not a
/// shmem object. Reads past EOF return 0; holes read as zeros.
pub fn read(inode: u16, offset: u64, dst: &mut [u8]) -> Option<Result<usize, i64>> {
    let mut state = STATE.lock();
    let obj = find(&mut state, inode)?;
    if offset >= obj.size {
        return Some(Ok(0));
    }
    let len = core::cmp::min(dst.len() as u64, obj.size - offset) as usize;
    let mut done = 0usize;
    while done < len {
        let pos = offset + done as u64;
        let page = (pos / PAGE_SIZE) as usize;
        let in_page = (pos % PAGE_SIZE) as usize;
        let take = core::cmp::min(len - done, PAGE_SIZE as usize - in_page);
        let phys = obj.page(page);
        if phys == 0 {
            dst[done..done + take].fill(0);
        } else {
            unsafe {
                core::ptr::copy_nonoverlapping(
                    pages::ptr(phys).add(in_page),
                    dst[done..].as_mut_ptr(),
                    take,
                );
            }
        }
        done += take;
    }
    Some(Ok(len))
}

/// Write `src` at `offset`, growing the object if needed. `None` if `inode`
/// is not a shmem object.
pub fn write(inode: u16, offset: u64, src: &[u8]) -> Option<Result<usize, i64>> {
    let mut state = STATE.lock();
    let obj = find(&mut state, inode)?;
    let end = match offset.checked_add(src.len() as u64) {
        Some(end) if end <= SHMEM_MAX_BYTES => end,
        _ => return Some(Err(-EFBIG)),
    };
    let mut done = 0usize;
    while done < src.len() {
        let pos = offset + done as u64;
        let page = (pos / PAGE_SIZE) as usize;
        let in_page = (pos % PAGE_SIZE) as usize;
        let take = core::cmp::min(src.len() - done, PAGE_SIZE as usize - in_page);
        let phys = match obj.page_or_alloc(page) {
            Ok(phys) => phys,
            Err(err) => {
                if done > 0 {
                    break;
                }
                return Some(Err(err));
            }
        };
        unsafe {
            core::ptr::copy_nonoverlapping(src[done..].as_ptr(), pages::ptr(phys).add(in_page), take);
        }
        done += take;
    }
    let written_end = offset + done as u64;
    debug_assert!(written_end <= end);
    if written_end > obj.size {
        obj.size = written_end;
    }
    Some(Ok(done))
}

/// Set the object size. Shrinking drops the object's references to pages
/// wholly beyond the new end and zeroes the tail of the last partial page.
pub fn truncate(inode: u16, len: u64) -> Option<Result<(), i64>> {
    let mut state = STATE.lock();
    let obj = find(&mut state, inode)?;
    if len > SHMEM_MAX_BYTES {
        return Some(Err(-EFBIG));
    }
    if len < obj.size {
        let first_free = len.div_ceil(PAGE_SIZE) as usize;
        obj.release_pages_from(first_free);
        let in_page = (len % PAGE_SIZE) as usize;
        if in_page != 0 {
            let phys = obj.page((len / PAGE_SIZE) as usize);
            if phys != 0 {
                unsafe {
                    core::ptr::write_bytes(pages::ptr(phys).add(in_page), 0, PAGE_SIZE as usize - in_page);
                }
            }
        }
    }
    obj.size = len;
    Some(Ok(()))
}

/// Physical address of the data page backing byte offset `page_off` (page
/// aligned), allocating it if needed. The caller maps it and takes its own
/// PMM share reference. `None` if `inode` is not a shmem object.
pub fn fault_page(inode: u16, page_off: u64) -> Option<Result<u64, i64>> {
    let mut state = STATE.lock();
    let obj = find(&mut state, inode)?;
    if page_off >= obj.size.div_ceil(PAGE_SIZE) * PAGE_SIZE {
        // Access beyond the end of the object (Linux would raise SIGBUS).
        return Some(Err(-EFBIG));
    }
    Some(obj.page_or_alloc((page_off / PAGE_SIZE) as usize))
}

#[cfg(test)]
pub(crate) fn reset_for_test() {
    let mut state = STATE.lock();
    for obj in state.objects.iter_mut() {
        if obj.used {
            obj.release_pages_from(0);
        }
        *obj = Object::EMPTY;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;
    use std::sync::Mutex;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn setup() -> std::sync::MutexGuard<'static, ()> {
        let g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_for_test();
        g
    }

    fn live() -> isize {
        pages::LIVE.load(Ordering::SeqCst)
    }

    #[test]
    fn objects_hold_megabytes_sparsely() {
        let _g = setup();
        let base = live();
        create(7).unwrap();
        // 3 MiB: a full-screen 1024x768 XRGB buffer.
        assert_eq!(truncate(7, 3 * 1024 * 1024), Some(Ok(())));
        assert_eq!(size(7), Some(3 * 1024 * 1024));
        assert_eq!(live(), base, "truncate does not allocate");

        let mut buf = [0xAAu8; 16];
        assert_eq!(read(7, 2 * 1024 * 1024, &mut buf), Some(Ok(16)));
        assert!(buf.iter().all(|b| *b == 0), "holes read as zeros");

        assert_eq!(write(7, 3 * 1024 * 1024 - 2, b"xyz"), Some(Ok(3)));
        assert_eq!(size(7), Some(3 * 1024 * 1024 + 1));
        let mut out = [0u8; 3];
        assert_eq!(read(7, 3 * 1024 * 1024 - 2, &mut out), Some(Ok(3)));
        assert_eq!(&out, b"xyz");
        // Write spans two data pages; one index page was enough.
        assert_eq!(live(), base + 3);
    }

    #[test]
    fn fault_pages_are_stable_and_bounded_by_size() {
        let _g = setup();
        create(8).unwrap();
        truncate(8, 2 * PAGE_SIZE).unwrap().unwrap();
        let a = fault_page(8, 0).unwrap().unwrap();
        let b = fault_page(8, PAGE_SIZE).unwrap().unwrap();
        assert_ne!(a, b);
        assert_eq!(fault_page(8, 0), Some(Ok(a)), "same page on refault");
        assert_eq!(fault_page(8, 2 * PAGE_SIZE), Some(Err(-EFBIG)));
        // Data written through a mapping is visible to read().
        unsafe { *pages::ptr(b) = 0x5A };
        let mut one = [0u8; 1];
        read(8, PAGE_SIZE, &mut one).unwrap().unwrap();
        assert_eq!(one[0], 0x5A);
        assert_eq!(fault_page(99, 0), None, "not a shmem inode");
    }

    #[test]
    fn shrink_frees_tail_pages_and_zeroes_partial_page() {
        let _g = setup();
        let base = live();
        create(9).unwrap();
        write(9, 0, &[1u8; 3 * PAGE_SIZE as usize]).unwrap().unwrap();
        assert_eq!(live(), base + 4); // 3 data + 1 index
        truncate(9, PAGE_SIZE + 10).unwrap().unwrap();
        assert_eq!(live(), base + 3, "third page released");
        truncate(9, 2 * PAGE_SIZE).unwrap().unwrap();
        let mut tail = [9u8; 4];
        read(9, PAGE_SIZE + 10, &mut tail).unwrap().unwrap();
        assert_eq!(tail, [0; 4], "bytes beyond the old end read back as zero");
        assert_eq!(truncate(9, SHMEM_MAX_BYTES + 1), Some(Err(-EFBIG)));
    }

    #[test]
    fn destroy_returns_every_page() {
        let _g = setup();
        let base = live();
        create(10).unwrap();
        write(10, 0, &[3u8; 3 * PAGE_SIZE as usize]).unwrap().unwrap();
        assert!(live() > base);
        destroy(10);
        assert!(!exists(10));
        assert_eq!(live(), base, "all pages returned");
        destroy(10); // idempotent
        assert_eq!(read(10, 0, &mut [0u8; 1]), None);
    }

    #[test]
    fn table_exhaustion_reports_enfile() {
        let _g = setup();
        for ino in 0..SHMEM_MAX_OBJECTS as u16 {
            create(100 + ino).unwrap();
        }
        assert_eq!(create(1), Err(-ENFILE));
    }
}
