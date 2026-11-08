use core::cmp;
use core::mem::size_of;
use core::ptr;
use core::sync::atomic::{AtomicI32, Ordering};

use arch_x86_64::addr::{PhysAddr, VirtAddr, KERNEL_VIRT_BASE, PAGE_SIZE};
use arch_x86_64::gdt;
use arch_x86_64::idt::InterruptFrame;
use arch_x86_64::pit;
use arch_x86_64::syscall as arch_syscall;

use crate::elf;
use crate::memory::{heap, pmm, vmm};
use crate::serial_println;
use crate::shm;
use crate::task::{self, TaskState, TASK_FLAG_USER};

const USER_STACK_TOP: u64 = 0x7FFF_FF00_0000;
const USER_STACK_SIZE: u64 = 32 * 1024;
const USER_STACK_PAGES: usize = (USER_STACK_SIZE / PAGE_SIZE) as usize;
const INIT_LOAD_BASE: u64 = 0x0040_0000;
const INIT_INTERP_BASE: u64 = 0x7000_0000;
const ELF_INTERP_MAX: usize = 256;
const EXEC_MAX_ARGV: usize = 64;
const EXEC_MAX_ENVP: usize = 64;
const EXEC_MAX_INTERP_IMAGE: usize = 2 * 1024 * 1024;

#[cfg(any(
    feature = "musl-ext2-exec",
    feature = "musl-ext2-cmd-smoke",
    feature = "musl-ext2-ping-smoke",
    feature = "musl-ext2-unix-rights-smoke",
    feature = "musl-ext2-epoll-smoke",
    feature = "musl-ext2-shared-buffer-smoke",
    feature = "musl-ext2-console-first-smoke",
    feature = "musl-ext2-gui-smoke",
    feature = "musl-ext2-gui-demo",
    feature = "musl-ext2-peet-gui-smoke",
    feature = "musl-ext2-peet-shell-demo",
    feature = "musl-ext2-quickjs-smoke",
    feature = "musl-ext2-micropython-smoke",
    feature = "musl-ext2-dropbear-smoke",
    feature = "musl-ext2-fpu-smoke",
    feature = "musl-ext2-proc-smoke",
    feature = "musl-ext2-wayland-smoke",
    feature = "musl-ext2-wayland-tests"
))]
const INIT_ARGV0: &[u8] = b"/init-ext2\0";
#[cfg(all(
    not(feature = "musl-ext2-exec"),
    not(feature = "musl-ext2-cmd-smoke"),
    not(feature = "musl-ext2-ping-smoke"),
    not(feature = "musl-ext2-unix-rights-smoke"),
    not(feature = "musl-ext2-epoll-smoke"),
    not(feature = "musl-ext2-shared-buffer-smoke"),
    not(feature = "musl-ext2-console-first-smoke"),
    not(feature = "musl-ext2-gui-smoke"),
    not(feature = "musl-ext2-gui-demo"),
    not(feature = "musl-ext2-peet-gui-smoke"),
    not(feature = "musl-ext2-peet-shell-demo"),
    not(feature = "musl-ext2-quickjs-smoke"),
    not(feature = "musl-ext2-micropython-smoke"),
    not(feature = "musl-ext2-dropbear-smoke"),
    not(feature = "musl-ext2-fpu-smoke"),
    not(feature = "musl-ext2-proc-smoke"),
    not(feature = "musl-ext2-wayland-smoke"),
    not(feature = "musl-ext2-wayland-tests"),
    feature = "musl-smoke"
))]
const INIT_ARGV0: &[u8] = b"/bin/echo\0";
#[cfg(all(
    not(feature = "musl-ext2-exec"),
    not(feature = "musl-ext2-cmd-smoke"),
    not(feature = "musl-ext2-ping-smoke"),
    not(feature = "musl-ext2-unix-rights-smoke"),
    not(feature = "musl-ext2-epoll-smoke"),
    not(feature = "musl-ext2-shared-buffer-smoke"),
    not(feature = "musl-ext2-console-first-smoke"),
    not(feature = "musl-ext2-gui-smoke"),
    not(feature = "musl-ext2-gui-demo"),
    not(feature = "musl-ext2-peet-gui-smoke"),
    not(feature = "musl-ext2-peet-shell-demo"),
    not(feature = "musl-ext2-quickjs-smoke"),
    not(feature = "musl-ext2-micropython-smoke"),
    not(feature = "musl-ext2-dropbear-smoke"),
    not(feature = "musl-ext2-fpu-smoke"),
    not(feature = "musl-ext2-proc-smoke"),
    not(feature = "musl-ext2-wayland-smoke"),
    not(feature = "musl-ext2-wayland-tests"),
    not(feature = "musl-smoke")
))]
const INIT_ARGV0: &[u8] = b"/init\0";
const INIT_ENV_PATH: &[u8] = b"PATH=/bin\0";
const INIT_ENV_TERM: &[u8] = b"TERM=peetos\0";
const INIT_INTERP_PATH: &[u8] = b"/lib/ld-peetos.so";

const AT_NULL: u64 = 0;
const AT_PHDR: u64 = 3;
const AT_PHENT: u64 = 4;
const AT_PHNUM: u64 = 5;
const AT_PAGESZ: u64 = 6;
const AT_BASE: u64 = 7;
const AT_ENTRY: u64 = 9;
const AT_UID: u64 = 11;
const AT_EUID: u64 = 12;
const AT_GID: u64 = 13;
const AT_EGID: u64 = 14;
const AT_SECURE: u64 = 23;
const AT_RANDOM: u64 = 25;
const AT_EXECFN: u64 = 31;

const EIO: i64 = 5;
const ENOENT: i64 = 2;
const ENOMEM: i64 = 12;
const EINVAL: i64 = 22;
const E2BIG: i64 = 7;
const EACCES: i64 = 13;

static INIT_PID: AtomicI32 = AtomicI32::new(task::INVALID_PID);

