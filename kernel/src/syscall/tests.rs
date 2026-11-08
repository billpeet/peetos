use super::*;
use std::sync::Mutex;

static EPOLL_TEST_LOCK: Mutex<()> = Mutex::new(());
static INPUT_TEST_LOCK: Mutex<()> = Mutex::new(());
static MMAP_TEST_LOCK: Mutex<()> = Mutex::new(());

fn reset_epoll_for_test() {
    unsafe {
        for idx in 0..EPOLL_MAX_INSTANCES {
            EPOLL_INSTANCES[idx] = EpollInstance::EMPTY;
        }
    }
}

fn reset_vm_for_test() {
    fs::init();
    crate::input::init();
    let _ = fs::mkdir("/temp");
    task::Task::reset_registry_for_test();
    init();
}

#[test]
fn user_range_validation() {
    assert!(user_range_ok(0x1000, 4));
    assert!(user_range_ok(USER_CANON_MAX, 1));
    assert!(!user_range_ok(USER_CANON_MAX + 1, 1));
    assert!(!user_range_ok(KERNEL_VIRT_BASE, 1));
    assert!(!user_range_ok(KERNEL_VIRT_BASE - 1, 2));
}

#[test]
fn page_align_up_rounds_correctly() {
    assert_eq!(page_align_up(0), Some(0));
    assert_eq!(page_align_up(1), Some(PAGE_SIZE));
    assert_eq!(page_align_up(PAGE_SIZE), Some(PAGE_SIZE));
}

#[test]
fn dev_fd_dir_entry_lists_open_descriptors_in_order() {
    let open = |fd: usize| matches!(fd, 0 | 1 | 2 | 7 | 12);
    let mut names = std::vec::Vec::new();
    let mut offset = DEV_FD_FIRST_OFFSET;
    while let Some(entry) = dev_fd_dir_entry(offset, open) {
        assert_eq!(entry.d_type, fs::DT_LNK);
        assert!(entry.next_offset > offset);
        names.push(std::string::String::from_utf8(entry.name[..entry.name_len].to_vec()).unwrap());
        offset = entry.next_offset;
    }
    assert_eq!(names, ["0", "1", "2", "7", "12"]);
    // "." and ".." offsets belong to the filesystem, not the synthesized list.
    assert!(dev_fd_dir_entry(0, open).is_none());
    assert!(dev_fd_dir_entry(1, open).is_none());
    // Resuming mid-listing continues after the last returned descriptor.
    let resumed = dev_fd_dir_entry(DEV_FD_FIRST_OFFSET + 8, open).unwrap();
    assert_eq!(&resumed.name[..resumed.name_len], b"12");
    assert!(dev_fd_dir_entry(DEV_FD_FIRST_OFFSET + 13, open).is_none());
}

#[test]
fn synthetic_fd_stat_identity_covers_non_file_kinds() {
    let sock = synthetic_fd_stat_identity(task::USER_FD_KIND_SOCKET).unwrap();
    assert_eq!(sock.1 & 0o170000, 0o140000);
    let pipe = synthetic_fd_stat_identity(task::USER_FD_KIND_PIPE).unwrap();
    assert_eq!(pipe.1 & 0o170000, 0o010000);
    let evfd = synthetic_fd_stat_identity(task::USER_FD_KIND_EVENTFD).unwrap();
    assert_eq!(evfd.1 & 0o170000, 0);
    let console = synthetic_fd_stat_identity(task::USER_FD_KIND_CONSOLE).unwrap();
    assert_eq!(console.1 & 0o170000, 0o020000);
    // Kinds sharing an id space must not collide on st_dev.
    assert_ne!(sock.0, pipe.0);
    assert_ne!(sock.0, evfd.0);
    // VFS files use the filesystem's own stat; empty slots have none.
    assert!(synthetic_fd_stat_identity(task::USER_FD_KIND_VFS).is_none());
    assert!(synthetic_fd_stat_identity(task::USER_FD_KIND_EMPTY).is_none());
}

#[test]
fn itimer_timeval_conversion_rounds_up_and_validates() {
    let tv = |s, us| LinuxTimeVal { tv_sec: s, tv_usec: us };
    assert_eq!(itimer_timeval_to_ticks(tv(0, 0)), Ok(0));
    // 100 Hz: anything up to 10 ms is one tick, never zero.
    assert_eq!(itimer_timeval_to_ticks(tv(0, 1)), Ok(1));
    assert_eq!(itimer_timeval_to_ticks(tv(0, 10_000)), Ok(1));
    assert_eq!(itimer_timeval_to_ticks(tv(0, 10_001)), Ok(2));
    assert_eq!(itimer_timeval_to_ticks(tv(2, 0)), Ok(2 * pit::TIMER_FREQUENCY as u64));
    assert_eq!(itimer_timeval_to_ticks(tv(-1, 0)), Err(-EINVAL));
    assert_eq!(itimer_timeval_to_ticks(tv(0, 1_000_000)), Err(-EINVAL));
}

#[test]
fn itimer_real_read_reports_remaining_and_interval() {
    let hz = pit::TIMER_FREQUENCY as u64;
    let disarmed = itimer_real_read(0, 0, 500);
    assert_eq!(disarmed, LinuxITimerVal::default());
    let armed = itimer_real_read(500 + 3 * hz / 2, hz, 500);
    assert_eq!(armed.it_value, LinuxTimeVal { tv_sec: 1, tv_usec: 500_000 });
    assert_eq!(armed.it_interval, LinuxTimeVal { tv_sec: 1, tv_usec: 0 });
    // Expired but not yet delivered still reads as armed.
    assert_ne!(itimer_real_read(400, 0, 500).it_value, LinuxTimeVal::default());
}

#[test]
fn alarm_seconds_left_rounds_like_linux() {
    let tv = |s, us| LinuxTimeVal { tv_sec: s, tv_usec: us };
    assert_eq!(alarm_seconds_left(tv(0, 0)), 0);
    assert_eq!(alarm_seconds_left(tv(0, 1)), 1);
    assert_eq!(alarm_seconds_left(tv(3, 499_999)), 3);
    assert_eq!(alarm_seconds_left(tv(3, 500_000)), 4);
}

#[test]
fn vm_mapped_inodes_collects_file_backed_regions_across_tasks() {
    let mut tables = [[VmMapping::EMPTY; MMAP_MAX_REGIONS_PER_TASK]; 2];
    tables[0][0] = VmMapping {
        start: MMAP_BASE_ADDR,
        len: PAGE_SIZE,
        file_offset: 0,
        file_inode: 7,
        prot: PROT_READ,
        map_shared: true,
        map_shared_can_write: false,
    };
    // Anonymous region: no inode.
    tables[0][1] = VmMapping {
        start: MMAP_BASE_ADDR + PAGE_SIZE,
        len: PAGE_SIZE,
        file_offset: 0,
        file_inode: task::USER_FD_NONE,
        prot: PROT_READ,
        map_shared: false,
        map_shared_can_write: false,
    };
    tables[1][3] = VmMapping { file_inode: 9, ..tables[0][0] };
    let mapped = vm_mapped_inodes(&tables);
    let set: std::vec::Vec<usize> = (0..fs::VFS_MAX_INODES).filter(|&i| mapped[i]).collect();
    assert_eq!(set, [7, 9]);
}

#[test]
fn vm_inode_pins_keep_unlinked_mapped_file_alive() {
    let _guard = MMAP_TEST_LOCK.lock().expect("mmap test lock");
    reset_vm_for_test();

    let opened = fs::open("/tmp/mapped-orphan", fs::O_CREAT | fs::O_RDWR, 0o600).expect("create");
    let inode = opened.inode;
    let slot = 5;
    unsafe {
        TASK_VM_MAPPINGS[slot][0] = VmMapping {
            start: MMAP_BASE_ADDR,
            len: PAGE_SIZE,
            file_offset: 0,
            file_inode: inode,
            prot: PROT_READ | PROT_WRITE,
            map_shared: true,
            map_shared_can_write: true,
        };
    }
    vm_sync_inode_pins();
    assert_eq!(fs::inode_open_refs(inode), 1);

    fs::unlink("/tmp/mapped-orphan").expect("unlink while mapped");
    assert!(fs::stat(inode).is_ok(), "mapping keeps the orphan alive");

    unsafe {
        TASK_VM_MAPPINGS[slot] = [VmMapping::EMPTY; MMAP_MAX_REGIONS_PER_TASK];
    }
    vm_sync_inode_pins();
    assert!(fs::stat(inode).is_err(), "last unmap frees it");
}

#[test]
fn syscall_lookup_finds_known_entry() {
    let def = lookup_syscall(SYS_GETPID).expect("syscall should exist");
    assert_eq!(def.name, "getpid");
}

