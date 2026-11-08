#[cfg(not(test))]
use arch_x86_64::addr::VirtAddr;
use arch_x86_64::addr::{PhysAddr, KERNEL_VIRT_BASE, PAGE_SIZE};

#[cfg(not(test))]
use crate::memory::{pmm, vmm};
use crate::sync::SpinLock;
use crate::task::{Task, MAX_TASK_SHM_ATTACH};

pub const ENOENT: i64 = 2;
pub const ENOMEM: i64 = 12;
pub const EEXIST: i64 = 17;
pub const EINVAL: i64 = 22;
pub const ENOSPC: i64 = 28;

pub const IPC_PRIVATE: i32 = 0;
pub const IPC_CREAT: i32 = 0x0200;
pub const IPC_EXCL: i32 = 0x0400;

pub const SHM_RDONLY: i32 = 0x1000;

pub const IPC_RMID: i32 = 0;

pub const SHM_MAX_SEGMENTS: usize = 16;
pub const SHM_MAX_SIZE: u64 = 1024 * 1024;

const SHM_MAX_PAGES: usize = (SHM_MAX_SIZE as usize) / (PAGE_SIZE as usize);
const SHM_AUTO_BASE: u64 = 0x0000_0000_4000_0000;

#[derive(Clone, Copy)]
struct ShmSegment {
    used: bool,
    id: i32,
    key: i32,
    size: u64,
    num_pages: u16,
    ref_count: u16,
    marked_for_removal: bool,
    pages: [u64; SHM_MAX_PAGES],
}

impl ShmSegment {
    const fn empty() -> Self {
        Self {
            used: false,
            id: 0,
            key: 0,
            size: 0,
            num_pages: 0,
            ref_count: 0,
            marked_for_removal: false,
            pages: [0; SHM_MAX_PAGES],
        }
    }
}

struct ShmState {
    next_id: i32,
    segments: [ShmSegment; SHM_MAX_SEGMENTS],
}

impl ShmState {
    const fn new() -> Self {
        Self {
            next_id: 1,
            segments: [ShmSegment::empty(); SHM_MAX_SEGMENTS],
        }
    }
}

static SHM_STATE: SpinLock<ShmState> = SpinLock::new(ShmState::new());

#[cfg(test)]
static mut TEST_NEXT_PAGE: u64 = 0x0020_0000;

#[cfg(not(test))]
#[inline]
fn alloc_phys_page() -> u64 {
    pmm::alloc_page().as_u64()
}

#[cfg(test)]
#[inline]
fn alloc_phys_page() -> u64 {
    unsafe {
        let page = TEST_NEXT_PAGE;
        TEST_NEXT_PAGE = TEST_NEXT_PAGE.saturating_add(PAGE_SIZE);
        page
    }
}

#[cfg(not(test))]
#[inline]
fn free_phys_page(page: u64) {
    if page != 0 {
        pmm::free_page(PhysAddr::new(page));
    }
}

#[cfg(test)]
#[inline]
fn free_phys_page(_page: u64) {}

#[inline]
fn user_range_ok(start: u64, len: u64) -> bool {
    if len == 0 || start >= KERNEL_VIRT_BASE {
        return false;
    }
    let end = match start.checked_add(len.saturating_sub(1)) {
        Some(v) => v,
        None => return false,
    };
    end < KERNEL_VIRT_BASE
}

fn find_by_key_locked(state: &ShmState, key: i32) -> Option<usize> {
    if key == IPC_PRIVATE {
        return None;
    }
    for idx in 0..SHM_MAX_SEGMENTS {
        if state.segments[idx].used && state.segments[idx].key == key {
            return Some(idx);
        }
    }
    None
}

fn find_by_id_locked(state: &ShmState, shmid: i32) -> Option<usize> {
    if shmid <= 0 {
        return None;
    }
    for idx in 0..SHM_MAX_SEGMENTS {
        if state.segments[idx].used && state.segments[idx].id == shmid {
            return Some(idx);
        }
    }
    None
}