#[cfg(feature = "musl-ext2-wayland-smoke")]
static INIT_ELF: &[u8] = include_bytes!("../../user/musl_execve_ext2_wayland_smoke.elf");
#[cfg(all(
    not(feature = "musl-ext2-wayland-smoke"),
    feature = "musl-ext2-wayland-tests"
))]
static INIT_ELF: &[u8] = include_bytes!("../../user/musl_execve_ext2_wayland_tests.elf");
#[cfg(all(
    not(feature = "musl-ext2-wayland-smoke"),
    not(feature = "musl-ext2-wayland-tests"),
    feature = "musl-ext2-proc-smoke"
))]
static INIT_ELF: &[u8] = include_bytes!("../../user/musl_execve_ext2_proc_smoke.elf");
#[cfg(all(
    not(feature = "musl-ext2-proc-smoke"),
    not(feature = "musl-ext2-wayland-smoke"),
    not(feature = "musl-ext2-wayland-tests"),
    feature = "musl-ext2-fpu-smoke"
))]
static INIT_ELF: &[u8] = include_bytes!("../../user/musl_execve_ext2_fpu_smoke.elf");
#[cfg(all(
    not(feature = "musl-ext2-fpu-smoke"),
    not(feature = "musl-ext2-proc-smoke"),
    not(feature = "musl-ext2-wayland-smoke"),
    not(feature = "musl-ext2-wayland-tests"),
    feature = "musl-ext2-dropbear-smoke"
))]
static INIT_ELF: &[u8] = include_bytes!("../../user/musl_execve_ext2_dropbear_smoke.elf");
#[cfg(all(
    not(feature = "musl-ext2-dropbear-smoke"),
    not(feature = "musl-ext2-fpu-smoke"),
    not(feature = "musl-ext2-proc-smoke"),
    not(feature = "musl-ext2-wayland-smoke"),
    not(feature = "musl-ext2-wayland-tests"),
    feature = "musl-ext2-micropython-smoke"
))]
static INIT_ELF: &[u8] = include_bytes!("../../user/musl_execve_ext2_micropython_smoke.elf");
#[cfg(all(
    not(feature = "musl-ext2-dropbear-smoke"),
    not(feature = "musl-ext2-fpu-smoke"),
    not(feature = "musl-ext2-proc-smoke"),
    not(feature = "musl-ext2-wayland-smoke"),
    not(feature = "musl-ext2-wayland-tests"),
    not(feature = "musl-ext2-micropython-smoke"),
    feature = "musl-ext2-quickjs-smoke"
))]
static INIT_ELF: &[u8] = include_bytes!("../../user/musl_execve_ext2_quickjs_smoke.elf");
#[cfg(all(
    not(feature = "musl-ext2-dropbear-smoke"),
    not(feature = "musl-ext2-fpu-smoke"),
    not(feature = "musl-ext2-proc-smoke"),
    not(feature = "musl-ext2-wayland-smoke"),
    not(feature = "musl-ext2-wayland-tests"),
    not(feature = "musl-ext2-micropython-smoke"),
    not(feature = "musl-ext2-quickjs-smoke"),
    feature = "musl-ext2-ping-smoke"
))]
static INIT_ELF: &[u8] = include_bytes!("../../user/musl_execve_ext2_ping_smoke.elf");
#[cfg(all(
    not(feature = "musl-ext2-dropbear-smoke"),
    not(feature = "musl-ext2-fpu-smoke"),
    not(feature = "musl-ext2-proc-smoke"),
    not(feature = "musl-ext2-wayland-smoke"),
    not(feature = "musl-ext2-wayland-tests"),
    not(feature = "musl-ext2-micropython-smoke"),
    not(feature = "musl-ext2-quickjs-smoke"),
    not(feature = "musl-ext2-ping-smoke"),
    feature = "musl-ext2-shared-buffer-smoke"
))]
static INIT_ELF: &[u8] = include_bytes!("../../user/musl_execve_ext2_shared_buffer_smoke.elf");
#[cfg(all(
    not(feature = "musl-ext2-dropbear-smoke"),
    not(feature = "musl-ext2-fpu-smoke"),
    not(feature = "musl-ext2-proc-smoke"),
    not(feature = "musl-ext2-wayland-smoke"),
    not(feature = "musl-ext2-wayland-tests"),
    not(feature = "musl-ext2-micropython-smoke"),
    not(feature = "musl-ext2-quickjs-smoke"),
    not(feature = "musl-ext2-ping-smoke"),
    not(feature = "musl-ext2-shared-buffer-smoke"),
    feature = "musl-ext2-epoll-smoke"
))]
static INIT_ELF: &[u8] = include_bytes!("../../user/musl_execve_ext2_epoll_smoke.elf");
#[cfg(all(
    not(feature = "musl-ext2-dropbear-smoke"),
    not(feature = "musl-ext2-fpu-smoke"),
    not(feature = "musl-ext2-proc-smoke"),
    not(feature = "musl-ext2-wayland-smoke"),
    not(feature = "musl-ext2-wayland-tests"),
    not(feature = "musl-ext2-micropython-smoke"),
    not(feature = "musl-ext2-quickjs-smoke"),
    not(feature = "musl-ext2-ping-smoke"),
    not(feature = "musl-ext2-shared-buffer-smoke"),
    not(feature = "musl-ext2-epoll-smoke"),
    feature = "musl-ext2-console-first-smoke"
))]
static INIT_ELF: &[u8] = include_bytes!("../../user/musl_execve_ext2_console_first_smoke.elf");
#[cfg(all(
    not(feature = "musl-ext2-dropbear-smoke"),
    not(feature = "musl-ext2-fpu-smoke"),
    not(feature = "musl-ext2-proc-smoke"),
    not(feature = "musl-ext2-wayland-smoke"),
    not(feature = "musl-ext2-wayland-tests"),
    not(feature = "musl-ext2-micropython-smoke"),
    not(feature = "musl-ext2-quickjs-smoke"),
    not(feature = "musl-ext2-ping-smoke"),
    not(feature = "musl-ext2-shared-buffer-smoke"),
    not(feature = "musl-ext2-epoll-smoke"),
    not(feature = "musl-ext2-console-first-smoke"),
    not(feature = "musl-ext2-gui-smoke"),
    feature = "musl-ext2-gui-demo"
))]
static INIT_ELF: &[u8] = include_bytes!("../../user/musl_execve_ext2_gui_demo.elf");
#[cfg(all(
    not(feature = "musl-ext2-dropbear-smoke"),
    not(feature = "musl-ext2-fpu-smoke"),
    not(feature = "musl-ext2-proc-smoke"),
    not(feature = "musl-ext2-wayland-smoke"),
    not(feature = "musl-ext2-wayland-tests"),
    not(feature = "musl-ext2-micropython-smoke"),
    not(feature = "musl-ext2-quickjs-smoke"),
    not(feature = "musl-ext2-ping-smoke"),
    not(feature = "musl-ext2-shared-buffer-smoke"),
    not(feature = "musl-ext2-epoll-smoke"),
    not(feature = "musl-ext2-console-first-smoke"),
    not(feature = "musl-ext2-gui-demo"),
    feature = "musl-ext2-gui-smoke"
))]
static INIT_ELF: &[u8] = include_bytes!("../../user/musl_execve_ext2_gui_smoke.elf");
#[cfg(all(
    not(feature = "musl-ext2-dropbear-smoke"),
    not(feature = "musl-ext2-fpu-smoke"),
    not(feature = "musl-ext2-proc-smoke"),
    not(feature = "musl-ext2-wayland-smoke"),
    not(feature = "musl-ext2-wayland-tests"),
    not(feature = "musl-ext2-micropython-smoke"),
    not(feature = "musl-ext2-quickjs-smoke"),
    not(feature = "musl-ext2-ping-smoke"),
    not(feature = "musl-ext2-shared-buffer-smoke"),
    not(feature = "musl-ext2-epoll-smoke"),
    not(feature = "musl-ext2-console-first-smoke"),
    not(feature = "musl-ext2-gui-smoke"),
    not(feature = "musl-ext2-gui-demo"),
    feature = "musl-ext2-peet-gui-smoke"
))]
static INIT_ELF: &[u8] = include_bytes!("../../user/musl_execve_ext2_peet_gui_smoke.elf");
#[cfg(all(
    not(feature = "musl-ext2-dropbear-smoke"),
    not(feature = "musl-ext2-fpu-smoke"),
    not(feature = "musl-ext2-proc-smoke"),
    not(feature = "musl-ext2-wayland-smoke"),
    not(feature = "musl-ext2-wayland-tests"),
    not(feature = "musl-ext2-micropython-smoke"),
    not(feature = "musl-ext2-quickjs-smoke"),
    not(feature = "musl-ext2-ping-smoke"),
    not(feature = "musl-ext2-shared-buffer-smoke"),
    not(feature = "musl-ext2-epoll-smoke"),
    not(feature = "musl-ext2-console-first-smoke"),
    not(feature = "musl-ext2-gui-smoke"),
    not(feature = "musl-ext2-gui-demo"),
    not(feature = "musl-ext2-peet-gui-smoke"),
    feature = "musl-ext2-peet-shell-demo"
))]
static INIT_ELF: &[u8] = include_bytes!("../../user/musl_execve_ext2_peet_shell_demo.elf");
#[cfg(all(
    not(feature = "musl-ext2-dropbear-smoke"),
    not(feature = "musl-ext2-fpu-smoke"),
    not(feature = "musl-ext2-proc-smoke"),
    not(feature = "musl-ext2-wayland-smoke"),
    not(feature = "musl-ext2-wayland-tests"),
    not(feature = "musl-ext2-micropython-smoke"),
    not(feature = "musl-ext2-quickjs-smoke"),
    not(feature = "musl-ext2-ping-smoke"),
    not(feature = "musl-ext2-shared-buffer-smoke"),
    not(feature = "musl-ext2-epoll-smoke"),
    not(feature = "musl-ext2-console-first-smoke"),
    not(feature = "musl-ext2-gui-smoke"),
    not(feature = "musl-ext2-gui-demo"),
    not(feature = "musl-ext2-peet-gui-smoke"),
    not(feature = "musl-ext2-peet-shell-demo"),
    feature = "musl-ext2-unix-rights-smoke"
))]
static INIT_ELF: &[u8] = include_bytes!("../../user/musl_execve_ext2_unix_rights_smoke.elf");
#[cfg(all(
    not(feature = "musl-ext2-dropbear-smoke"),
    not(feature = "musl-ext2-fpu-smoke"),
    not(feature = "musl-ext2-proc-smoke"),
    not(feature = "musl-ext2-wayland-smoke"),
    not(feature = "musl-ext2-wayland-tests"),
    not(feature = "musl-ext2-micropython-smoke"),
    not(feature = "musl-ext2-quickjs-smoke"),
    not(feature = "musl-ext2-ping-smoke"),
    not(feature = "musl-ext2-shared-buffer-smoke"),
    not(feature = "musl-ext2-epoll-smoke"),
    not(feature = "musl-ext2-console-first-smoke"),
    not(feature = "musl-ext2-gui-smoke"),
    not(feature = "musl-ext2-gui-demo"),
    not(feature = "musl-ext2-peet-gui-smoke"),
    not(feature = "musl-ext2-peet-shell-demo"),
    not(feature = "musl-ext2-unix-rights-smoke"),
    feature = "musl-ext2-cmd-smoke"
))]
static INIT_ELF: &[u8] = include_bytes!("../../user/musl_execve_ext2_smoke.elf");
#[cfg(all(
    not(feature = "musl-ext2-micropython-smoke"),
    not(feature = "musl-ext2-dropbear-smoke"),
    not(feature = "musl-ext2-fpu-smoke"),
    not(feature = "musl-ext2-proc-smoke"),
    not(feature = "musl-ext2-wayland-smoke"),
    not(feature = "musl-ext2-wayland-tests"),
    not(feature = "musl-ext2-quickjs-smoke"),
    not(feature = "musl-ext2-ping-smoke"),
    not(feature = "musl-ext2-shared-buffer-smoke"),
    not(feature = "musl-ext2-epoll-smoke"),
    not(feature = "musl-ext2-console-first-smoke"),
    not(feature = "musl-ext2-gui-smoke"),
    not(feature = "musl-ext2-gui-demo"),
    not(feature = "musl-ext2-peet-gui-smoke"),
    not(feature = "musl-ext2-peet-shell-demo"),
    not(feature = "musl-ext2-unix-rights-smoke"),
    not(feature = "musl-ext2-cmd-smoke"),
    feature = "musl-ext2-exec"
))]
static INIT_ELF: &[u8] = include_bytes!("../../user/musl_execve_ext2.elf");
#[cfg(all(
    not(feature = "musl-ext2-exec"),
    not(feature = "musl-ext2-cmd-smoke"),
    not(feature = "musl-ext2-ping-smoke"),
    not(feature = "musl-ext2-unix-rights-smoke"),
    not(feature = "musl-ext2-epoll-smoke"),
    not(feature = "musl-ext2-shared-buffer-smoke"),
    not(feature = "musl-ext2-console-first-smoke"),
    not(feature = "musl-ext2-gui-smoke"),
    not(feature = "musl-ext2-gui-demo"),
    not(feature = "musl-ext2-peet-gui-smoke"),
    not(feature = "musl-ext2-peet-shell-demo"),
    not(feature = "musl-ext2-quickjs-smoke"),
    not(feature = "musl-ext2-micropython-smoke"),
    not(feature = "musl-ext2-dropbear-smoke"),
    not(feature = "musl-ext2-fpu-smoke"),
    not(feature = "musl-ext2-proc-smoke"),
    not(feature = "musl-ext2-wayland-smoke"),
    not(feature = "musl-ext2-wayland-tests"),
    feature = "musl-smoke"
))]
static INIT_ELF: &[u8] = include_bytes!("../../user/musl_echo.elf");
#[cfg(all(
    not(feature = "musl-ext2-exec"),
    not(feature = "musl-ext2-cmd-smoke"),
    not(feature = "musl-ext2-ping-smoke"),
    not(feature = "musl-ext2-unix-rights-smoke"),
    not(feature = "musl-ext2-epoll-smoke"),
    not(feature = "musl-ext2-shared-buffer-smoke"),
    not(feature = "musl-ext2-console-first-smoke"),
    not(feature = "musl-ext2-gui-smoke"),
    not(feature = "musl-ext2-gui-demo"),
    not(feature = "musl-ext2-peet-gui-smoke"),
    not(feature = "musl-ext2-peet-shell-demo"),
    not(feature = "musl-ext2-quickjs-smoke"),
    not(feature = "musl-ext2-micropython-smoke"),
    not(feature = "musl-ext2-dropbear-smoke"),
    not(feature = "musl-ext2-fpu-smoke"),
    not(feature = "musl-ext2-proc-smoke"),
    not(feature = "musl-ext2-wayland-smoke"),
    not(feature = "musl-ext2-wayland-tests"),
    not(feature = "musl-smoke")
))]
static INIT_ELF: &[u8] = include_bytes!("../../user/hello_dyn.elf");
static LD_PEETOS_SO: &[u8] = include_bytes!("../../user/ld-peetos.so");

