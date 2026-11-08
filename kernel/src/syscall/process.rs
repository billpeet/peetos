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
    epoll_drop_watches_for_owner(owner_pid);
    teardown_user_address_space(task_ptr);
    task::task_destroy(task_ptr);
}

pub(super) fn clone_parent_fds_for_child(
    parent: *mut task::Task,
    child: *mut task::Task,
) -> Result<(), i64> {
    unsafe {
        (*child).user_fd_kind = (*parent).user_fd_kind;
        (*child).user_fd_inode = (*parent).user_fd_inode;
        (*child).user_fd_offset = (*parent).user_fd_offset;
        (*child).user_fd_flags = (*parent).user_fd_flags;
    }

    for idx in 0..task::MAX_TASK_FDS {
        let (kind, inode, flags) = unsafe {
            (
                (*child).user_fd_kind[idx],
                (*child).user_fd_inode[idx],
                (*child).user_fd_flags[idx],
            )
        };
        if kind == task::USER_FD_KIND_PIPE && inode != task::USER_FD_NONE {
            let retain_read = fd_can_read(flags);
            let retain_write = fd_can_write(flags);
            ipc::pipe_retain(inode, retain_read, retain_write).map_err(|err| -ipc_to_errno(err))?;
        }
        if kind == task::USER_FD_KIND_PTY_MASTER && inode != task::USER_FD_NONE {
            crate::pty::master_retain(inode);
        }
        if kind == task::USER_FD_KIND_PTY_SLAVE && inode != task::USER_FD_NONE {
            crate::pty::slave_retain(inode);
        }
        if kind == task::USER_FD_KIND_SOCKET && inode != task::USER_FD_NONE {
            socket::socket_retain(inode)?;
        }
        if kind == task::USER_FD_KIND_EPOLL && inode != task::USER_FD_NONE {
            epoll_retain(inode)?;
        }
    }

    Ok(())
}

