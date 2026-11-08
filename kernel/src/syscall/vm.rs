//! User virtual memory: mmap regions, page faults, the file-backed page cache, and brk.

use super::*;

#[inline]
pub(super) fn vm_page_cache_eligible_inode(inode: u16) -> bool {
    fs::inode_backend_is_ext2(inode)
}

#[inline]
pub(super) fn mmap_mapping_is_framebuffer(region: &VmMapping) -> bool {
    region.file_inode != task::USER_FD_NONE && fs::inode_is_framebuffer(region.file_inode)
}

#[inline]
pub(super) fn mmap_region_is_reclaimable(region: &VmMapping) -> bool {
    if region.file_inode == task::USER_FD_NONE {
        return false;
    }
    if mmap_mapping_is_framebuffer(region) {
        return false;
    }
    if region.map_shared {
        return true;
    }
    (region.prot & PROT_WRITE) == 0
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum UnmappedPageRelease {
    Freed,
    CacheBacked,
    Device,
}

pub(super) fn release_unmapped_user_page(
    mapping: Option<VmMapping>,
    phys: PhysAddr,
) -> UnmappedPageRelease {
    if let Some(mapping) = mapping {
        if mmap_mapping_is_framebuffer(&mapping) {
            return UnmappedPageRelease::Device;
        }
        if mapping.map_shared
            && mapping.file_inode != task::USER_FD_NONE
            && vm_page_cache_release_mapped_phys(phys)
        {
            return UnmappedPageRelease::CacheBacked;
        }
    }

    pmm::free_page(phys);
    UnmappedPageRelease::Freed
}

pub(super) fn vm_page_cache_reclaim_unmapped_pages(target_pages: usize) -> usize {
    if target_pages == 0 {
        return 0;
    }
    let mut reclaimed = 0usize;
    for idx in 0..VM_PAGE_CACHE_MAX_PAGES {
        if reclaimed >= target_pages {
            break;
        }
        let (valid, page_phys, mapped_count) = unsafe {
            let entry = VM_PAGE_CACHE[idx];
            (entry.valid, entry.page_phys, entry.mapped_count)
        };
        if !valid || mapped_count != 0 || page_phys == 0 {
            continue;
        }
        vm_page_cache_free_entry(idx);
        let now_empty = unsafe { !VM_PAGE_CACHE[idx].valid };
        if now_empty {
            reclaimed += 1;
        }
    }
    reclaimed
}

pub(super) fn reclaim_user_vm_pages(target_pages: usize) -> usize {
    if target_pages == 0 {
        return 0;
    }

    let mut reclaimed = vm_page_cache_reclaim_unmapped_pages(target_pages);
    if reclaimed >= target_pages {
        return reclaimed;
    }

    for task_idx in 0..task::MAX_TASKS {
        let task_ptr = task::get_by_index(task_idx);
        if task_ptr.is_null() {
            continue;
        }
        let (is_user, pml4) = unsafe {
            (
                ((*task_ptr).flags & TASK_FLAG_USER) != 0,
                PhysAddr::new((*task_ptr).pml4_phys),
            )
        };
        if !is_user || pml4.as_u64() == 0 {
            continue;
        }

        let slot = match mmap_prepare_task_slot(task_ptr) {
            Some(slot) => slot,
            None => continue,
        };

        for region_idx in 0..MMAP_MAX_REGIONS_PER_TASK {
            let region = unsafe { TASK_VM_MAPPINGS[slot][region_idx] };
            if region.is_empty() || !mmap_region_is_reclaimable(&region) {
                continue;
            }

            let mut page = region.start;
            while page < region.end() {
                let virt = VirtAddr::new(page);
                let phys = match vmm::get_physical_in(pml4, virt) {
                    Some(phys) => PhysAddr::new(phys.as_u64() & !(PAGE_SIZE - 1)),
                    None => {
                        page = page.saturating_add(PAGE_SIZE);
                        continue;
                    }
                };

                if vmm::unmap_page_in(pml4, virt) == 0 {
                    if release_unmapped_user_page(Some(region), phys) == UnmappedPageRelease::Freed
                    {
                        reclaimed += 1;
                    }

                    if reclaimed >= target_pages {
                        return reclaimed;
                    }
                }
                page = page.saturating_add(PAGE_SIZE);
            }
        }
    }

    reclaimed += vm_page_cache_reclaim_unmapped_pages(target_pages.saturating_sub(reclaimed));
    reclaimed
}

pub(super) fn alloc_user_vm_page_with_reclaim() -> PhysAddr {
    let page = pmm::alloc_page();
    if page.as_u64() != 0 {
        return page;
    }
    let _ = reclaim_user_vm_pages(USER_VM_RECLAIM_TARGET_PAGES);
    pmm::alloc_page()
}

pub(super) fn vm_page_cache_free_entry(idx: usize) {
    let should_writeback = unsafe {
        let entry = VM_PAGE_CACHE[idx];
        entry.valid && entry.mapped_count == 0 && entry.dirty
    };
    if should_writeback {
        let _ = vm_page_cache_writeback_idx(idx);
    }
    unsafe {
        let entry = &mut VM_PAGE_CACHE[idx];
        if entry.valid && entry.mapped_count != 0 {
            return;
        }
        if entry.valid && entry.page_phys != 0 {
            pmm::free_page(PhysAddr::new(entry.page_phys));
        }
        *entry = VmPageCacheEntry::EMPTY;
    }
}

pub(super) fn vm_page_cache_reset() {
    for idx in 0..VM_PAGE_CACHE_MAX_PAGES {
        vm_page_cache_free_entry(idx);
    }
    unsafe {
        VM_PAGE_CACHE_CLOCK_HAND = 0;
    }
}

pub(super) fn vm_page_cache_lookup_idx(inode: u16, page_off: u64) -> Option<usize> {
    unsafe {
        for idx in 0..VM_PAGE_CACHE_MAX_PAGES {
            let entry = &mut VM_PAGE_CACHE[idx];
            if entry.valid && entry.inode == inode && entry.page_off == page_off {
                entry.referenced = true;
                return Some(idx);
            }
        }
    }
    None
}

pub(super) fn vm_page_cache_acquire_slot() -> Result<usize, i64> {
    for idx in 0..VM_PAGE_CACHE_MAX_PAGES {
        unsafe {
            if !VM_PAGE_CACHE[idx].valid {
                return Ok(idx);
            }
        }
    }

    let mut scanned = 0usize;
    while scanned < VM_PAGE_CACHE_MAX_PAGES * 2 {
        let idx = unsafe {
            let idx = VM_PAGE_CACHE_CLOCK_HAND;
            VM_PAGE_CACHE_CLOCK_HAND = (VM_PAGE_CACHE_CLOCK_HAND + 1) % VM_PAGE_CACHE_MAX_PAGES;
            idx
        };
        unsafe {
            if VM_PAGE_CACHE[idx].referenced {
                VM_PAGE_CACHE[idx].referenced = false;
                scanned += 1;
                continue;
            }
            if VM_PAGE_CACHE[idx].mapped_count != 0 {
                scanned += 1;
                continue;
            }
        }
        vm_page_cache_writeback_idx(idx)?;
        vm_page_cache_free_entry(idx);
        return Ok(idx);
    }
    Err(-ENOMEM)
}

pub(super) fn vm_page_cache_fill_entry(idx: usize, inode: u16, page_off: u64) -> Result<(), i64> {
    let page_phys = alloc_user_vm_page_with_reclaim();
    if page_phys.as_u64() == 0 {
        return Err(-ENOMEM);
    }

    let page_ptr = page_phys.to_virt().as_u64() as *mut u8;
    unsafe {
        ptr::write_bytes(page_ptr, 0, PAGE_SIZE as usize);
    }
    let page_buf = unsafe { core::slice::from_raw_parts_mut(page_ptr, PAGE_SIZE as usize) };

    let mut loaded = 0usize;
    while loaded < page_buf.len() {
        let read_n = fs::read(inode, page_off + loaded as u64, &mut page_buf[loaded..])
            .map_err(|err| -fs_to_errno(err))?;
        if read_n == 0 {
            break;
        }
        loaded += read_n;
    }

    unsafe {
        VM_PAGE_CACHE[idx] = VmPageCacheEntry {
            inode,
            page_off,
            page_phys: page_phys.as_u64(),
            loaded_len: loaded as u16,
            valid: true,
            referenced: true,
            mapped_count: 0,
            dirty: false,
        };
    }
    Ok(())
}

pub(super) fn vm_page_cache_get_entry(inode: u16, page_off: u64) -> Result<VmPageCacheEntry, i64> {
    if let Some(idx) = vm_page_cache_lookup_idx(inode, page_off) {
        unsafe {
            return Ok(VM_PAGE_CACHE[idx]);
        }
    }

    let idx = vm_page_cache_acquire_slot()?;
    vm_page_cache_fill_entry(idx, inode, page_off)?;
    unsafe { Ok(VM_PAGE_CACHE[idx]) }
}

pub(super) fn vm_page_cache_pin_mapped_page(
    inode: u16,
    page_off: u64,
    writable: bool,
) -> Result<VmPageCacheEntry, i64> {
    let idx = if let Some(idx) = vm_page_cache_lookup_idx(inode, page_off) {
        idx
    } else {
        let idx = vm_page_cache_acquire_slot()?;
        vm_page_cache_fill_entry(idx, inode, page_off)?;
        idx
    };

    unsafe {
        let entry = &mut VM_PAGE_CACHE[idx];
        entry.mapped_count = entry.mapped_count.saturating_add(1);
        entry.referenced = true;
        if writable {
            entry.dirty = true;
        }
        Ok(*entry)
    }
}

pub(crate) fn vm_page_cache_release_mapped_phys(phys: PhysAddr) -> bool {
    let page_phys = phys.as_u64() & !(PAGE_SIZE - 1);
    unsafe {
        for idx in 0..VM_PAGE_CACHE_MAX_PAGES {
            let entry = &mut VM_PAGE_CACHE[idx];
            if !entry.valid || entry.page_phys != page_phys {
                continue;
            }
            if entry.mapped_count != 0 {
                entry.mapped_count -= 1;
            }
            entry.referenced = true;
            return true;
        }
    }
    false
}

pub(super) fn vm_page_cache_mark_dirty_by_phys(phys: PhysAddr) -> bool {
    let page_phys = phys.as_u64() & !(PAGE_SIZE - 1);
    unsafe {
        for idx in 0..VM_PAGE_CACHE_MAX_PAGES {
            let entry = &mut VM_PAGE_CACHE[idx];
            if !entry.valid || entry.page_phys != page_phys {
                continue;
            }
            entry.dirty = true;
            entry.referenced = true;
            return true;
        }
    }
    false
}

pub(super) fn vm_page_cache_writeback_idx(idx: usize) -> Result<usize, i64> {
    let (inode, page_off, page_phys, loaded_len, valid, dirty) = unsafe {
        let entry = VM_PAGE_CACHE[idx];
        (
            entry.inode,
            entry.page_off,
            entry.page_phys,
            entry.loaded_len,
            entry.valid,
            entry.dirty,
        )
    };

    if !valid || !dirty {
        return Ok(0);
    }
    if loaded_len == 0 {
        unsafe {
            if VM_PAGE_CACHE[idx].valid {
                VM_PAGE_CACHE[idx].dirty = false;
            }
        }
        return Ok(0);
    }

    let total = loaded_len as usize;
    let page_ptr = PhysAddr::new(page_phys).to_virt().as_u64() as *const u8;
    let page = unsafe { core::slice::from_raw_parts(page_ptr, PAGE_SIZE as usize) };
    let mut written = 0usize;
    while written < total {
        let write_res = fs::write(
            inode,
            page_off.saturating_add(written as u64),
            &page[written..total],
            false,
        )
        .map_err(|err| -fs_to_errno(err))?;
        if write_res.written == 0 {
            return Err(-EIO);
        }
        written += write_res.written;
    }

    unsafe {
        let entry = &mut VM_PAGE_CACHE[idx];
        if entry.valid
            && entry.page_phys == page_phys
            && entry.inode == inode
            && entry.page_off == page_off
        {
            entry.dirty = false;
            if written > entry.loaded_len as usize {
                entry.loaded_len = written as u16;
            }
        }
    }
    Ok(written)
}

pub(super) fn vm_page_cache_writeback_page(inode: u16, page_off: u64) -> Result<usize, i64> {
    unsafe {
        for idx in 0..VM_PAGE_CACHE_MAX_PAGES {
            let entry = VM_PAGE_CACHE[idx];
            if entry.valid && entry.inode == inode && entry.page_off == page_off {
                return vm_page_cache_writeback_idx(idx);
            }
        }
    }
    Ok(0)
}

pub(super) fn vm_page_cache_writeback_inode(inode: u16) -> Result<usize, i64> {
    let mut written = 0usize;
    for idx in 0..VM_PAGE_CACHE_MAX_PAGES {
        let should_flush = unsafe { VM_PAGE_CACHE[idx].valid && VM_PAGE_CACHE[idx].inode == inode };
        if should_flush {
            written = written.saturating_add(vm_page_cache_writeback_idx(idx)?);
        }
    }
    Ok(written)
}

pub(super) fn vm_page_cache_writeback_all() -> Result<usize, i64> {
    let mut written = 0usize;
    for idx in 0..VM_PAGE_CACHE_MAX_PAGES {
        written = written.saturating_add(vm_page_cache_writeback_idx(idx)?);
    }
    Ok(written)
}

pub(super) fn vm_page_cache_read(inode: u16, offset: u64, dst: &mut [u8]) -> Result<usize, i64> {
    if dst.is_empty() {
        return Ok(0);
    }
    if !vm_page_cache_eligible_inode(inode) {
        refresh_procfs_runtime();
        return fs::read(inode, offset, dst).map_err(|err| -fs_to_errno(err));
    }

    let mut copied = 0usize;
    while copied < dst.len() {
        let file_off = offset.saturating_add(copied as u64);
        let page_off = align_down(file_off, PAGE_SIZE);
        let in_page = (file_off - page_off) as usize;
        let entry = vm_page_cache_get_entry(inode, page_off)?;
        let loaded = entry.loaded_len as usize;
        if in_page >= loaded {
            break;
        }

        let available = loaded - in_page;
        let take = cmp::min(available, dst.len() - copied);
        let page_ptr = PhysAddr::new(entry.page_phys).to_virt().as_u64() as *const u8;
        unsafe {
            ptr::copy_nonoverlapping(page_ptr.add(in_page), dst.as_mut_ptr().add(copied), take);
        }
        copied += take;
        if take < PAGE_SIZE as usize - in_page {
            break;
        }
    }
    Ok(copied)
}

pub(super) fn vm_page_cache_invalidate_inode(inode: u16) {
    for idx in 0..VM_PAGE_CACHE_MAX_PAGES {
        unsafe {
            if VM_PAGE_CACHE[idx].valid && VM_PAGE_CACHE[idx].inode == inode {
                vm_page_cache_free_entry(idx);
            }
        }
    }
}

pub(super) fn vm_page_cache_invalidate_inode_range(inode: u16, offset: u64, len: u64) {
    if len == 0 {
        return;
    }
    let start = align_down(offset, PAGE_SIZE);
    let end = match offset.checked_add(len) {
        Some(v) => page_align_up(v).unwrap_or(u64::MAX),
        None => u64::MAX,
    };

    for idx in 0..VM_PAGE_CACHE_MAX_PAGES {
        let should_invalidate = unsafe {
            let entry = VM_PAGE_CACHE[idx];
            if !entry.valid || entry.inode != inode {
                false
            } else {
                let page_end = entry.page_off.saturating_add(PAGE_SIZE);
                start < page_end && entry.page_off < end
            }
        };
        if should_invalidate {
            vm_page_cache_free_entry(idx);
        }
    }
}

pub(super) fn ensure_user_pages_unmapped(pml4: PhysAddr, start: u64, end: u64) -> Result<(), i64> {
    let mut addr = start;
    while addr < end {
        if vmm::get_physical_in(pml4, VirtAddr::new(addr)).is_some() {
            return Err(ENOMEM);
        }
        addr += PAGE_SIZE;
    }
    Ok(())
}

pub(super) fn map_user_pages(pml4: PhysAddr, start: u64, end: u64) -> Result<(), i64> {
    let mut addr = start;
    while addr < end {
        let phys = alloc_user_vm_page_with_reclaim();
        if phys.as_u64() == 0 {
            return Err(ENOMEM);
        }

        unsafe {
            ptr::write_bytes(phys.to_virt().as_u64() as *mut u8, 0, PAGE_SIZE as usize);
        }

        if vmm::map_page_in(
            pml4,
            VirtAddr::new(addr),
            phys,
            vmm::PTE_PRESENT | vmm::PTE_WRITE | vmm::PTE_USER | vmm::PTE_NO_EXECUTE,
        ) != 0
        {
            pmm::free_page(phys);
            return Err(ENOMEM);
        }

        addr += PAGE_SIZE;
    }

    Ok(())
}

pub(super) fn unmap_user_pages_in_regions(
    pml4: PhysAddr,
    start: u64,
    end: u64,
    regions: Option<&[VmMapping; MMAP_MAX_REGIONS_PER_TASK]>,
) {
    let mut addr = start;
    while addr < end {
        let virt = VirtAddr::new(addr);
        if let Some(phys) = vmm::get_physical_in(pml4, virt) {
            if vmm::unmap_page_in(pml4, virt) == 0 {
                let phys_page = PhysAddr::new(phys.as_u64() & !(PAGE_SIZE - 1));
                let mapping = regions.and_then(|entries| mmap_regions_find_mapping(entries, addr));
                let _ = release_unmapped_user_page(mapping, phys_page);
            }
        }
        addr += PAGE_SIZE;
    }
}

pub(super) fn unmap_user_pages(pml4: PhysAddr, start: u64, end: u64) {
    unmap_user_pages_in_regions(pml4, start, end, None);
}

pub(super) fn mmap_flags_are_supported(flags: u32) -> bool {
    let supported = MAP_SHARED
        | MAP_PRIVATE
        | MAP_FIXED
        | MAP_ANONYMOUS
        | MAP_DENYWRITE
        | MAP_EXECUTABLE
        | MAP_FIXED_NOREPLACE;
    (flags & !supported) == 0
}

pub(super) fn mmap_parse_sharing_mode(flags: u32) -> Result<(bool, bool), i64> {
    let is_private = (flags & MAP_PRIVATE) != 0;
    let is_shared = (flags & MAP_SHARED) != 0;
    if is_private == is_shared {
        return Err(-EINVAL);
    }
    Ok((is_private, is_shared))
}

pub(super) fn msync_flags_are_valid(flags: u32) -> bool {
    let supported = MS_ASYNC | MS_SYNC | MS_INVALIDATE;
    if (flags & !supported) != 0 {
        return false;
    }
    if (flags & MS_ASYNC) != 0 && (flags & MS_SYNC) != 0 {
        return false;
    }
    true
}

pub(super) fn mmap_prot_to_pte_flags(prot: u32) -> Result<u64, i64> {
    if (prot & !(PROT_READ | PROT_WRITE | PROT_EXEC)) != 0 {
        return Err(-EINVAL);
    }

    let mut flags = vmm::PTE_PRESENT;
    if prot != PROT_NONE {
        flags |= vmm::PTE_USER;
    }
    if (prot & PROT_WRITE) != 0 {
        flags |= vmm::PTE_WRITE;
    }
    if (prot & PROT_EXEC) == 0 {
        flags |= vmm::PTE_NO_EXECUTE;
    }

    Ok(flags)
}

pub(super) fn mmap_task_slot(task_ptr: *mut task::Task) -> Option<usize> {
    // Threads share their leader's mapping table.
    let task_ptr = task::leader_of(task_ptr);
    if task_ptr.is_null() {
        return None;
    }
    for idx in 0..task::MAX_TASKS {
        if task::get_by_index(idx) == task_ptr {
            return Some(idx);
        }
    }
    None
}

pub(super) fn mmap_prepare_task_slot(task_ptr: *mut task::Task) -> Option<usize> {
    let task_ptr = task::leader_of(task_ptr);
    let slot = mmap_task_slot(task_ptr)?;
    unsafe {
        let pid = (*task_ptr).pid;
        if TASK_VM_OWNER_PID[slot] != pid {
            TASK_VM_OWNER_PID[slot] = pid;
            TASK_VM_MAPPINGS[slot] = [VmMapping::EMPTY; MMAP_MAX_REGIONS_PER_TASK];
        }
    }
    Some(slot)
}

pub(super) fn mmap_clear_task_regions(task_ptr: *mut task::Task) {
    let slot = match mmap_task_slot(task_ptr) {
        Some(slot) => slot,
        None => return,
    };
    unsafe {
        TASK_VM_OWNER_PID[slot] = task::INVALID_PID;
        TASK_VM_MAPPINGS[slot] = [VmMapping::EMPTY; MMAP_MAX_REGIONS_PER_TASK];
    }
    vm_sync_inode_pins();
}

/// Inodes on which the VM currently holds one fs open reference.
pub(super) static mut VM_INODE_PINNED: [bool; fs::VFS_MAX_INODES] = [false; fs::VFS_MAX_INODES];

/// Which inodes some task's region table maps.
pub(super) fn vm_mapped_inodes(
    tables: &[[VmMapping; MMAP_MAX_REGIONS_PER_TASK]],
) -> [bool; fs::VFS_MAX_INODES] {
    let mut mapped = [false; fs::VFS_MAX_INODES];
    for table in tables {
        for region in table.iter() {
            if !region.is_empty()
                && region.file_inode != task::USER_FD_NONE
                && (region.file_inode as usize) < fs::VFS_MAX_INODES
            {
                mapped[region.file_inode as usize] = true;
            }
        }
    }
    mapped
}

/// Hold one fs open reference per inode that any task maps, so a file that
/// is unlinked (and closed) while mapped, or the hidden file behind a
/// `MAP_SHARED | MAP_ANONYMOUS` region, lives exactly as long as its
/// mappings. Called after every change to the set of mapped files.
pub(super) fn vm_sync_inode_pins() {
    let mapped = unsafe { vm_mapped_inodes(&*core::ptr::addr_of!(TASK_VM_MAPPINGS)) };
    for inode in 0..fs::VFS_MAX_INODES {
        let pinned = unsafe { VM_INODE_PINNED[inode] };
        if mapped[inode] && !pinned {
            fs::inode_open_retain(inode as u16);
            unsafe { VM_INODE_PINNED[inode] = true };
        } else if !mapped[inode] && pinned {
            unsafe { VM_INODE_PINNED[inode] = false };
            if fs::inode_open_release(inode as u16) {
                vfs_inode_freed(inode as u16);
            }
        }
    }
}

/// Temporary open reference on a freshly created hidden file; dropping it
/// frees the file unless a mapping pinned it meanwhile.
struct HiddenFileRef(u16);

impl Drop for HiddenFileRef {
    fn drop(&mut self) {
        if fs::inode_open_release(self.0) {
            vfs_inode_freed(self.0);
        }
    }
}

/// Backing object for `MAP_SHARED | MAP_ANONYMOUS`: an unlinked tmpfs file
/// (in the memfd storage dir) whose pages live in the shared page cache, so
/// `fork`ed children see the same memory. The returned guard holds the only
/// reference until `vm_sync_inode_pins` pins the new mapping.
fn mmap_shared_anon_backing(task_ptr: *mut task::Task, len: u64) -> Result<HiddenFileRef, i64> {
    memfd_storage_dir_ready()?;
    let pid = unsafe { (*task_ptr).pid };
    let mut path_buf = [0u8; fs::VFS_MAX_PATH];
    for _ in 0..MEMFD_CREATE_MAX_RETRIES {
        let sequence = MEMFD_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = memfd_build_path(pid, sequence, b"shared-anon", &mut path_buf)?;
        let opened = match fs::open(path, fs::O_CREAT | fs::O_EXCL | fs::O_RDWR, 0o600) {
            Ok(opened) => opened,
            Err(fs::EEXIST) => continue,
            Err(fs::ENOSPC) => return Err(-ENOMEM),
            Err(err) => return Err(-fs_to_errno(err)),
        };
        fs::inode_open_retain(opened.inode);
        let guard = HiddenFileRef(opened.inode);
        // Page-backed storage (crate::shmem), freed with the inode, so the
        // region is not limited by tmpfs's inline file size.
        crate::shmem::create(opened.inode)?;
        vfs_truncate_to(opened.inode, len).map_err(|err| -fs_to_errno(err))?;
        let _ = fs::unlink(path);
        return Ok(guard);
    }
    Err(-EAGAIN)
}

pub(super) fn mmap_regions_find_free(
    regions: &[VmMapping; MMAP_MAX_REGIONS_PER_TASK],
) -> Option<usize> {
    for (idx, region) in regions.iter().enumerate() {
        if region.is_empty() {
            return Some(idx);
        }
    }
    None
}

pub(super) fn mmap_regions_overlaps(
    regions: &[VmMapping; MMAP_MAX_REGIONS_PER_TASK],
    start: u64,
    end: u64,
) -> bool {
    for region in regions {
        if region.is_empty() {
            continue;
        }
        if start < region.end() && region.start < end {
            return true;
        }
    }
    false
}

pub(super) fn mmap_regions_find_mapping(
    regions: &[VmMapping; MMAP_MAX_REGIONS_PER_TASK],
    addr: u64,
) -> Option<VmMapping> {
    for region in regions {
        if region.is_empty() {
            continue;
        }
        if addr >= region.start && addr < region.end() {
            return Some(*region);
        }
    }
    None
}

pub(super) fn mmap_regions_insert(
    regions: &mut [VmMapping; MMAP_MAX_REGIONS_PER_TASK],
    mapping: VmMapping,
) -> Result<(), i64> {
    if mapping.is_empty() {
        return Err(-EINVAL);
    }
    if mmap_regions_overlaps(regions, mapping.start, mapping.end()) {
        return Err(-EINVAL);
    }

    let slot = match mmap_regions_find_free(regions) {
        Some(slot) => slot,
        None => return Err(-ENOMEM),
    };
    regions[slot] = mapping;
    Ok(())
}

#[inline]
pub(super) fn mmap_region_allows_prot(region: &VmMapping, prot: u32) -> bool {
    if region.map_shared && (prot & PROT_WRITE) != 0 && !region.map_shared_can_write {
        return false;
    }
    true
}

pub(super) fn mmap_regions_split_at(
    regions: &mut [VmMapping; MMAP_MAX_REGIONS_PER_TASK],
    cut: u64,
) -> Result<(), i64> {
    for idx in 0..MMAP_MAX_REGIONS_PER_TASK {
        let region = regions[idx];
        if region.is_empty() || cut <= region.start || cut >= region.end() {
            continue;
        }

        let free_idx = match mmap_regions_find_free(regions) {
            Some(slot) => slot,
            None => return Err(-ENOMEM),
        };

        let mut right = region;
        right.start = cut;
        right.len = region.end() - cut;
        if right.file_inode != task::USER_FD_NONE {
            right.file_offset = region.file_offset.saturating_add(cut - region.start);
        }

        regions[idx].len = cut - region.start;
        regions[free_idx] = right;
        return Ok(());
    }
    Ok(())
}

pub(super) fn mmap_regions_range_mapped(
    regions: &[VmMapping; MMAP_MAX_REGIONS_PER_TASK],
    start: u64,
    end: u64,
) -> bool {
    let mut cursor = start;
    while cursor < end {
        let region = match mmap_regions_find_mapping(regions, cursor) {
            Some(region) => region,
            None => return false,
        };
        let next = cmp::min(region.end(), end);
        if next <= cursor {
            return false;
        }
        cursor = next;
    }
    true
}

pub(super) fn mmap_regions_unmap_range(
    regions: &mut [VmMapping; MMAP_MAX_REGIONS_PER_TASK],
    start: u64,
    end: u64,
) -> Result<(), i64> {
    let mut next = *regions;
    mmap_regions_unmap_range_in_place(&mut next, start, end)?;
    *regions = next;
    Ok(())
}

pub(super) fn mmap_regions_unmap_range_in_place(
    regions: &mut [VmMapping; MMAP_MAX_REGIONS_PER_TASK],
    start: u64,
    end: u64,
) -> Result<(), i64> {
    for idx in 0..MMAP_MAX_REGIONS_PER_TASK {
        let region = regions[idx];
        if region.is_empty() {
            continue;
        }

        let region_start = region.start;
        let region_end = region.end();
        if end <= region_start || start >= region_end {
            continue;
        }

        if start <= region_start && end >= region_end {
            regions[idx] = VmMapping::EMPTY;
            continue;
        }

        if start <= region_start {
            let delta = end - region_start;
            regions[idx].start = end;
            regions[idx].len = region_end - end;
            if region.file_inode != task::USER_FD_NONE {
                regions[idx].file_offset = region.file_offset.saturating_add(delta);
            }
            continue;
        }

        if end >= region_end {
            regions[idx].len = start - region_start;
            continue;
        }

        let free_idx = match mmap_regions_find_free(regions) {
            Some(slot) => slot,
            None => return Err(-ENOMEM),
        };
        let mut right = region;
        right.start = end;
        right.len = region_end - end;
        if right.file_inode != task::USER_FD_NONE {
            right.file_offset = region.file_offset.saturating_add(end - region_start);
        }

        regions[idx].len = start - region_start;
        regions[free_idx] = right;
    }
    Ok(())
}

pub(super) fn mmap_regions_protect_range(
    regions: &mut [VmMapping; MMAP_MAX_REGIONS_PER_TASK],
    start: u64,
    end: u64,
    prot: u32,
) -> Result<(), i64> {
    let mut next = *regions;
    mmap_regions_protect_range_in_place(&mut next, start, end, prot)?;
    *regions = next;
    Ok(())
}

pub(super) fn mmap_regions_protect_range_in_place(
    regions: &mut [VmMapping; MMAP_MAX_REGIONS_PER_TASK],
    start: u64,
    end: u64,
    prot: u32,
) -> Result<(), i64> {
    if !mmap_regions_range_mapped(regions, start, end) {
        return Err(-ENOMEM);
    }

    mmap_regions_split_at(regions, start)?;
    mmap_regions_split_at(regions, end)?;

    for region in regions.iter() {
        if region.is_empty() {
            continue;
        }
        if region.start >= start && region.end() <= end && !mmap_region_allows_prot(region, prot) {
            return Err(-EINVAL);
        }
    }

    for region in regions.iter_mut() {
        if region.is_empty() {
            continue;
        }
        if region.start >= start && region.end() <= end {
            region.prot = prot;
        }
    }
    Ok(())
}

pub(super) fn mmap_task_overlaps(task_ptr: *mut task::Task, start: u64, end: u64) -> bool {
    let slot = match mmap_prepare_task_slot(task_ptr) {
        Some(slot) => slot,
        None => return false,
    };
    unsafe { mmap_regions_overlaps(&TASK_VM_MAPPINGS[slot], start, end) }
}

pub(super) fn mmap_range_available(
    task_ptr: *mut task::Task,
    pml4: PhysAddr,
    start: u64,
    end: u64,
) -> bool {
    if mmap_task_overlaps(task_ptr, start, end) {
        return false;
    }

    let mut addr = start;
    while addr < end {
        if vmm::get_physical_in(pml4, VirtAddr::new(addr)).is_some() {
            return false;
        }
        addr += PAGE_SIZE;
    }
    true
}

pub(super) fn mmap_find_range(
    ctx: &SyscallContext,
    pml4: PhysAddr,
    hint: u64,
    len: u64,
) -> Result<u64, i64> {
    let (brk, stack_bottom) =
        unsafe { ((*crate::task::leader_of(ctx.current)).user_brk, (*crate::task::leader_of(ctx.current)).user_stack_bottom) };
    if stack_bottom == 0 {
        return Err(-ENOMEM);
    }

    let brk_start = page_align_up(brk.saturating_add(MMAP_BRK_GAP)).unwrap_or(MMAP_BASE_ADDR);
    let search_base = cmp::max(MMAP_BASE_ADDR, brk_start);
    let search_end = stack_bottom.saturating_sub(MMAP_STACK_GUARD);
    if search_end <= search_base {
        return Err(-ENOMEM);
    }

    let mut cursor = if hint == 0 {
        search_base
    } else {
        cmp::max(search_base, align_down(hint, PAGE_SIZE))
    };

    while cursor < search_end {
        let end = match cursor.checked_add(len) {
            Some(end) => end,
            None => break,
        };
        if end > search_end || end >= KERNEL_VIRT_BASE || cursor < PAGE_SIZE {
            break;
        }
        if mmap_range_available(ctx.current, pml4, cursor, end) {
            return Ok(cursor);
        }
        cursor = match cursor.checked_add(PAGE_SIZE) {
            Some(next) => next,
            None => break,
        };
    }

    Err(-ENOMEM)
}

pub(super) fn mmap_file_inode(
    task_ptr: *mut task::Task,
    fd: i32,
    require_readable: bool,
    require_writable: bool,
) -> Result<(u16, u32), i64> {
    let idx = fd_index(fd)?;
    if !fd_slot_is_open(task_ptr, idx) {
        return Err(-EBADF);
    }

    let (kind, inode, fd_flags) = unsafe {
        (
            (*crate::task::leader_of(task_ptr)).user_fd_kind[idx],
            (*crate::task::leader_of(task_ptr)).user_fd_inode[idx],
            (*crate::task::leader_of(task_ptr)).user_fd_flags[idx],
        )
    };
    if kind != task::USER_FD_KIND_VFS {
        return Err(-EBADF);
    }
    if require_readable && !fd_can_read(fd_flags) {
        return Err(-EACCES);
    }
    if require_writable && !fd_can_write(fd_flags) {
        return Err(-EACCES);
    }

    Ok((inode, fd_flags))
}

pub(super) fn mmap_validate_shared_write_window(
    inode: u16,
    offset: u64,
    len: u64,
) -> Result<(), i64> {
    let end = offset.checked_add(len).ok_or(-EINVAL)?;
    let stat = vfs_stat(inode).map_err(|err| -fs_to_errno(err))?;
    // `len` is page-rounded; like Linux, the mapping may cover the partial
    // page holding the end of the file.
    let limit = stat.size.div_ceil(PAGE_SIZE) * PAGE_SIZE;
    if end > limit {
        return Err(-EINVAL);
    }
    Ok(())
}

pub(super) fn mmap_validate_framebuffer_window(
    offset: u64,
    len: u64,
    prot: u32,
    is_shared: bool,
) -> Result<(), i64> {
    if !is_shared || (prot & PROT_EXEC) != 0 {
        return Err(-EINVAL);
    }
    match framebuffer::mmap_size() {
        Some(_) if framebuffer::mmap_range_valid(offset, len) => Ok(()),
        Some(_) => Err(-EINVAL),
        None => Err(-ENXIO),
    }
}

pub(super) fn mmap_regions_total_pages(
    regions: &[VmMapping; MMAP_MAX_REGIONS_PER_TASK],
) -> Result<u64, i64> {
    let mut pages = 0u64;
    for region in regions {
        if region.is_empty() {
            continue;
        }
        pages = pages.checked_add(region.len / PAGE_SIZE).ok_or(-ENOMEM)?;
    }
    Ok(pages)
}

pub(super) fn set_program_break(ctx: &SyscallContext, requested: u64) -> i64 {
    let pml4 = match ctx.user_pml4() {
        Ok(pml4) => pml4,
        Err(err) => return err,
    };

    let (current, start, stack_bottom) = unsafe {
        (
            (*crate::task::leader_of(ctx.current)).user_brk,
            (*crate::task::leader_of(ctx.current)).user_brk_start,
            (*crate::task::leader_of(ctx.current)).user_stack_bottom,
        )
    };

    if current == 0 || start == 0 || stack_bottom == 0 {
        return -EINVAL;
    }

    if requested == 0 {
        return current as i64;
    }

    let heap_limit = stack_bottom.saturating_sub(PAGE_SIZE);
    if requested < start || requested >= heap_limit || requested >= KERNEL_VIRT_BASE {
        return current as i64;
    }

    let old_pages_end = match page_align_up(current) {
        Some(v) => v,
        None => return current as i64,
    };
    let new_pages_end = match page_align_up(requested) {
        Some(v) => v,
        None => return current as i64,
    };

    if new_pages_end > old_pages_end {
        if ensure_user_pages_unmapped(pml4, old_pages_end, new_pages_end).is_err() {
            return current as i64;
        }
        if map_user_pages(pml4, old_pages_end, new_pages_end).is_err() {
            unmap_user_pages(pml4, old_pages_end, new_pages_end);
            return current as i64;
        }
    } else if new_pages_end < old_pages_end {
        unmap_user_pages(pml4, new_pages_end, old_pages_end);
    }

    unsafe {
        (*crate::task::leader_of(ctx.current)).user_brk = requested;
    }
    requested as i64
}

pub(super) fn sys_mmap(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let addr_hint = call.arg(0);
    let length = call.arg(1);
    let prot = call.arg(2) as u32;
    let flags = call.arg(3) as u32;
    let fd_raw = call.arg(4) as i64;
    let offset = call.arg(5);

    if length == 0 {
        return -EINVAL;
    }
    if !mmap_flags_are_supported(flags) {
        return -EINVAL;
    }

    let (_is_private, is_shared) = match mmap_parse_sharing_mode(flags) {
        Ok(mode) => mode,
        Err(err) => return err,
    };

    let is_anon = (flags & MAP_ANONYMOUS) != 0;
    let is_fixed_noreplace = (flags & MAP_FIXED_NOREPLACE) != 0;
    let is_fixed = (flags & MAP_FIXED) != 0 || is_fixed_noreplace;

    let len = match page_align_up(length) {
        Some(v) if v > 0 => v,
        _ => return -EINVAL,
    };
    let pml4 = match ctx.user_pml4() {
        Ok(v) => v,
        Err(err) => return err,
    };

    let mut shared_anon: Option<HiddenFileRef> = None;
    let file_src = if is_anon {
        if fd_raw != -1 {
            return -EINVAL;
        }
        if offset != 0 {
            return -EINVAL;
        }
        if is_shared {
            let backing = match mmap_shared_anon_backing(ctx.current, len) {
                Ok(backing) => backing,
                Err(err) => return err,
            };
            let inode = backing.0;
            shared_anon = Some(backing);
            Some((inode, 0, true))
        } else {
            None
        }
    } else {
        if fd_raw < 0 || fd_raw > i32::MAX as i64 {
            return -EBADF;
        }
        if (offset & (PAGE_SIZE - 1)) != 0 {
            return -EINVAL;
        }
        if offset.checked_add(len).is_none() {
            return -EINVAL;
        }
        let fd = fd_raw as i32;
        let require_writable = is_shared && (prot & PROT_WRITE) != 0;
        match mmap_file_inode(ctx.current, fd, true, require_writable) {
            Ok((inode, fd_flags)) => {
                if require_writable {
                    if let Err(err) = seals_allow_write(fd_seals(fd_flags, inode)) {
                        return err;
                    }
                }
                let shared_can_write = is_shared && fd_can_write(fd_flags);
                if fs::inode_is_framebuffer(inode) {
                    if let Err(err) = mmap_validate_framebuffer_window(offset, len, prot, is_shared)
                    {
                        return err;
                    }
                } else if is_shared && (prot & PROT_WRITE) != 0 {
                    if let Err(err) = mmap_validate_shared_write_window(inode, offset, len) {
                        return err;
                    }
                }
                Some((inode, offset, shared_can_write))
            }
            Err(err) => return err,
        }
    };

    let slot = match mmap_prepare_task_slot(ctx.current) {
        Some(slot) => slot,
        None => return -ESRCH,
    };
    let mut projected_regions = unsafe { TASK_VM_MAPPINGS[slot] };

    let start = if is_fixed {
        if (addr_hint & (PAGE_SIZE - 1)) != 0 || addr_hint < MMAP_BASE_ADDR {
            return -EINVAL;
        }
        let end = match addr_hint.checked_add(len) {
            Some(v) => v,
            None => return -EINVAL,
        };
        if end >= KERNEL_VIRT_BASE {
            return -EINVAL;
        }

        if is_fixed_noreplace {
            if !mmap_range_available(ctx.current, pml4, addr_hint, end) {
                return -EEXIST;
            }
        } else {
            let unmap_result = mmap_regions_unmap_range(&mut projected_regions, addr_hint, end);
            if let Err(err) = unmap_result {
                return err;
            }
        }
        addr_hint
    } else {
        let hint = if addr_hint == 0 {
            0
        } else {
            align_down(addr_hint, PAGE_SIZE)
        };
        match mmap_find_range(ctx, pml4, hint, len) {
            Ok(addr) => addr,
            Err(err) => return err,
        }
    };

    let mapped_pages = len / PAGE_SIZE;
    let projected_pages = match mmap_regions_total_pages(&projected_regions) {
        Ok(pages) => pages,
        Err(err) => return err,
    };
    if projected_pages.saturating_add(mapped_pages) > MMAP_MAX_PAGES_PER_TASK {
        return -ENOMEM;
    }

    let mapping = VmMapping {
        start,
        len,
        file_offset: file_src.map(|(_, off, _)| off).unwrap_or(0),
        file_inode: file_src
            .map(|(inode, _, _)| inode)
            .unwrap_or(task::USER_FD_NONE),
        prot,
        map_shared: is_shared,
        map_shared_can_write: file_src.map(|(_, _, can)| can).unwrap_or(false),
    };
    let insert_result = mmap_regions_insert(&mut projected_regions, mapping);
    match insert_result {
        Ok(()) => {
            if is_fixed && !is_fixed_noreplace {
                let end = match start.checked_add(len) {
                    Some(v) => v,
                    None => return -EINVAL,
                };
                let old_regions = unsafe { TASK_VM_MAPPINGS[slot] };
                unmap_user_pages_in_regions(pml4, start, end, Some(&old_regions));
            }
            unsafe {
                TASK_VM_MAPPINGS[slot] = projected_regions;
            }
            // Pin newly mapped files (and unpin ones a MAP_FIXED replaced)
            // before the shared-anon creation reference is dropped.
            vm_sync_inode_pins();
            drop(shared_anon);
            start as i64
        }
        Err(err) => err,
    }
}

pub(super) const MREMAP_MAYMOVE: u32 = 1;

/// Move the present PTEs of `[old_start, old_start + len)` to `new_start`
/// without copying or freeing the backing pages, so anonymous contents,
/// page-cache pins (tracked by physical address), and dirty state all follow
/// the mapping. On failure every page already moved is put back.
pub(super) fn mremap_move_ptes(pml4: PhysAddr, old_start: u64, new_start: u64, len: u64) -> Result<(), i64> {
    let mut off = 0u64;
    while off < len {
        let from = VirtAddr::new(old_start + off);
        let to = VirtAddr::new(new_start + off);
        if vmm::move_page_in(pml4, from, to).is_err() {
            // Roll back the pages moved so far.
            let mut back = 0u64;
            while back < off {
                let _ = vmm::move_page_in(
                    pml4,
                    VirtAddr::new(new_start + back),
                    VirtAddr::new(old_start + back),
                );
                back += PAGE_SIZE;
            }
            return Err(-ENOMEM);
        }
        off += PAGE_SIZE;
    }
    Ok(())
}

/// `mremap(old_addr, old_size, new_size, flags)`. Supports shrinking, growing
/// in place, and `MREMAP_MAYMOVE` relocation within a single existing mapping.
/// `MREMAP_FIXED` and `MREMAP_DONTUNMAP` are not supported.
pub(super) fn sys_mremap(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let old_addr = call.arg(0);
    let old_size = call.arg(1);
    let new_size = call.arg(2);
    let flags = call.arg(3) as u32;

    if (flags & !MREMAP_MAYMOVE) != 0 {
        return -EINVAL;
    }
    if (old_addr & (PAGE_SIZE - 1)) != 0 || old_addr < MMAP_BASE_ADDR {
        return -EINVAL;
    }
    let old_len = match page_align_up(old_size) {
        Some(v) if v > 0 => v,
        _ => return -EINVAL,
    };
    let new_len = match page_align_up(new_size) {
        Some(v) if v > 0 => v,
        _ => return -EINVAL,
    };
    let old_end = match old_addr.checked_add(old_len) {
        Some(v) if v < KERNEL_VIRT_BASE => v,
        _ => return -EINVAL,
    };

    let pml4 = match ctx.user_pml4() {
        Ok(v) => v,
        Err(err) => return err,
    };
    let slot = match mmap_prepare_task_slot(ctx.current) {
        Some(slot) => slot,
        None => return -ESRCH,
    };
    let regions = unsafe { TASK_VM_MAPPINGS[slot] };
    let region = match mmap_regions_find_mapping(&regions, old_addr) {
        Some(region) => region,
        None => return -EFAULT,
    };
    let region_end = region.start + region.len;
    if old_end > region_end {
        // The range must lie inside one mapping.
        return -EFAULT;
    }
    if mmap_mapping_is_framebuffer(&region) && new_len != old_len {
        return -EINVAL;
    }

    if new_len == old_len {
        return old_addr as i64;
    }

    if new_len < old_len {
        let mut next = regions;
        if let Err(err) = mmap_regions_unmap_range(&mut next, old_addr + new_len, old_end) {
            return err;
        }
        unmap_user_pages_in_regions(pml4, old_addr + new_len, old_end, Some(&regions));
        unsafe {
            TASK_VM_MAPPINGS[slot] = next;
        }
        return old_addr as i64;
    }

    // Growing: enforce the per-task page budget and file window rules.
    let total = match mmap_regions_total_pages(&regions) {
        Ok(pages) => pages,
        Err(err) => return err,
    };
    if total.saturating_add((new_len - old_len) / PAGE_SIZE) > MMAP_MAX_PAGES_PER_TASK {
        return -ENOMEM;
    }
    let is_file = region.file_inode != task::USER_FD_NONE;
    let file_offset = if is_file {
        region.file_offset + (old_addr - region.start)
    } else {
        0
    };
    if is_file && region.map_shared && (region.prot & PROT_WRITE) != 0 {
        if let Err(err) = mmap_validate_shared_write_window(region.file_inode, file_offset, new_len) {
            return err;
        }
    }
    let mut piece = VmMapping {
        start: old_addr,
        len: new_len,
        file_offset,
        ..region
    };

    let grow_end = match old_addr.checked_add(new_len) {
        Some(v) if v < KERNEL_VIRT_BASE => v,
        _ => return -ENOMEM,
    };
    if old_end == region_end && mmap_range_available(ctx.current, pml4, old_end, grow_end) {
        let mut next = regions;
        if let Err(err) = mmap_regions_unmap_range(&mut next, old_addr, old_end) {
            return err;
        }
        if let Err(err) = mmap_regions_insert(&mut next, piece) {
            return err;
        }
        unsafe {
            TASK_VM_MAPPINGS[slot] = next;
        }
        return old_addr as i64;
    }

    if (flags & MREMAP_MAYMOVE) == 0 {
        return -ENOMEM;
    }
    let new_start = match mmap_find_range(ctx, pml4, 0, new_len) {
        Ok(addr) => addr,
        Err(err) => return err,
    };
    piece.start = new_start;
    let mut next = regions;
    if let Err(err) = mmap_regions_unmap_range(&mut next, old_addr, old_end) {
        return err;
    }
    if let Err(err) = mmap_regions_insert(&mut next, piece) {
        return err;
    }
    if let Err(err) = mremap_move_ptes(pml4, old_addr, new_start, old_len) {
        return err;
    }
    unsafe {
        TASK_VM_MAPPINGS[slot] = next;
    }
    vm_sync_inode_pins();
    new_start as i64
}

pub(super) fn sys_munmap(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let addr = call.arg(0);
    let length = call.arg(1);
    if length == 0 {
        return -EINVAL;
    }
    if (addr & (PAGE_SIZE - 1)) != 0 || addr < MMAP_BASE_ADDR {
        return -EINVAL;
    }

    let len = match page_align_up(length) {
        Some(v) if v > 0 => v,
        _ => return -EINVAL,
    };
    let end = match addr.checked_add(len) {
        Some(v) => v,
        None => return -EINVAL,
    };
    if end >= KERNEL_VIRT_BASE {
        return -EINVAL;
    }

    let pml4 = match ctx.user_pml4() {
        Ok(v) => v,
        Err(err) => return err,
    };
    let slot = match mmap_prepare_task_slot(ctx.current) {
        Some(slot) => slot,
        None => return -ESRCH,
    };
    let old_regions = unsafe { TASK_VM_MAPPINGS[slot] };
    let mut next_regions = old_regions;
    let unmap_result = mmap_regions_unmap_range(&mut next_regions, addr, end);
    if let Err(err) = unmap_result {
        return err;
    }
    unmap_user_pages_in_regions(pml4, addr, end, Some(&old_regions));
    unsafe {
        TASK_VM_MAPPINGS[slot] = next_regions;
    }
    vm_sync_inode_pins();
    0
}

pub(super) fn sys_mprotect(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let addr = call.arg(0);
    let length = call.arg(1);
    let prot = call.arg(2) as u32;

    if length == 0 {
        return 0;
    }
    if (addr & (PAGE_SIZE - 1)) != 0 || addr < MMAP_BASE_ADDR {
        return -EINVAL;
    }

    let len = match page_align_up(length) {
        Some(v) if v > 0 => v,
        _ => return -EINVAL,
    };
    let end = match addr.checked_add(len) {
        Some(v) => v,
        None => return -EINVAL,
    };
    if end >= KERNEL_VIRT_BASE {
        return -EINVAL;
    }

    let pte_flags = match mmap_prot_to_pte_flags(prot) {
        Ok(v) => v,
        Err(err) => return err,
    };
    let pml4 = match ctx.user_pml4() {
        Ok(v) => v,
        Err(err) => return err,
    };
    let slot = match mmap_prepare_task_slot(ctx.current) {
        Some(slot) => slot,
        None => return -ESRCH,
    };
    let protect_result =
        unsafe { mmap_regions_protect_range(&mut TASK_VM_MAPPINGS[slot], addr, end, prot) };
    if let Err(err) = protect_result {
        return err;
    }

    let mut page = addr;
    while page < end {
        let phys = match vmm::get_physical_in(pml4, VirtAddr::new(page)) {
            Some(phys) => PhysAddr::new(phys.as_u64() & !(PAGE_SIZE - 1)),
            None => {
                page += PAGE_SIZE;
                continue;
            }
        };

        // Pages still shared copy-on-write with another process must stay
        // write-protected; they become COW rather than writable.
        let page_flags = vmm::cow_adjust_flags(pte_flags, pmm::page_is_shared(phys));
        if vmm::map_page_in(pml4, VirtAddr::new(page), phys, page_flags) != 0 {
            return -ENOMEM;
        }
        let mapping = unsafe { mmap_regions_find_mapping(&TASK_VM_MAPPINGS[slot], page) };
        if let Some(mapping) = mapping {
            if mapping.map_shared
                && mapping.file_inode != task::USER_FD_NONE
                && (mapping.prot & PROT_WRITE) != 0
            {
                let _ = vm_page_cache_mark_dirty_by_phys(phys);
            }
        }
        page += PAGE_SIZE;
    }

    0
}

pub(super) fn mmap_prot_allows_fault_access(prot: u32, error_code: u64) -> bool {
    if prot == PROT_NONE {
        return false;
    }
    if (error_code & PAGE_FAULT_ERROR_WRITE) != 0 {
        return (prot & PROT_WRITE) != 0;
    }
    if (error_code & PAGE_FAULT_ERROR_INSTR) != 0 {
        return (prot & PROT_EXEC) != 0;
    }
    (prot & (PROT_READ | PROT_WRITE)) != 0
}

pub(super) fn mmap_fault_map_page(
    pml4: PhysAddr,
    page_addr: u64,
    mapping: VmMapping,
) -> Result<(), i64> {
    if vmm::get_physical_in(pml4, VirtAddr::new(page_addr)).is_some() {
        return Ok(());
    }

    if mmap_mapping_is_framebuffer(&mapping) {
        let map_delta = page_addr.checked_sub(mapping.start).ok_or(-EINVAL)?;
        let page_off = mapping.file_offset.checked_add(map_delta).ok_or(-EINVAL)?;
        let page_phys = framebuffer::mmap_phys_page(page_off).ok_or(-EINVAL)?;
        let pte_flags = mmap_prot_to_pte_flags(mapping.prot)?;
        if vmm::map_page_in(
            pml4,
            VirtAddr::new(page_addr),
            PhysAddr::new(page_phys),
            pte_flags,
        ) != 0
        {
            return Err(-ENOMEM);
        }
        return Ok(());
    }

    if mapping.map_shared
        && mapping.file_inode != task::USER_FD_NONE
        && crate::shmem::exists(mapping.file_inode)
    {
        // memfd pages are owned by the shmem object; each mapping takes its
        // own PMM share reference, dropped by the normal unmap/exit paths.
        let map_delta = page_addr.checked_sub(mapping.start).ok_or(-EINVAL)?;
        let file_off = mapping.file_offset.checked_add(map_delta).ok_or(-EINVAL)?;
        let page_off = align_down(file_off, PAGE_SIZE);
        let phys = match crate::shmem::fault_page(mapping.file_inode, page_off) {
            Some(Ok(phys)) => PhysAddr::new(phys),
            Some(Err(err)) => return Err(err),
            None => return Err(-EINVAL),
        };
        if !pmm::page_share(phys) {
            return Err(-ENOMEM);
        }
        let pte_flags = mmap_prot_to_pte_flags(mapping.prot)?;
        if vmm::map_page_in(pml4, VirtAddr::new(page_addr), phys, pte_flags) != 0 {
            pmm::free_page(phys);
            return Err(-ENOMEM);
        }
        return Ok(());
    }

    if mapping.map_shared && mapping.file_inode != task::USER_FD_NONE {
        let map_delta = page_addr.checked_sub(mapping.start).ok_or(-EINVAL)?;
        let file_off = mapping.file_offset.checked_add(map_delta).ok_or(-EINVAL)?;
        let page_off = align_down(file_off, PAGE_SIZE);
        let writable = (mapping.prot & PROT_WRITE) != 0;
        let entry = vm_page_cache_pin_mapped_page(mapping.file_inode, page_off, writable)?;

        let pte_flags = mmap_prot_to_pte_flags(mapping.prot)?;
        if vmm::map_page_in(
            pml4,
            VirtAddr::new(page_addr),
            PhysAddr::new(entry.page_phys),
            pte_flags,
        ) != 0
        {
            let _ = vm_page_cache_release_mapped_phys(PhysAddr::new(entry.page_phys));
            return Err(-ENOMEM);
        }
        return Ok(());
    }

    let page_phys = alloc_user_vm_page_with_reclaim();
    if page_phys.as_u64() == 0 {
        return Err(-ENOMEM);
    }
    let page_ptr = page_phys.to_virt().as_u64() as *mut u8;
    unsafe {
        ptr::write_bytes(page_ptr, 0, PAGE_SIZE as usize);
    }

    if mapping.file_inode != task::USER_FD_NONE {
        let map_delta = page_addr.checked_sub(mapping.start).ok_or(-EINVAL)?;
        let file_off = mapping.file_offset.checked_add(map_delta).ok_or(-EINVAL)?;
        let to_read = cmp::min(PAGE_SIZE, mapping.end().saturating_sub(page_addr)) as usize;
        if to_read > 0 {
            let page_slice =
                unsafe { core::slice::from_raw_parts_mut(page_ptr, PAGE_SIZE as usize) };
            let _ = read_vfs_bytes(mapping.file_inode, file_off, &mut page_slice[..to_read])?;
        }
    }

    let pte_flags = mmap_prot_to_pte_flags(mapping.prot)?;
    if vmm::map_page_in(pml4, VirtAddr::new(page_addr), page_phys, pte_flags) != 0 {
        pmm::free_page(page_phys);
        return Err(-ENOMEM);
    }
    Ok(())
}

/// Move the mmap region table from `from`'s task slot to `to`'s, after `to`
/// took over the process from `from` (see `task::take_over_group_leadership`).
pub(super) fn mmap_move_task_regions(from: *mut task::Task, to: *mut task::Task) {
    let (Some(from_slot), Some(to_slot)) = (task::table_slot(from), task::table_slot(to)) else {
        return;
    };
    unsafe {
        TASK_VM_MAPPINGS[to_slot] = TASK_VM_MAPPINGS[from_slot];
        TASK_VM_OWNER_PID[to_slot] = (*to).pid;
        TASK_VM_MAPPINGS[from_slot] = [VmMapping::EMPTY; MMAP_MAX_REGIONS_PER_TASK];
        TASK_VM_OWNER_PID[from_slot] = task::INVALID_PID;
    }
}

pub(super) fn mmap_clone_task_regions(
    parent: *mut task::Task,
    child: *mut task::Task,
) -> Result<(), i64> {
    let parent_slot = match mmap_prepare_task_slot(parent) {
        Some(slot) => slot,
        None => return Err(-ESRCH),
    };
    let child_slot = match mmap_task_slot(child) {
        Some(slot) => slot,
        None => return Err(-ESRCH),
    };

    unsafe {
        TASK_VM_OWNER_PID[child_slot] = (*child).pid;
        TASK_VM_MAPPINGS[child_slot] = TASK_VM_MAPPINGS[parent_slot];
    }
    Ok(())
}

/// Resolve a write fault on a present copy-on-write page of the current task.
/// Kernel code reaches user memory through the physical map, so it must call
/// `vmm::resolve_cow_in` itself before writing (see `copy_user_bytes`); CR0.WP
/// is not set, so kernel-mode writes would not fault here.
fn try_handle_cow_fault(fault_addr: u64, error_code: u64) -> bool {
    let write_to_present = PAGE_FAULT_ERROR_PRESENT | PAGE_FAULT_ERROR_WRITE;
    if (error_code & write_to_present) != write_to_present {
        return false;
    }
    if fault_addr >= KERNEL_VIRT_BASE || fault_addr > USER_CANON_MAX {
        return false;
    }
    let current = task::current();
    if current.is_null() {
        return false;
    }
    let pml4 = unsafe { PhysAddr::new((*current).pml4_phys) };
    if pml4.as_u64() == 0 {
        return false;
    }
    match vmm::resolve_cow_in(pml4, VirtAddr::new(fault_addr)) {
        vmm::CowResolve::Resolved => true,
        vmm::CowResolve::NotCow => false,
        vmm::CowResolve::NoMemory => {
            sys_log!(
                "[SYSCALL] out of memory breaking copy-on-write at 0x{:x}",
                fault_addr
            );
            false
        }
    }
}

pub fn try_handle_page_fault(fault_addr: u64, error_code: u64) -> bool {
    if try_handle_cow_fault(fault_addr, error_code) {
        return true;
    }
    if (error_code & PAGE_FAULT_ERROR_USER) == 0 {
        return false;
    }
    if (error_code & PAGE_FAULT_ERROR_PRESENT) != 0 {
        return false;
    }
    if fault_addr < PAGE_SIZE || fault_addr >= KERNEL_VIRT_BASE || fault_addr > USER_CANON_MAX {
        return false;
    }

    let current = task::current();
    if current.is_null() {
        return false;
    }
    let is_user = unsafe { ((*current).flags & TASK_FLAG_USER) != 0 };
    if !is_user {
        return false;
    }

    let pml4 = unsafe { PhysAddr::new((*current).pml4_phys) };
    if pml4.as_u64() == 0 {
        return false;
    }

    let slot = match mmap_prepare_task_slot(current) {
        Some(slot) => slot,
        None => return false,
    };
    let page_addr = align_down(fault_addr, PAGE_SIZE);
    let mapping = unsafe { mmap_regions_find_mapping(&TASK_VM_MAPPINGS[slot], page_addr) };
    let mapping = match mapping {
        Some(mapping) => mapping,
        None => return false,
    };
    if !mmap_prot_allows_fault_access(mapping.prot, error_code) {
        return false;
    }

    mmap_fault_map_page(pml4, page_addr, mapping).is_ok()
}

pub fn terminate_current_user_fault(
    frame: &InterruptFrame,
    error_code: u64,
    fault_addr: u64,
) -> bool {
    if (error_code & PAGE_FAULT_ERROR_USER) == 0 {
        return false;
    }

    let current = task::current();
    if current.is_null() {
        return false;
    }
    let is_user = unsafe { ((*current).flags & TASK_FLAG_USER) != 0 };
    if !is_user {
        return false;
    }

    let pid = unsafe { (*current).pid };
    let rip = unsafe { ptr::read_unaligned(ptr::addr_of!(frame.rip)) };
    let rsp = unsafe { ptr::read_unaligned(ptr::addr_of!(frame.rsp)) };
    sys_log!(
        "[SYSCALL] pid {} terminated after page fault at 0x{:x} (err=0x{:x}, rip=0x{:x}, rsp=0x{:x})",
        pid,
        fault_addr,
        error_code,
        rip,
        rsp
    );

    let ctx = SyscallContext::new(current);
    let _ = sys_exit_common(&ctx, exit_code_for_signal(signal::SIGSEGV));
    true
}

pub(super) fn sys_brk(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    set_program_break(ctx, call.arg(0))
}

// ---------------------------------------------------------------------------
// msync (26)
// ---------------------------------------------------------------------------
pub(super) fn sys_msync(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let addr = call.arg(0);
    let length = call.arg(1);
    let flags = call.arg(2) as u32;

    if length == 0 {
        return 0;
    }
    if (addr & (PAGE_SIZE - 1)) != 0 || addr < MMAP_BASE_ADDR {
        return -EINVAL;
    }
    if !msync_flags_are_valid(flags) {
        return -EINVAL;
    }

    let len = match page_align_up(length) {
        Some(v) if v > 0 => v,
        _ => return -EINVAL,
    };
    let end = match addr.checked_add(len) {
        Some(v) => v,
        None => return -EINVAL,
    };
    if end >= KERNEL_VIRT_BASE {
        return -EINVAL;
    }

    let slot = match mmap_prepare_task_slot(ctx.current) {
        Some(slot) => slot,
        None => return -ESRCH,
    };

    if unsafe { !mmap_regions_range_mapped(&TASK_VM_MAPPINGS[slot], addr, end) } {
        return -ENOMEM;
    }

    let mut page = addr;
    while page < end {
        let mapping = unsafe { mmap_regions_find_mapping(&TASK_VM_MAPPINGS[slot], page) };
        let mapping = match mapping {
            Some(mapping) => mapping,
            None => return -ENOMEM,
        };

        if mapping.map_shared
            && mapping.file_inode != task::USER_FD_NONE
            && (mapping.prot & PROT_WRITE) != 0
        {
            let map_delta = match page.checked_sub(mapping.start) {
                Some(v) => v,
                None => return -EINVAL,
            };
            let file_off = match mapping.file_offset.checked_add(map_delta) {
                Some(v) => v,
                None => return -EINVAL,
            };
            let page_off = align_down(file_off, PAGE_SIZE);
            if let Err(err) = vm_page_cache_writeback_page(mapping.file_inode, page_off) {
                return err;
            }
        }

        page = page.saturating_add(PAGE_SIZE);
    }

    sync_ext2_backing();
    flush_ext2_backing();
    0
}