#[derive(Clone, Copy)]
struct UserStackPlan<const ARGC: usize, const ENVC: usize> {
    initial_rsp: u64,
    argc: u64,
    argv_ptr: u64,
    envp_ptr: u64,
    auxv_ptr: u64,
    argv_addrs: [u64; ARGC],
    env_addrs: [u64; ENVC],
}

#[derive(Clone, Copy)]
struct DynamicUserStackPlan {
    initial_rsp: u64,
    argc: u64,
    argv_ptr: u64,
    envp_ptr: u64,
    auxv_ptr: u64,
}

pub struct ExecImageStart {
    pub entry_rip: u64,
    pub stack_ptr: u64,
}

struct OwnedExecImage {
    ptr: *mut u8,
    len: usize,
}

impl OwnedExecImage {
    fn as_slice(&self) -> &[u8] {
        unsafe { core::slice::from_raw_parts(self.ptr as *const u8, self.len) }
    }
}

impl Drop for OwnedExecImage {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            heap::kfree(self.ptr);
            self.ptr = ptr::null_mut();
            self.len = 0;
        }
    }
}

enum ExecImageSource {
    Embedded(&'static [u8]),
    Owned(OwnedExecImage),
}

impl ExecImageSource {
    fn as_slice(&self) -> &[u8] {
        match self {
            ExecImageSource::Embedded(data) => data,
            ExecImageSource::Owned(data) => data.as_slice(),
        }
    }
}

#[inline]
fn align_down(value: u64, align: u64) -> u64 {
    value & !(align - 1)
}

fn cstr_bytes_len(buf: &[u8]) -> Option<usize> {
    for (idx, byte) in buf.iter().enumerate() {
        if *byte == 0 {
            return Some(idx);
        }
    }
    None
}

fn interp_path_bytes(buf: &[u8; ELF_INTERP_MAX]) -> Option<&[u8]> {
    let len = cstr_bytes_len(buf)?;
    if len == 0 {
        return None;
    }
    Some(&buf[..len])
}

fn resolve_interp_image(path: &[u8]) -> Option<&'static [u8]> {
    if path == INIT_INTERP_PATH {
        return Some(LD_PEETOS_SO);
    }
    None
}