#[test]
fn syscall_lookup_includes_vfs_extensions() {
    assert_eq!(lookup_syscall(SYS_PIPE).map(|d| d.name), Some("pipe"));
    assert_eq!(lookup_syscall(SYS_SELECT).map(|d| d.name), Some("select"));
    assert_eq!(lookup_syscall(SYS_STAT).map(|d| d.name), Some("stat"));
    assert_eq!(lookup_syscall(SYS_LSTAT).map(|d| d.name), Some("lstat"));
    assert_eq!(lookup_syscall(SYS_KILL).map(|d| d.name), Some("kill"));
    assert_eq!(lookup_syscall(SYS_UNAME).map(|d| d.name), Some("uname"));
    assert_eq!(lookup_syscall(SYS_MMAP).map(|d| d.name), Some("mmap"));
    assert_eq!(
        lookup_syscall(SYS_MPROTECT).map(|d| d.name),
        Some("mprotect")
    );
    assert_eq!(lookup_syscall(SYS_MUNMAP).map(|d| d.name), Some("munmap"));
    assert_eq!(lookup_syscall(SYS_MSYNC).map(|d| d.name), Some("msync"));
    assert_eq!(
        lookup_syscall(SYS_RT_SIGACTION).map(|d| d.name),
        Some("rt_sigaction")
    );
    assert_eq!(
        lookup_syscall(SYS_RT_SIGPROCMASK).map(|d| d.name),
        Some("rt_sigprocmask")
    );
    assert_eq!(
        lookup_syscall(SYS_RT_SIGRETURN).map(|d| d.name),
        Some("rt_sigreturn")
    );
    assert_eq!(lookup_syscall(SYS_WRITEV).map(|d| d.name), Some("writev"));
    assert_eq!(lookup_syscall(SYS_DUP).map(|d| d.name), Some("dup"));
    assert_eq!(lookup_syscall(SYS_DUP2).map(|d| d.name), Some("dup2"));
    assert_eq!(lookup_syscall(SYS_FCNTL).map(|d| d.name), Some("fcntl"));
    assert_eq!(lookup_syscall(SYS_FSYNC).map(|d| d.name), Some("fsync"));
    assert_eq!(lookup_syscall(SYS_SHMGET).map(|d| d.name), Some("shmget"));
    assert_eq!(lookup_syscall(SYS_SHMAT).map(|d| d.name), Some("shmat"));
    assert_eq!(lookup_syscall(SYS_SHMDT).map(|d| d.name), Some("shmdt"));
    assert_eq!(lookup_syscall(SYS_SHMCTL).map(|d| d.name), Some("shmctl"));
    assert_eq!(lookup_syscall(SYS_UMASK).map(|d| d.name), Some("umask"));
    assert_eq!(lookup_syscall(SYS_MKDIRAT).map(|d| d.name), Some("mkdirat"));
    assert_eq!(lookup_syscall(SYS_MKDIR).map(|d| d.name), Some("mkdir"));
    assert_eq!(lookup_syscall(SYS_SETITIMER).map(|d| d.name), Some("setitimer"));
    assert_eq!(lookup_syscall(SYS_GETITIMER).map(|d| d.name), Some("getitimer"));
    assert_eq!(lookup_syscall(SYS_ALARM).map(|d| d.name), Some("alarm"));
    assert_eq!(lookup_syscall(SYS_RMDIR).map(|d| d.name), Some("rmdir"));
    assert_eq!(lookup_syscall(SYS_FUTEX).map(|d| d.name), Some("futex"));
    assert_eq!(
        lookup_syscall(SYS_NEWFSTATAT).map(|d| d.name),
        Some("newfstatat")
    );
    assert_eq!(
        lookup_syscall(SYS_GETDENTS64).map(|d| d.name),
        Some("getdents64")
    );
    assert_eq!(
        lookup_syscall(SYS_UNLINKAT).map(|d| d.name),
        Some("unlinkat")
    );
    assert_eq!(
        lookup_syscall(SYS_RENAMEAT).map(|d| d.name),
        Some("renameat")
    );
    assert_eq!(lookup_syscall(SYS_DUP3).map(|d| d.name), Some("dup3"));
    assert_eq!(lookup_syscall(SYS_PIPE2).map(|d| d.name), Some("pipe2"));
    assert_eq!(lookup_syscall(SYS_EXECVE).map(|d| d.name), Some("execve"));
    assert_eq!(
        lookup_syscall(SYS_ARCH_PRCTL).map(|d| d.name),
        Some("arch_prctl")
    );
    assert_eq!(lookup_syscall(SYS_CLONE).map(|d| d.name), Some("clone"));
    assert_eq!(lookup_syscall(SYS_FORK).map(|d| d.name), Some("fork"));
    assert_eq!(lookup_syscall(SYS_VFORK).map(|d| d.name), Some("vfork"));
    assert_eq!(lookup_syscall(SYS_WAIT4).map(|d| d.name), Some("wait4"));
    assert_eq!(lookup_syscall(SYS_IOCTL).map(|d| d.name), Some("ioctl"));
    assert_eq!(lookup_syscall(SYS_ACCESS).map(|d| d.name), Some("access"));
    assert_eq!(
        lookup_syscall(SYS_FACCESSAT).map(|d| d.name),
        Some("faccessat")
    );
    assert_eq!(
        lookup_syscall(SYS_FACCESSAT2).map(|d| d.name),
        Some("faccessat2")
    );
    assert_eq!(lookup_syscall(SYS_GETCWD).map(|d| d.name), Some("getcwd"));
    assert_eq!(lookup_syscall(SYS_CHDIR).map(|d| d.name), Some("chdir"));
    assert_eq!(lookup_syscall(SYS_FCHDIR).map(|d| d.name), Some("fchdir"));
    assert_eq!(lookup_syscall(SYS_RENAME).map(|d| d.name), Some("rename"));
    assert_eq!(lookup_syscall(SYS_LINK).map(|d| d.name), Some("link"));
    assert_eq!(lookup_syscall(SYS_UNLINK).map(|d| d.name), Some("unlink"));
    assert_eq!(
        lookup_syscall(SYS_READLINK).map(|d| d.name),
        Some("readlink")
    );
    assert_eq!(lookup_syscall(SYS_LINKAT).map(|d| d.name), Some("linkat"));
    assert_eq!(
        lookup_syscall(SYS_SYMLINKAT).map(|d| d.name),
        Some("symlinkat")
    );
    assert_eq!(
        lookup_syscall(SYS_READLINKAT).map(|d| d.name),
        Some("readlinkat")
    );
    assert_eq!(
        lookup_syscall(SYS_UTIMENSAT).map(|d| d.name),
        Some("utimensat")
    );
    assert_eq!(
        lookup_syscall(SYS_GETTIMEOFDAY).map(|d| d.name),
        Some("gettimeofday")
    );
    assert_eq!(
        lookup_syscall(SYS_CLOCK_GETTIME).map(|d| d.name),
        Some("clock_gettime")
    );
    assert_eq!(
        lookup_syscall(SYS_NANOSLEEP).map(|d| d.name),
        Some("nanosleep")
    );
    assert_eq!(lookup_syscall(SYS_GETUID).map(|d| d.name), Some("getuid"));
    assert_eq!(lookup_syscall(SYS_GETEUID).map(|d| d.name), Some("geteuid"));
    assert_eq!(lookup_syscall(SYS_GETGID).map(|d| d.name), Some("getgid"));
    assert_eq!(lookup_syscall(SYS_GETEGID).map(|d| d.name), Some("getegid"));
    assert_eq!(lookup_syscall(SYS_SETPGID).map(|d| d.name), Some("setpgid"));
    assert_eq!(lookup_syscall(SYS_SETSID).map(|d| d.name), Some("setsid"));
    assert_eq!(lookup_syscall(SYS_GETSID).map(|d| d.name), Some("getsid"));
    assert_eq!(lookup_syscall(SYS_UTIMES).map(|d| d.name), Some("utimes"));
    assert_eq!(
        lookup_syscall(SYS_FUTIMESAT).map(|d| d.name),
        Some("futimesat")
    );
    assert_eq!(
        lookup_syscall(SYS_COPY_FILE_RANGE).map(|d| d.name),
        Some("copy_file_range")
    );
    assert_eq!(
        lookup_syscall(SYS_MEMFD_CREATE).map(|d| d.name),
        Some("memfd_create")
    );
    assert_eq!(
        lookup_syscall(SYS_EPOLL_CREATE1).map(|d| d.name),
        Some("epoll_create1")
    );
    assert_eq!(
        lookup_syscall(SYS_EPOLL_CTL).map(|d| d.name),
        Some("epoll_ctl")
    );
    assert_eq!(
        lookup_syscall(SYS_EPOLL_WAIT).map(|d| d.name),
        Some("epoll_wait")
    );
    assert_eq!(
        lookup_syscall(SYS_EPOLL_PWAIT).map(|d| d.name),
        Some("epoll_pwait")
    );
}

#[test]
fn mmap_prot_translation_sets_expected_bits() {
    let rw = mmap_prot_to_pte_flags(PROT_READ | PROT_WRITE).expect("rw flags");
    assert_ne!(rw & vmm::PTE_USER, 0);
    assert_ne!(rw & vmm::PTE_WRITE, 0);
    assert_ne!(rw & vmm::PTE_NO_EXECUTE, 0);

    let rx = mmap_prot_to_pte_flags(PROT_READ | PROT_EXEC).expect("rx flags");
    assert_ne!(rx & vmm::PTE_USER, 0);
    assert_eq!(rx & vmm::PTE_WRITE, 0);
    assert_eq!(rx & vmm::PTE_NO_EXECUTE, 0);

    let none = mmap_prot_to_pte_flags(PROT_NONE).expect("none flags");
    assert_eq!(none & vmm::PTE_USER, 0);
}

#[test]
fn mmap_flag_validation_rejects_unknown_bits() {
    assert!(mmap_flags_are_supported(MAP_PRIVATE | MAP_ANONYMOUS));
    assert!(!mmap_flags_are_supported(MAP_PRIVATE | 0x8000_0000));
}

#[test]
fn mmap_parse_sharing_mode_requires_exactly_one_mode() {
    assert_eq!(
        mmap_parse_sharing_mode(MAP_PRIVATE).expect("private mode"),
        (true, false)
    );
    assert_eq!(
        mmap_parse_sharing_mode(MAP_SHARED).expect("shared mode"),
        (false, true)
    );
    assert_eq!(mmap_parse_sharing_mode(0), Err(-EINVAL));
    assert_eq!(
        mmap_parse_sharing_mode(MAP_PRIVATE | MAP_SHARED),
        Err(-EINVAL)
    );
}

#[test]
fn mmap_regions_unmap_range_splits_middle_mapping() {
    let mut regions = [VmMapping::EMPTY; MMAP_MAX_REGIONS_PER_TASK];
    regions[0] = VmMapping {
        start: 0x1000,
        len: 0x4000,
        file_offset: 0,
        file_inode: task::USER_FD_NONE,
        prot: PROT_READ,
        map_shared: false,
        map_shared_can_write: false,
    };

    mmap_regions_unmap_range(&mut regions, 0x2000, 0x3000).expect("unmap split should work");

    let left = mmap_regions_find_mapping(&regions, 0x1000).expect("left segment");
    let right = mmap_regions_find_mapping(&regions, 0x3000).expect("right segment");
    assert_eq!(left.start, 0x1000);
    assert_eq!(left.len, 0x1000);
    assert_eq!(right.start, 0x3000);
    assert_eq!(right.len, 0x2000);
    assert!(mmap_regions_find_mapping(&regions, 0x2000).is_none());
}

#[test]
fn mmap_regions_unmap_range_adjusts_file_offsets() {
    let mut front_trim = [VmMapping::EMPTY; MMAP_MAX_REGIONS_PER_TASK];
    front_trim[0] = VmMapping {
        start: 0x1000,
        len: 0x4000,
        file_offset: 0x8000,
        file_inode: 12,
        prot: PROT_READ,
        map_shared: true,
        map_shared_can_write: true,
    };
    mmap_regions_unmap_range(&mut front_trim, 0x1000, 0x2000).expect("front trim");
    let trimmed = mmap_regions_find_mapping(&front_trim, 0x2000).expect("trimmed mapping");
    assert_eq!(trimmed.start, 0x2000);
    assert_eq!(trimmed.len, 0x3000);
    assert_eq!(trimmed.file_offset, 0x9000);

    let mut middle_split = [VmMapping::EMPTY; MMAP_MAX_REGIONS_PER_TASK];
    middle_split[0] = VmMapping {
        start: 0x1000,
        len: 0x4000,
        file_offset: 0xA000,
        file_inode: 15,
        prot: PROT_READ,
        map_shared: true,
        map_shared_can_write: true,
    };
    mmap_regions_unmap_range(&mut middle_split, 0x2000, 0x3000).expect("middle split");
    let left = mmap_regions_find_mapping(&middle_split, 0x1000).expect("left split");
    let right = mmap_regions_find_mapping(&middle_split, 0x3000).expect("right split");
    assert_eq!(left.file_offset, 0xA000);
    assert_eq!(right.file_offset, 0xC000);
}

#[test]
fn mmap_regions_unmap_range_rolls_back_when_split_slot_unavailable() {
    let mut regions = [VmMapping::EMPTY; MMAP_MAX_REGIONS_PER_TASK];
    regions[0] = VmMapping {
        start: 0x1000,
        len: 0x3000,
        file_offset: 0,
        file_inode: task::USER_FD_NONE,
        prot: PROT_READ,
        map_shared: false,
        map_shared_can_write: false,
    };
    for (idx, region) in regions.iter_mut().enumerate().skip(1) {
        *region = VmMapping {
            start: 0x5000 + ((idx as u64 - 1) * 0x2000),
            len: 0x1000,
            file_offset: 0,
            file_inode: task::USER_FD_NONE,
            prot: PROT_READ,
            map_shared: false,
            map_shared_can_write: false,
        };
    }
    let original = regions;

    assert_eq!(
        mmap_regions_unmap_range(&mut regions, 0x2000, 0x3000),
        Err(-ENOMEM)
    );
    assert_eq!(regions, original);
}

#[test]
fn mmap_regions_protect_range_splits_and_updates_target() {
    let mut regions = [VmMapping::EMPTY; MMAP_MAX_REGIONS_PER_TASK];
    regions[0] = VmMapping {
        start: 0x4000,
        len: 0x4000,
        file_offset: 0,
        file_inode: task::USER_FD_NONE,
        prot: PROT_READ,
        map_shared: false,
        map_shared_can_write: false,
    };

    mmap_regions_protect_range(&mut regions, 0x5000, 0x6000, PROT_READ | PROT_WRITE)
        .expect("mprotect split should work");

    let left = mmap_regions_find_mapping(&regions, 0x4000).expect("left segment");
    let middle = mmap_regions_find_mapping(&regions, 0x5000).expect("middle segment");
    let right = mmap_regions_find_mapping(&regions, 0x6000).expect("right segment");
    assert_eq!(left.prot, PROT_READ);
    assert_eq!(middle.prot, PROT_READ | PROT_WRITE);
    assert_eq!(right.prot, PROT_READ);
}

#[test]
fn mmap_regions_protect_range_rejects_shared_write_upgrade_without_mutation() {
    let mut regions = [VmMapping::EMPTY; MMAP_MAX_REGIONS_PER_TASK];
    regions[0] = VmMapping {
        start: 0x7000,
        len: 0x3000,
        file_offset: 0,
        file_inode: 5,
        prot: PROT_READ,
        map_shared: true,
        map_shared_can_write: false,
    };
    let original = regions;

    assert_eq!(
        mmap_regions_protect_range(&mut regions, 0x8000, 0x9000, PROT_READ | PROT_WRITE),
        Err(-EINVAL)
    );
    assert_eq!(regions, original);
}

#[test]
fn mmap_regions_protect_range_rolls_back_when_split_slot_unavailable() {
    let mut regions = [VmMapping::EMPTY; MMAP_MAX_REGIONS_PER_TASK];
    regions[0] = VmMapping {
        start: 0x1000,
        len: 0x3000,
        file_offset: 0,
        file_inode: task::USER_FD_NONE,
        prot: PROT_READ,
        map_shared: false,
        map_shared_can_write: false,
    };
    for (idx, region) in regions.iter_mut().enumerate().skip(1) {
        *region = VmMapping {
            start: 0x5000 + ((idx as u64 - 1) * 0x2000),
            len: 0x1000,
            file_offset: 0,
            file_inode: task::USER_FD_NONE,
            prot: PROT_READ,
            map_shared: false,
            map_shared_can_write: false,
        };
    }
    let original = regions;

    assert_eq!(
        mmap_regions_protect_range(&mut regions, 0x2000, 0x3000, PROT_READ | PROT_WRITE),
        Err(-ENOMEM)
    );
    assert_eq!(regions, original);
}

#[test]
fn mmap_region_allows_prot_accepts_shared_write_when_capable() {
    let region = VmMapping {
        start: 0x9000,
        len: 0x1000,
        file_offset: 0,
        file_inode: task::USER_FD_NONE,
        prot: PROT_READ,
        map_shared: true,
        map_shared_can_write: true,
    };
    assert!(mmap_region_allows_prot(&region, PROT_READ | PROT_WRITE));
}