fn find_free_segment_slot_locked(state: &ShmState) -> Option<usize> {
    for idx in 0..SHM_MAX_SEGMENTS {
        if !state.segments[idx].used {
            return Some(idx);
        }
    }
    None
}

fn free_segment_pages(seg: &mut ShmSegment) {
    for idx in 0..seg.num_pages as usize {
        let page = seg.pages[idx];
        if page != 0 {
            free_phys_page(page);
            seg.pages[idx] = 0;
        }
    }
    seg.num_pages = 0;
}

fn maybe_destroy_segment(seg: &mut ShmSegment) {
    if seg.used && seg.marked_for_removal && seg.ref_count == 0 {
        free_segment_pages(seg);
        *seg = ShmSegment::empty();
    }
}

#[cfg(not(test))]
fn map_segment_pages(
    pml4: PhysAddr,
    start: u64,
    seg: &ShmSegment,
    readonly: bool,
) -> Result<(), i64> {
    let mut flags = vmm::PTE_PRESENT | vmm::PTE_USER | vmm::PTE_NO_EXECUTE;
    if !readonly {
        flags |= vmm::PTE_WRITE;
    }

    let num_pages = seg.num_pages as usize;
    let mut mapped = 0usize;
    while mapped < num_pages {
        let virt = VirtAddr::new(start + mapped as u64 * PAGE_SIZE);
        let phys = PhysAddr::new(seg.pages[mapped]);
        if vmm::map_page_in(pml4, virt, phys, flags) != 0 {
            let mut rollback = 0usize;
            while rollback < mapped {
                let rollback_virt = VirtAddr::new(start + rollback as u64 * PAGE_SIZE);
                let _ = vmm::unmap_page_in(pml4, rollback_virt);
                rollback += 1;
            }
            return Err(ENOMEM);
        }
        mapped += 1;
    }

    Ok(())
}

#[cfg(test)]
fn map_segment_pages(
    _pml4: PhysAddr,
    _start: u64,
    _seg: &ShmSegment,
    _readonly: bool,
) -> Result<(), i64> {
    Ok(())
}

#[cfg(not(test))]
fn unmap_segment_pages(pml4: PhysAddr, start: u64, size: u64) {
    let num_pages = ((size + PAGE_SIZE - 1) / PAGE_SIZE) as usize;
    let mut idx = 0usize;
    while idx < num_pages {
        let _ = vmm::unmap_page_in(pml4, VirtAddr::new(start + idx as u64 * PAGE_SIZE));
        idx += 1;
    }
}

#[cfg(test)]
fn unmap_segment_pages(_pml4: PhysAddr, _start: u64, _size: u64) {}

#[cfg(not(test))]
fn span_is_unmapped(pml4: PhysAddr, start: u64, size: u64) -> bool {
    let num_pages = ((size + PAGE_SIZE - 1) / PAGE_SIZE) as usize;
    let mut idx = 0usize;
    while idx < num_pages {
        if vmm::get_physical_in(pml4, VirtAddr::new(start + idx as u64 * PAGE_SIZE)).is_some() {
            return false;
        }
        idx += 1;
    }
    true
}

#[cfg(test)]
fn span_is_unmapped(_pml4: PhysAddr, _start: u64, _size: u64) -> bool {
    true
}

pub fn init() {
    let mut state = SHM_STATE.lock();
    state.next_id = 1;
    let mut idx = 0usize;
    while idx < SHM_MAX_SEGMENTS {
        state.segments[idx] = ShmSegment::empty();
        idx += 1;
    }
    #[cfg(test)]
    unsafe {
        TEST_NEXT_PAGE = 0x0020_0000;
    }
}