fn fs_to_errno(err: i64) -> i64 {
    match err {
        fs::ENOENT => ENOENT,
        fs::EIO => EIO,
        fs::EISDIR => EACCES,
        fs::EINVAL => EINVAL,
        fs::ENOSPC => ENOMEM,
        _ => EIO,
    }
}

fn free_address_space(pml4: PhysAddr) {
    if pml4.as_u64() == 0 {
        return;
    }
    vmm::clear_user_space(pml4);
    vmm::destroy_address_space(pml4);
}

fn map_user_stack(pml4: PhysAddr) -> Option<(u64, u64)> {
    let stack_top = USER_STACK_TOP;
    let stack_bottom = stack_top.checked_sub(USER_STACK_SIZE)?;

    for i in 0..USER_STACK_PAGES {
        let page_phys = pmm::alloc_page();
        if page_phys.as_u64() == 0 {
            return None;
        }

        unsafe {
            ptr::write_bytes(
                page_phys.to_virt().as_u64() as *mut u8,
                0,
                PAGE_SIZE as usize,
            );
        }

        let page_vaddr = stack_bottom + (i as u64 * PAGE_SIZE);
        if vmm::map_page_in(
            pml4,
            VirtAddr::new(page_vaddr),
            page_phys,
            vmm::PTE_PRESENT | vmm::PTE_WRITE | vmm::PTE_USER | vmm::PTE_NO_EXECUTE,
        ) != 0
        {
            return None;
        }
    }

    Some((stack_bottom, stack_top))
}

fn plan_user_stack_dynamic(
    stack_bottom: u64,
    stack_top: u64,
    argv: &[&[u8]],
    env: &[&[u8]],
    aux_count: usize,
    argv_addrs: &mut [u64],
    env_addrs: &mut [u64],
) -> Option<DynamicUserStackPlan> {
    if stack_bottom >= stack_top {
        return None;
    }
    if argv_addrs.len() < argv.len() || env_addrs.len() < env.len() {
        return None;
    }

    let mut sp = stack_top;

    for idx in (0..env.len()).rev() {
        if env[idx].is_empty() || env[idx][env[idx].len() - 1] != 0 {
            return None;
        }
        let len = env[idx].len() as u64;
        sp = sp.checked_sub(len)?;
        if sp < stack_bottom {
            return None;
        }
        env_addrs[idx] = sp;
    }

    for idx in (0..argv.len()).rev() {
        if argv[idx].is_empty() || argv[idx][argv[idx].len() - 1] != 0 {
            return None;
        }
        let len = argv[idx].len() as u64;
        sp = sp.checked_sub(len)?;
        if sp < stack_bottom {
            return None;
        }
        argv_addrs[idx] = sp;
    }

    sp = align_down(sp, 16);
    if sp < stack_bottom {
        return None;
    }

    let aux_qwords = (aux_count + 1).checked_mul(2)?;
    let qwords = 1usize
        .checked_add(argv.len().checked_add(1)?)?
        .checked_add(env.len().checked_add(1)?)?
        .checked_add(aux_qwords)?;

    if (qwords & 1) != 0 {
        sp = sp.checked_sub(size_of::<u64>() as u64)?;
        if sp < stack_bottom {
            return None;
        }
    }

    let table_bytes = (qwords as u64).checked_mul(size_of::<u64>() as u64)?;
    let initial_rsp = sp.checked_sub(table_bytes)?;
    if initial_rsp < stack_bottom {
        return None;
    }

    let mut cursor = initial_rsp.checked_add(size_of::<u64>() as u64)?;
    let argv_ptr = cursor;
    cursor = cursor.checked_add(((argv.len() + 1) * size_of::<u64>()) as u64)?;
    let envp_ptr = cursor;
    cursor = cursor.checked_add(((env.len() + 1) * size_of::<u64>()) as u64)?;
    let auxv_ptr = cursor;
    let aux_bytes = ((aux_count + 1) * 2 * size_of::<u64>()) as u64;
    if auxv_ptr.checked_add(aux_bytes)? > sp {
        return None;
    }

    Some(DynamicUserStackPlan {
        initial_rsp,
        argc: argv.len() as u64,
        argv_ptr,
        envp_ptr,
        auxv_ptr,
    })
}

fn populate_user_stack_dynamic(
    pml4: PhysAddr,
    stack_bottom: u64,
    stack_top: u64,
    argv: &[&[u8]],
    env: &[&[u8]],
    aux: &[(u64, u64)],
    argv_addrs: &mut [u64],
    env_addrs: &mut [u64],
) -> Option<DynamicUserStackPlan> {
    let plan = plan_user_stack_dynamic(
        stack_bottom,
        stack_top,
        argv,
        env,
        aux.len(),
        argv_addrs,
        env_addrs,
    )?;

    for idx in 0..argv.len() {
        if !write_user_bytes(pml4, argv_addrs[idx], argv[idx]) {
            return None;
        }
    }
    for idx in 0..env.len() {
        if !write_user_bytes(pml4, env_addrs[idx], env[idx]) {
            return None;
        }
    }

    if !write_user_u64(pml4, plan.initial_rsp, plan.argc) {
        return None;
    }

    let mut cursor = plan.argv_ptr;
    for ptr in argv_addrs.iter().take(argv.len()) {
        if !write_user_u64(pml4, cursor, *ptr) {
            return None;
        }
        cursor += size_of::<u64>() as u64;
    }
    if !write_user_u64(pml4, cursor, 0) {
        return None;
    }

    cursor = plan.envp_ptr;
    for ptr in env_addrs.iter().take(env.len()) {
        if !write_user_u64(pml4, cursor, *ptr) {
            return None;
        }
        cursor += size_of::<u64>() as u64;
    }
    if !write_user_u64(pml4, cursor, 0) {
        return None;
    }

    cursor = plan.auxv_ptr;
    for (kind, value) in aux {
        if !write_user_u64(pml4, cursor, *kind) {
            return None;
        }
        cursor += size_of::<u64>() as u64;
        if !write_user_u64(pml4, cursor, *value) {
            return None;
        }
        cursor += size_of::<u64>() as u64;
    }
    if !write_user_u64(pml4, cursor, AT_NULL) {
        return None;
    }
    cursor += size_of::<u64>() as u64;
    if !write_user_u64(pml4, cursor, 0) {
        return None;
    }

    Some(plan)
}

fn read_vfs_image(path: &str, max_size: usize) -> Result<OwnedExecImage, i64> {
    let inode = fs::lookup(path).map_err(fs_to_errno)?;
    let stat = fs::stat(inode).map_err(fs_to_errno)?;
    if stat.file_type != fs::FileType::Regular {
        return Err(EACCES);
    }

    let file_len = usize::try_from(stat.size).map_err(|_| EINVAL)?;
    if file_len == 0 || file_len > max_size {
        return Err(EINVAL);
    }

    let ptr = heap::kmalloc(file_len);
    if ptr.is_null() {
        return Err(ENOMEM);
    }

    let image = OwnedExecImage { ptr, len: file_len };
    let dst = unsafe { core::slice::from_raw_parts_mut(image.ptr, image.len) };

    let mut offset = 0usize;
    while offset < dst.len() {
        let read = fs::read(inode, offset as u64, &mut dst[offset..]).map_err(fs_to_errno)?;
        if read == 0 {
            return Err(EIO);
        }
        offset += read;
    }

    Ok(image)
}

fn resolve_exec_interp_image(path: &[u8]) -> Result<ExecImageSource, i64> {
    if let Some(image) = resolve_interp_image(path) {
        return Ok(ExecImageSource::Embedded(image));
    }

    let path = core::str::from_utf8(path).map_err(|_| EINVAL)?;
    let image = read_vfs_image(path, EXEC_MAX_INTERP_IMAGE)?;
    Ok(ExecImageSource::Owned(image))
}