#[test]
fn mmap_region_reclaimable_only_for_safe_file_backed_cases() {
    let shared_file = VmMapping {
        start: 0x1000,
        len: PAGE_SIZE,
        file_offset: 0,
        file_inode: 7,
        prot: PROT_READ | PROT_WRITE,
        map_shared: true,
        map_shared_can_write: true,
    };
    assert!(mmap_region_is_reclaimable(&shared_file));

    let private_ro_file = VmMapping {
        start: 0x2000,
        len: PAGE_SIZE,
        file_offset: 0,
        file_inode: 8,
        prot: PROT_READ,
        map_shared: false,
        map_shared_can_write: false,
    };
    assert!(mmap_region_is_reclaimable(&private_ro_file));
}

#[test]
fn mmap_region_reclaimable_rejects_unsafe_cases() {
    let anon_shared = VmMapping {
        start: 0x3000,
        len: PAGE_SIZE,
        file_offset: 0,
        file_inode: task::USER_FD_NONE,
        prot: PROT_READ | PROT_WRITE,
        map_shared: true,
        map_shared_can_write: true,
    };
    assert!(!mmap_region_is_reclaimable(&anon_shared));

    let private_writable_file = VmMapping {
        start: 0x4000,
        len: PAGE_SIZE,
        file_offset: 0,
        file_inode: 9,
        prot: PROT_READ | PROT_WRITE,
        map_shared: false,
        map_shared_can_write: false,
    };
    assert!(!mmap_region_is_reclaimable(&private_writable_file));
}

#[test]
fn mmap_region_reclaimable_rejects_framebuffer_device_mapping() {
    let _guard = MMAP_TEST_LOCK.lock().expect("mmap test lock");
    let fb = fs::open("/dev/fb0", fs::O_RDWR, 0).expect("open /dev/fb0");
    let region = VmMapping {
        start: 0x5000,
        len: PAGE_SIZE,
        file_offset: 0,
        file_inode: fb.inode,
        prot: PROT_READ | PROT_WRITE,
        map_shared: true,
        map_shared_can_write: true,
    };
    assert!(!mmap_region_is_reclaimable(&region));
}

#[test]
fn release_unmapped_user_page_treats_framebuffer_as_device_mapping() {
    let _guard = MMAP_TEST_LOCK.lock().expect("mmap test lock");
    let fb = fs::open("/dev/fb0", fs::O_RDWR, 0).expect("open /dev/fb0");
    let region = VmMapping {
        start: 0x6000,
        len: PAGE_SIZE,
        file_offset: 0,
        file_inode: fb.inode,
        prot: PROT_READ | PROT_WRITE,
        map_shared: true,
        map_shared_can_write: true,
    };
    assert_eq!(
        release_unmapped_user_page(Some(region), PhysAddr::new(0x2000)),
        UnmappedPageRelease::Device
    );
}

#[test]
fn msync_flag_validation_rejects_unknown_or_conflicting_bits() {
    assert!(msync_flags_are_valid(0));
    assert!(msync_flags_are_valid(MS_SYNC));
    assert!(msync_flags_are_valid(MS_ASYNC));
    assert!(msync_flags_are_valid(MS_SYNC | MS_INVALIDATE));
    assert!(!msync_flags_are_valid(MS_SYNC | MS_ASYNC));
    assert!(!msync_flags_are_valid(0x8000_0000));
}

#[test]
fn mmap_prot_fault_access_checks_expected_modes() {
    assert!(mmap_prot_allows_fault_access(PROT_READ, 0));
    assert!(mmap_prot_allows_fault_access(
        PROT_WRITE,
        PAGE_FAULT_ERROR_WRITE
    ));
    assert!(!mmap_prot_allows_fault_access(
        PROT_READ,
        PAGE_FAULT_ERROR_WRITE
    ));
    assert!(mmap_prot_allows_fault_access(
        PROT_EXEC,
        PAGE_FAULT_ERROR_INSTR
    ));
    assert!(!mmap_prot_allows_fault_access(
        PROT_READ,
        PAGE_FAULT_ERROR_INSTR
    ));
    assert!(!mmap_prot_allows_fault_access(PROT_NONE, 0));
}

#[test]
fn vm_page_cache_release_unknown_phys_returns_false() {
    assert!(!vm_page_cache_release_mapped_phys(PhysAddr::new(
        0x1234_5000
    )));
}

#[test]
fn arch_prctl_set_fs_rejects_kernel_addresses() {
    assert!(arch_prctl_set_fs_addr_ok(0));
    assert!(arch_prctl_set_fs_addr_ok(USER_CANON_MAX));
    assert!(!arch_prctl_set_fs_addr_ok(USER_CANON_MAX + 1));
    assert!(!arch_prctl_set_fs_addr_ok(KERNEL_VIRT_BASE));
}

#[test]
fn fd_cloexec_roundtrip_helpers() {
    let fd_flags = fd_flags_from_open_flags(fs::O_RDWR | fs::O_APPEND | O_CLOEXEC);
    assert!(fd_has_cloexec(fd_flags));
    assert_eq!(fd_open_flags(fd_flags), fs::O_RDWR | fs::O_APPEND);

    let cleared = fd_set_cloexec(fd_flags, false);
    assert!(!fd_has_cloexec(cleared));
    assert_eq!(fd_open_flags(cleared), fs::O_RDWR | fs::O_APPEND);
}

#[test]
fn fcntl_setfl_applies_nonblock_without_touching_cloexec_or_access_mode() {
    let initial = fd_flags_from_open_flags(fs::O_RDONLY | O_CLOEXEC);
    let updated = fd_apply_fcntl_setfl(initial, task::USER_FD_KIND_PIPE, O_NONBLOCK);
    assert!(fd_has_cloexec(updated));
    assert_eq!(fd_open_flags(updated) & fs::O_ACCMODE, fs::O_RDONLY);
    assert!((fd_open_flags(updated) & O_NONBLOCK) != 0);
}

#[test]
fn fcntl_setfl_ignores_unsettable_bits_for_descriptor_kind() {
    let initial = fd_flags_from_open_flags(fs::O_RDONLY);
    let updated = fd_apply_fcntl_setfl(
        initial,
        task::USER_FD_KIND_SOCKET,
        fs::O_APPEND | O_NONBLOCK,
    );
    assert_eq!(fd_open_flags(updated) & fs::O_ACCMODE, fs::O_RDONLY);
    assert!((fd_open_flags(updated) & O_NONBLOCK) != 0);
    assert_eq!(fd_open_flags(updated) & fs::O_APPEND, 0);
}

#[test]
fn fcntl_setfl_preserves_internal_memfd_tag() {
    let initial = fd_flags_from_open_flags(fs::O_RDWR) | FD_INTERNAL_MEMFD;
    let updated = fd_apply_fcntl_setfl(initial, task::USER_FD_KIND_VFS, fs::O_APPEND);
    assert!(fd_is_memfd(updated));
    assert_eq!(fd_open_flags(updated) & fs::O_APPEND, fs::O_APPEND);
}

#[test]
fn socket_raw_type_parsing_rejects_unknown_bits() {
    assert_eq!(
        socket_type_base_and_open_flags(SOCK_STREAM | SOCK_NONBLOCK),
        Ok((SOCK_STREAM, fs::O_RDWR | O_NONBLOCK))
    );
    assert_eq!(
        socket_type_base_and_open_flags(SOCK_DGRAM),
        Ok((SOCK_DGRAM, fs::O_RDWR))
    );
    assert_eq!(
        socket_type_base_and_open_flags(SOCK_STREAM | (1 << 30)),
        Err(-EINVAL)
    );
}

#[test]
fn accept4_flag_parsing_rejects_unsupported_bits() {
    assert_eq!(
        accept4_open_flags(SOCK_CLOEXEC | SOCK_NONBLOCK),
        Ok(fs::O_RDWR | O_CLOEXEC | O_NONBLOCK)
    );
    assert_eq!(accept4_open_flags(0x40), Err(-EINVAL));
}

#[test]
fn memfd_flag_validation_accepts_cloexec_and_sealing() {
    let flags = |cloexec, allow_sealing| MemfdFlags {
        cloexec,
        allow_sealing,
    };
    assert_eq!(memfd_validate_flags(0), Ok(flags(false, false)));
    assert_eq!(memfd_validate_flags(MFD_CLOEXEC), Ok(flags(true, false)));
    assert_eq!(memfd_validate_flags(MFD_ALLOW_SEALING), Ok(flags(false, true)));
    assert_eq!(
        memfd_validate_flags(MFD_CLOEXEC | MFD_ALLOW_SEALING),
        Ok(flags(true, true))
    );
    assert_eq!(memfd_validate_flags(0x1000), Err(-EINVAL));
}

#[test]
fn framebuffer_ioctl_validates_arg_pointer_and_availability() {
    let ctx = SyscallContext::new(core::ptr::null_mut());
    assert_eq!(ioctl_framebuffer(&ctx, FBIOGETINFO, 0), -EFAULT);
    assert_eq!(ioctl_framebuffer(&ctx, FBIOGETINFO, 0x1000), -ENXIO);
    assert_eq!(ioctl_framebuffer(&ctx, 0xDEAD, 0x1000), -ENOTTY);
}

#[test]
fn memfd_name_sanitize_rewrites_unsupported_bytes() {
    let mut out = [0u8; MEMFD_NAME_MAX];
    let len = memfd_name_sanitize(b"gui/buffer:\xff", &mut out);
    assert_eq!(&out[..len], b"gui_buffer__");

    let empty_len = memfd_name_sanitize(b"", &mut out);
    assert_eq!(&out[..empty_len], b"anon");
}

#[test]
fn memfd_build_path_includes_pid_sequence_and_name() {
    let mut buf = [0u8; fs::VFS_MAX_PATH];
    let path = memfd_build_path(42, 7, b"front/buf#1", &mut buf).expect("path");
    assert_eq!(path, "/temp/.memfd/mfd-42-7-front_buf_1");
}

#[test]
fn memfd_storage_dir_ready_creates_internal_dir_and_is_idempotent() {
    let _guard = MMAP_TEST_LOCK.lock().expect("mmap test lock");
    reset_vm_for_test();

    assert_eq!(fs::lookup("/temp/.memfd"), Err(fs::ENOENT));
    assert_eq!(memfd_storage_dir_ready(), Ok(()));

    let inode = fs::lookup("/temp/.memfd").expect("memfd storage dir");
    assert_eq!(fs::inode_is_dir(inode), Ok(true));
    assert_eq!(memfd_storage_dir_ready(), Ok(()));
}

#[test]
fn mmap_validate_shared_write_window_tracks_resize_and_bounds() {
    let _guard = MMAP_TEST_LOCK.lock().expect("mmap test lock");
    reset_vm_for_test();

    let opened = fs::open("/temp/shared-buffer", fs::O_CREAT | fs::O_RDWR, 0o600)
        .expect("shared buffer create");
    let inode = opened.inode;

    assert_eq!(
        mmap_validate_shared_write_window(inode, 0, PAGE_SIZE),
        Err(-EINVAL)
    );

    fs::truncate_to(inode, PAGE_SIZE * 2).expect("extend backing file");
    assert_eq!(
        mmap_validate_shared_write_window(inode, 0, PAGE_SIZE * 2),
        Ok(())
    );
    assert_eq!(
        mmap_validate_shared_write_window(inode, PAGE_SIZE, PAGE_SIZE),
        Ok(())
    );

    fs::truncate_to(inode, PAGE_SIZE).expect("shrink backing file");
    assert_eq!(
        mmap_validate_shared_write_window(inode, 0, PAGE_SIZE),
        Ok(())
    );
    assert_eq!(
        mmap_validate_shared_write_window(inode, 0, PAGE_SIZE * 2),
        Err(-EINVAL)
    );
    assert_eq!(
        mmap_validate_shared_write_window(inode, PAGE_SIZE, PAGE_SIZE),
        Err(-EINVAL)
    );
}

#[test]
fn memfd_object_accounting_deduplicates_by_inode() {
    let mut task = task::Task::for_test(63);
    let task_ptr = &mut task as *mut task::Task;
    let memfd_flags = fd_flags_from_open_flags(fs::O_RDWR) | FD_INTERNAL_MEMFD;

    fd_write_slot(task_ptr, 3, task::USER_FD_KIND_VFS, 40, 0, memfd_flags);
    fd_write_slot(task_ptr, 4, task::USER_FD_KIND_VFS, 40, 0, memfd_flags);
    fd_write_slot(task_ptr, 5, task::USER_FD_KIND_VFS, 41, 0, memfd_flags);
    fd_write_slot(
        task_ptr,
        6,
        task::USER_FD_KIND_VFS,
        80,
        0,
        fd_flags_from_open_flags(fs::O_RDWR),
    );

    assert_eq!(task_memfd_object_count(task_ptr), 2);
    assert!(task_memfd_contains_inode(task_ptr, 40));
    assert!(!task_memfd_contains_inode(task_ptr, 80));
}