pub fn shm_get(key: i32, size: u64, flags: i32) -> Result<i32, i64> {
    if size == 0 || size > SHM_MAX_SIZE {
        return Err(EINVAL);
    }

    let mut state = SHM_STATE.lock();

    if let Some(existing_idx) = find_by_key_locked(&state, key) {
        if (flags & IPC_CREAT) != 0 && (flags & IPC_EXCL) != 0 {
            return Err(EEXIST);
        }
        return Ok(state.segments[existing_idx].id);
    }

    if key != IPC_PRIVATE && (flags & IPC_CREAT) == 0 {
        return Err(ENOENT);
    }

    let slot = match find_free_segment_slot_locked(&state) {
        Some(slot) => slot,
        None => return Err(ENOSPC),
    };

    let num_pages = ((size + PAGE_SIZE - 1) / PAGE_SIZE) as usize;
    if num_pages == 0 || num_pages > SHM_MAX_PAGES {
        return Err(EINVAL);
    }

    let mut seg = ShmSegment::empty();
    seg.used = true;
    seg.id = state.next_id;
    state.next_id = state.next_id.saturating_add(1).max(1);
    seg.key = key;
    seg.size = size;
    seg.num_pages = num_pages as u16;

    for idx in 0..num_pages {
        let page = alloc_phys_page();
        if page == 0 {
            free_segment_pages(&mut seg);
            return Err(ENOMEM);
        }
        seg.pages[idx] = page;
    }

    state.segments[slot] = seg;
    Ok(seg.id)
}

pub fn shm_attach(task: *mut Task, shmid: i32, addr: u64, flags: i32) -> Result<u64, i64> {
    if task.is_null() {
        return Err(EINVAL);
    }

    if flags & !SHM_RDONLY != 0 {
        return Err(EINVAL);
    }

    let pml4 = unsafe { PhysAddr::new((*task).pml4_phys) };
    if pml4.as_u64() == 0 {
        return Err(EINVAL);
    }

    let mut attach_slot = None;
    let mut idx = 0usize;
    while idx < MAX_TASK_SHM_ATTACH {
        if unsafe { (*crate::task::leader_of(task)).user_shm_id[idx] == 0 } {
            attach_slot = Some(idx);
            break;
        }
        idx += 1;
    }
    let attach_slot = match attach_slot {
        Some(slot) => slot,
        None => return Err(ENOSPC),
    };

    let mut state = SHM_STATE.lock();
    let seg_idx = match find_by_id_locked(&state, shmid) {
        Some(idx) => idx,
        None => return Err(ENOENT),
    };
    if state.segments[seg_idx].marked_for_removal {
        return Err(ENOENT);
    }

    let seg = state.segments[seg_idx];

    let chosen_addr = if addr == 0 {
        SHM_AUTO_BASE + (attach_slot as u64 * SHM_MAX_SIZE)
    } else {
        if (addr & (PAGE_SIZE - 1)) != 0 {
            return Err(EINVAL);
        }
        addr
    };

    if !user_range_ok(chosen_addr, seg.num_pages as u64 * PAGE_SIZE) {
        return Err(EINVAL);
    }

    if !span_is_unmapped(pml4, chosen_addr, seg.num_pages as u64 * PAGE_SIZE) {
        return Err(ENOMEM);
    }

    map_segment_pages(pml4, chosen_addr, &seg, (flags & SHM_RDONLY) != 0)?;

    unsafe {
        (*crate::task::leader_of(task)).user_shm_id[attach_slot] = shmid;
        (*crate::task::leader_of(task)).user_shm_addr[attach_slot] = chosen_addr;
        (*crate::task::leader_of(task)).user_shm_size[attach_slot] = seg.num_pages as u64 * PAGE_SIZE;
        (*crate::task::leader_of(task)).user_shm_flags[attach_slot] = flags as u32;
    }

    if state.segments[seg_idx].ref_count < u16::MAX {
        state.segments[seg_idx].ref_count += 1;
    }

    Ok(chosen_addr)
}