#[inline]
fn auxv_value_slot(auxv_ptr: u64, index: usize) -> Option<u64> {
    auxv_ptr.checked_add((index as u64).checked_mul(16)? + 8)
}

fn write_auxv_value_at_index(pml4: PhysAddr, auxv_ptr: u64, index: usize, value: u64) -> bool {
    let slot = match auxv_value_slot(auxv_ptr, index) {
        Some(slot) => slot,
        None => return false,
    };
    write_user_u64(pml4, slot, value)
}

fn fill_startup_random(buf: &mut [u8; 16]) {
    let mut state = 0x9E37_79B9_7F4A_7C15u64 ^ pit::ticks().wrapping_mul(0xA076_1D64_78BD_642F);
    for chunk in buf.chunks_mut(8) {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        let sample = state.wrapping_mul(0x2545_F491_4F6C_DD1D).to_le_bytes();
        chunk.copy_from_slice(&sample[..chunk.len()]);
    }
}

fn install_auxv_runtime_data(
    pml4: PhysAddr,
    stack_bottom: u64,
    initial_rsp: u64,
    auxv_ptr: u64,
    random_index: usize,
    execfn_index: usize,
    execfn: &[u8],
) -> bool {
    if execfn.is_empty() || execfn[execfn.len() - 1] != 0 {
        return false;
    }

    let mut random = [0u8; 16];
    fill_startup_random(&mut random);

    let random_addr = match initial_rsp.checked_sub(random.len() as u64) {
        Some(addr) => align_down(addr, 16),
        None => return false,
    };
    let execfn_addr = match random_addr.checked_sub(execfn.len() as u64) {
        Some(addr) => addr,
        None => return false,
    };
    if execfn_addr < stack_bottom {
        return false;
    }

    if !write_user_bytes(pml4, random_addr, &random) {
        return false;
    }
    if !write_user_bytes(pml4, execfn_addr, execfn) {
        return false;
    }
    if !write_auxv_value_at_index(pml4, auxv_ptr, random_index, random_addr) {
        return false;
    }
    if !write_auxv_value_at_index(pml4, auxv_ptr, execfn_index, execfn_addr) {
        return false;
    }

    true
}

fn user_range_ok(start: u64, len: usize) -> bool {
    if len == 0 {
        return true;
    }
    if start >= KERNEL_VIRT_BASE {
        return false;
    }
    let end = match start.checked_add((len as u64).saturating_sub(1)) {
        Some(v) => v,
        None => return false,
    };
    end < KERNEL_VIRT_BASE
}

fn write_user_bytes(pml4: PhysAddr, user_dst: u64, src: &[u8]) -> bool {
    if src.is_empty() {
        return true;
    }
    if !user_range_ok(user_dst, src.len()) {
        return false;
    }

    let mut remaining = src.len();
    let mut written = 0usize;
    while remaining > 0 {
        let vaddr = user_dst + written as u64;
        let page_vaddr = vaddr & !(PAGE_SIZE - 1);
        let page_off = (vaddr & (PAGE_SIZE - 1)) as usize;
        let bytes_in_page = cmp::min(remaining, PAGE_SIZE as usize - page_off);

        let page_phys = match vmm::get_physical_in(pml4, VirtAddr::new(page_vaddr)) {
            Some(phys) => phys,
            None => return false,
        };
        let dst_ptr = (PhysAddr::new(page_phys.as_u64() + page_off as u64)
            .to_virt()
            .as_u64()) as *mut u8;

        unsafe {
            ptr::copy_nonoverlapping(src.as_ptr().add(written), dst_ptr, bytes_in_page);
        }

        remaining -= bytes_in_page;
        written += bytes_in_page;
    }

    true
}

fn write_user_u64(pml4: PhysAddr, user_dst: u64, value: u64) -> bool {
    write_user_bytes(pml4, user_dst, &value.to_le_bytes())
}

fn plan_user_stack<const ARGC: usize, const ENVC: usize, const AUXC: usize>(
    stack_bottom: u64,
    stack_top: u64,
    argv: [&[u8]; ARGC],
    env: [&[u8]; ENVC],
) -> Option<UserStackPlan<ARGC, ENVC>> {
    if stack_bottom >= stack_top {
        return None;
    }

    let mut sp = stack_top;
    let mut argv_addrs = [0u64; ARGC];
    let mut env_addrs = [0u64; ENVC];

    for idx in (0..ENVC).rev() {
        if env[idx].is_empty() || env[idx][env[idx].len() - 1] != 0 {
            return None;
        }
        let len = env[idx].len() as u64;
        sp = sp.checked_sub(len)?;
        if sp < stack_bottom {
            return None;
        }
        env_addrs[idx] = sp;
    }

    for idx in (0..ARGC).rev() {
        if argv[idx].is_empty() || argv[idx][argv[idx].len() - 1] != 0 {
            return None;
        }
        let len = argv[idx].len() as u64;
        sp = sp.checked_sub(len)?;
        if sp < stack_bottom {
            return None;
        }
        argv_addrs[idx] = sp;
    }

    sp = align_down(sp, 16);
    if sp < stack_bottom {
        return None;
    }

    let aux_qwords = (AUXC + 1).checked_mul(2)?;
    let qwords = 1usize
        .checked_add(ARGC.checked_add(1)?)?
        .checked_add(ENVC.checked_add(1)?)?
        .checked_add(aux_qwords)?;

    if (qwords & 1) != 0 {
        sp = sp.checked_sub(size_of::<u64>() as u64)?;
        if sp < stack_bottom {
            return None;
        }
    }

    let table_bytes = (qwords as u64).checked_mul(size_of::<u64>() as u64)?;
    let initial_rsp = sp.checked_sub(table_bytes)?;
    if initial_rsp < stack_bottom {
        return None;
    }

    let mut cursor = initial_rsp.checked_add(size_of::<u64>() as u64)?;
    let argv_ptr = cursor;
    cursor = cursor.checked_add(((ARGC + 1) * size_of::<u64>()) as u64)?;
    let envp_ptr = cursor;
    cursor = cursor.checked_add(((ENVC + 1) * size_of::<u64>()) as u64)?;
    let auxv_ptr = cursor;
    let aux_bytes = ((AUXC + 1) * 2 * size_of::<u64>()) as u64;
    if auxv_ptr.checked_add(aux_bytes)? > sp {
        return None;
    }

    Some(UserStackPlan {
        initial_rsp,
        argc: ARGC as u64,
        argv_ptr,
        envp_ptr,
        auxv_ptr,
        argv_addrs,
        env_addrs,
    })
}

fn populate_user_stack<const ARGC: usize, const ENVC: usize, const AUXC: usize>(
    pml4: PhysAddr,
    stack_bottom: u64,
    stack_top: u64,
    argv: [&[u8]; ARGC],
    env: [&[u8]; ENVC],
    aux: [(u64, u64); AUXC],
) -> Option<UserStackPlan<ARGC, ENVC>> {
    let plan = plan_user_stack::<ARGC, ENVC, AUXC>(stack_bottom, stack_top, argv, env)?;

    for idx in 0..ARGC {
        if !write_user_bytes(pml4, plan.argv_addrs[idx], argv[idx]) {
            return None;
        }
    }
    for idx in 0..ENVC {
        if !write_user_bytes(pml4, plan.env_addrs[idx], env[idx]) {
            return None;
        }
    }

    if !write_user_u64(pml4, plan.initial_rsp, plan.argc) {
        return None;
    }

    let mut cursor = plan.argv_ptr;
    for ptr in plan.argv_addrs {
        if !write_user_u64(pml4, cursor, ptr) {
            return None;
        }
        cursor += size_of::<u64>() as u64;
    }
    if !write_user_u64(pml4, cursor, 0) {
        return None;
    }

    cursor = plan.envp_ptr;
    for ptr in plan.env_addrs {
        if !write_user_u64(pml4, cursor, ptr) {
            return None;
        }
        cursor += size_of::<u64>() as u64;
    }
    if !write_user_u64(pml4, cursor, 0) {
        return None;
    }

    cursor = plan.auxv_ptr;
    for (kind, value) in aux {
        if !write_user_u64(pml4, cursor, kind) {
            return None;
        }
        cursor += size_of::<u64>() as u64;
        if !write_user_u64(pml4, cursor, value) {
            return None;
        }
        cursor += size_of::<u64>() as u64;
    }
    if !write_user_u64(pml4, cursor, AT_NULL) {
        return None;
    }
    cursor += size_of::<u64>() as u64;
    if !write_user_u64(pml4, cursor, 0) {
        return None;
    }

    Some(plan)
}

