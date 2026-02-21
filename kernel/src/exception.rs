use core::ptr;

use arch_x86_64::idt;

use crate::serial_println;

pub fn handle(frame: &mut idt::InterruptFrame) -> bool {
    let int_no = read_u64(ptr::addr_of!(frame.int_no)) as u8;
    if int_no != idt::EXCEPTION_PAGE_FAULT {
        return false;
    }

    let error_code = read_u64(ptr::addr_of!(frame.error_code));
    let fault_addr = read_cr2();
    if crate::syscall::try_handle_page_fault(fault_addr, error_code) {
        return true;
    }
    crate::syscall::terminate_current_user_fault(error_code, fault_addr)
}

pub fn report(frame: &idt::InterruptFrame) {
    let int_no = read_u64(ptr::addr_of!(frame.int_no));
    let error_code = read_u64(ptr::addr_of!(frame.error_code));
    let rip = read_u64(ptr::addr_of!(frame.rip));
    let cs = read_u64(ptr::addr_of!(frame.cs));
    let rflags = read_u64(ptr::addr_of!(frame.rflags));
    let rsp = read_u64(ptr::addr_of!(frame.rsp));
    let ss = read_u64(ptr::addr_of!(frame.ss));

    let rax = read_u64(ptr::addr_of!(frame.rax));
    let rbx = read_u64(ptr::addr_of!(frame.rbx));
    let rcx = read_u64(ptr::addr_of!(frame.rcx));
    let rdx = read_u64(ptr::addr_of!(frame.rdx));
    let rsi = read_u64(ptr::addr_of!(frame.rsi));
    let rdi = read_u64(ptr::addr_of!(frame.rdi));
    let rbp = read_u64(ptr::addr_of!(frame.rbp));

    serial_println!("\n=== CPU EXCEPTION ===");
    serial_println!("Vector: {} ({})", int_no, idt::exception_name(int_no as u8));
    serial_println!("Error Code: 0x{:x}", error_code);
    serial_println!(
        "RIP=0x{:016x} CS=0x{:04x} RFLAGS=0x{:016x}",
        rip,
        cs,
        rflags
    );
    serial_println!("RSP=0x{:016x} SS=0x{:04x}", rsp, ss);
    serial_println!("RAX=0x{:016x} RBX=0x{:016x}", rax, rbx);
    serial_println!("RCX=0x{:016x} RDX=0x{:016x}", rcx, rdx);
    serial_println!("RSI=0x{:016x} RDI=0x{:016x}", rsi, rdi);
    serial_println!("RBP=0x{:016x}", rbp);

    if int_no as u8 == idt::EXCEPTION_PAGE_FAULT {
        let cr2 = read_cr2();
        serial_println!("CR2 (fault addr): 0x{:016x}", cr2);
        let reason = decode_page_fault(error_code);
        serial_println!("PF: {}", reason.summary);
        for line in reason.details {
            serial_println!("  - {}", line);
        }
    }

    #[cfg(feature = "vga")]
    {
        crate::vga::print_exception(int_no, error_code, rip, rsp);
    }
}

struct PageFaultReason {
    summary: &'static str,
    details: [&'static str; 4],
}

fn decode_page_fault(error_code: u64) -> PageFaultReason {
    let present = (error_code & 0x1) != 0;
    let write = (error_code & 0x2) != 0;
    let user = (error_code & 0x4) != 0;
    let reserved = (error_code & 0x8) != 0;
    let instruction = (error_code & 0x10) != 0;

    let summary = if present {
        "protection violation"
    } else {
        "non-present page"
    };
    let details = [
        if write { "write" } else { "read" },
        if user { "user-mode" } else { "kernel-mode" },
        if reserved {
            "reserved-bit set"
        } else {
            "reserved-bit clear"
        },
        if instruction {
            "instruction fetch"
        } else {
            "data access"
        },
    ];

    PageFaultReason { summary, details }
}

#[cfg(target_os = "none")]
fn read_cr2() -> u64 {
    let value: u64;
    unsafe {
        core::arch::asm!("mov {}, cr2", out(reg) value, options(nomem, nostack, preserves_flags));
    }
    value
}

#[cfg(not(target_os = "none"))]
fn read_cr2() -> u64 {
    0
}

#[inline]
fn read_u64(ptr: *const u64) -> u64 {
    unsafe { core::ptr::read_unaligned(ptr) }
}