pub fn shm_detach(task: *mut Task, addr: u64) -> Result<(), i64> {
    if task.is_null() {
        return Err(EINVAL);
    }

    let mut found_slot = None;
    let mut idx = 0usize;
    while idx < MAX_TASK_SHM_ATTACH {
        let is_match =
            unsafe { (*crate::task::leader_of(task)).user_shm_id[idx] != 0 && (*crate::task::leader_of(task)).user_shm_addr[idx] == addr };
        if is_match {
            found_slot = Some(idx);
            break;
        }
        idx += 1;
    }

    let slot = match found_slot {
        Some(slot) => slot,
        None => return Err(ENOENT),
    };

    let (shmid, vaddr, size, pml4) = unsafe {
        (
            (*crate::task::leader_of(task)).user_shm_id[slot],
            (*crate::task::leader_of(task)).user_shm_addr[slot],
            (*crate::task::leader_of(task)).user_shm_size[slot],
            PhysAddr::new((*task).pml4_phys),
        )
    };

    unsafe {
        (*crate::task::leader_of(task)).user_shm_id[slot] = 0;
        (*crate::task::leader_of(task)).user_shm_addr[slot] = 0;
        (*crate::task::leader_of(task)).user_shm_size[slot] = 0;
        (*crate::task::leader_of(task)).user_shm_flags[slot] = 0;
    }

    if pml4.as_u64() != 0 {
        unmap_segment_pages(pml4, vaddr, size);
    }

    let mut state = SHM_STATE.lock();
    if let Some(seg_idx) = find_by_id_locked(&state, shmid) {
        if state.segments[seg_idx].ref_count > 0 {
            state.segments[seg_idx].ref_count -= 1;
        }
        maybe_destroy_segment(&mut state.segments[seg_idx]);
    }

    Ok(())
}

pub fn shm_ctl(shmid: i32, cmd: i32) -> Result<(), i64> {
    if cmd != IPC_RMID {
        return Err(EINVAL);
    }

    let mut state = SHM_STATE.lock();
    let seg_idx = match find_by_id_locked(&state, shmid) {
        Some(idx) => idx,
        None => return Err(ENOENT),
    };
    state.segments[seg_idx].marked_for_removal = true;
    maybe_destroy_segment(&mut state.segments[seg_idx]);
    Ok(())
}

/// True if `addr` lies inside one of `task`'s attachments.
pub fn task_attachment_contains(task: *mut Task, addr: u64) -> bool {
    if task.is_null() {
        return false;
    }
    let mut idx = 0usize;
    while idx < MAX_TASK_SHM_ATTACH {
        let (id, start, size) = unsafe {
            (
                (*crate::task::leader_of(task)).user_shm_id[idx],
                (*crate::task::leader_of(task)).user_shm_addr[idx],
                (*crate::task::leader_of(task)).user_shm_size[idx],
            )
        };
        if id != 0 && addr >= start && addr < start.saturating_add(size) {
            return true;
        }
        idx += 1;
    }
    false
}

/// Give a freshly forked `child` the same attachments as `parent`, mapping
/// the shared segment pages into the child's address space (fork must not
/// turn shm into private copy-on-write memory).
pub fn fork_attachments(parent: *mut Task, child: *mut Task) -> Result<(), i64> {
    if parent.is_null() || child.is_null() {
        return Err(EINVAL);
    }
    let child_pml4 = unsafe { PhysAddr::new((*child).pml4_phys) };
    if child_pml4.as_u64() == 0 {
        return Err(EINVAL);
    }

    let mut idx = 0usize;
    while idx < MAX_TASK_SHM_ATTACH {
        let (shmid, addr, size, flags) = unsafe {
            (
                (*crate::task::leader_of(parent)).user_shm_id[idx],
                (*crate::task::leader_of(parent)).user_shm_addr[idx],
                (*crate::task::leader_of(parent)).user_shm_size[idx],
                (*crate::task::leader_of(parent)).user_shm_flags[idx],
            )
        };
        idx += 1;
        if shmid == 0 {
            continue;
        }

        let mut state = SHM_STATE.lock();
        let seg_idx = match find_by_id_locked(&state, shmid) {
            Some(seg_idx) => seg_idx,
            None => continue,
        };
        let seg = state.segments[seg_idx];
        map_segment_pages(
            child_pml4,
            addr,
            &seg,
            (flags as i32 & SHM_RDONLY) != 0,
        )?;
        unsafe {
            (*crate::task::leader_of(child)).user_shm_id[idx - 1] = shmid;
            (*crate::task::leader_of(child)).user_shm_addr[idx - 1] = addr;
            (*crate::task::leader_of(child)).user_shm_size[idx - 1] = size;
            (*crate::task::leader_of(child)).user_shm_flags[idx - 1] = flags;
        }
        if state.segments[seg_idx].ref_count < u16::MAX {
            state.segments[seg_idx].ref_count += 1;
        }
    }
    Ok(())
}