#[test]
fn memfd_object_limit_blocks_new_inode_only() {
    let mut task = task::Task::for_test(64);
    let task_ptr = &mut task as *mut task::Task;
    let memfd_flags = fd_flags_from_open_flags(fs::O_RDWR) | FD_INTERNAL_MEMFD;
    let mut fd = 3usize;
    while fd < 3 + MEMFD_MAX_BUFFERS_PER_TASK {
        fd_write_slot(
            task_ptr,
            fd,
            task::USER_FD_KIND_VFS,
            (200 + fd) as u16,
            0,
            memfd_flags,
        );
        fd += 1;
    }

    assert_eq!(
        task_memfd_object_count(task_ptr),
        MEMFD_MAX_BUFFERS_PER_TASK
    );
    assert_eq!(enforce_memfd_create_limit(task_ptr), Err(-EMFILE));
    assert_eq!(enforce_memfd_object_limit_for_inode(task_ptr, 205), Ok(()));
    assert_eq!(
        enforce_memfd_object_limit_for_inode(task_ptr, 999),
        Err(-EMFILE)
    );
}

#[test]
fn mmap_clone_task_regions_copies_shared_writable_mapping_metadata_per_task() {
    let _guard = MMAP_TEST_LOCK.lock().expect("mmap test lock");
    reset_vm_for_test();

    let mut parent = task::Task::for_test(201);
    let mut child = task::Task::for_test(202);
    let parent_ptr = &mut parent as *mut task::Task;
    let child_ptr = &mut child as *mut task::Task;

    task::Task::install_for_test(0, parent_ptr);
    task::Task::install_for_test(1, child_ptr);

    let parent_slot = mmap_prepare_task_slot(parent_ptr).expect("parent slot");
    let mapping = VmMapping {
        start: 0x4000_0000,
        len: PAGE_SIZE * 2,
        file_offset: PAGE_SIZE,
        file_inode: 77,
        prot: PROT_READ | PROT_WRITE,
        map_shared: true,
        map_shared_can_write: true,
    };
    unsafe {
        TASK_VM_MAPPINGS[parent_slot][0] = mapping;
    }

    mmap_clone_task_regions(parent_ptr, child_ptr).expect("clone shared mappings");
    let child_slot = mmap_prepare_task_slot(child_ptr).expect("child slot");
    assert_eq!(unsafe { TASK_VM_MAPPINGS[child_slot][0] }, mapping);

    unsafe {
        TASK_VM_MAPPINGS[parent_slot][0].len = PAGE_SIZE;
    }
    assert_eq!(unsafe { TASK_VM_MAPPINGS[child_slot][0] }, mapping);
}

#[test]
fn mmap_clear_task_regions_only_clears_target_task_slot() {
    let _guard = MMAP_TEST_LOCK.lock().expect("mmap test lock");
    reset_vm_for_test();

    let mut parent = task::Task::for_test(211);
    let mut child = task::Task::for_test(212);
    let parent_ptr = &mut parent as *mut task::Task;
    let child_ptr = &mut child as *mut task::Task;

    task::Task::install_for_test(0, parent_ptr);
    task::Task::install_for_test(1, child_ptr);

    let parent_slot = mmap_prepare_task_slot(parent_ptr).expect("parent slot");
    let child_slot = mmap_prepare_task_slot(child_ptr).expect("child slot");
    let parent_mapping = VmMapping {
        start: 0x5000_0000,
        len: PAGE_SIZE,
        file_offset: 0,
        file_inode: 91,
        prot: PROT_READ | PROT_WRITE,
        map_shared: true,
        map_shared_can_write: true,
    };
    let child_mapping = VmMapping {
        start: 0x6000_0000,
        len: PAGE_SIZE,
        file_offset: PAGE_SIZE,
        file_inode: 91,
        prot: PROT_READ | PROT_WRITE,
        map_shared: true,
        map_shared_can_write: true,
    };
    unsafe {
        TASK_VM_MAPPINGS[parent_slot][0] = parent_mapping;
        TASK_VM_MAPPINGS[child_slot][0] = child_mapping;
    }

    mmap_clear_task_regions(parent_ptr);

    assert!(unsafe { TASK_VM_MAPPINGS[parent_slot][0].is_empty() });
    assert_eq!(unsafe { TASK_VM_MAPPINGS[child_slot][0] }, child_mapping);
}

#[test]
fn fd_dup_helpers_apply_requested_cloexec() {
    let mut task = task::Task::for_test(7);
    let task_ptr = &mut task as *mut task::Task;
    fd_write_slot(
        task_ptr,
        5,
        task::USER_FD_KIND_VFS,
        42,
        11,
        fd_set_cloexec(fs::O_RDONLY, true),
    );

    let plain = fd_dup_from(task_ptr, 5, 0, false).expect("dup without cloexec");
    let plain_idx = plain as usize;
    assert_eq!(plain_idx, 0);
    assert!(!fd_has_cloexec(unsafe {
        (*crate::task::leader_of(task_ptr)).user_fd_flags[plain_idx]
    }));
    assert_eq!(unsafe { (*crate::task::leader_of(task_ptr)).user_fd_offset[plain_idx] }, 11);

    let clo = fd_dup_from(task_ptr, 5, 1, true).expect("dup with cloexec");
    let clo_idx = clo as usize;
    assert_eq!(clo_idx, 1);
    assert!(fd_has_cloexec(unsafe {
        (*crate::task::leader_of(task_ptr)).user_fd_flags[clo_idx]
    }));
    assert_eq!(unsafe { (*crate::task::leader_of(task_ptr)).user_fd_inode[clo_idx] }, 42);
}

#[test]
fn epoll_create1_validates_flags_and_installs_fd() {
    let _guard = EPOLL_TEST_LOCK.lock().expect("epoll test lock");
    reset_epoll_for_test();

    let mut task = task::Task::for_test(31);
    let ctx = SyscallContext::new(&mut task as *mut task::Task);

    let bad = SyscallArgs {
        num: SYS_EPOLL_CREATE1,
        args: [0x1000, 0, 0, 0, 0, 0],
    };
    assert_eq!(sys_epoll_create1(&ctx, &bad), -EINVAL);

    let good = SyscallArgs {
        num: SYS_EPOLL_CREATE1,
        args: [EPOLL_CLOEXEC as u64, 0, 0, 0, 0, 0],
    };
    let epfd = sys_epoll_create1(&ctx, &good);
    assert!(epfd >= 0);
    let idx = epfd as usize;
    assert_eq!(task.user_fd_kind[idx], task::USER_FD_KIND_EPOLL);
    assert!(fd_has_cloexec(task.user_fd_flags[idx]));
    let _ = fd_close(ctx.current, epfd as i32);
}

#[test]
fn epoll_pwait_validates_its_sigmask() {
    let _guard = EPOLL_TEST_LOCK.lock().expect("epoll test lock");
    reset_epoll_for_test();

    let mut task = task::Task::for_test(32);
    let ctx = SyscallContext::new(&mut task as *mut task::Task);

    let null_sigmask = SyscallArgs {
        num: SYS_EPOLL_PWAIT,
        args: [0, 1, 0, 0, 0, 8],
    };
    assert_eq!(sys_epoll_pwait(&ctx, &null_sigmask), -EINVAL);

    // A sigmask is now honoured: a bad sigset size or an unreadable pointer
    // fails before the mask changes.
    let bad_size = SyscallArgs {
        num: SYS_EPOLL_PWAIT,
        args: [0, 1, 1, 0, 0x1000, 4],
    };
    assert_eq!(sys_epoll_pwait(&ctx, &bad_size), -EINVAL);
    let unreadable = SyscallArgs {
        num: SYS_EPOLL_PWAIT,
        args: [0, 1, 1, 0, 0x1000, 8],
    };
    // (A host test task has no address space, so the copy fails with EINVAL.)
    let r = sys_epoll_pwait(&ctx, &unreadable);
    assert!(r == -EFAULT || r == -EINVAL, "got {r}");
    assert!(!task.restore_sigmask);
}

#[test]
fn epoll_ctl_add_mod_del_enforces_duplicate_and_missing_semantics() {
    let _guard = EPOLL_TEST_LOCK.lock().expect("epoll test lock");
    reset_epoll_for_test();

    let mut owner = task::Task::for_test(41);
    let owner_ptr = &mut owner as *mut task::Task;
    let epoll_id = epoll_alloc_instance().expect("epoll alloc");
    let inst = epoll_instance_get_mut(epoll_id).expect("epoll instance");

    assert_eq!(
        epoll_ctl_add(
            inst,
            41,
            owner_ptr,
            5,
            task::USER_FD_KIND_VFS,
            9,
            EPOLLIN,
            0xAA
        ),
        0
    );
    assert_eq!(
        epoll_ctl_add(
            inst,
            41,
            owner_ptr,
            5,
            task::USER_FD_KIND_VFS,
            9,
            EPOLLIN,
            0xAA
        ),
        -EEXIST
    );
    assert_eq!(
        epoll_ctl_mod(inst, 41, 7, task::USER_FD_KIND_VFS, 9, EPOLLOUT, 0xBB),
        -ENOENT
    );
    assert_eq!(
        epoll_ctl_mod(inst, 41, 5, task::USER_FD_KIND_VFS, 9, EPOLLOUT, 0xCC),
        0
    );
    let slot = epoll_watch_find(inst, 41, 5).expect("watch slot");
    assert_eq!(inst.watches[slot].events, EPOLLOUT);
    assert_eq!(inst.watches[slot].data, 0xCC);
    assert_eq!(epoll_ctl_del(inst, 41, 6), -ENOENT);
    assert_eq!(epoll_ctl_del(inst, 41, 5), 0);
    assert_eq!(inst.watch_count, 0);
}

#[test]
fn epoll_owner_watch_limit_and_owner_cleanup_work_across_instances() {
    let _guard = EPOLL_TEST_LOCK.lock().expect("epoll test lock");
    reset_epoll_for_test();

    let mut owner = task::Task::for_test(44);
    let owner_ptr = &mut owner as *mut task::Task;
    let epoll_a = epoll_alloc_instance().expect("epoll a");
    let epoll_b = epoll_alloc_instance().expect("epoll b");
    let inst_a = epoll_instance_get_mut(epoll_a).expect("epoll instance a");

    for fd in 0..40 {
        assert_eq!(
            epoll_ctl_add(
                inst_a,
                44,
                owner_ptr,
                fd,
                task::USER_FD_KIND_VFS,
                (1000 + fd) as u16,
                EPOLLIN,
                fd as u64,
            ),
            0
        );
    }

    let inst_b = epoll_instance_get_mut(epoll_b).expect("epoll instance b");
    for fd in 40..EPOLL_MAX_WATCHES_PER_TASK as i32 {
        assert_eq!(
            epoll_ctl_add(
                inst_b,
                44,
                owner_ptr,
                fd,
                task::USER_FD_KIND_VFS,
                (1000 + fd) as u16,
                EPOLLIN,
                fd as u64,
            ),
            0
        );
    }
    assert_eq!(
        epoll_ctl_add(
            inst_b,
            44,
            owner_ptr,
            999,
            task::USER_FD_KIND_VFS,
            1999,
            EPOLLIN,
            0,
        ),
        -ENOSPC
    );
    assert_eq!(epoll_watch_count_for_owner(44), EPOLL_MAX_WATCHES_PER_TASK);

    epoll_drop_watches_for_owner(44);
    assert_eq!(epoll_watch_count_for_owner(44), 0);
    assert_eq!(inst_a.watch_count, 0);
    assert_eq!(inst_b.watch_count, 0);
}

#[test]
fn epoll_collect_ready_is_level_triggered_for_vfs_fds() {
    let _guard = EPOLL_TEST_LOCK.lock().expect("epoll test lock");
    reset_epoll_for_test();

    let mut owner = task::Task::for_test(51);
    let owner_ptr = &mut owner as *mut task::Task;
    fd_write_slot(owner_ptr, 3, task::USER_FD_KIND_VFS, 12, 0, fs::O_RDONLY);

    let epoll_id = epoll_alloc_instance().expect("epoll alloc");
    let inst = epoll_instance_get_mut(epoll_id).expect("epoll instance");
    assert_eq!(
        epoll_ctl_add(
            inst,
            51,
            owner_ptr,
            3,
            task::USER_FD_KIND_VFS,
            12,
            EPOLLIN,
            0x1234
        ),
        0
    );

    let mut out = [(0u32, 0u64); 2];
    let ready1 = epoll_collect_ready(inst, &mut out);
    assert_eq!(ready1, 1);
    assert_eq!(out[0], (EPOLLIN, 0x1234));

    out[0] = (0, 0);
    let ready2 = epoll_collect_ready(inst, &mut out);
    assert_eq!(ready2, 1);
    assert_eq!(out[0], (EPOLLIN, 0x1234));
}

#[test]
fn epoll_collect_ready_drops_stale_watch_after_fd_close() {
    let _guard = EPOLL_TEST_LOCK.lock().expect("epoll test lock");
    reset_epoll_for_test();

    let mut owner = task::Task::for_test(61);
    let owner_ptr = &mut owner as *mut task::Task;
    fd_write_slot(owner_ptr, 4, task::USER_FD_KIND_VFS, 77, 0, fs::O_RDONLY);

    let epoll_id = epoll_alloc_instance().expect("epoll alloc");
    let inst = epoll_instance_get_mut(epoll_id).expect("epoll instance");
    assert_eq!(
        epoll_ctl_add(
            inst,
            61,
            owner_ptr,
            4,
            task::USER_FD_KIND_VFS,
            77,
            EPOLLIN,
            0x55
        ),
        0
    );

    let _ = fd_close(owner_ptr, 4);
    let mut out = [(0u32, 0u64); 1];
    let ready = epoll_collect_ready(inst, &mut out);
    assert_eq!(ready, 0);
    assert_eq!(inst.watch_count, 0);
    assert!(epoll_watch_find(inst, 61, 4).is_none());
}

