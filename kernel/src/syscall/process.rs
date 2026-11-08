//! Process lifecycle: fork/clone, execve, wait, exit, ids and process groups.

use super::*;

#[derive(Clone, Copy)]
pub(super) struct ExecCString {
    pub(super) len: usize,
    pub(super) bytes: [u8; EXEC_MAX_STR_LEN],
}

impl ExecCString {
    const EMPTY: Self = Self {
        len: 0,
        bytes: [0; EXEC_MAX_STR_LEN],
    };

    pub(super) fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

pub(super) fn arch_prctl_set_fs_addr_ok(addr: u64) -> bool {
    addr <= USER_CANON_MAX && addr < KERNEL_VIRT_BASE
}

pub(super) fn read_exec_cstr_vector(
    task_ptr: *mut task::Task,
    vector_ptr: u64,
    out: &mut [ExecCString],
) -> Result<usize, i64> {
    if vector_ptr == 0 {
        return Ok(0);
    }

    for idx in 0..out.len() {
        let slot_ptr = vector_ptr + (idx * size_of::<u64>()) as u64;
        let entry_ptr = match read_u64_from_user(task_ptr, slot_ptr) {
            Ok(ptr) => ptr,
            Err(err) => return Err(-err),
        };
        if entry_ptr == 0 {
            return Ok(idx);
        }

        let len = copy_cstr_from_user_task(task_ptr, entry_ptr, &mut out[idx].bytes)?;
        out[idx].len = len + 1;
    }

    Err(-E2BIG)
}

pub(super) struct ExecImage {
    pub(super) ptr: *mut u8,
    pub(super) len: usize,
}

impl ExecImage {
    pub(super) fn as_slice(&self) -> &[u8] {
        unsafe { core::slice::from_raw_parts(self.ptr as *const u8, self.len) }
    }
}

pub(super) fn read_exec_image_from_inode(inode: u16) -> Result<ExecImage, i64> {
    let stat = fs::stat(inode).map_err(|err| -fs_to_errno(err))?;
    if stat.file_type != fs::FileType::Regular {
        return Err(-EACCES);
    }

    let size = usize::try_from(stat.size).map_err(|_| -EINVAL)?;
    if size == 0 || size > EXEC_MAX_IMAGE_SIZE {
        return Err(-EINVAL);
    }

    let ptr = heap::kmalloc(size);
    if ptr.is_null() {
        return Err(-ENOMEM);
    }

    let image = ExecImage { ptr, len: size };
    let dst = unsafe { core::slice::from_raw_parts_mut(ptr, size) };
    let mut copied = 0usize;
    while copied < dst.len() {
        let read = fs::read(inode, copied as u64, &mut dst[copied..]).map_err(|err| {
            heap::kfree(ptr);
            -fs_to_errno(err)
        })?;
        if read == 0 {
            heap::kfree(ptr);
            return Err(-EIO);
        }
        copied += read;
    }

    Ok(image)
}

/// execve replaces the whole process, so the other threads must go first.
/// A non-leader caller first becomes the leader (taking over the process id
/// and process-wide state), as on Linux.
pub(super) fn exec_prepare_thread_group(ctx: &SyscallContext) -> Result<(), i64> {
    let current = ctx.current;
    let old_leader = task::leader_of(current);
    if old_leader != current {
        task::take_over_group_leadership(current);
        mmap_move_task_regions(old_leader, current);
    }
    let tgid = unsafe { (*current).tgid };
    if task::live_threads_in_group(tgid) > 1 {
        kill_other_threads(current, 0);
    }
    Ok(())
}

pub(super) fn execve_common(
    ctx: &SyscallContext,
    filename_ptr: u64,
    argv_ptr: u64,
    envp_ptr: u64,
) -> Result<user::ExecImageStart, i64> {
    let mut path_buf = [0u8; fs::VFS_MAX_PATH];
    let path = read_user_path(filename_ptr, &mut path_buf)?;
    if path.is_empty() {
        return Err(-ENOENT);
    }
    if path.len() + 1 > fs::VFS_MAX_PATH {
        return Err(-EINVAL);
    }

    let argv_buf_size = size_of::<ExecCString>() * EXEC_MAX_ARGV;
    let argv_buf_ptr = heap::kmalloc(argv_buf_size) as *mut ExecCString;
    if argv_buf_ptr.is_null() {
        return Err(-ENOMEM);
    }
    unsafe {
        ptr::write_bytes(argv_buf_ptr as *mut u8, 0, argv_buf_size);
    }
    let argv_buf = unsafe { core::slice::from_raw_parts_mut(argv_buf_ptr, EXEC_MAX_ARGV) };
    let argc = match read_exec_cstr_vector(ctx.current, argv_ptr, argv_buf) {
        Ok(argc) => argc,
        Err(err) => {
            heap::kfree(argv_buf_ptr as *mut u8);
            return Err(err);
        }
    };

    let env_buf_size = size_of::<ExecCString>() * EXEC_MAX_ENVP;
    let env_buf_ptr = heap::kmalloc(env_buf_size) as *mut ExecCString;
    if env_buf_ptr.is_null() {
        heap::kfree(argv_buf_ptr as *mut u8);
        return Err(-ENOMEM);
    }
    unsafe {
        ptr::write_bytes(env_buf_ptr as *mut u8, 0, env_buf_size);
    }
    let env_buf = unsafe { core::slice::from_raw_parts_mut(env_buf_ptr, EXEC_MAX_ENVP) };
    let envc = match read_exec_cstr_vector(ctx.current, envp_ptr, env_buf) {
        Ok(envc) => envc,
        Err(err) => {
            heap::kfree(env_buf_ptr as *mut u8);
            heap::kfree(argv_buf_ptr as *mut u8);
            return Err(err);
        }
    };

    let mut argv = [&[][..]; EXEC_MAX_ARGV];
    for idx in 0..argc {
        argv[idx] = argv_buf[idx].as_bytes();
    }
    let mut env = [&[][..]; EXEC_MAX_ENVP];
    for idx in 0..envc {
        env[idx] = env_buf[idx].as_bytes();
    }

    let dir_inode = match resolve_dir_for_path(ctx, fs::AT_FDCWD, path) {
        Ok(d) => d,
        Err(err) => {
            heap::kfree(env_buf_ptr as *mut u8);
            heap::kfree(argv_buf_ptr as *mut u8);
            return Err(err);
        }
    };
    let inode = match fs::lookupat(dir_inode, path) {
        Ok(i) => i,
        Err(err) => {
            heap::kfree(env_buf_ptr as *mut u8);
            heap::kfree(argv_buf_ptr as *mut u8);
            return Err(-fs_to_errno(err));
        }
    };
    let image = match read_exec_image_from_inode(inode) {
        Ok(image) => image,
        Err(err) => {
            heap::kfree(env_buf_ptr as *mut u8);
            heap::kfree(argv_buf_ptr as *mut u8);
            return Err(err);
        }
    };
    let execfn_len = path.len();
    path_buf[execfn_len] = 0;
    let execfn = &path_buf[..execfn_len + 1];
    // Point of no return: the new image was read, so the other threads go
    // now (failures before this leave the process untouched).
    if let Err(err) = exec_prepare_thread_group(ctx) {
        heap::kfree(env_buf_ptr as *mut u8);
        heap::kfree(argv_buf_ptr as *mut u8);
        heap::kfree(image.ptr);
        return Err(err);
    }
    let result = user::exec_current_image(image.as_slice(), &argv[..argc], &env[..envc], execfn);
    heap::kfree(env_buf_ptr as *mut u8);
    heap::kfree(argv_buf_ptr as *mut u8);
    heap::kfree(image.ptr);
    if result.is_ok() {
        mmap_clear_task_regions(ctx.current);
        close_cloexec_fds(ctx.current);
    }
    result
}

#[inline]
pub(super) fn wait_status_from_exit_code(code: i32) -> i32 {
    (code & 0xFF) << 8
}

pub(super) fn fork_cleanup_child(task_ptr: *mut task::Task) {
    if task_ptr.is_null() {
        return;
    }
    let owner_pid = unsafe { (*task_ptr).pid };
    mmap_clear_task_regions(task_ptr);
    close_all_fds(task_ptr);
    super::file::release_cwd(task_ptr);
    epoll_drop_watches_for_owner(owner_pid);
    shm::cleanup_task(task_ptr);
    teardown_user_address_space(task_ptr);
    task::task_destroy(task_ptr);
}

pub(super) fn clone_parent_fds_for_child(
    parent: *mut task::Task,
    child: *mut task::Task,
) -> Result<(), i64> {
    unsafe {
        (*crate::task::leader_of(child)).user_fd_kind = (*crate::task::leader_of(parent)).user_fd_kind;
        (*crate::task::leader_of(child)).user_fd_inode = (*crate::task::leader_of(parent)).user_fd_inode;
        (*crate::task::leader_of(child)).user_fd_offset = (*crate::task::leader_of(parent)).user_fd_offset;
        (*crate::task::leader_of(child)).user_fd_flags = (*crate::task::leader_of(parent)).user_fd_flags;
    }

    for idx in 0..task::MAX_TASK_FDS {
        let (kind, inode, flags) = unsafe {
            (
                (*crate::task::leader_of(child)).user_fd_kind[idx],
                (*crate::task::leader_of(child)).user_fd_inode[idx],
                (*crate::task::leader_of(child)).user_fd_flags[idx],
            )
        };
        fd_object_retain(kind, inode, flags)?;
    }

    Ok(())
}

pub(super) fn create_fork_child(
    ctx: &SyscallContext,
    child_frame: &InterruptFrame,
    req: &CloneRequest,
) -> Result<i64, i64> {
    if req.is_thread() {
        return create_thread_child(ctx, child_frame, req);
    }
    let parent = ctx.current;
    if parent.is_null() {
        return Err(-ESRCH);
    }
    reap_exited_threads();
    let parent_pml4 = unsafe { PhysAddr::new((*parent).pml4_phys) };
    if parent_pml4.as_u64() == 0 {
        return Err(-EINVAL);
    }

    let child = task::task_alloc("fork", TASK_FLAG_USER);
    if child.is_null() {
        return Err(-ENOMEM);
    }

    let child_pml4 = vmm::create_address_space();
    if child_pml4.as_u64() == 0 {
        task::task_destroy(child);
        return Err(-ENOMEM);
    }
    // Shared file mappings, device mappings and shm stay shared after fork:
    // keep them out of the COW clone. The child refaults file/device pages
    // from the inherited mapping metadata and re-attaches shm below.
    let parent_regions = mmap_prepare_task_slot(parent)
        .map(|slot| unsafe { TASK_VM_MAPPINGS[slot] })
        .unwrap_or([VmMapping::EMPTY; MMAP_MAX_REGIONS_PER_TASK]);
    let skip_shared = |virt: u64| -> bool {
        if let Some(region) = mmap_regions_find_mapping(&parent_regions, virt) {
            if mmap_mapping_is_framebuffer(&region)
                || (region.map_shared && region.file_inode != task::USER_FD_NONE)
            {
                return true;
            }
        }
        shm::task_attachment_contains(parent, virt)
    };
    if vmm::clone_user_space(parent_pml4, child_pml4, &skip_shared) != 0 {
        vmm::clear_user_space(child_pml4);
        vmm::destroy_address_space(child_pml4);
        task::task_destroy(child);
        return Err(-ENOMEM);
    }

    unsafe {
        (*child).pml4_phys = child_pml4.as_u64();

        (*crate::task::leader_of(child)).user_entry = (*crate::task::leader_of(parent)).user_entry;
        (*crate::task::leader_of(child)).user_stack_bottom = (*crate::task::leader_of(parent)).user_stack_bottom;
        (*crate::task::leader_of(child)).user_stack_top = (*crate::task::leader_of(parent)).user_stack_top;
        (*crate::task::leader_of(child)).user_stack_ptr = (*crate::task::leader_of(parent)).user_stack_ptr;
        (*crate::task::leader_of(child)).user_argc = (*crate::task::leader_of(parent)).user_argc;
        (*crate::task::leader_of(child)).user_argv = (*crate::task::leader_of(parent)).user_argv;
        (*crate::task::leader_of(child)).user_envp = (*crate::task::leader_of(parent)).user_envp;
        (*crate::task::leader_of(child)).user_auxv = (*crate::task::leader_of(parent)).user_auxv;
        (*child).user_fs_base = (*parent).user_fs_base;
        // The parent is the running task, so the live registers are its
        // user FPU/SSE state; the child resumes with an identical copy.
        (*child).fpu.save();
        (*child).user_clear_tid = 0;
        (*crate::task::leader_of(child)).user_brk_start = (*crate::task::leader_of(parent)).user_brk_start;
        (*crate::task::leader_of(child)).user_brk = (*crate::task::leader_of(parent)).user_brk;
        (*crate::task::leader_of(child)).user_phdr_addr = (*crate::task::leader_of(parent)).user_phdr_addr;
        (*crate::task::leader_of(child)).user_phdr_count = (*crate::task::leader_of(parent)).user_phdr_count;
        (*crate::task::leader_of(child)).user_phdr_size = (*crate::task::leader_of(parent)).user_phdr_size;
        (*crate::task::leader_of(child)).user_base_addr = (*crate::task::leader_of(parent)).user_base_addr;
        (*crate::task::leader_of(child)).user_interp_base = (*crate::task::leader_of(parent)).user_interp_base;
        (*crate::task::leader_of(child)).user_is_dynamic = (*crate::task::leader_of(parent)).user_is_dynamic;
        (*crate::task::leader_of(child)).user_uid = (*crate::task::leader_of(parent)).user_uid;
        (*crate::task::leader_of(child)).user_gid = (*crate::task::leader_of(parent)).user_gid;
        (*crate::task::leader_of(child)).user_cwd_inode = (*crate::task::leader_of(parent)).user_cwd_inode;
        fs::inode_retain((*crate::task::leader_of(child)).user_cwd_inode);
        (*crate::task::leader_of(child)).pgid = (*crate::task::leader_of(parent)).pgid;
        (*crate::task::leader_of(child)).sid = (*crate::task::leader_of(parent)).sid;
        (*crate::task::leader_of(child)).ctty_type = (*crate::task::leader_of(parent)).ctty_type;
        (*crate::task::leader_of(child)).ctty_id = (*crate::task::leader_of(parent)).ctty_id;
        (*crate::task::leader_of(child)).signal_handlers = (*crate::task::leader_of(parent)).signal_handlers;
        (*crate::task::leader_of(child)).signal_flags = (*crate::task::leader_of(parent)).signal_flags;
        (*crate::task::leader_of(child)).signal_mask_saved = (*crate::task::leader_of(parent)).signal_mask_saved;
        (*crate::task::leader_of(child)).signal_restorer = (*crate::task::leader_of(parent)).signal_restorer;
        (*child).signal_blocked = (*parent).signal_blocked;
        (*child).sig_pending = 0;
        (*crate::task::leader_of(child)).user_shm_id = [0; task::MAX_TASK_SHM_ATTACH];
        (*crate::task::leader_of(child)).user_shm_addr = [0; task::MAX_TASK_SHM_ATTACH];
        (*crate::task::leader_of(child)).user_shm_size = [0; task::MAX_TASK_SHM_ATTACH];
        (*crate::task::leader_of(child)).user_shm_flags = [0; task::MAX_TASK_SHM_ATTACH];
    }

    if let Err(err) = mmap_clone_task_regions(parent, child) {
        fork_cleanup_child(child);
        return Err(err);
    }
    if let Err(err) = shm::fork_attachments(parent, child) {
        fork_cleanup_child(child);
        return Err(-shm_to_errno(err));
    }

    if let Err(err) = clone_parent_fds_for_child(parent, child) {
        fork_cleanup_child(child);
        return Err(err);
    }
    if let Err(err) = apply_clone_tid_flags(parent, child, req) {
        fork_cleanup_child(child);
        return Err(err);
    }

    start_cloned_task(child, child_frame);
    Ok(unsafe { (*child).pid as i64 })
}

pub(super) fn start_cloned_task(child: *mut task::Task, child_frame: &InterruptFrame) {
    unsafe {
        let mut frame_addr = (*child).kernel_stack_top - size_of::<InterruptFrame>() as u64;
        frame_addr &= !0xF;
        let frame_ptr = frame_addr as *mut InterruptFrame;
        ptr::write(frame_ptr, *child_frame);
        (*child).saved_rsp = frame_addr;
        (*child).state = TaskState::Ready;
    }
    crate::sched::add_task(child);
}

/// Handle CLONE_SETTLS / CLONE_*_SETTID / CLONE_CHILD_CLEARTID for a new task.
pub(super) fn apply_clone_tid_flags(
    parent: *mut task::Task,
    child: *mut task::Task,
    req: &CloneRequest,
) -> Result<(), i64> {
    let tid = unsafe { (*child).pid };
    unsafe {
        if (req.flags & CLONE_SETTLS) != 0 {
            (*child).user_fs_base = req.tls;
        }
        if (req.flags & CLONE_CHILD_CLEARTID) != 0 {
            (*child).user_clear_tid = req.child_tid;
        }
    }
    if (req.flags & CLONE_PARENT_SETTID) != 0 {
        write_i32_to_user(parent, req.parent_tid, tid).map_err(|err| -err)?;
    }
    if (req.flags & CLONE_CHILD_SETTID) != 0 {
        // Written through the child's page tables: after a COW fork this
        // gives the child its own copy of the page.
        write_i32_to_user(child, req.child_tid, tid).map_err(|err| -err)?;
    }
    Ok(())
}

/// `clone(CLONE_THREAD | ...)`: a new task in the caller's thread group,
/// sharing its address space and all process-wide state.
pub(super) fn create_thread_child(
    ctx: &SyscallContext,
    child_frame: &InterruptFrame,
    req: &CloneRequest,
) -> Result<i64, i64> {
    let parent = ctx.current;
    if parent.is_null() {
        return Err(-ESRCH);
    }
    let leader = task::leader_of(parent);
    if unsafe { ((*leader).flags & task::TASK_FLAG_GROUP_EXIT) != 0 } {
        return Err(-EAGAIN);
    }
    reap_exited_threads();

    let child = task::task_alloc("thread", TASK_FLAG_USER);
    if child.is_null() {
        return Err(-ENOMEM);
    }
    task::join_thread_group(child, parent);
    unsafe {
        (*child).name = (*parent).name;
        (*child).pml4_phys = (*parent).pml4_phys;
        (*child).user_fs_base = (*parent).user_fs_base;
        (*child).signal_blocked = (*parent).signal_blocked;
        (*child).sig_pending = 0;
        // The creating thread is running, so the live registers are its user
        // FPU/SSE state; the new thread starts with a copy, as on Linux.
        (*child).fpu.save();
    }
    if let Err(err) = apply_clone_tid_flags(parent, child, req) {
        task::task_destroy(child);
        return Err(err);
    }

    start_cloned_task(child, child_frame);
    Ok(unsafe { (*child).pid as i64 })
}

pub(super) fn build_child_frame_from_syscall_frame(frame: &SyscallFrame) -> InterruptFrame {
    InterruptFrame {
        r15: frame.r15,
        r14: frame.r14,
        r13: frame.r13,
        r12: frame.r12,
        r11: frame.r11,
        r10: frame.r10,
        r9: frame.r9,
        r8: frame.r8,
        rbp: frame.rbp,
        rdi: frame.rdi,
        rsi: frame.rsi,
        rdx: frame.rdx,
        rcx: frame.rcx,
        rbx: frame.rbx,
        rax: 0,
        int_no: 0,
        error_code: 0,
        rip: frame.rcx,
        cs: gdt::USER_CS as u64,
        rflags: frame.r11,
        rsp: frame.rsp,
        ss: gdt::USER_DS as u64,
    }
}

pub(super) fn build_child_frame_from_int80_frame(frame: &InterruptFrame) -> InterruptFrame {
    let mut child = *frame;
    child.rax = 0;
    child.int_no = 0;
    child.error_code = 0;
    child
}

/// Decoded `clone` arguments (x86-64 order: flags, stack, ptid, ctid, tls).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CloneRequest {
    pub(super) flags: u64,
    pub(super) child_stack: u64,
    pub(super) parent_tid: u64,
    pub(super) child_tid: u64,
    pub(super) tls: u64,
}