pub fn cleanup_task(task: *mut Task) {
    if task.is_null() {
        return;
    }

    let mut idx = 0usize;
    while idx < MAX_TASK_SHM_ATTACH {
        let addr = unsafe { (*crate::task::leader_of(task)).user_shm_addr[idx] };
        if addr != 0 {
            let _ = shm_detach(task, addr);
        }
        idx += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task;
    use std::sync::Mutex;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn shm_get_reuses_existing_key() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        init();
        let first = shm_get(7, 4096, IPC_CREAT).expect("create first");
        let second = shm_get(7, 4096, IPC_CREAT).expect("reuse existing");
        assert_eq!(first, second);
    }

    #[test]
    fn shm_get_exclusive_rejects_existing() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        init();
        let _ = shm_get(11, 4096, IPC_CREAT).expect("create");
        let err = shm_get(11, 4096, IPC_CREAT | IPC_EXCL).unwrap_err();
        assert_eq!(err, EEXIST);
    }

    #[test]
    fn shm_get_without_create_requires_existing_key() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        init();
        let err = shm_get(42, 4096, 0).unwrap_err();
        assert_eq!(err, ENOENT);
    }

    #[test]
    fn shm_attach_detach_and_remove_lifecycle() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        init();
        let shmid = shm_get(99, PAGE_SIZE, IPC_CREAT).expect("create segment");

        let mut t = task::Task::for_test(1);
        t.flags = task::TASK_FLAG_USER;
        t.pml4_phys = 0x1000;
        let task_ptr = &mut t as *mut task::Task;

        let addr = shm_attach(task_ptr, shmid, 0, 0).expect("attach");
        assert_eq!(addr, SHM_AUTO_BASE);
        assert_eq!(unsafe { (*crate::task::leader_of(task_ptr)).user_shm_id[0] }, shmid);

        shm_ctl(shmid, IPC_RMID).expect("mark remove");
        assert!(shm_detach(task_ptr, addr).is_ok());
        assert_eq!(unsafe { (*crate::task::leader_of(task_ptr)).user_shm_id[0] }, 0);
        assert_eq!(shm_attach(task_ptr, shmid, 0, 0).unwrap_err(), ENOENT);
    }

    #[test]
    fn fork_attachments_shares_segment_with_child() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        init();
        let shmid = shm_get(123, PAGE_SIZE, IPC_CREAT).expect("create segment");

        let mut parent = task::Task::for_test(1);
        parent.flags = task::TASK_FLAG_USER;
        parent.pml4_phys = 0x1000;
        let parent_ptr = &mut parent as *mut task::Task;
        let mut child = task::Task::for_test(2);
        child.flags = task::TASK_FLAG_USER;
        child.pml4_phys = 0x2000;
        let child_ptr = &mut child as *mut task::Task;

        let addr = shm_attach(parent_ptr, shmid, 0, SHM_RDONLY).expect("attach");
        assert!(task_attachment_contains(parent_ptr, addr));
        assert!(task_attachment_contains(parent_ptr, addr + PAGE_SIZE - 1));
        assert!(!task_attachment_contains(parent_ptr, addr + PAGE_SIZE));

        fork_attachments(parent_ptr, child_ptr).expect("fork attachments");
        assert_eq!(child.user_shm_id[0], shmid);
        assert_eq!(child.user_shm_addr[0], addr);
        assert_eq!(child.user_shm_flags[0], SHM_RDONLY as u32);

        // The segment survives until both processes detach.
        shm_ctl(shmid, IPC_RMID).expect("mark remove");
        shm_detach(parent_ptr, addr).expect("parent detach");
        {
            let state = SHM_STATE.lock();
            assert!(find_by_id_locked(&state, shmid).is_some());
        }
        shm_detach(child_ptr, addr).expect("child detach");
        let state = SHM_STATE.lock();
        assert!(find_by_id_locked(&state, shmid).is_none());
    }
}
