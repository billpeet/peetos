use core::mem::size_of;
use core::ptr;

use arch_x86_64::addr::{VirtAddr, PAGE_SIZE};
use arch_x86_64::gdt;
use arch_x86_64::idt::InterruptFrame;

use crate::memory::{pmm, vmm};
use crate::serial_println;
use crate::task::{self, TaskState, TASK_FLAG_USER};

const USER_CODE_ADDR: u64 = 0x0000_0000_0040_0000;
const USER_STACK_TOP: u64 = 0x0000_0000_0080_0000;

const USER_PROG: &[u8] = &[
    0x48, 0xC7, 0xC0, 0x01, 0x00, 0x00, 0x00, // mov rax, 1 (SYS_EXIT)
    0x48, 0xC7, 0xC7, 0x2A, 0x00, 0x00, 0x00, // mov rdi, 42
    0xCD, 0x80, // int 0x80
    0xF4,       // hlt
    0xEB, 0xFE, // jmp .
];

pub fn spawn_init() {
    serial_println!("[USER] Setting up initial user task...");

    let pml4 = vmm::create_address_space();
    if pml4.as_u64() == 0 {
        serial_println!("[USER] ERROR: failed to create address space");
        return;
    }

    let code_phys = pmm::alloc_page();
    if code_phys.as_u64() == 0 {
        serial_println!("[USER] ERROR: failed to allocate code page");
        return;
    }

    unsafe {
        let code_ptr = code_phys.to_virt().as_u64() as *mut u8;
        ptr::write_bytes(code_ptr, 0, PAGE_SIZE as usize);
        ptr::copy_nonoverlapping(USER_PROG.as_ptr(), code_ptr, USER_PROG.len());
    }

    if vmm::map_page_in(pml4, VirtAddr::new(USER_CODE_ADDR), code_phys, vmm::VMM_FLAG_USER_RO) != 0 {
        serial_println!("[USER] ERROR: failed to map code page");
        return;
    }

    let stack_phys = pmm::alloc_page();
    if stack_phys.as_u64() == 0 {
        serial_println!("[USER] ERROR: failed to allocate stack page");
        return;
    }

    let stack_base = USER_STACK_TOP - PAGE_SIZE;
    if vmm::map_page_in(pml4, VirtAddr::new(stack_base), stack_phys, vmm::VMM_FLAG_USER_RW) != 0 {
        serial_println!("[USER] ERROR: failed to map stack page");
        return;
    }

    let task_ptr = task::task_alloc("init", TASK_FLAG_USER);
    if task_ptr.is_null() {
        serial_println!("[USER] ERROR: failed to allocate task");
        return;
    }

    unsafe {
        (*task_ptr).pml4_phys = pml4.as_u64();

        let stack_top = (*task_ptr).kernel_stack_top;
        let mut frame_addr = stack_top - size_of::<InterruptFrame>() as u64;
        frame_addr &= !0xF;

        let frame = frame_addr as *mut InterruptFrame;
        ptr::write_bytes(frame as *mut u8, 0, size_of::<InterruptFrame>());

        (*frame).ss = gdt::USER_DS as u64;
        (*frame).rsp = USER_STACK_TOP - 16;
        (*frame).rflags = 0x202;
        (*frame).cs = gdt::USER_CS as u64;
        (*frame).rip = USER_CODE_ADDR;
        (*frame).int_no = 0;
        (*frame).error_code = 0;

        (*task_ptr).saved_rsp = frame_addr;
        (*task_ptr).state = TaskState::Ready;
    }

    crate::sched::add_task(task_ptr);

    serial_println!("[USER] User task ready");
}