impl CloneRequest {
    /// Plain `fork()`/`vfork()`.
    pub(super) const FORK: Self = Self {
        flags: SIGCHLD_EXIT_SIGNAL,
        child_stack: 0,
        parent_tid: 0,
        child_tid: 0,
        tls: 0,
    };

    #[inline]
    pub(super) fn is_thread(&self) -> bool {
        (self.flags & CLONE_THREAD) != 0
    }
}

pub(super) fn validate_clone_args(call: &SyscallArgs) -> Result<CloneRequest, i64> {
    let req = CloneRequest {
        flags: call.arg(0),
        child_stack: call.arg(1),
        parent_tid: call.arg(2),
        child_tid: call.arg(3),
        tls: call.arg(4),
    };
    let flags = req.flags;

    let supported = CLONE_CSIGNAL
        | CLONE_THREAD_SHARE
        | CLONE_VFORK
        | CLONE_SYSVSEM
        | CLONE_DETACHED
        | CLONE_TID_FLAGS;
    if (flags & !supported) != 0 {
        return Err(-EINVAL);
    }

    if req.is_thread() {
        // Threads must share everything and get their own stack.
        if (flags & CLONE_THREAD_SHARE) != CLONE_THREAD_SHARE {
            return Err(-EINVAL);
        }
        if (flags & (CLONE_VFORK | CLONE_CSIGNAL)) != 0 {
            return Err(-EINVAL);
        }
        if req.child_stack == 0 {
            return Err(-EINVAL);
        }
    } else {
        if (flags & (CLONE_FS | CLONE_FILES | CLONE_SIGHAND)) != 0 {
            return Err(-EINVAL);
        }
        if (flags & CLONE_CSIGNAL) != SIGCHLD_EXIT_SIGNAL {
            return Err(-EINVAL);
        }
        // CLONE_VM is only accepted together with CLONE_VFORK, and both are
        // handled as a (copy-on-write) fork.
        let clone_vm = (flags & CLONE_VM) != 0;
        let clone_vfork = (flags & CLONE_VFORK) != 0;
        if clone_vm != clone_vfork {
            return Err(-EINVAL);
        }
    }

    if (flags & CLONE_PARENT_SETTID) != 0 && !user_range_ok(req.parent_tid, 4) {
        return Err(-EFAULT);
    }
    if (flags & (CLONE_CHILD_SETTID | CLONE_CHILD_CLEARTID)) != 0
        && !user_range_ok(req.child_tid, 4)
    {
        return Err(-EFAULT);
    }
    if (flags & CLONE_SETTLS) != 0 && !arch_prctl_set_fs_addr_ok(req.tls) {
        return Err(-EINVAL);
    }
    if req.child_stack != 0 && !user_range_ok(req.child_stack, 1) {
        return Err(-EFAULT);
    }
    Ok(req)
}

