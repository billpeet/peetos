//! Copying data between kernel and user address spaces.

use super::*;

pub(super) fn user_range_ok(start: u64, len: usize) -> bool {
    if len == 0 {
        return true;
    }

    if start > USER_CANON_MAX || start >= KERNEL_VIRT_BASE {
        return false;
    }

    let end = match start.checked_add((len as u64).saturating_sub(1)) {
        Some(end) => end,
        None => return false,
    };

    end <= USER_CANON_MAX && end < KERNEL_VIRT_BASE
}

pub(super) fn copy_user_bytes(
    pml4: PhysAddr,
    user_addr: u64,
    kernel_buf: *mut u8,
    len: usize,
    to_user: bool,
) -> Result<(), i64> {
    if len == 0 {
        return Ok(());
    }
    if !user_range_ok(user_addr, len) {
        return Err(EFAULT);
    }

    let mut remaining = len;
    let mut copied = 0usize;

    while remaining > 0 {
        let vaddr = user_addr + copied as u64;
        let page_vaddr = vaddr & !(PAGE_SIZE - 1);
        let page_off = (vaddr & (PAGE_SIZE - 1)) as usize;
        let bytes_in_page = cmp::min(remaining, PAGE_SIZE as usize - page_off);

        let page_phys = match vmm::get_physical_in(pml4, VirtAddr::new(page_vaddr)) {
            Some(phys) => phys,
            None => return Err(EFAULT),
        };

        let page_ptr = (PhysAddr::new(page_phys.as_u64() + page_off as u64)
            .to_virt()
            .as_u64()) as *mut u8;
        let kernel_ptr = unsafe { kernel_buf.add(copied) };

        unsafe {
            if to_user {
                ptr::copy_nonoverlapping(kernel_ptr as *const u8, page_ptr, bytes_in_page);
            } else {
                ptr::copy_nonoverlapping(page_ptr as *const u8, kernel_ptr, bytes_in_page);
            }
        }

        copied += bytes_in_page;
        remaining -= bytes_in_page;
    }

    Ok(())
}

pub(super) fn copy_from_user_task(
    task_ptr: *mut task::Task,
    user_src: u64,
    dst: &mut [u8],
) -> Result<(), i64> {
    if task_ptr.is_null() {
        return Err(ESRCH);
    }
    let pml4 = unsafe { PhysAddr::new((*task_ptr).pml4_phys) };
    if pml4.as_u64() == 0 {
        return Err(EINVAL);
    }
    copy_user_bytes(pml4, user_src, dst.as_mut_ptr(), dst.len(), false)
}

pub(super) fn copy_to_user_task(
    task_ptr: *mut task::Task,
    user_dst: u64,
    src: &[u8],
) -> Result<(), i64> {
    if task_ptr.is_null() {
        return Err(ESRCH);
    }
    let pml4 = unsafe { PhysAddr::new((*task_ptr).pml4_phys) };
    if pml4.as_u64() == 0 {
        return Err(EINVAL);
    }
    copy_user_bytes(pml4, user_dst, src.as_ptr() as *mut u8, src.len(), true)
}

pub(super) fn write_struct_to_user<T: Copy>(
    task_ptr: *mut task::Task,
    user_dst: u64,
    value: &T,
) -> Result<(), i64> {
    let bytes =
        unsafe { core::slice::from_raw_parts((value as *const T) as *const u8, size_of::<T>()) };
    copy_to_user_task(task_ptr, user_dst, bytes)
}

pub(super) fn read_struct_from_user<T: Copy>(
    task_ptr: *mut task::Task,
    user_src: u64,
) -> Result<T, i64> {
    let mut raw = MaybeUninit::<T>::uninit();
    let bytes =
        unsafe { core::slice::from_raw_parts_mut(raw.as_mut_ptr() as *mut u8, size_of::<T>()) };
    copy_from_user_task(task_ptr, user_src, bytes)?;
    Ok(unsafe { raw.assume_init() })
}

pub(crate) fn copy_from_user_current(user_src: u64, dst: &mut [u8]) -> Result<(), i64> {
    copy_from_user_task(task::current(), user_src, dst)
}

pub(crate) fn copy_to_user_current(user_dst: u64, src: &[u8]) -> Result<(), i64> {
    copy_to_user_task(task::current(), user_dst, src)
}

pub(crate) fn copy_cstr_from_user_current(user_src: u64, out: &mut [u8]) -> Result<usize, i64> {
    if out.is_empty() {
        return Err(EINVAL);
    }

    for (idx, dst_byte) in out.iter_mut().enumerate() {
        let mut byte = [0u8; 1];
        copy_from_user_current(user_src + idx as u64, &mut byte)?;
        *dst_byte = byte[0];
        if byte[0] == 0 {
            return Ok(idx);
        }
    }

    Err(EINVAL)
}

pub(super) fn copy_cstr_from_user_task(
    task_ptr: *mut task::Task,
    user_src: u64,
    out: &mut [u8],
) -> Result<usize, i64> {
    if out.is_empty() {
        return Err(-EINVAL);
    }

    for (idx, dst_byte) in out.iter_mut().enumerate() {
        let mut byte = [0u8; 1];
        if let Err(err) = copy_from_user_task(task_ptr, user_src + idx as u64, &mut byte) {
            return Err(-err);
        }
        *dst_byte = byte[0];
        if byte[0] == 0 {
            return Ok(idx);
        }
    }

    Err(-E2BIG)
}

pub(super) fn read_u64_from_user(task_ptr: *mut task::Task, user_src: u64) -> Result<u64, i64> {
    let mut raw = [0u8; 8];
    copy_from_user_task(task_ptr, user_src, &mut raw)?;
    Ok(u64::from_ne_bytes(raw))
}

pub(super) fn write_u64_to_user(
    task_ptr: *mut task::Task,
    user_dst: u64,
    value: u64,
) -> Result<(), i64> {
    copy_to_user_task(task_ptr, user_dst, &value.to_ne_bytes())
}

pub(super) fn read_i32_from_user(task_ptr: *mut task::Task, user_src: u64) -> Result<i32, i64> {
    let mut raw = [0u8; 4];
    copy_from_user_task(task_ptr, user_src, &mut raw)?;
    Ok(i32::from_ne_bytes(raw))
}

pub(super) fn write_i32_to_user(
    task_ptr: *mut task::Task,
    user_dst: u64,
    value: i32,
) -> Result<(), i64> {
    copy_to_user_task(task_ptr, user_dst, &value.to_ne_bytes())
}

pub(super) fn read_user_u32(task_ptr: *mut task::Task, user_src: u64) -> Result<u32, i64> {
    let mut raw = [0u8; 4];
    copy_from_user_task(task_ptr, user_src, &mut raw)?;
    Ok(u32::from_ne_bytes(raw))
}