#[test]
fn epoll_revents_always_include_hup_and_err_bits() {
    let events = poll_revents_to_epoll_events(0, POLLHUP | POLLERR);
    assert_eq!(events & EPOLLHUP, EPOLLHUP);
    assert_eq!(events & EPOLLERR, EPOLLERR);
}

#[test]
fn mmap_regions_total_pages_sums_non_empty_regions() {
    let mut regions = [VmMapping::EMPTY; MMAP_MAX_REGIONS_PER_TASK];
    regions[0] = VmMapping {
        start: 0x1000,
        len: PAGE_SIZE,
        file_offset: 0,
        file_inode: task::USER_FD_NONE,
        prot: PROT_READ,
        map_shared: false,
        map_shared_can_write: false,
    };
    regions[1] = VmMapping {
        start: 0x3000,
        len: PAGE_SIZE * 2,
        file_offset: 0,
        file_inode: task::USER_FD_NONE,
        prot: PROT_READ,
        map_shared: false,
        map_shared_can_write: false,
    };
    assert_eq!(mmap_regions_total_pages(&regions).expect("page count"), 3);
    assert!(mmap_regions_total_pages(&regions).expect("count") < MMAP_MAX_PAGES_PER_TASK);
}

#[test]
fn epoll_pipe_read_hup_matches_poll_core_revents() {
    let _guard = EPOLL_TEST_LOCK.lock().expect("epoll test lock");
    reset_epoll_for_test();
    ipc::init();

    let mut owner = task::Task::for_test(71);
    let owner_ptr = &mut owner as *mut task::Task;
    let pipe = ipc::pipe_open().expect("pipe open");

    fd_write_slot(owner_ptr, 3, task::USER_FD_KIND_PIPE, pipe, 0, fs::O_RDONLY);
    fd_write_slot(owner_ptr, 4, task::USER_FD_KIND_PIPE, pipe, 0, fs::O_WRONLY);

    ipc::pipe_close(pipe, false, true).expect("close writer end");

    let requested = EPOLLIN;
    let poll_requested = epoll_events_to_poll_mask(requested);
    let revents = poll_fd_revents(owner_ptr, 3, poll_requested);
    let expected_events = poll_revents_to_epoll_events(requested, revents);
    assert_eq!(expected_events & EPOLLHUP, EPOLLHUP);

    let epoll_id = epoll_alloc_instance().expect("epoll alloc");
    let inst = epoll_instance_get_mut(epoll_id).expect("epoll instance");
    assert_eq!(
        epoll_ctl_add(
            inst,
            71,
            owner_ptr,
            3,
            task::USER_FD_KIND_PIPE,
            pipe,
            requested,
            0xA1
        ),
        0
    );

    let mut out = [(0u32, 0u64); 1];
    let ready = epoll_collect_ready(inst, &mut out);
    assert_eq!(ready, 1);
    assert_eq!(out[0], (expected_events, 0xA1));
}

#[test]
fn epoll_pipe_write_err_matches_poll_core_revents() {
    let _guard = EPOLL_TEST_LOCK.lock().expect("epoll test lock");
    reset_epoll_for_test();
    ipc::init();

    let mut owner = task::Task::for_test(72);
    let owner_ptr = &mut owner as *mut task::Task;
    let pipe = ipc::pipe_open().expect("pipe open");

    fd_write_slot(owner_ptr, 3, task::USER_FD_KIND_PIPE, pipe, 0, fs::O_RDONLY);
    fd_write_slot(owner_ptr, 4, task::USER_FD_KIND_PIPE, pipe, 0, fs::O_WRONLY);

    ipc::pipe_close(pipe, true, false).expect("close reader end");

    let requested = EPOLLOUT;
    let poll_requested = epoll_events_to_poll_mask(requested);
    let revents = poll_fd_revents(owner_ptr, 4, poll_requested);
    let expected_events = poll_revents_to_epoll_events(requested, revents);
    assert_eq!(expected_events & EPOLLERR, EPOLLERR);

    let epoll_id = epoll_alloc_instance().expect("epoll alloc");
    let inst = epoll_instance_get_mut(epoll_id).expect("epoll instance");
    assert_eq!(
        epoll_ctl_add(
            inst,
            72,
            owner_ptr,
            4,
            task::USER_FD_KIND_PIPE,
            pipe,
            requested,
            0xB2
        ),
        0
    );

    let mut out = [(0u32, 0u64); 1];
    let ready = epoll_collect_ready(inst, &mut out);
    assert_eq!(ready, 1);
    assert_eq!(out[0], (expected_events, 0xB2));
}

#[test]
fn poll_reports_input_devices_only_when_events_are_queued() {
    let _guard = INPUT_TEST_LOCK.lock().expect("input test lock");
    crate::input::init();
    fs::init();

    let mut owner = task::Task::for_test(73);
    let owner_ptr = &mut owner as *mut task::Task;
    let opened = fs::open("/dev/input/event0", fs::O_RDONLY, 0).expect("open event0");
    fd_write_slot(
        owner_ptr,
        3,
        task::USER_FD_KIND_VFS,
        opened.inode,
        0,
        fs::O_RDONLY,
    );

    assert_eq!(poll_fd_revents(owner_ptr, 3, POLLIN | POLLOUT), 0);

    crate::input::push_keyboard_key(crate::input::KEY_A, true);
    assert_eq!(poll_fd_revents(owner_ptr, 3, POLLIN | POLLOUT), POLLIN);
}

#[test]
fn cmsg_align_rounds_up_to_word_boundary() {
    assert_eq!(cmsg_align(1), size_of::<usize>());
    assert_eq!(cmsg_align(size_of::<usize>()), size_of::<usize>());
    assert_eq!(cmsg_align(size_of::<usize>() + 1), size_of::<usize>() * 2);
}

#[test]
fn unix_fd_transfer_rejects_non_transferable_kind() {
    let mut task = task::Task::for_test(21);
    let task_ptr = &mut task as *mut task::Task;
    fd_write_slot(
        task_ptr,
        6,
        task::USER_FD_KIND_CONSOLE,
        task::USER_FD_NONE,
        0,
        fs::O_RDWR,
    );
    assert_eq!(unix_fd_transfer_from_task(task_ptr, 6), Err(-EOPNOTSUPP));
}

#[test]
fn install_received_right_sets_cloexec_when_requested() {
    let mut task = task::Task::for_test(22);
    let task_ptr = &mut task as *mut task::Task;

    let right = socket::UnixRightsFd {
        kind: task::USER_FD_KIND_VFS,
        inode: 77,
        flags: fs::O_RDWR,
        offset: 123,
    };
    let fd = install_received_right(task_ptr, right, true).expect("install right");
    let idx = fd as usize;
    assert!(fd_has_cloexec(unsafe { (*crate::task::leader_of(task_ptr)).user_fd_flags[idx] }));
    assert_eq!(unsafe { (*crate::task::leader_of(task_ptr)).user_fd_inode[idx] }, 77);
    assert_eq!(unsafe { (*crate::task::leader_of(task_ptr)).user_fd_offset[idx] }, 123);
}

#[test]
fn close_cloexec_fds_keeps_non_cloexec_entries() {
    let mut task = task::Task::for_test(8);
    let task_ptr = &mut task as *mut task::Task;
    fd_write_slot(
        task_ptr,
        3,
        task::USER_FD_KIND_VFS,
        10,
        0,
        fd_set_cloexec(fs::O_RDONLY, true),
    );
    fd_write_slot(task_ptr, 4, task::USER_FD_KIND_VFS, 11, 3, fs::O_RDWR);

    close_cloexec_fds(task_ptr);

    assert!(!fd_slot_is_open(task_ptr, 3));
    assert!(fd_slot_is_open(task_ptr, 4));
    assert_eq!(unsafe { (*crate::task::leader_of(task_ptr)).user_fd_inode[4] }, 11);
}

#[test]
fn unknown_syscall_returns_enosys() {
    let ret = dispatch_syscall(SyscallArgs {
        num: 0xFFFF,
        args: [0; 6],
    });
    assert_eq!(ret, -ENOSYS);
}

#[test]
fn user_only_syscall_requires_user_task() {
    let ret = dispatch_syscall(SyscallArgs {
        num: SYS_BRK,
        args: [0; 6],
    });
    assert_eq!(ret, -EPERM);
}

#[test]
fn set_tid_requires_user_task() {
    let ret = dispatch_syscall(SyscallArgs {
        num: SYS_SET_TID_ADDRESS,
        args: [0; 6],
    });
    assert_eq!(ret, -EPERM);
}

#[test]
fn getpid_without_task_returns_esrch() {
    let ret = dispatch_syscall(SyscallArgs {
        num: SYS_GETPID,
        args: [0; 6],
    });
    assert_eq!(ret, -ESRCH);
}

#[test]
fn access_mode_validation_rejects_unknown_bits() {
    assert!(access_mode_is_valid(F_OK));
    assert!(access_mode_is_valid(R_OK | W_OK | X_OK));
    assert!(!access_mode_is_valid(0x80));
}

#[test]
fn access_allowed_uses_owner_bits_for_current_uid() {
    let stat = fs::FileStat {
        ino: 1,
        mode: STAT_S_IRUSR | STAT_S_IWUSR,
        uid: 1000,
        gid: 1000,
        nlink: 1,
        size: 0,
        blksize: 0,
        blocks: 0,
        atime: 0,
        mtime: 0,
        ctime: 0,
        file_type: fs::FileType::Regular,
    };
    assert!(access_allowed(&stat, R_OK, 1000, 1000));
    assert!(!access_allowed(&stat, X_OK, 1000, 1000));
}

#[test]
fn ticks_to_seconds_nanos_matches_pit_frequency() {
    assert_eq!(ticks_to_seconds_nanos(100), (1, 0));
    assert_eq!(ticks_to_seconds_nanos(123), (1, 230_000_000));
}

#[test]
fn timespec_to_sleep_ticks_rounds_up_partial_tick() {
    let one_ns = LinuxTimeSpec {
        tv_sec: 0,
        tv_nsec: 1,
    };
    assert_eq!(timespec_to_sleep_ticks(&one_ns), Ok(1));

    let ten_ms = LinuxTimeSpec {
        tv_sec: 0,
        tv_nsec: 10_000_000,
    };
    assert_eq!(timespec_to_sleep_ticks(&ten_ms), Ok(1));

    let invalid = LinuxTimeSpec {
        tv_sec: 0,
        tv_nsec: 1_000_000_000,
    };
    assert_eq!(timespec_to_sleep_ticks(&invalid), Err(-EINVAL));
}

#[test]
fn utimens_timespec_special_values_are_handled() {
    assert_eq!(
        timespec_to_unix_seconds(
            LinuxTimeSpec {
                tv_sec: -1,
                tv_nsec: UTIME_NOW,
            },
            77,
            11
        ),
        Ok(77)
    );
    assert_eq!(
        timespec_to_unix_seconds(
            LinuxTimeSpec {
                tv_sec: -1,
                tv_nsec: UTIME_OMIT,
            },
            77,
            11
        ),
        Ok(11)
    );
}

#[test]
fn utimens_timespec_rejects_invalid_nsec_or_negative_sec() {
    assert_eq!(
        timespec_to_unix_seconds(
            LinuxTimeSpec {
                tv_sec: -1,
                tv_nsec: 0,
            },
            1,
            1
        ),
        Err(-EINVAL)
    );
    assert_eq!(
        timespec_to_unix_seconds(
            LinuxTimeSpec {
                tv_sec: 0,
                tv_nsec: 1_000_000_000,
            },
            1,
            1
        ),
        Err(-EINVAL)
    );
}

#[test]
fn wait_status_encodes_exit_code_for_waitpid_macros() {
    assert_eq!(wait_status_from_exit_code(0), 0);
    assert_eq!(wait_status_from_exit_code(1), 0x0100);
    assert_eq!(wait_status_from_exit_code(255), 0xFF00_u32 as i32);
    // exit(134) and death by SIGABRT (6) must decode differently.
    assert_eq!(wait_status_from_exit_code(134), 134 << 8);
    let killed = wait_status_from_exit_code(exit_code_for_signal(signal::SIGABRT));
    assert_eq!(killed & 0x7f, signal::SIGABRT as i32); // WIFSIGNALED, WTERMSIG
    assert_eq!(killed & 0xff00, 0);
    let segv = wait_status_from_exit_code(exit_code_for_signal(signal::SIGSEGV));
    assert_eq!(segv, signal::SIGSEGV as i32);
}