pub(super) fn build_clone_child_frame_from_syscall(
    frame: &SyscallFrame,
    child_stack: u64,
) -> Result<InterruptFrame, i64> {
    let mut child = build_child_frame_from_syscall_frame(frame);
    if child_stack != 0 {
        child.rsp = child_stack;
    }
    Ok(child)
}

pub(super) fn build_clone_child_frame_from_int80(
    frame: &InterruptFrame,
    child_stack: u64,
) -> Result<InterruptFrame, i64> {
    let mut child = build_child_frame_from_int80_frame(frame);
    if child_stack != 0 {
        child.rsp = child_stack;
    }
    Ok(child)
}

pub(super) fn sys_futex(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let uaddr = call.arg(0);
    if !user_range_ok(uaddr, 4) {
        return -EFAULT;
    }

    let op = call.arg(1) as u32;
    let cmd = op & 0x7F;
    let val = call.arg(2) as u32;

    match cmd {
        ipc::FUTEX_WAIT | ipc::FUTEX_WAIT_BITSET => {
            // Bitsets are accepted but not used to filter wakeups: a zero
            // mask is invalid, any other mask behaves like "match all".
            if cmd == ipc::FUTEX_WAIT_BITSET && call.arg(5) as u32 == 0 {
                return -EINVAL;
            }
            let deadline = match futex_deadline(ctx, call.arg(3), cmd == ipc::FUTEX_WAIT_BITSET) {
                Ok(deadline) => deadline,
                Err(err) => return err,
            };
            let current = match read_user_u32(ctx.current, uaddr) {
                Ok(v) => v,
                Err(err) => return -err,
            };
            if current != val {
                return -EAGAIN;
            }
            if let Some(deadline) = deadline {
                if pit::ticks() >= deadline {
                    return -ETIMEDOUT;
                }
            }
            match ipc::futex_wait(ctx.current, uaddr, deadline) {
                Ok(()) => 0,
                Err(err) => -ipc_to_errno(err),
            }
        }
        ipc::FUTEX_WAKE => ipc::futex_wake(uaddr, val) as i64,
        ipc::FUTEX_WAKE_BITSET => {
            if call.arg(5) as u32 == 0 {
                return -EINVAL;
            }
            ipc::futex_wake(uaddr, val) as i64
        }
        ipc::FUTEX_REQUEUE | ipc::FUTEX_CMP_REQUEUE => {
            // The timeout slot carries the requeue limit for these ops.
            let nr_requeue = call.arg(3) as u32;
            let uaddr2 = call.arg(4);
            if !user_range_ok(uaddr2, 4) {
                return -EFAULT;
            }
            if cmd == ipc::FUTEX_CMP_REQUEUE {
                let current = match read_user_u32(ctx.current, uaddr) {
                    Ok(v) => v,
                    Err(err) => return -err,
                };
                if current != call.arg(5) as u32 {
                    return -EAGAIN;
                }
            }
            ipc::futex_requeue(uaddr, uaddr2, val, nr_requeue) as i64
        }
        _ => -ENOSYS,
    }
}