pub fn spawn_init() -> bool {
    serial_println!("[USER] Setting up initial user task...");
    if cfg!(feature = "musl-ext2-proc-smoke") {
        serial_println!("[USER] musl-ext2-proc-smoke init image selected");
    } else if cfg!(feature = "musl-ext2-dropbear-smoke") {
        serial_println!("[USER] musl-ext2-dropbear-smoke init image selected");
    } else if cfg!(feature = "musl-ext2-micropython-smoke") {
        serial_println!("[USER] musl-ext2-micropython-smoke init image selected");
    } else if cfg!(feature = "musl-ext2-quickjs-smoke") {
        serial_println!("[USER] musl-ext2-quickjs-smoke init image selected");
    } else if cfg!(feature = "musl-ext2-ping-smoke") {
        serial_println!("[USER] musl-ext2-ping-smoke init image selected");
    } else if cfg!(feature = "musl-ext2-shared-buffer-smoke") {
        serial_println!("[USER] musl-ext2-shared-buffer-smoke init image selected");
    } else if cfg!(feature = "musl-ext2-epoll-smoke") {
        serial_println!("[USER] musl-ext2-epoll-smoke init image selected");
    } else if cfg!(feature = "musl-ext2-console-first-smoke") {
        serial_println!("[USER] musl-ext2-console-first-smoke init image selected");
    } else if cfg!(feature = "musl-ext2-gui-smoke") {
        serial_println!("[USER] musl-ext2-gui-smoke init image selected");
    } else if cfg!(feature = "musl-ext2-peet-gui-smoke") {
        serial_println!("[USER] musl-ext2-peet-gui-smoke init image selected");
    } else if cfg!(feature = "musl-ext2-unix-rights-smoke") {
        serial_println!("[USER] musl-ext2-unix-rights-smoke init image selected");
    } else if cfg!(feature = "musl-ext2-cmd-smoke") {
        serial_println!("[USER] musl-ext2-cmd-smoke init image selected");
    } else if cfg!(feature = "musl-ext2-exec") {
        serial_println!("[USER] musl-ext2-exec init image selected");
    } else if cfg!(feature = "musl-smoke") {
        serial_println!("[USER] musl-smoke init image selected");
    }

    let pml4 = vmm::create_address_space();
    if pml4.as_u64() == 0 {
        serial_println!("[USER] ERROR: failed to create address space");
        return false;
    }

    let mut elf_result = if elf::is_dynamic(INIT_ELF) {
        elf::load_at(INIT_ELF, pml4, INIT_LOAD_BASE)
    } else {
        elf::load(INIT_ELF, pml4)
    };
    if !elf_result.success {
        serial_println!("[USER] ERROR: failed to load init ELF");
        free_address_space(pml4);
        return false;
    }

    let mut entry_rip = elf_result.entry_point;
    let mut interp_base = 0u64;
    if let Some(path) = interp_path_bytes(&elf_result.interp_path) {
        let interp_image = match resolve_interp_image(path) {
            Some(image) => image,
            None => {
                serial_println!("[USER] ERROR: unsupported PT_INTERP path");
                free_address_space(pml4);
                return false;
            }
        };

        let interp_result = elf::load_at(interp_image, pml4, INIT_INTERP_BASE);
        if !interp_result.success {
            serial_println!("[USER] ERROR: failed to load PT_INTERP image");
            free_address_space(pml4);
            return false;
        }

        interp_base = interp_result.base_addr;
        entry_rip = interp_result.entry_point;
    }
    elf_result.interp_base = interp_base;

    let (stack_bottom, stack_top) = match map_user_stack(pml4) {
        Some(range) => range,
        None => {
            serial_println!("[USER] ERROR: failed to allocate/map user stack");
            free_address_space(pml4);
            return false;
        }
    };

    const AUX_IDX_RANDOM: usize = 11;
    const AUX_IDX_EXECFN: usize = 12;
    let auxv = [
        (AT_PHDR, elf_result.phdr_addr),
        (AT_PHENT, elf_result.phdr_size as u64),
        (AT_PHNUM, elf_result.phdr_count as u64),
        (AT_PAGESZ, PAGE_SIZE),
        (AT_BASE, interp_base),
        (AT_ENTRY, elf_result.entry_point),
        (AT_UID, 0),
        (AT_EUID, 0),
        (AT_GID, 0),
        (AT_EGID, 0),
        (AT_SECURE, 0),
        (AT_RANDOM, 0),
        (AT_EXECFN, 0),
    ];
    #[cfg(all(
        feature = "musl-smoke",
        not(feature = "musl-ext2-exec"),
        not(feature = "musl-ext2-cmd-smoke"),
        not(feature = "musl-ext2-ping-smoke"),
        not(feature = "musl-ext2-unix-rights-smoke"),
        not(feature = "musl-ext2-epoll-smoke"),
        not(feature = "musl-ext2-shared-buffer-smoke"),
        not(feature = "musl-ext2-quickjs-smoke"),
        not(feature = "musl-ext2-micropython-smoke"),
        not(feature = "musl-ext2-dropbear-smoke")
    ))]
    let stack_plan = match populate_user_stack::<3, 2, 13>(
        pml4,
        stack_bottom,
        stack_top,
        [INIT_ARGV0, b"hello\0", b"from-musl\0"],
        [INIT_ENV_PATH, INIT_ENV_TERM],
        auxv,
    ) {
        Some(plan) => plan,
        None => {
            serial_println!("[USER] ERROR: failed to build user stack");
            free_address_space(pml4);
            return false;
        }
    };
    #[cfg(any(
        feature = "musl-ext2-exec",
        feature = "musl-ext2-cmd-smoke",
        feature = "musl-ext2-ping-smoke",
        feature = "musl-ext2-unix-rights-smoke",
        feature = "musl-ext2-epoll-smoke",
        feature = "musl-ext2-shared-buffer-smoke",
        feature = "musl-ext2-console-first-smoke",
        feature = "musl-ext2-gui-smoke",
        feature = "musl-ext2-gui-demo",
        feature = "musl-ext2-quickjs-smoke",
        feature = "musl-ext2-micropython-smoke",
        feature = "musl-ext2-dropbear-smoke",
        not(feature = "musl-smoke")
    ))]
    let stack_plan = match populate_user_stack::<1, 2, 13>(
        pml4,
        stack_bottom,
        stack_top,
        [INIT_ARGV0],
        [INIT_ENV_PATH, INIT_ENV_TERM],
        auxv,
    ) {
        Some(plan) => plan,
        None => {
            serial_println!("[USER] ERROR: failed to build user stack");
            free_address_space(pml4);
            return false;
        }
    };
    if !install_auxv_runtime_data(
        pml4,
        stack_bottom,
        stack_plan.initial_rsp,
        stack_plan.auxv_ptr,
        AUX_IDX_RANDOM,
        AUX_IDX_EXECFN,
        INIT_ARGV0,
    ) {
        serial_println!("[USER] ERROR: failed to finalize user auxv");
        free_address_space(pml4);
        return false;
    }

    let task_ptr = task::task_alloc("init", TASK_FLAG_USER);
    if task_ptr.is_null() {
        serial_println!("[USER] ERROR: failed to allocate task");
        free_address_space(pml4);
        return false;
    }

    unsafe {
        (*task_ptr).pml4_phys = pml4.as_u64();
        (*crate::task::leader_of(task_ptr)).user_entry = elf_result.entry_point;
        (*crate::task::leader_of(task_ptr)).user_stack_bottom = stack_bottom;
        (*crate::task::leader_of(task_ptr)).user_stack_top = stack_top;
        (*crate::task::leader_of(task_ptr)).user_stack_ptr = stack_plan.initial_rsp;
        (*crate::task::leader_of(task_ptr)).user_argc = stack_plan.argc;
        (*crate::task::leader_of(task_ptr)).user_argv = stack_plan.argv_ptr;
        (*crate::task::leader_of(task_ptr)).user_envp = stack_plan.envp_ptr;
        (*crate::task::leader_of(task_ptr)).user_auxv = stack_plan.auxv_ptr;
        (*task_ptr).user_clear_tid = 0;
        (*crate::task::leader_of(task_ptr)).user_brk_start = elf_result.program_break;
        (*crate::task::leader_of(task_ptr)).user_brk = elf_result.program_break;
        (*crate::task::leader_of(task_ptr)).user_phdr_addr = elf_result.phdr_addr;
        (*crate::task::leader_of(task_ptr)).user_phdr_count = elf_result.phdr_count;
        (*crate::task::leader_of(task_ptr)).user_phdr_size = elf_result.phdr_size;
        (*crate::task::leader_of(task_ptr)).user_base_addr = elf_result.base_addr;
        (*crate::task::leader_of(task_ptr)).user_interp_base = interp_base;
        (*crate::task::leader_of(task_ptr)).user_is_dynamic = elf_result.is_dynamic;
        (*crate::task::leader_of(task_ptr)).ctty_type = crate::task::CTTY_CONSOLE;
        (*crate::task::leader_of(task_ptr)).ctty_id = 0;

        let stack_top = (*task_ptr).kernel_stack_top;
        let mut frame_addr = stack_top - size_of::<InterruptFrame>() as u64;
        frame_addr &= !0xF;

        let frame = frame_addr as *mut InterruptFrame;
        ptr::write_bytes(frame as *mut u8, 0, size_of::<InterruptFrame>());

        (*frame).ss = gdt::USER_DS as u64;
        (*frame).rsp = stack_plan.initial_rsp;
        (*frame).rflags = 0x202;
        (*frame).cs = gdt::USER_CS as u64;
        (*frame).rip = entry_rip;
        (*frame).int_no = 0;
        (*frame).error_code = 0;

        (*task_ptr).saved_rsp = frame_addr;
        (*task_ptr).state = TaskState::Ready;
    }

    INIT_PID.store(unsafe { (*task_ptr).pid }, Ordering::Release);

    task::user_stdio_init(task_ptr);

    crate::sched::add_task(task_ptr);

    serial_println!(
        "[USER] User task ready (entry=0x{:x}, start=0x{:x}, rsp=0x{:x})",
        elf_result.entry_point,
        entry_rip,
        stack_plan.initial_rsp
    );
    true
}