#[test]
fn pid_arg_zero_maps_to_current_pid() {
    assert_eq!(pid_from_syscall_arg_or_current(0, 42), Ok(42));
    assert_eq!(pid_from_syscall_arg_or_current(7, 42), Ok(7));
    assert_eq!(
        pid_from_syscall_arg_or_current((-1_i64) as u64, 42),
        Err(-EINVAL)
    );
}

#[test]
fn pgid_arg_zero_maps_to_target_pid() {
    assert_eq!(pgid_from_syscall_arg_or_target(0, 55), Ok(55));
    assert_eq!(pgid_from_syscall_arg_or_target(9, 55), Ok(9));
    assert_eq!(
        pgid_from_syscall_arg_or_target((-1_i64) as u64, 55),
        Err(-EINVAL)
    );
}

#[test]
fn setsid_rejects_process_group_leader() {
    let mut task = task::Task::for_test(99);
    let ctx = SyscallContext::new(&mut task as *mut task::Task);
    let call = SyscallArgs {
        num: 0,
        args: [0; 6],
    };
    assert_eq!(sys_setsid(&ctx, &call), -EPERM);
}

#[test]
fn setsid_promotes_non_leader_to_session_leader() {
    let mut task = task::Task::for_test(100);
    task.pgid = 77;
    task.sid = 77;
    let ctx = SyscallContext::new(&mut task as *mut task::Task);
    let call = SyscallArgs {
        num: 0,
        args: [0; 6],
    };

    assert_eq!(sys_setsid(&ctx, &call), 100);
    assert_eq!(task.pgid, 100);
    assert_eq!(task.sid, 100);
}

#[test]
fn getsid_rejects_negative_pid_argument() {
    let mut task = task::Task::for_test(10);
    let ctx = SyscallContext::new(&mut task as *mut task::Task);
    let call = SyscallArgs {
        num: 0,
        args: [(-1_i64) as u64, 0, 0, 0, 0, 0],
    };
    assert_eq!(sys_getsid(&ctx, &call), -EINVAL);
}

#[test]
fn clone_args_accept_fork_compatible_flags() {
    let call = SyscallArgs {
        num: SYS_CLONE,
        args: [SIGCHLD_EXIT_SIGNAL, 0, 0, 0, 0, 0],
    };
    assert_eq!(validate_clone_args(&call), Ok(CloneRequest::FORK));
}

#[test]
fn clone_args_accept_vfork_compat_flags() {
    let call = SyscallArgs {
        num: SYS_CLONE,
        args: [CLONE_VM | CLONE_VFORK | SIGCHLD_EXIT_SIGNAL, 0, 0, 0, 0, 0],
    };
    let req = validate_clone_args(&call).expect("vfork flags");
    assert!(!req.is_thread());
}

#[test]
fn clone_args_reject_partial_vm_vfork_combo() {
    let call = SyscallArgs {
        num: SYS_CLONE,
        args: [CLONE_VM | SIGCHLD_EXIT_SIGNAL, 0, 0, 0, 0, 0],
    };
    assert_eq!(validate_clone_args(&call), Err(-EINVAL));
}

/// Flags musl's pthread_create passes to clone.
const MUSL_PTHREAD_FLAGS: u64 = CLONE_VM
    | CLONE_FS
    | CLONE_FILES
    | CLONE_SIGHAND
    | CLONE_THREAD
    | CLONE_SYSVSEM
    | CLONE_SETTLS
    | CLONE_PARENT_SETTID
    | CLONE_CHILD_CLEARTID
    | CLONE_DETACHED;

#[test]
fn clone_args_ignore_tid_pointers_without_flags() {
    let call = SyscallArgs {
        num: SYS_CLONE,
        args: [SIGCHLD_EXIT_SIGNAL, 0, 1, 0, 0, 0],
    };
    let req = validate_clone_args(&call).expect("pointer without flag is ignored");
    assert_eq!(req.parent_tid, 1);
    assert!(!req.is_thread());
}

#[test]
fn clone_args_accept_musl_pthread_flags() {
    let call = SyscallArgs {
        num: SYS_CLONE,
        args: [MUSL_PTHREAD_FLAGS, 0x7000_0000, 0x6000_0010, 0x6000_0020, 0x6000_1000, 0],
    };
    let req = validate_clone_args(&call).expect("musl pthread flags");
    assert!(req.is_thread());
    assert_eq!(req.child_stack, 0x7000_0000);
    assert_eq!(req.parent_tid, 0x6000_0010);
    assert_eq!(req.child_tid, 0x6000_0020);
    assert_eq!(req.tls, 0x6000_1000);
}

#[test]
fn clone_args_reject_thread_without_full_sharing_or_stack() {
    let no_files = SyscallArgs {
        num: SYS_CLONE,
        args: [MUSL_PTHREAD_FLAGS & !CLONE_FILES, 0x7000_0000, 0, 0, 0, 0],
    };
    assert_eq!(validate_clone_args(&no_files), Err(-EINVAL));

    let no_stack = SyscallArgs {
        num: SYS_CLONE,
        args: [MUSL_PTHREAD_FLAGS, 0, 0x6000_0010, 0x6000_0020, 0x6000_1000, 0],
    };
    assert_eq!(validate_clone_args(&no_stack), Err(-EINVAL));

    let with_signal = SyscallArgs {
        num: SYS_CLONE,
        args: [
            MUSL_PTHREAD_FLAGS | SIGCHLD_EXIT_SIGNAL,
            0x7000_0000,
            0x6000_0010,
            0x6000_0020,
            0x6000_1000,
            0,
        ],
    };
    assert_eq!(validate_clone_args(&with_signal), Err(-EINVAL));
}

#[test]
fn clone_args_reject_partial_sharing_for_processes() {
    let call = SyscallArgs {
        num: SYS_CLONE,
        args: [CLONE_FILES | SIGCHLD_EXIT_SIGNAL, 0, 0, 0, 0, 0],
    };
    assert_eq!(validate_clone_args(&call), Err(-EINVAL));
}

#[test]
fn clone_args_reject_kernel_tid_pointers() {
    let call = SyscallArgs {
        num: SYS_CLONE,
        args: [
            MUSL_PTHREAD_FLAGS,
            0x7000_0000,
            KERNEL_VIRT_BASE,
            0x6000_0020,
            0x6000_1000,
            0,
        ],
    };
    assert_eq!(validate_clone_args(&call), Err(-EFAULT));
}

fn sample_syscall_frame() -> SyscallFrame {
    SyscallFrame {
        r15: 15,
        r14: 14,
        r13: 13,
        r12: 12,
        rbp: 0xb0,
        rbx: 0xb1,
        r9: 9,
        r8: 8,
        r10: 10,
        rdx: 0xd0,
        rsi: 0x51,
        rdi: 0xd1,
        rax: 0,
        r11: 0x246,
        rcx: 0x40_1002,
        rsp: 0x7fff_0000,
    }
}

#[test]
fn syscall_frame_gregs_roundtrip_through_ucontext() {
    let frame = sample_syscall_frame();
    let gregs = frame.to_gregs();
    assert_eq!(gregs[REG_RIP], 0x40_1002, "RIP comes from the SYSCALL return address");
    assert_eq!(gregs[REG_EFL], 0x246);
    assert_eq!(gregs[REG_RDI], 0xd1);

    // A handler may redirect execution (as musl's cancellation does) and
    // try to set privileged RFLAGS bits; only user-modifiable bits survive.
    let mut edited = gregs;
    edited[REG_RIP] = 0x40_2000;
    edited[REG_EFL] = 0x3000 | 0x100 | 0x1; // IOPL, TF, CF
    let mut restored = sample_syscall_frame();
    restored.load_gregs(&edited);
    assert_eq!(restored.rcx, 0x40_2000);
    assert_eq!(restored.r11, 0x202 | 0x1);
    assert_eq!(restored.rdi, 0xd1);
    assert_eq!(sanitize_rflags(0), 0x202);
}

#[test]
fn restart_rewinds_over_the_syscall_instruction() {
    let mut frame = sample_syscall_frame();
    frame.rax = to_sysret(-ERESTARTSYS);
    restart_syscall(&mut frame, SYS_READ);
    assert_eq!(frame.rax, SYS_READ);
    assert_eq!(frame.rcx, 0x40_1000);
}

// ---------------------------------------------------------------------------
// fd object refcounting, eventfd/timerfd/signalfd, flock, memfd seals
// ---------------------------------------------------------------------------

static EVFD_TEST_LOCK: Mutex<()> = Mutex::new(());

fn evfd_test_setup() -> std::sync::MutexGuard<'static, ()> {
    let guard = EVFD_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    reset_epoll_for_test();
    ipc::init();
    crate::evfd::reset_for_test();
    flock_reset_for_test();
    memfd_seals_reset_for_test();
    guard
}

#[test]
fn dup2_then_close_keeps_pipe_writer_alive() {
    // Regression: dup/dup2 did not retain pipes, so the shell idiom
    // `dup2(pipe_w, 1); close(pipe_w)` dropped the writer count to zero and
    // readers saw EOF while fd 1 was still open.
    let _guard = evfd_test_setup();
    let mut task = task::Task::for_test(301);
    let task_ptr = &mut task as *mut task::Task;
    let pipe = ipc::pipe_open().expect("pipe open");
    fd_write_slot(task_ptr, 3, task::USER_FD_KIND_PIPE, pipe, 0, fs::O_RDONLY);
    fd_write_slot(task_ptr, 4, task::USER_FD_KIND_PIPE, pipe, 0, fs::O_WRONLY);

    assert_eq!(fd_dup_to(task_ptr, 4, 1, false, false), Ok(1));
    fd_close(task_ptr, 4).expect("close original writer");
    assert_eq!(poll_fd_revents(task_ptr, 3, POLLIN) & POLLHUP, 0, "writer still open via fd 1");

    fd_close(task_ptr, 1).expect("close dup'd writer");
    assert_eq!(poll_fd_revents(task_ptr, 3, POLLIN) & POLLHUP, POLLHUP);
    fd_close(task_ptr, 3).expect("close reader");
}

#[test]
fn eventfd_slot_polls_and_survives_dup_until_last_close() {
    let _guard = evfd_test_setup();
    let mut task = task::Task::for_test(302);
    let task_ptr = &mut task as *mut task::Task;
    let id = crate::evfd::eventfd_create(0, false).expect("eventfd");
    fd_write_slot(task_ptr, 3, task::USER_FD_KIND_EVENTFD, id, 0, fs::O_RDWR);

    assert_eq!(poll_fd_revents(task_ptr, 3, POLLIN | POLLOUT), POLLOUT);
    crate::evfd::eventfd_write(id, 2).expect("write");
    assert_eq!(poll_fd_revents(task_ptr, 3, POLLIN | POLLOUT), POLLIN | POLLOUT);
    assert!(epoll_target_kind_supported(task::USER_FD_KIND_EVENTFD));

    assert_eq!(fd_dup_from(task_ptr, 3, 5, true), Ok(5));
    fd_close(task_ptr, 3).expect("close original");
    assert_eq!(crate::evfd::eventfd_read(id), Ok(2), "object alive through dup");
    fd_close(task_ptr, 5).expect("close dup");
    assert_eq!(crate::evfd::eventfd_read(id), Err(-EBADF), "freed on last close");
}

#[test]
fn timerfd_slot_reports_readable_after_deadline() {
    let _guard = evfd_test_setup();
    let mut task = task::Task::for_test(303);
    let task_ptr = &mut task as *mut task::Task;
    let set_now = |ns: u64| EVFD_TEST_NOW_NS.store(ns, core::sync::atomic::Ordering::Relaxed);
    set_now(1_000_000);
    let id = crate::evfd::timerfd_create(CLOCK_MONOTONIC).expect("timerfd");
    fd_write_slot(task_ptr, 3, task::USER_FD_KIND_TIMERFD, id, 0, fs::O_RDWR);
    assert_eq!(poll_fd_revents(task_ptr, 3, POLLIN), 0, "disarmed");
    crate::evfd::timerfd_settime(id, evfd_now_ns(), false, 5_000_000, 0).expect("arm");
    assert_eq!(poll_fd_revents(task_ptr, 3, POLLIN), 0, "before deadline");
    set_now(6_000_000);
    // A timerfd is readable, never writable.
    assert_eq!(poll_fd_revents(task_ptr, 3, POLLIN | POLLOUT), POLLIN);
    set_now(0);
}