/// Convert a futex timeout argument to an absolute PIT-tick deadline.
/// FUTEX_WAIT takes a relative timeout; FUTEX_WAIT_BITSET an absolute one
/// (CLOCK_MONOTONIC and CLOCK_REALTIME share the tick clock here).
fn futex_deadline(ctx: &SyscallContext, ts_ptr: u64, absolute: bool) -> Result<Option<u64>, i64> {
    if ts_ptr == 0 {
        return Ok(None);
    }
    let ts = read_struct_from_user::<LinuxTimeSpec>(ctx.current, ts_ptr).map_err(|err| -err)?;
    let ticks = timespec_to_sleep_ticks(&ts)?;
    Ok(Some(if absolute {
        ticks
    } else {
        pit::ticks().saturating_add(ticks)
    }))
}

pub(super) fn sys_yield(_ctx: &SyscallContext, _call: &SyscallArgs) -> i64 {
    crate::sched::request_preempt();
    0
}

pub(super) fn child_pid_matches(wait_pid: i64, child_pid: i32) -> bool {
    if wait_pid == -1 {
        return true;
    }
    if wait_pid > 0 {
        return child_pid == wait_pid as i32;
    }
    false
}

pub(super) fn find_waitable_child(
    parent_pid: i32,
    wait_pid: i64,
    found_match: &mut bool,
) -> Option<*mut task::Task> {
    for idx in 0..task::MAX_TASKS {
        let child = task::get_by_index(idx);
        if child.is_null() {
            continue;
        }
        let (child_ppid, child_pid, child_state, child_flags) = unsafe {
            (
                (*crate::task::leader_of(child)).ppid,
                (*child).pid,
                (*child).state,
                (*child).flags,
            )
        };
        // Only processes (thread group leaders) are waitable.
        if (child_flags & task::TASK_FLAG_THREAD) != 0 {
            continue;
        }
        if child_ppid != parent_pid {
            continue;
        }
        if !child_pid_matches(wait_pid, child_pid) {
            continue;
        }
        *found_match = true;
        // A leader that exited before its threads is not reaped until the
        // whole group is gone.
        if child_state == TaskState::Zombie && task::live_threads_in_group(child_pid) == 0 {
            return Some(child);
        }
    }
    None
}