pub(super) fn create_fork_child(
    ctx: &SyscallContext,
    child_frame: &InterruptFrame,
) -> Result<i64, i64> {
    let parent = ctx.current;
    if parent.is_null() {
        return Err(-ESRCH);
    }
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
    if vmm::clone_user_space(parent_pml4, child_pml4) != 0 {
        vmm::clear_user_space(child_pml4);
        vmm::destroy_address_space(child_pml4);
        task::task_destroy(child);
        return Err(-ENOMEM);
    }

    unsafe {
        (*child).pml4_phys = child_pml4.as_u64();

        (*child).user_entry = (*parent).user_entry;
        (*child).user_stack_bottom = (*parent).user_stack_bottom;
        (*child).user_stack_top = (*parent).user_stack_top;
        (*child).user_stack_ptr = (*parent).user_stack_ptr;
        (*child).user_argc = (*parent).user_argc;
        (*child).user_argv = (*parent).user_argv;
        (*child).user_envp = (*parent).user_envp;
        (*child).user_auxv = (*parent).user_auxv;
        (*child).user_fs_base = (*parent).user_fs_base;
        // The parent is the running task, so the live registers are its
        // user FPU/SSE state; the child resumes with an identical copy.
        (*child).fpu.save();
        (*child).user_clear_tid = 0;
        (*child).user_brk_start = (*parent).user_brk_start;
        (*child).user_brk = (*parent).user_brk;
        (*child).user_phdr_addr = (*parent).user_phdr_addr;
        (*child).user_phdr_count = (*parent).user_phdr_count;
        (*child).user_phdr_size = (*parent).user_phdr_size;
        (*child).user_base_addr = (*parent).user_base_addr;
        (*child).user_interp_base = (*parent).user_interp_base;
        (*child).user_is_dynamic = (*parent).user_is_dynamic;
        (*child).user_uid = (*parent).user_uid;
        (*child).user_gid = (*parent).user_gid;
        (*child).user_cwd_inode = (*parent).user_cwd_inode;
        (*child).pgid = (*parent).pgid;
        (*child).sid = (*parent).sid;
        (*child).ctty_type = (*parent).ctty_type;
        (*child).ctty_id = (*parent).ctty_id;
        (*child).signal_handlers = (*parent).signal_handlers;
        (*child).signal_flags = (*parent).signal_flags;
        (*child).signal_mask_saved = (*parent).signal_mask_saved;
        (*child).signal_restorer = (*parent).signal_restorer;
        (*child).signal_blocked = (*parent).signal_blocked;
        (*child).sig_pending = 0;
        (*child).user_shm_id = [0; task::MAX_TASK_SHM_ATTACH];
        (*child).user_shm_addr = [0; task::MAX_TASK_SHM_ATTACH];
        (*child).user_shm_size = [0; task::MAX_TASK_SHM_ATTACH];
        (*child).user_shm_flags = [0; task::MAX_TASK_SHM_ATTACH];
    }

    mmap_clone_task_regions(parent, child)?;
    mmap_fork_reconcile_shared_file_mappings(child)?;

    if let Err(err) = clone_parent_fds_for_child(parent, child) {
        fork_cleanup_child(child);
        return Err(err);
    }

    unsafe {
        let mut frame_addr = (*child).kernel_stack_top - size_of::<InterruptFrame>() as u64;
        frame_addr &= !0xF;
        let frame_ptr = frame_addr as *mut InterruptFrame;
        ptr::write(frame_ptr, *child_frame);
        (*child).saved_rsp = frame_addr;
        (*child).state = TaskState::Ready;
    }

    crate::sched::add_task(child);
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

pub(super) fn validate_clone_args(call: &SyscallArgs) -> Result<u64, i64> {
    let flags = call.arg(0);
    let child_stack = call.arg(1);
    let parent_tid = call.arg(2);
    let child_tid = call.arg(3);
    let tls = call.arg(4);

    if (flags & !(CLONE_CSIGNAL | CLONE_VM | CLONE_VFORK)) != 0 {
        return Err(-EINVAL);
    }
    if (flags & CLONE_CSIGNAL) != SIGCHLD_EXIT_SIGNAL {
        return Err(-EINVAL);
    }
    let clone_vm = (flags & CLONE_VM) != 0;
    let clone_vfork = (flags & CLONE_VFORK) != 0;
    if clone_vm != clone_vfork {
        return Err(-EINVAL);
    }
    if parent_tid != 0 || child_tid != 0 || tls != 0 {
        return Err(-EINVAL);
    }
    if child_stack != 0 && !user_range_ok(child_stack, 1) {
        return Err(-EFAULT);
    }
    Ok(child_stack)
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
        ipc::FUTEX_WAIT => {
            if call.arg(3) != 0 {
                return -EINVAL;
            }
            let current = match read_user_u32(ctx.current, uaddr) {
                Ok(v) => v,
                Err(err) => return -err,
            };
            if current != val {
                return -EAGAIN;
            }
            match ipc::futex_wait(ctx.current, uaddr) {
                Ok(()) => 0,
                Err(err) => -ipc_to_errno(err),
            }
        }
        ipc::FUTEX_WAKE => ipc::futex_wake(uaddr, val) as i64,
        _ => -EINVAL,
    }
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
        let (child_ppid, child_pid, child_state) =
            unsafe { ((*child).ppid, (*child).pid, (*child).state) };
        if child_ppid != parent_pid {
            continue;
        }
        if !child_pid_matches(wait_pid, child_pid) {
            continue;
        }
        *found_match = true;
        if child_state == TaskState::Zombie {
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

    let parent_pid = unsafe { (*ctx.current).pid };
    loop {
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

    let caller_pid = unsafe { (*ctx.current).pid };
    let caller_sid = unsafe { (*ctx.current).sid };

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
            (*target).ppid,
            (*target).sid,
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
        (*target).pgid = new_pgid;
    }
    0
}

pub(super) fn sys_setsid(ctx: &SyscallContext, _call: &SyscallArgs) -> i64 {
    if !ctx.has_task() {
        return -ESRCH;
    }

    let pid = unsafe { (*ctx.current).pid };
    let pgid = unsafe { (*ctx.current).pgid };
    if pgid == pid || process_group_exists(pid) {
        return -EPERM;
    }

    unsafe {
        (*ctx.current).sid = pid;
        (*ctx.current).pgid = pid;
        (*ctx.current).ctty_type = task::CTTY_NONE;
        (*ctx.current).ctty_id = 0;
    }
    pid as i64
}

pub(super) fn sys_getsid(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    if !ctx.has_task() {
        return -ESRCH;
    }

    let caller_pid = unsafe { (*ctx.current).pid };
    let target_pid = match pid_from_syscall_arg_or_current(call.arg(0), caller_pid) {
        Ok(pid) => pid,
        Err(err) => return err,
    };
    let target = lookup_user_task(target_pid);
    if target.is_null() {
        return -ESRCH;
    }
    unsafe { (*target).sid as i64 }
}

pub(super) fn sys_getuid(ctx: &SyscallContext, _call: &SyscallArgs) -> i64 {
    if !ctx.has_task() {
        return -ESRCH;
    }
    unsafe { (*ctx.current).user_uid as i64 }
}

pub(super) fn sys_geteuid(ctx: &SyscallContext, _call: &SyscallArgs) -> i64 {
    sys_getuid(ctx, _call)
}

pub(super) fn sys_getgid(ctx: &SyscallContext, _call: &SyscallArgs) -> i64 {
    if !ctx.has_task() {
        return -ESRCH;
    }
    unsafe { (*ctx.current).user_gid as i64 }
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
    match ctx.pid() {
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

pub(super) fn sys_exit_common(ctx: &SyscallContext, code: i32) -> i64 {
    if !ctx.has_task() {
        return -ESRCH;
    }

    let owner_pid = unsafe { (*ctx.current).pid };
    clear_child_tid(ctx);
    mmap_clear_task_regions(ctx.current);
    close_all_fds(ctx.current);
    epoll_drop_watches_for_owner(owner_pid);
    shm::cleanup_task(ctx.current);
    teardown_user_address_space(ctx.current);
    ipc::remove_task_waiters(ctx.current);

    unsafe {
        (*ctx.current).exit_code = code;
        (*ctx.current).state = TaskState::Zombie;
    }
    crate::user::maybe_restore_console_after_init_exit(ctx.current, code);
    crate::sched::request_preempt();
    0
}

pub(super) fn sys_exit(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    sys_exit_common(ctx, call.arg(0) as i32)
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
        let pgid = unsafe { (*ctx.current).pgid };
        return pgid as i64;
    }

    // For pid > 0, look up that task's pgid.
    let target = task::get_by_pid(pid);
    if target.is_null() {
        return -ESRCH;
    }
    let pgid = unsafe { (*target).pgid };
    pgid as i64
}