#[test]
fn signalfd_reports_and_dequeues_pending_masked_signals() {
    let _guard = evfd_test_setup();
    let mut task = task::Task::for_test(304);
    let task_ptr = &mut task as *mut task::Task;
    // User mask for SIGUSR1 (bit 9 in the Linux sigset layout).
    let mask = signalfd_mask_from_user(1u64 << (signal::SIGUSR1 - 1));
    assert_eq!(mask, signal::signal_bit(signal::SIGUSR1));
    assert_eq!(
        signalfd_mask_from_user(u64::MAX) & signal::signal_bit(signal::SIGKILL),
        0,
        "SIGKILL is never readable"
    );
    let id = crate::evfd::signalfd_create(mask).expect("signalfd");
    fd_write_slot(task_ptr, 3, task::USER_FD_KIND_SIGNALFD, id, 0, fs::O_RDWR);
    assert_eq!(poll_fd_revents(task_ptr, 3, POLLIN), 0);

    task.sig_pending = signal::signal_bit(signal::SIGTERM);
    assert_eq!(poll_fd_revents(task_ptr, 3, POLLIN), 0, "SIGTERM not in mask");
    task.sig_pending |= signal::signal_bit(signal::SIGUSR1);
    assert_eq!(poll_fd_revents(task_ptr, 3, POLLIN), POLLIN);

    assert_eq!(signalfd_dequeue(task_ptr, mask), Some(signal::SIGUSR1));
    assert_eq!(task.sig_pending, signal::signal_bit(signal::SIGTERM));
    assert_eq!(signalfd_dequeue(task_ptr, mask), None);

    let info = signalfd_siginfo_bytes(signal::SIGUSR1);
    assert_eq!(u32::from_ne_bytes([info[0], info[1], info[2], info[3]]), signal::SIGUSR1);
    assert!(info[4..].iter().all(|b| *b == 0));
}

#[test]
fn timespec_conversion_validates_nanoseconds() {
    assert_eq!(timespec_parts_to_ns(1, 500), Ok(1_000_000_500));
    assert_eq!(timespec_parts_to_ns(0, 1_000_000_000), Err(-EINVAL));
    assert_eq!(timespec_parts_to_ns(-1, 0), Err(-EINVAL));
    assert_eq!(ns_to_timespec_parts(2_000_000_001), (2, 1));
}

#[test]
fn flock_table_holds_one_lock_per_wayland_auto_socket() {
    let _guard = evfd_test_setup();
    // wl_display_add_socket_auto holds wayland-0.lock .. wayland-32.lock.
    for i in 0..33u16 {
        assert_eq!(flock_try_acquire(100 + i, 1, 3 + i as i32, true), Ok(()));
    }
    for i in 0..33u16 {
        flock_release_fd(1, 3 + i as i32);
    }
}

#[test]
fn flock_exclusive_conflicts_and_releases_on_unlock() {
    let _guard = evfd_test_setup();
    assert_eq!(flock_try_acquire(7, 1, 3, true), Ok(()));
    assert_eq!(flock_try_acquire(7, 1, 3, true), Ok(()), "re-lock by owner");
    assert_eq!(flock_try_acquire(7, 2, 4, false), Err(-EAGAIN));
    assert_eq!(flock_try_acquire(8, 2, 4, true), Ok(()), "other inode");
    flock_release_fd(1, 3);
    assert_eq!(flock_try_acquire(7, 2, 4, false), Ok(()));
    assert_eq!(flock_try_acquire(7, 3, 5, false), Ok(()), "shared + shared");
    assert_eq!(flock_try_acquire(7, 1, 3, true), Err(-EAGAIN));
    // Downgrading/upgrading your own lock only conflicts with others.
    flock_release_fd(3, 5);
    assert_eq!(flock_try_acquire(7, 2, 4, true), Ok(()));
}

#[test]
fn flock_is_released_when_the_fd_closes() {
    let _guard = evfd_test_setup();
    let mut task = task::Task::for_test(305);
    let task_ptr = &mut task as *mut task::Task;
    fd_write_slot(task_ptr, 3, task::USER_FD_KIND_VFS, 9, 0, fs::O_RDWR);
    assert_eq!(flock_try_acquire(9, 305, 3, true), Ok(()));
    assert_eq!(flock_try_acquire(9, 999, 4, true), Err(-EAGAIN));
    fd_close(task_ptr, 3).expect("close");
    assert_eq!(flock_try_acquire(9, 999, 4, true), Ok(()));
}

#[test]
fn memfd_seals_follow_allow_sealing_and_block_resizes() {
    let _guard = evfd_test_setup();
    memfd_seals_register(40, false);
    assert_eq!(memfd_seals_get(40), F_SEAL_SEAL);
    assert_eq!(memfd_seals_add(40, F_SEAL_SHRINK), Err(-EPERM));

    memfd_seals_register(41, true);
    assert_eq!(memfd_seals_get(41), 0);
    memfd_seals_add(41, F_SEAL_SHRINK).expect("add shrink seal");
    let seals = memfd_seals_get(41);
    assert_eq!(seals_allow_resize(seals, 4096, 8192), Ok(()));
    assert_eq!(seals_allow_resize(seals, 4096, 0), Err(-EPERM));
    assert_eq!(seals_allow_write(seals), Ok(()));

    memfd_seals_add(41, F_SEAL_GROW | F_SEAL_WRITE | F_SEAL_SEAL).expect("add more");
    let seals = memfd_seals_get(41);
    assert_eq!(seals_allow_resize(seals, 4096, 8192), Err(-EPERM));
    assert_eq!(seals_allow_write(seals), Err(-EPERM));
    assert_eq!(memfd_seals_add(41, F_SEAL_SHRINK), Err(-EPERM), "sealed seals");
    assert_eq!(memfd_seals_add(41, 0x100), Err(-EINVAL));

    // Re-registering an inode (a new memfd reusing it) resets its seals.
    memfd_seals_register(41, true);
    assert_eq!(memfd_seals_get(41), 0);
    // Non-memfd fds are never sealed.
    assert_eq!(fd_seals(fs::O_RDWR, 41), 0);
    assert_eq!(fd_seals(FD_INTERNAL_MEMFD, 41), 0);
}

#[test]
fn ucred_layout_matches_linux() {
    let bytes = ucred_bytes(socket::UnixCred {
        pid: 7,
        uid: 1000,
        gid: 100,
    });
    assert_eq!(&bytes[0..4], &7i32.to_ne_bytes());
    assert_eq!(&bytes[4..8], &1000u32.to_ne_bytes());
    assert_eq!(&bytes[8..12], &100u32.to_ne_bytes());
}

#[test]
fn lock_free_syscalls_are_only_id_and_yield_calls() {
    assert!(is_lock_free_syscall(SYS_YIELD));
    assert!(is_lock_free_syscall(SYS_GETPID));
    assert!(is_lock_free_syscall(SYS_GETTID));
    assert!(is_lock_free_syscall(SYS_GETPPID));
    assert!(is_lock_free_syscall(SYS_CLOCK_GETTIME));
    assert!(is_lock_free_syscall(SYS_GETTIMEOFDAY));
    assert!(is_lock_free_syscall(SYS_NANOSLEEP));
    // Anything touching user memory or shared kernel state stays locked.
    assert!(!is_lock_free_syscall(SYS_EXECVE));
    assert!(!is_lock_free_syscall(SYS_RT_SIGRETURN));
    assert!(!is_lock_free_syscall(SYS_FORK));
    assert!(!is_lock_free_syscall(SYS_WRITE));
}

#[test]
fn lock_free_return_falls_back_for_signals_and_exit() {
    assert!(!needs_locked_return(0, 0, 0));
    // Pending but blocked: still lock-free.
    assert!(!needs_locked_return(1 << 9, 1 << 9, 0));
    // Pending and deliverable.
    assert!(needs_locked_return(1 << 9, 0, 0));
    // Told to exit by a sibling thread.
    assert!(needs_locked_return(0, 0, task::TASK_FLAG_EXITING));
}

#[test]
fn process_exit_sends_sigchld_to_parent() {
    let _guard = evfd_test_setup();
    task::Task::reset_registry_for_test();
    let mut parent = task::Task::for_test(410);
    let mut child = task::Task::for_test(411);
    child.ppid = 410;
    let parent_ptr = &mut parent as *mut task::Task;
    let child_ptr = &mut child as *mut task::Task;
    task::Task::install_for_test(0, parent_ptr);
    task::Task::install_for_test(1, child_ptr);

    // Default disposition ignores SIGCHLD: nothing is queued.
    notify_parent_of_exit(child_ptr);
    assert_eq!(parent.sig_pending & signal::signal_bit(signal::SIGCHLD), 0);

    // A parent that blocks SIGCHLD (e.g. to read it from a signalfd) sees it.
    parent.signal_blocked |= signal::signal_bit(signal::SIGCHLD);
    notify_parent_of_exit(child_ptr);
    assert_ne!(parent.sig_pending & signal::signal_bit(signal::SIGCHLD), 0);

    // No parent: nothing to do (and no crash).
    child.ppid = 999;
    notify_parent_of_exit(child_ptr);

    task::Task::install_for_test(0, core::ptr::null_mut());
    task::Task::install_for_test(1, core::ptr::null_mut());
}

#[test]
fn shared_write_window_allows_partial_last_page() {
    let _guard = MMAP_TEST_LOCK.lock().expect("mmap test lock");
    reset_vm_for_test();
    let opened = fs::open("/temp/window-test", fs::O_CREAT | fs::O_RDWR, 0o600).expect("create");
    let inode = opened.inode;
    fs::write(inode, 0, &[7u8; 100], false).expect("write");
    // A 100-byte file can be mapped with one (page-rounded) page...
    assert_eq!(mmap_validate_shared_write_window(inode, 0, PAGE_SIZE), Ok(()));
    // ...but not with two.
    assert_eq!(mmap_validate_shared_write_window(inode, 0, 2 * PAGE_SIZE), Err(-EINVAL));
}

#[test]
fn memfd_shmem_object_lives_until_last_fd_and_mapping_are_gone() {
    let _guard = MMAP_TEST_LOCK.lock().expect("mmap test lock");
    reset_vm_for_test();
    crate::shmem::reset_for_test();
    let mut task = task::Task::for_test(420);
    let task_ptr = &mut task as *mut task::Task;
    task::Task::install_for_test(0, task_ptr);

    // What memfd_create does: tmpfs inode + fd (open reference) + shmem
    // object, then unlink so only references keep it alive.
    let opened = fs::open("/temp/memfd-life", fs::O_CREAT | fs::O_RDWR, 0o600).expect("create");
    let inode = opened.inode;
    let fd = fd_alloc(task_ptr, task::USER_FD_KIND_VFS, inode, fs::O_RDWR | FD_INTERNAL_MEMFD)
        .expect("fd") as i32;
    crate::shmem::create(inode).expect("shmem");
    fs::unlink("/temp/memfd-life").expect("unlink");
    assert_eq!(vfs_truncate_to(inode, 3 * 1024 * 1024), Ok(()));
    assert_eq!(vfs_file_size(inode), Ok(3 * 1024 * 1024));

    // dup + close the original: the object survives through the dup.
    let dup = fd_dup_from(task_ptr, fd, 8, false).expect("dup") as i32;
    fd_close(task_ptr, fd).expect("close original");
    assert!(crate::shmem::exists(inode));

    // A mapping keeps the object alive after the last fd closes.
    let slot = mmap_prepare_task_slot(task_ptr).expect("vm slot");
    unsafe {
        TASK_VM_MAPPINGS[slot][0] = VmMapping {
            start: 0x1_0000_0000,
            len: PAGE_SIZE,
            file_offset: 0,
            file_inode: inode,
            prot: PROT_READ | PROT_WRITE,
            map_shared: true,
            map_shared_can_write: true,
        };
    }
    vm_sync_inode_pins();
    fd_close(task_ptr, dup).expect("close dup");
    assert!(crate::shmem::exists(inode), "still mapped");

    // Dropping the mapping frees the inode and with it the shmem object.
    mmap_clear_task_regions(task_ptr);
    assert!(!crate::shmem::exists(inode));

    task::Task::install_for_test(0, core::ptr::null_mut());
}

fn anon_region(start: u64, pages: u64) -> VmMapping {
    VmMapping {
        start,
        len: pages * PAGE_SIZE,
        file_offset: 0,
        file_inode: task::USER_FD_NONE,
        prot: PROT_READ | PROT_WRITE,
        map_shared: false,
        map_shared_can_write: false,
    }
}

#[test]
fn adjacent_compatible_regions_merge_on_insert() {
    let mut regions = [VmMapping::EMPTY; MMAP_MAX_REGIONS_PER_TASK];
    let base = 0x1_0000_0000u64;
    mmap_regions_insert(&mut regions, anon_region(base, 2)).unwrap();
    mmap_regions_insert(&mut regions, anon_region(base + 4 * PAGE_SIZE, 1)).unwrap();
    // Fills the gap: all three become one region.
    mmap_regions_insert(&mut regions, anon_region(base + 2 * PAGE_SIZE, 2)).unwrap();
    let used: Vec<_> = regions.iter().filter(|r| !r.is_empty()).collect();
    assert_eq!(used.len(), 1);
    assert_eq!((used[0].start, used[0].len), (base, 5 * PAGE_SIZE));

    // Different protection does not merge.
    let mut ro = anon_region(base + 5 * PAGE_SIZE, 1);
    ro.prot = PROT_READ;
    mmap_regions_insert(&mut regions, ro).unwrap();
    assert_eq!(regions.iter().filter(|r| !r.is_empty()).count(), 2);
}