pub(super) fn sys_wait4(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let wait_pid = call.arg(0) as i64;
    let user_status = call.arg(1);
    let options = call.arg(2) as u32;
    let user_rusage = call.arg(3);

    if wait_pid < -1 || wait_pid == 0 {
        return -EINVAL;
    }
    if (options & !WNOHANG) != 0 {
        return -EINVAL;
    }
    if user_rusage != 0 {
        return -ENOSYS;
    }
    if ctx.current.is_null() {
        return -ESRCH;
    }

    let parent_pid = unsafe { (*ctx.current).tgid };
    loop {
        reap_exited_threads();
        let mut found_match = false;
        let child = find_waitable_child(parent_pid, wait_pid, &mut found_match);
        if let Some(child_task) = child {
            let (pid, exit_code, child_pml4) = unsafe {
                (
                    (*child_task).pid,
                    (*child_task).exit_code,
                    PhysAddr::new((*child_task).pml4_phys),
                )
            };
            let status = wait_status_from_exit_code(exit_code);

            if user_status != 0 {
                if let Err(err) = write_i32_to_user(ctx.current, user_status, status) {
                    return -err;
                }
            }

            if child_pml4.as_u64() != 0 {
                vmm::clear_user_space(child_pml4);
                vmm::destroy_address_space(child_pml4);
                unsafe {
                    (*child_task).pml4_phys = 0;
                }
            }
            task::task_destroy(child_task);
            return pid as i64;
        }

        if !found_match {
            return -ECHILD;
        }
        if (options & WNOHANG) != 0 {
            return 0;
        }
        if !crate::sched::is_running() {
            return -EAGAIN;
        }
        if signal::wait_interrupted(ctx.current) {
            return -ERESTARTSYS;
        }
        crate::sched::yield_now();
    }
}

