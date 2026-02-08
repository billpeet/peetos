use core::mem::size_of;
use core::ptr;

use arch_x86_64::addr::{VirtAddr, PAGE_SIZE};
use arch_x86_64::gdt;
use arch_x86_64::idt::InterruptFrame;

use crate::elf;
use crate::memory::{pmm, vmm};
use crate::serial_println;
use crate::task::{self, TaskState, TASK_FLAG_USER};

const USER_STACK_TOP: u64 = 0x7FFF_FF00_0000;
const USER_STACK_SIZE: u64 = 32 * 1024;
const USER_STACK_PAGES: usize = (USER_STACK_SIZE / PAGE_SIZE) as usize;

static HELLO_ELF: &[u8] = include_bytes!("../../user/hello.elf");

pub fn spawn_init() {
    serial_println!("[USER] Setting up initial user task...");

    let pml4 = vmm::create_address_space();
    if pml4.as_u64() == 0 {
        serial_println!("[USER] ERROR: failed to create address space");
        return;
    }

    let elf_result = elf::load(HELLO_ELF, pml4);
    if !elf_result.success {
        serial_println!("[USER] ERROR: failed to load ELF");
        vmm::destroy_address_space(pml4);
        return;
    }

    let stack_bottom = USER_STACK_TOP - USER_STACK_SIZE;
    for i in 0..USER_STACK_PAGES {
        let page_phys = pmm::alloc_page();
        if page_phys.as_u64() == 0 {
            serial_println!("[USER] ERROR: failed to allocate user stack");
            vmm::destroy_address_space(pml4);
            return;
        }

        unsafe {
            ptr::write_bytes(page_phys.to_virt().as_u64() as *mut u8, 0, PAGE_SIZE as usize);
        }

        let page_vaddr = stack_bottom + (i as u64 * PAGE_SIZE);
        if vmm::map_page_in(
            pml4,
            VirtAddr::new(page_vaddr),
            page_phys,
            vmm::PTE_PRESENT | vmm::PTE_WRITE | vmm::PTE_USER,
        ) != 0
        {
            serial_println!("[USER] ERROR: failed to map user stack");
            vmm::destroy_address_space(pml4);
            return;
        }
    }

    let task_ptr = task::task_alloc("init", TASK_FLAG_USER);
    if task_ptr.is_null() {
        serial_println!("[USER] ERROR: failed to allocate task");
        vmm::destroy_address_space(pml4);
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
        (*frame).rsp = USER_STACK_TOP - 8;
        (*frame).rflags = 0x202;
        (*frame).cs = gdt::USER_CS as u64;
        (*frame).rip = elf_result.entry_point;
        (*frame).int_no = 0;
        (*frame).error_code = 0;

        (*task_ptr).saved_rsp = frame_addr;
        (*task_ptr).state = TaskState::Ready;
    }

    crate::sched::add_task(task_ptr);

    serial_println!("[USER] User task ready (entry=0x{:x})", elf_result.entry_point);
}