#[test]
fn full_region_table_rejects_unmergeable_insert_without_changes() {
    let mut regions = [VmMapping::EMPTY; MMAP_MAX_REGIONS_PER_TASK];
    let base = 0x4_0000_0000u64;
    // Fill every slot with non-adjacent regions (gaps prevent merging).
    for i in 0..MMAP_MAX_REGIONS_PER_TASK as u64 {
        mmap_regions_insert(&mut regions, anon_region(base + i * 4 * PAGE_SIZE, 1)).unwrap();
    }
    let before = regions;
    let far = anon_region(base + (MMAP_MAX_REGIONS_PER_TASK as u64 * 4 + 64) * PAGE_SIZE, 1);
    assert_eq!(mmap_regions_insert(&mut regions, far), Err(-ENOMEM));
    assert_eq!(regions, before, "failed insert must not modify the table");
    // A region adjacent to an existing one still fits by merging.
    mmap_regions_insert(&mut regions, anon_region(base + PAGE_SIZE, 1)).unwrap();
}

#[test]
fn region_table_size_budget() {
    // Tables live in a static array and heap scratch copies, never on the
    // 16 KiB kernel stack; keep one table at most half a stack anyway.
    assert!(core::mem::size_of::<RegionTable>() <= 8192);
    assert!(MMAP_MAX_REGIONS_PER_TASK >= 256);
}

fn used_regions(regions: &RegionTable) -> usize {
    regions.iter().filter(|r| !r.is_empty()).count()
}

/// Fill every slot with one-page regions separated by gaps (no merging).
fn fill_region_table(regions: &mut RegionTable, base: u64) {
    for i in 0..MMAP_MAX_REGIONS_PER_TASK as u64 {
        mmap_regions_insert(regions, anon_region(base + i * 4 * PAGE_SIZE, 1)).unwrap();
    }
}

#[test]
fn region_scratch_copies_and_drops() {
    let mut regions = [VmMapping::EMPTY; MMAP_MAX_REGIONS_PER_TASK];
    fill_region_table(&mut regions, 0x5_0000_0000);
    let mut scratch = RegionScratch::copy_of(&regions).expect("scratch alloc");
    assert_eq!(*scratch, regions);
    mmap_regions_unmap_range(&mut scratch, 0x5_0000_0000, 0x5_0000_0000 + PAGE_SIZE).unwrap();
    assert_eq!(used_regions(&scratch), MMAP_MAX_REGIONS_PER_TASK - 1);
    assert_eq!(used_regions(&regions), MMAP_MAX_REGIONS_PER_TASK, "source unchanged");
}

#[test]
fn unmap_hole_in_full_table_fails_without_changes() {
    let mut regions = [VmMapping::EMPTY; MMAP_MAX_REGIONS_PER_TASK];
    let base = 0x6_0000_0000u64;
    for i in 0..MMAP_MAX_REGIONS_PER_TASK as u64 {
        mmap_regions_insert(&mut regions, anon_region(base + i * 8 * PAGE_SIZE, 3)).unwrap();
    }
    let before = regions;
    // Punching a hole in the middle of a region needs a free slot.
    assert_eq!(
        mmap_regions_unmap_range(&mut regions, base + PAGE_SIZE, base + 2 * PAGE_SIZE),
        Err(-ENOMEM)
    );
    assert_eq!(regions, before);
    // Trimming an end needs none.
    mmap_regions_unmap_range(&mut regions, base, base + PAGE_SIZE).unwrap();
}

#[test]
fn mprotect_split_then_restore_merges_back() {
    // RELRO-style: protect the middle of a mapping, then restore it.
    let mut regions = [VmMapping::EMPTY; MMAP_MAX_REGIONS_PER_TASK];
    let base = 0x7_0000_0000u64;
    mmap_regions_insert(&mut regions, anon_region(base, 3)).unwrap();
    mmap_regions_protect_range(&mut regions, base + PAGE_SIZE, base + 2 * PAGE_SIZE, PROT_READ)
        .unwrap();
    assert_eq!(used_regions(&regions), 3);
    mmap_regions_protect_range(
        &mut regions,
        base + PAGE_SIZE,
        base + 2 * PAGE_SIZE,
        PROT_READ | PROT_WRITE,
    )
    .unwrap();
    assert_eq!(used_regions(&regions), 1);
    let only = regions.iter().find(|r| !r.is_empty()).unwrap();
    assert_eq!((only.start, only.len), (base, 3 * PAGE_SIZE));
}

#[test]
fn mprotect_on_full_table_fails_without_changes_unless_no_split() {
    let mut regions = [VmMapping::EMPTY; MMAP_MAX_REGIONS_PER_TASK];
    let base = 0x8_0000_0000u64;
    for i in 0..MMAP_MAX_REGIONS_PER_TASK as u64 {
        mmap_regions_insert(&mut regions, anon_region(base + i * 8 * PAGE_SIZE, 3)).unwrap();
    }
    let before = regions;
    assert_eq!(
        mmap_regions_protect_range(&mut regions, base + PAGE_SIZE, base + 2 * PAGE_SIZE, PROT_READ),
        Err(-ENOMEM)
    );
    assert_eq!(regions, before);
    // Same protection: nothing to split, so it succeeds on a full table.
    mmap_regions_protect_range(
        &mut regions,
        base + PAGE_SIZE,
        base + 2 * PAGE_SIZE,
        PROT_READ | PROT_WRITE,
    )
    .unwrap();
    assert_eq!(regions, before);
    // Whole region: no split either.
    mmap_regions_protect_range(&mut regions, base, base + 3 * PAGE_SIZE, PROT_READ).unwrap();
    assert_eq!(used_regions(&regions), MMAP_MAX_REGIONS_PER_TASK);
}

#[test]
fn mprotect_rejects_unmapped_and_disallowed_ranges_without_changes() {
    let mut regions = [VmMapping::EMPTY; MMAP_MAX_REGIONS_PER_TASK];
    let base = 0x9_0000_0000u64;
    mmap_regions_insert(&mut regions, anon_region(base, 2)).unwrap();
    let mut shared_ro = anon_region(base + 2 * PAGE_SIZE, 2);
    shared_ro.prot = PROT_READ;
    shared_ro.map_shared = true;
    shared_ro.file_inode = 5;
    mmap_regions_insert(&mut regions, shared_ro).unwrap();
    let before = regions;
    // Gap after the mapping.
    assert_eq!(
        mmap_regions_protect_range(&mut regions, base, base + 6 * PAGE_SIZE, PROT_READ),
        Err(-ENOMEM)
    );
    // Writable over a read-only shared file mapping, spanning both regions.
    assert_eq!(
        mmap_regions_protect_range(
            &mut regions,
            base + PAGE_SIZE,
            base + 3 * PAGE_SIZE,
            PROT_READ | PROT_WRITE
        ),
        Err(-EINVAL)
    );
    assert_eq!(regions, before);
}

#[test]
fn file_regions_merge_only_with_contiguous_offsets() {
    let file = |start: u64, off: u64| VmMapping {
        start,
        len: PAGE_SIZE,
        file_offset: off,
        file_inode: 9,
        prot: PROT_READ,
        map_shared: true,
        map_shared_can_write: false,
    };
    let base = 0x2_0000_0000u64;
    assert!(mmap_regions_can_merge(&file(base, 0), &file(base + PAGE_SIZE, PAGE_SIZE)));
    assert!(!mmap_regions_can_merge(&file(base, 0), &file(base + PAGE_SIZE, 0)));
    let mut other = file(base + PAGE_SIZE, PAGE_SIZE);
    other.file_inode = 10;
    assert!(!mmap_regions_can_merge(&file(base, 0), &other));
}

#[test]
fn many_adjacent_anonymous_maps_use_one_slot() {
    // What musl's allocator does while libxkbcommon compiles a keymap.
    let mut regions = [VmMapping::EMPTY; MMAP_MAX_REGIONS_PER_TASK];
    let base = 0x3_0000_0000u64;
    for i in 0..(2 * MMAP_MAX_REGIONS_PER_TASK as u64) {
        mmap_regions_insert(&mut regions, anon_region(base + i * PAGE_SIZE, 1)).unwrap();
    }
    assert_eq!(regions.iter().filter(|r| !r.is_empty()).count(), 1);
}

#[test]
fn mprotect_target_classifies_exec_image_and_mmap_ranges() {
    // musl's loader applies RELRO to the program and itself, which execve
    // loaded below MMAP_BASE_ADDR (0x400000 and 0x7000_0000).
    assert_eq!(
        mprotect_target(0x7000_3000, 0x1800),
        Ok(MprotectTarget::ExecImage { end: 0x7000_5000 })
    );
    assert_eq!(
        mprotect_target(0x40_1000, PAGE_SIZE),
        Ok(MprotectTarget::ExecImage { end: 0x40_2000 })
    );
    assert_eq!(
        mprotect_target(MMAP_BASE_ADDR, PAGE_SIZE),
        Ok(MprotectTarget::Regions { end: MMAP_BASE_ADDR + PAGE_SIZE })
    );
    // Straddling the boundary, page zero, misalignment, and kernel space fail.
    assert_eq!(mprotect_target(MMAP_BASE_ADDR - PAGE_SIZE, 2 * PAGE_SIZE), Err(-EINVAL));
    assert_eq!(mprotect_target(0, PAGE_SIZE), Err(-EINVAL));
    assert_eq!(mprotect_target(0x40_1001, PAGE_SIZE), Err(-EINVAL));
    assert_eq!(mprotect_target(KERNEL_VIRT_BASE - PAGE_SIZE, PAGE_SIZE), Err(-EINVAL));
    // The kernel half, including the shared direct map, is never user memory.
    assert_eq!(mprotect_target(arch_x86_64::addr::DIRECT_MAP_BASE, PAGE_SIZE), Err(-EINVAL));
    assert_eq!(mprotect_target(USER_SPACE_END - PAGE_SIZE, 2 * PAGE_SIZE), Err(-EINVAL));
}

#[test]
fn user_stack_region_is_8mib_below_stack_top_and_lazy_anonymous() {
    let r = user_stack_region();
    assert_eq!(r.end(), user::USER_STACK_TOP);
    assert_eq!(r.len, 8 * 1024 * 1024);
    assert_eq!(r.file_inode, task::USER_FD_NONE);
    assert_eq!(r.prot, PROT_READ | PROT_WRITE);
    assert!(!r.map_shared);
    assert!(r.start >= MMAP_BASE_ADDR && r.end() <= USER_SPACE_END);
    let mut regions = [VmMapping::EMPTY; MMAP_MAX_REGIONS_PER_TASK];
    mmap_regions_insert(&mut regions, r).unwrap();
    let deep = user::USER_STACK_TOP - 7 * 1024 * 1024;
    assert_eq!(mmap_regions_find_mapping(&regions, deep).map(|m| m.start), Some(r.start));
    assert!(mmap_regions_find_mapping(&regions, r.start - PAGE_SIZE).is_none(), "below the reserve faults");
}

#[test]
fn fadvise_accepts_hints_on_files_and_rejects_pipes_and_bad_args() {
    assert_eq!(fadvise_result(task::USER_FD_KIND_VFS, 0, 4), 0); // POSIX_FADV_DONTNEED
    assert_eq!(fadvise_result(task::USER_FD_KIND_VFS, 4096, 2), 0); // SEQUENTIAL
    assert_eq!(fadvise_result(task::USER_FD_KIND_PIPE, 0, 0), -ESPIPE);
    assert_eq!(fadvise_result(task::USER_FD_KIND_SOCKET, 0, 0), -ESPIPE);
    assert_eq!(fadvise_result(task::USER_FD_KIND_VFS, -1, 0), -EINVAL);
    assert_eq!(fadvise_result(task::USER_FD_KIND_VFS, 0, 6), -EINVAL);
}

#[test]
fn prctl_flags_and_membarrier_query() {
    assert_eq!(prctl_flag_result(PR_SET_PDEATHSIG, 9), 0);
    assert_eq!(prctl_flag_result(PR_SET_PDEATHSIG, 65), -EINVAL);
    assert_eq!(prctl_flag_result(PR_GET_DUMPABLE, 0), 1);
    assert_eq!(prctl_flag_result(PR_SET_NO_NEW_PRIVS, 1), 0);
    assert_eq!(prctl_flag_result(9999, 0), -EINVAL);
    assert_eq!(membarrier_result(0), 0);
    assert_eq!(membarrier_result(16), -EINVAL);
}

#[test]
fn temporary_sigmask_restores_on_exit_or_through_the_frame() {
    let mut t = task::Task::for_test(77);
    let tp: *mut task::Task = &mut t;
    let usr1 = crate::signal::signal_bit(10);
    t.signal_blocked = usr1;
    // epoll_pwait with an empty mask: SIGUSR1 deliverable during the wait.
    crate::signal::set_temporary_mask(tp, 0);
    assert_eq!(t.signal_blocked, 0);
    assert!(t.restore_sigmask && t.saved_sigmask == usr1);
    // Nested install keeps the original saved mask.
    crate::signal::set_temporary_mask(tp, 0);
    assert_eq!(t.saved_sigmask, usr1);
    // No signal delivered: syscall exit restores the original.
    crate::signal::restore_temporary_mask(tp);
    assert_eq!(t.signal_blocked, usr1);
    assert!(!t.restore_sigmask);
}