pub(super) fn sys_fork(_ctx: &SyscallContext, _call: &SyscallArgs) -> i64 {
    // fork needs direct trap-frame access and is handled in entry stubs.
    -ENOSYS
}

pub(super) fn sys_clone(_ctx: &SyscallContext, _call: &SyscallArgs) -> i64 {
    // clone needs direct trap-frame access and is handled in entry stubs.
    -ENOSYS
}

pub(super) fn sys_vfork(_ctx: &SyscallContext, _call: &SyscallArgs) -> i64 {
    // vfork is handled in entry stubs as a clone/fork compatibility path.
    -ENOSYS
}

pub(super) fn sys_getpid(ctx: &SyscallContext, _call: &SyscallArgs) -> i64 {
    match ctx.pid() {
        Ok(pid) => pid,
        Err(err) => err,
    }
}

pub(super) fn pid_from_syscall_arg_or_current(raw: u64, current_pid: i32) -> Result<i32, i64> {
    let pid = raw as i64;
    if pid < 0 || pid > i32::MAX as i64 {
        return Err(-EINVAL);
    }
    if pid == 0 {
        return Ok(current_pid);
    }
    Ok(pid as i32)
}

pub(super) fn pgid_from_syscall_arg_or_target(raw: u64, target_pid: i32) -> Result<i32, i64> {
    let pgid = raw as i64;
    if pgid < 0 || pgid > i32::MAX as i64 {
        return Err(-EINVAL);
    }
    if pgid == 0 {
        return Ok(target_pid);
    }
    Ok(pgid as i32)
}

pub(super) fn lookup_user_task(pid: i32) -> *mut task::Task {
    let task_ptr = task::get_by_pid(pid);
    if task_ptr.is_null() {
        return ptr::null_mut();
    }
    let is_user = unsafe { ((*task_ptr).flags & TASK_FLAG_USER) != 0 };
    if !is_user {
        return ptr::null_mut();
    }
    task_ptr
}

pub(super) fn sys_setpgid(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    if !ctx.has_task() {
        return -ESRCH;
    }

    let caller_pid = unsafe { (*ctx.current).tgid };
    let caller_sid = unsafe { (*crate::task::leader_of(ctx.current)).sid };

    let target_pid = match pid_from_syscall_arg_or_current(call.arg(0), caller_pid) {
        Ok(pid) => pid,
        Err(err) => return err,
    };
    let new_pgid = match pgid_from_syscall_arg_or_target(call.arg(1), target_pid) {
        Ok(pgid) => pgid,
        Err(err) => return err,
    };
    if new_pgid <= 0 {
        return -EINVAL;
    }

    let target = lookup_user_task(target_pid);
    if target.is_null() {
        return -ESRCH;
    }

    let (target_pid_value, target_ppid, target_sid, target_state) = unsafe {
        (
            (*target).pid,
            (*crate::task::leader_of(target)).ppid,
            (*crate::task::leader_of(target)).sid,
            (*target).state,
        )
    };

    if target_state == TaskState::Zombie || target_state == TaskState::Dead {
        return -ESRCH;
    }
    if target_pid_value != caller_pid && target_ppid != caller_pid {
        return -ESRCH;
    }
    if target_sid != caller_sid {
        return -EPERM;
    }
    if target_pid_value == target_sid {
        return -EPERM;
    }
    if new_pgid != target_pid_value && !process_group_exists_in_session(target_sid, new_pgid) {
        return -EPERM;
    }

    unsafe {
        (*crate::task::leader_of(target)).pgid = new_pgid;
    }
    0
}

pub(super) fn sys_setsid(ctx: &SyscallContext, _call: &SyscallArgs) -> i64 {
    if !ctx.has_task() {
        return -ESRCH;
    }

    let pid = unsafe { (*ctx.current).tgid };
    let pgid = unsafe { (*crate::task::leader_of(ctx.current)).pgid };
    if pgid == pid || process_group_exists(pid) {
        return -EPERM;
    }

    unsafe {
        (*crate::task::leader_of(ctx.current)).sid = pid;
        (*crate::task::leader_of(ctx.current)).pgid = pid;
        (*crate::task::leader_of(ctx.current)).ctty_type = task::CTTY_NONE;
        (*crate::task::leader_of(ctx.current)).ctty_id = 0;
    }
    pid as i64
}

pub(super) fn sys_getsid(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    if !ctx.has_task() {
        return -ESRCH;
    }

    let caller_pid = unsafe { (*ctx.current).tgid };
    let target_pid = match pid_from_syscall_arg_or_current(call.arg(0), caller_pid) {
        Ok(pid) => pid,
        Err(err) => return err,
    };
    let target = lookup_user_task(target_pid);
    if target.is_null() {
        return -ESRCH;
    }
    unsafe { (*crate::task::leader_of(target)).sid as i64 }
}

pub(super) fn sys_getuid(ctx: &SyscallContext, _call: &SyscallArgs) -> i64 {
    if !ctx.has_task() {
        return -ESRCH;
    }
    unsafe { (*crate::task::leader_of(ctx.current)).user_uid as i64 }
}