fn init_pid() -> task::Pid {
    INIT_PID.load(Ordering::Acquire)
}

fn exiting_pid_needs_console_fallback(pid: task::Pid) -> bool {
    pid != task::INVALID_PID
        && pid == init_pid()
        && crate::display_mode::compositor_active()
}

pub fn maybe_restore_console_after_init_exit(task_ptr: *mut task::Task, exit_code: i32) {
    if task_ptr.is_null() {
        return;
    }

    let pid = unsafe { (*task_ptr).pid };
    if !exiting_pid_needs_console_fallback(pid) {
        return;
    }

    crate::display_mode::activate_console();
    crate::fb_console::set_active(true, true);
    crate::mouse::sync_display_owner();
    INIT_PID.store(task::INVALID_PID, Ordering::Release);
    serial_println!(
        "[GUI] Compositor-owned init exited (pid={}, code={}); framebuffer ownership returned to console",
        pid,
        exit_code
    );
}

pub fn exec_current_image(
    image: &[u8],
    argv: &[&[u8]],
    env: &[&[u8]],
    execfn: &[u8],
) -> Result<ExecImageStart, i64> {
    if argv.len() > EXEC_MAX_ARGV || env.len() > EXEC_MAX_ENVP {
        return Err(-E2BIG);
    }
    if execfn.is_empty() || execfn[execfn.len() - 1] != 0 {
        return Err(-EINVAL);
    }

    let current = task::current();
    if current.is_null() {
        return Err(-EINVAL);
    }
    if unsafe { ((*current).flags & TASK_FLAG_USER) == 0 } {
        return Err(-EINVAL);
    }

    let new_pml4 = vmm::create_address_space();
    if new_pml4.as_u64() == 0 {
        return Err(-ENOMEM);
    }

    let mut elf_result = if elf::is_dynamic(image) {
        elf::load_at(image, new_pml4, INIT_LOAD_BASE)
    } else {
        elf::load(image, new_pml4)
    };
    if !elf_result.success {
        free_address_space(new_pml4);
        return Err(-EINVAL);
    }

    let mut entry_rip = elf_result.entry_point;
    let mut interp_base = 0u64;
    if let Some(path) = interp_path_bytes(&elf_result.interp_path) {
        let interp_image = match resolve_exec_interp_image(path) {
            Ok(image) => image,
            Err(err) => {
                free_address_space(new_pml4);
                return Err(-err);
            }
        };
        let interp_result = elf::load_at(interp_image.as_slice(), new_pml4, INIT_INTERP_BASE);
        if !interp_result.success {
            free_address_space(new_pml4);
            return Err(-EINVAL);
        }
        interp_base = interp_result.base_addr;
        entry_rip = interp_result.entry_point;
    }
    elf_result.interp_base = interp_base;

    let (stack_bottom, stack_top) = match map_user_stack(new_pml4) {
        Some(range) => range,
        None => {
            free_address_space(new_pml4);
            return Err(-ENOMEM);
        }
    };

    let (uid, gid) = unsafe { ((*crate::task::leader_of(current)).user_uid as u64, (*crate::task::leader_of(current)).user_gid as u64) };
    const AUX_IDX_RANDOM: usize = 11;
    const AUX_IDX_EXECFN: usize = 12;
    let auxv = [
        (AT_PHDR, elf_result.phdr_addr),
        (AT_PHENT, elf_result.phdr_size as u64),
        (AT_PHNUM, elf_result.phdr_count as u64),
        (AT_PAGESZ, PAGE_SIZE),
        (AT_BASE, interp_base),
        (AT_ENTRY, elf_result.entry_point),
        (AT_UID, uid),
        (AT_EUID, uid),
        (AT_GID, gid),
        (AT_EGID, gid),
        (AT_SECURE, 0),
        (AT_RANDOM, 0),
        (AT_EXECFN, 0),
    ];

    let mut argv_addrs = [0u64; EXEC_MAX_ARGV];
    let mut env_addrs = [0u64; EXEC_MAX_ENVP];
    let stack_plan = match populate_user_stack_dynamic(
        new_pml4,
        stack_bottom,
        stack_top,
        argv,
        env,
        &auxv,
        &mut argv_addrs,
        &mut env_addrs,
    ) {
        Some(plan) => plan,
        None => {
            free_address_space(new_pml4);
            return Err(-EINVAL);
        }
    };
    if !install_auxv_runtime_data(
        new_pml4,
        stack_bottom,
        stack_plan.initial_rsp,
        stack_plan.auxv_ptr,
        AUX_IDX_RANDOM,
        AUX_IDX_EXECFN,
        execfn,
    ) {
        free_address_space(new_pml4);
        return Err(-EINVAL);
    }

    let old_pml4 = unsafe { PhysAddr::new((*current).pml4_phys) };

    shm::cleanup_task(current);

    unsafe {
        (*current).pml4_phys = new_pml4.as_u64();
        (*crate::task::leader_of(current)).user_entry = elf_result.entry_point;
        (*crate::task::leader_of(current)).user_stack_bottom = stack_bottom;
        (*crate::task::leader_of(current)).user_stack_top = stack_top;
        (*crate::task::leader_of(current)).user_stack_ptr = stack_plan.initial_rsp;
        (*crate::task::leader_of(current)).user_argc = stack_plan.argc;
        (*crate::task::leader_of(current)).user_argv = stack_plan.argv_ptr;
        (*crate::task::leader_of(current)).user_envp = stack_plan.envp_ptr;
        (*crate::task::leader_of(current)).user_auxv = stack_plan.auxv_ptr;
        (*current).user_fs_base = 0;
        (*current).user_clear_tid = 0;
        (*crate::task::leader_of(current)).user_brk_start = elf_result.program_break;
        (*crate::task::leader_of(current)).user_brk = elf_result.program_break;
        (*crate::task::leader_of(current)).user_phdr_addr = elf_result.phdr_addr;
        (*crate::task::leader_of(current)).user_phdr_count = elf_result.phdr_count;
        (*crate::task::leader_of(current)).user_phdr_size = elf_result.phdr_size;
        (*crate::task::leader_of(current)).user_base_addr = elf_result.base_addr;
        (*crate::task::leader_of(current)).user_interp_base = interp_base;
        (*crate::task::leader_of(current)).user_is_dynamic = elf_result.is_dynamic;
        (*current).sig_pending = 0;
        (*crate::task::leader_of(current)).signal_handlers = [0; task::SIG_MAX_TASK_SIGNALS];
        (*crate::task::leader_of(current)).signal_flags = [0; task::SIG_MAX_TASK_SIGNALS];
        (*crate::task::leader_of(current)).signal_mask_saved = [0; task::SIG_MAX_TASK_SIGNALS];
        (*crate::task::leader_of(current)).signal_restorer = 0;
    }

    vmm::switch(new_pml4);
    arch_syscall::set_user_fs_base(0);
    // A new image starts from the ABI-default FPU/SSE state, not the
    // pre-exec image's registers.
    arch_x86_64::fpu::reset_current();
    if old_pml4.as_u64() != 0 && old_pml4 != new_pml4 {
        free_address_space(old_pml4);
    }

    Ok(ExecImageStart {
        entry_rip,
        stack_ptr: stack_plan.initial_rsp,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_user_stack_aligns_rsp_and_tables() {
        let plan = plan_user_stack::<1, 2, 6>(
            0x7fff_0000_0000,
            0x7fff_0000_4000,
            [b"/init\0"],
            [b"PATH=/bin\0", b"TERM=peetos\0"],
        )
        .expect("stack plan should fit");

        assert_eq!(plan.argc, 1);
        assert_eq!(plan.initial_rsp & 0xF, 0);
        assert!(plan.argv_addrs[0] < 0x7fff_0000_4000);
        assert_eq!(plan.envp_ptr - plan.argv_ptr, 16);
        assert_eq!(plan.auxv_ptr - plan.envp_ptr, 24);
    }

    #[test]
    fn plan_user_stack_rejects_non_nul_strings() {
        assert!(
            plan_user_stack::<1, 0, 1>(0x1000, 0x4000, [b"/init"], []).is_none(),
            "argv entry must be NUL-terminated"
        );
    }

    #[test]
    fn plan_user_stack_rejects_tiny_stack() {
        assert!(plan_user_stack::<1, 2, 6>(
            0x2000,
            0x2050,
            [b"/init\0"],
            [b"PATH=/bin\0", b"TERM=peetos\0"],
        )
        .is_none());
    }

    #[test]
    fn interp_path_extracts_expected_bytes() {
        let mut raw = [0u8; ELF_INTERP_MAX];
        raw[..INIT_INTERP_PATH.len()].copy_from_slice(INIT_INTERP_PATH);
        raw[INIT_INTERP_PATH.len()] = 0;

        let path = interp_path_bytes(&raw).expect("path should parse");
        assert_eq!(path, INIT_INTERP_PATH);
    }

    #[test]
    fn interp_path_empty_returns_none() {
        let raw = [0u8; ELF_INTERP_MAX];
        assert!(interp_path_bytes(&raw).is_none());
    }

    #[test]
    fn resolve_interp_image_matches_known_path() {
        assert!(resolve_interp_image(INIT_INTERP_PATH).is_some());
        assert!(resolve_interp_image(b"/bad/interp").is_none());
    }

    #[test]
    fn dynamic_stack_plan_accepts_runtime_vectors() {
        let argv: [&[u8]; 2] = [b"/bin/sh\0", b"-c\0"];
        let env: [&[u8]; 1] = [b"PATH=/bin\0"];
        let mut argv_addrs = [0u64; 4];
        let mut env_addrs = [0u64; 4];

        let plan = plan_user_stack_dynamic(
            0x7fff_0000_0000,
            0x7fff_0000_4000,
            &argv,
            &env,
            2,
            &mut argv_addrs,
            &mut env_addrs,
        )
        .expect("dynamic stack plan should fit");

        assert_eq!(plan.argc, 2);
        assert_eq!(plan.initial_rsp & 0xF, 0);
        assert!(argv_addrs[0] < 0x7fff_0000_4000);
        assert!(env_addrs[0] < 0x7fff_0000_4000);
    }

    #[test]
    fn dynamic_stack_plan_rejects_non_nul_strings() {
        let argv: [&[u8]; 1] = [b"/bin/sh"];
        let env: [&[u8]; 0] = [];
        let mut argv_addrs = [0u64; 2];
        let mut env_addrs = [0u64; 1];

        assert!(plan_user_stack_dynamic(
            0x7fff_0000_0000,
            0x7fff_0000_2000,
            &argv,
            &env,
            1,
            &mut argv_addrs,
            &mut env_addrs,
        )
        .is_none());
    }

    #[test]
    fn auxv_value_slot_tracks_value_word_offsets() {
        assert_eq!(auxv_value_slot(0x4000, 0), Some(0x4008));
        assert_eq!(auxv_value_slot(0x4000, 1), Some(0x4018));
        assert_eq!(auxv_value_slot(0x4000, 2), Some(0x4028));
    }

    #[test]
    fn auxv_value_slot_rejects_overflow() {
        assert_eq!(auxv_value_slot(u64::MAX - 8, 0), Some(u64::MAX));
        assert!(auxv_value_slot(u64::MAX - 7, 0).is_none());
        assert!(auxv_value_slot(0x1000, usize::MAX).is_none());
    }

    #[test]
    fn startup_random_fills_nonzero_bytes() {
        let mut random = [0u8; 16];
        fill_startup_random(&mut random);
        assert!(random.iter().any(|b| *b != 0));
        assert_ne!(&random[..8], &random[8..]);
    }

    #[test]
    fn console_fallback_only_triggers_for_compositor_owned_init() {
        INIT_PID.store(42, Ordering::Release);
        crate::display_mode::activate_console();
        assert!(!exiting_pid_needs_console_fallback(42));

        crate::display_mode::activate_compositor();
        assert!(exiting_pid_needs_console_fallback(42));
        assert!(!exiting_pid_needs_console_fallback(7));

        INIT_PID.store(task::INVALID_PID, Ordering::Release);
        assert!(!exiting_pid_needs_console_fallback(42));
        crate::display_mode::activate_console();
    }
}