pub(super) fn sys_geteuid(ctx: &SyscallContext, _call: &SyscallArgs) -> i64 {
    sys_getuid(ctx, _call)
}

pub(super) fn sys_getgid(ctx: &SyscallContext, _call: &SyscallArgs) -> i64 {
    if !ctx.has_task() {
        return -ESRCH;
    }
    unsafe { (*crate::task::leader_of(ctx.current)).user_gid as i64 }
}

pub(super) fn sys_getegid(ctx: &SyscallContext, _call: &SyscallArgs) -> i64 {
    sys_getgid(ctx, _call)
}

pub(super) fn sys_execve(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    match execve_common(ctx, call.arg(0), call.arg(1), call.arg(2)) {
        Ok(_) => 0,
        Err(err) => err,
    }
}

pub(super) fn sys_getppid(ctx: &SyscallContext, _call: &SyscallArgs) -> i64 {
    match ctx.ppid() {
        Ok(ppid) => ppid,
        Err(err) => err,
    }
}

pub(super) fn sys_gettid(ctx: &SyscallContext, _call: &SyscallArgs) -> i64 {
    match ctx.tid() {
        Ok(tid) => tid,
        Err(err) => err,
    }
}

pub(super) fn sys_arch_prctl(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let code = call.arg(0);
    let addr = call.arg(1);

    match code {
        ARCH_SET_FS => {
            if !arch_prctl_set_fs_addr_ok(addr) {
                return -EINVAL;
            }

            unsafe {
                (*ctx.current).user_fs_base = addr;
            }
            arch_syscall::set_user_fs_base(addr);
            0
        }
        ARCH_GET_FS => {
            if !user_range_ok(addr, size_of::<u64>()) {
                return -EFAULT;
            }

            let fs_base = unsafe { (*ctx.current).user_fs_base };
            match write_u64_to_user(ctx.current, addr, fs_base) {
                Ok(()) => 0,
                Err(err) => -err,
            }
        }
        ARCH_SET_GS | ARCH_GET_GS => -EINVAL,
        _ => -EINVAL,
    }
}

pub(super) fn sys_set_tid_address(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let clear_tid = call.arg(0);
    if clear_tid != 0 && !user_range_ok(clear_tid, CLEAR_TID_SIZE) {
        return -EFAULT;
    }

    unsafe {
        (*ctx.current).user_clear_tid = clear_tid;
    }

    sys_gettid(ctx, call)
}

pub(super) fn clear_child_tid(ctx: &SyscallContext) {
    let clear_tid = unsafe { (*ctx.current).user_clear_tid };
    if clear_tid == 0 {
        return;
    }

    let zero = [0u8; CLEAR_TID_SIZE];
    let _ = copy_to_user_task(ctx.current, clear_tid, &zero);
    let _ = ipc::futex_wake(clear_tid, 1);
    unsafe {
        (*ctx.current).user_clear_tid = 0;
    }
}

pub(super) fn teardown_user_address_space(task_ptr: *mut task::Task) {
    if task_ptr.is_null() {
        return;
    }
    let pml4 = unsafe { PhysAddr::new((*task_ptr).pml4_phys) };
    if pml4.as_u64() == 0 {
        return;
    }
    vmm::clear_user_space(pml4);
    vmm::destroy_address_space(pml4);
    unsafe {
        (*task_ptr).pml4_phys = 0;
    }
}

/// Free a process's shared state once its last thread has exited.
pub(super) fn release_process_resources(leader: *mut task::Task) {
    let tgid = unsafe { (*leader).tgid };
    mmap_clear_task_regions(leader);
    close_all_fds(leader);
    super::file::release_cwd(leader);
    epoll_drop_watches_for_owner(tgid);
    shm::cleanup_task(leader);
    // Dying non-leader threads have already dropped their reference to the
    // address space; the leader's copy is the canonical one.
    teardown_user_address_space(leader);
    for idx in 0..task::MAX_TASKS {
        let t = task::get_by_index(idx);
        if !t.is_null() && unsafe { (*t).tgid } == tgid {
            unsafe {
                (*t).pml4_phys = 0;
            }
        }
    }
}

/// Free the task structs of exited non-leader threads. Exited threads never
/// run again, so this is safe from any other task's context.
pub(super) fn reap_exited_threads() {
    let current = task::current();
    for idx in 0..task::MAX_TASKS {
        let t = task::get_by_index(idx);
        if t.is_null() || t == current {
            continue;
        }
        let reapable = unsafe {
            ((*t).flags & task::TASK_FLAG_THREAD) != 0 && (*t).state == TaskState::Zombie
        };
        if reapable {
            ipc::remove_task_waiters(t);
            task::task_destroy(t);
        }
    }
}

/// Make sibling thread `t` exit (for exit_group or execve). Returns true
/// while `t` is still executing on another CPU; the caller must retry after
/// letting that CPU reschedule.
///
/// - Parked in user mode: terminated immediately.
/// - Inside the kernel: flagged and woken, and it exits when its syscall
///   unwinds.
/// - Running on another CPU: that CPU is kicked.
pub(super) fn kill_sibling_thread(t: *mut task::Task, code: i32) -> bool {
    if t.is_null() || !task::is_live(t) {
        return false;
    }
    let is_leader = task::leader_of(t) == t;
    unsafe {
        (*t).flags |= task::TASK_FLAG_EXITING;
        (*t).exit_code = code;
        if !is_leader {
            // The process (or the exec'd image) may outlive this thread.
            (*t).user_clear_tid = 0;
        }
    }
    match crate::sched::detach_for_kill(t) {
        crate::sched::KillDetach::RunningElsewhere => return true,
        crate::sched::KillDetach::Detached => {
            ipc::remove_task_waiters(t);
        }
        crate::sched::KillDetach::InKernel => {
            let state = unsafe { (*t).state };
            if state == TaskState::Blocked || state == TaskState::Sleeping {
                crate::sched::unblock(t);
            }
        }
    }
    if !is_leader {
        unsafe {
            (*t).pml4_phys = 0;
        }
    }
    false
}

/// Make every other thread of `current`'s process exit.
///
/// A sibling executing on another CPU cannot be torn down in place. Its CPU
/// is kicked with a reschedule IPI, and this CPU briefly drops the big kernel
/// lock so the sibling can be preempted. The sibling is finished once it is
/// parked or blocked.
pub(super) fn kill_other_threads(current: *mut task::Task, code: i32) {
    let tgid = unsafe { (*current).tgid };
    loop {
        let mut running_elsewhere = false;
        for idx in 0..task::MAX_TASKS {
            let t = task::get_by_index(idx);
            if t.is_null() || t == current || unsafe { (*t).tgid } != tgid {
                continue;
            }
            running_elsewhere |= kill_sibling_thread(t, code);
        }
        if !running_elsewhere {
            return;
        }
        crate::bkl::relax();
    }
}

/// Stop running the current task after it became a zombie. The scheduler
/// never picks a zombie again, so this does not return on the real kernel.
pub(super) fn leave_exited_task(ctx: &SyscallContext) {
    if ctx.current == task::current() {
        crate::sched::yield_now();
    }
}

/// Exit the calling thread. When it is the last live thread of its process,
/// the process's resources are released and the leader becomes waitable.
pub(super) fn exit_current_thread(ctx: &SyscallContext, code: i32) -> i64 {
    if !ctx.has_task() {
        return -ESRCH;
    }
    let current = ctx.current;
    if !task::is_live(current) {
        return 0;
    }
    let leader = task::leader_of(current);
    let tgid = unsafe { (*current).tgid };

    clear_child_tid(ctx);
    ipc::remove_task_waiters(current);
    unsafe {
        (*current).exit_code = code;
        (*current).state = TaskState::Zombie;
    }

    if task::live_threads_in_group(tgid) == 0 {
        let status = unsafe {
            if ((*leader).flags & task::TASK_FLAG_GROUP_EXIT) != 0 {
                (*leader).exit_code
            } else {
                code
            }
        };
        unsafe {
            (*leader).exit_code = status;
        }
        release_process_resources(leader);
        crate::user::maybe_restore_console_after_init_exit(leader, status);
    }

    reap_exited_threads();
    crate::sched::request_preempt();
    leave_exited_task(ctx);
    0
}

/// Terminate the whole process (exit_group, fatal signals, fatal faults).
pub(super) fn sys_exit_common(ctx: &SyscallContext, code: i32) -> i64 {
    if !ctx.has_task() {
        return -ESRCH;
    }
    let leader = task::leader_of(ctx.current);
    unsafe {
        if ((*leader).flags & task::TASK_FLAG_GROUP_EXIT) == 0 {
            (*leader).flags |= task::TASK_FLAG_GROUP_EXIT;
            (*leader).exit_code = code;
        }
    }
    let status = unsafe { (*leader).exit_code };
    kill_other_threads(ctx.current, status);
    exit_current_thread(ctx, status)
}

pub(super) fn sys_exit(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    exit_current_thread(ctx, call.arg(0) as i32)
}

pub(super) fn sys_exit_group(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    sys_exit_common(ctx, call.arg(0) as i32)
}

// ---------------------------------------------------------------------------
// prlimit64 (302) — stub returning success
// ---------------------------------------------------------------------------
pub(super) fn sys_prlimit64(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let new_limit_ptr = call.arg(2);
    let old_limit_ptr = call.arg(3);

    // If old_limit_ptr is set, write a zeroed rlimit (cur=RLIM_INFINITY, max=RLIM_INFINITY).
    if old_limit_ptr != 0 {
        let rlimit = [u64::MAX, u64::MAX]; // [rlim_cur, rlim_max]
        if let Err(err) = write_struct_to_user(ctx.current, old_limit_ptr, &rlimit) {
            return -err;
        }
    }
    let _ = new_limit_ptr;
    0
}

// ---------------------------------------------------------------------------
// getrusage (98) — stub returning zeroed struct
// ---------------------------------------------------------------------------
pub(super) fn sys_getrusage(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let _who = call.arg(0) as i32;
    let usage_ptr = call.arg(1);

    if usage_ptr == 0 {
        return -EFAULT;
    }

    // struct rusage is 144 bytes on x86-64.
    let zeroed = [0u8; 144];
    match copy_to_user_task(ctx.current, usage_ptr, &zeroed) {
        Ok(()) => 0,
        Err(err) => -err,
    }
}

// ---------------------------------------------------------------------------
// sched_getaffinity (204)
// ---------------------------------------------------------------------------
pub(super) fn sys_sched_getaffinity(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let _pid = call.arg(0) as i32;
    let cpusetsize = call.arg(1) as usize;
    let mask_ptr = call.arg(2);

    if mask_ptr == 0 || cpusetsize == 0 {
        return -EINVAL;
    }

    // Single-CPU stub: set bit 0 in the affinity mask.
    let write_len = cmp::min(cpusetsize, 128);
    let mut mask = [0u8; 128];
    mask[0] = 1; // CPU 0 present

    match copy_to_user_task(ctx.current, mask_ptr, &mask[..write_len]) {
        Ok(()) => write_len as i64,
        Err(err) => -err,
    }
}

// ---------------------------------------------------------------------------
// getpgid (121)
// ---------------------------------------------------------------------------
pub(super) fn sys_getpgid(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let pid = call.arg(0) as i32;

    if pid < 0 {
        return -EINVAL;
    }

    if pid == 0 {
        // Return current task's pgid.
        let pgid = unsafe { (*crate::task::leader_of(ctx.current)).pgid };
        return pgid as i64;
    }

    // For pid > 0, look up that task's pgid.
    let target = task::get_by_pid(pid);
    if target.is_null() {
        return -ESRCH;
    }
    let pgid = unsafe { (*crate::task::leader_of(target)).pgid };
    pgid as i64
}
