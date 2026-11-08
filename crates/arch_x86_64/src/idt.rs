#[cfg(target_os = "none")]
use core::arch::asm;
#[cfg(target_os = "none")]
use core::mem::size_of;
use core::ptr;

#[cfg(target_os = "none")]
use crate::interrupts;
#[cfg(target_os = "none")]
use crate::{irq, lapic};

pub const IDT_ENTRIES: usize = 256;

pub const IDT_GATE_INTERRUPT: u8 = 0x8E;
pub const IDT_GATE_TRAP: u8 = 0x8F;
pub const IDT_GATE_USER: u8 = 0xEE;

pub const KERNEL_CODE_SELECTOR: u16 = 0x08;
pub const EXCEPTION_PAGE_FAULT: u8 = 14;
pub const SYSCALL_VECTOR: u8 = 0x80;
/// First vector reserved for inter-processor interrupts (LAPIC-delivered).
pub const IPI_VECTOR_FIRST: u8 = 0xF0;
/// Number of IPI vectors with installed stubs (`0xF0..0xF4`).
pub const IPI_VECTOR_COUNT: u8 = 4;

pub type InterruptHandler = extern "C" fn(frame: &mut InterruptFrame);
pub type PostHandlerHook = fn(frame: &mut InterruptFrame) -> u64;
/// Runs before a vector's normal handler; the kernel takes its big lock here.
pub type EntryHook = fn();
/// Called from assembly with the frame that is about to be restored, after
/// any stack switch. The kernel drops its big lock here when that frame
/// returns to ring 3 or to an idle context.
pub type ExitHook = extern "C" fn(frame: *const InterruptFrame);

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct IdtEntry {
    offset_low: u16,
    selector: u16,
    ist: u8,
    type_attr: u8,
    offset_mid: u16,
    offset_high: u32,
    reserved: u32,
}

impl IdtEntry {
    const fn missing() -> Self {
        Self {
            offset_low: 0,
            selector: 0,
            ist: 0,
            type_attr: 0,
            offset_mid: 0,
            offset_high: 0,
            reserved: 0,
        }
    }
}

#[cfg(any(target_os = "none", test))]
#[repr(C, packed)]
#[derive(Clone, Copy)]
struct Idtr {
    limit: u16,
    base: u64,
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct InterruptFrame {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rbp: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rbx: u64,
    pub rax: u64,
    pub int_no: u64,
    pub error_code: u64,
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

static mut IDT: [IdtEntry; IDT_ENTRIES] = [IdtEntry::missing(); IDT_ENTRIES];
#[cfg(target_os = "none")]
static mut IDTR: Idtr = Idtr { limit: 0, base: 0 };
static mut HANDLERS: [Option<InterruptHandler>; IDT_ENTRIES] = [None; IDT_ENTRIES];
static mut POST_HANDLER_HOOK: Option<PostHandlerHook> = None;
/// Handlers that run before the entry hook and must not need kernel locks.
/// A vector with only a fast handler skips the entry hook, the normal
/// handler table and the post-handler hook entirely.
static mut FAST_HANDLERS: [Option<InterruptHandler>; IDT_ENTRIES] = [None; IDT_ENTRIES];
static mut ENTRY_HOOK: Option<EntryHook> = None;
#[cfg(target_os = "none")]
#[no_mangle]
static mut ARCH_X86_64_EXIT_HOOK: u64 = 0;

static EXCEPTION_NAMES: [&str; 32] = [
    "Divide Error (#DE)",
    "Debug (#DB)",
    "Non-Maskable Interrupt",
    "Breakpoint (#BP)",
    "Overflow (#OF)",
    "Bound Range Exceeded (#BR)",
    "Invalid Opcode (#UD)",
    "Device Not Available (#NM)",
    "Double Fault (#DF)",
    "Coprocessor Segment Overrun",
    "Invalid TSS (#TS)",
    "Segment Not Present (#NP)",
    "Stack-Segment Fault (#SS)",
    "General Protection Fault (#GP)",
    "Page Fault (#PF)",
    "Reserved",
    "x87 FPU Error (#MF)",
    "Alignment Check (#AC)",
    "Machine Check (#MC)",
    "SIMD Floating-Point (#XM)",
    "Virtualization Exception (#VE)",
    "Control Protection (#CP)",
    "Reserved",
    "Reserved",
    "Reserved",
    "Reserved",
    "Reserved",
    "Reserved",
    "Hypervisor Injection (#HV)",
    "VMM Communication (#VC)",
    "Security Exception (#SX)",
    "Reserved",
];

pub fn exception_name(vector: u8) -> &'static str {
    if (vector as usize) < EXCEPTION_NAMES.len() {
        EXCEPTION_NAMES[vector as usize]
    } else {
        "Unknown Interrupt"
    }
}

fn fill_entry(entry: &mut IdtEntry, handler: u64, selector: u16, type_attr: u8) {
    entry.offset_low = (handler & 0xFFFF) as u16;
    entry.offset_mid = ((handler >> 16) & 0xFFFF) as u16;
    entry.offset_high = ((handler >> 32) & 0xFFFFFFFF) as u32;
    entry.selector = selector;
    entry.ist = 0;
    entry.type_attr = type_attr;
    entry.reserved = 0;
}

unsafe fn write_entry(
    idt_ptr: *mut IdtEntry,
    vector: u8,
    handler: u64,
    selector: u16,
    type_attr: u8,
) {
    let mut entry = IdtEntry::missing();
    fill_entry(&mut entry, handler, selector, type_attr);
    ptr::write(idt_ptr.add(vector as usize), entry);
}

pub fn set_gate(vector: u8, handler: u64, selector: u16, type_attr: u8) {
    unsafe {
        let idt_ptr = ptr::addr_of_mut!(IDT) as *mut IdtEntry;
        write_entry(idt_ptr, vector, handler, selector, type_attr);
    }
}

pub fn register_handler(vector: u8, handler: InterruptHandler) {
    unsafe {
        HANDLERS[vector as usize] = Some(handler);
    }
}

pub fn set_post_handler_hook(hook: PostHandlerHook) {
    unsafe {
        POST_HANDLER_HOOK = Some(hook);
    }
}

/// Register a lock-free handler for `vector` (see `FAST_HANDLERS`).
pub fn register_fast_handler(vector: u8, handler: InterruptHandler) {
    unsafe {
        FAST_HANDLERS[vector as usize] = Some(handler);
    }
}

pub fn set_entry_hook(hook: EntryHook) {
    unsafe {
        ENTRY_HOOK = Some(hook);
    }
}

/// Install the hook every interrupt/scheduler exit path calls before
/// restoring a frame.
pub fn set_exit_hook(hook: ExitHook) {
    #[cfg(target_os = "none")]
    unsafe {
        ARCH_X86_64_EXIT_HOOK = hook as usize as u64;
    }
    #[cfg(not(target_os = "none"))]
    let _ = hook;
}

/// True for vectors served only by a fast handler (no kernel lock taken).
pub fn is_lock_free(vector: u8) -> bool {
    unsafe {
        FAST_HANDLERS[vector as usize].is_some() && HANDLERS[vector as usize].is_none()
    }
}

#[cfg(target_os = "none")]
pub fn install_syscall_gate() {
    set_gate(
        SYSCALL_VECTOR,
        isr128 as *const () as u64,
        KERNEL_CODE_SELECTOR,
        IDT_GATE_USER,
    );
}

#[cfg(not(target_os = "none"))]
pub fn install_syscall_gate() {}

#[cfg(target_os = "none")]
pub fn init() {
    unsafe {
        let idt_ptr = ptr::addr_of_mut!(IDT) as *mut IdtEntry;
        ptr::write_bytes(idt_ptr, 0, IDT_ENTRIES);

        let exceptions: [unsafe extern "C" fn(); 32] = [
            isr0, isr1, isr2, isr3, isr4, isr5, isr6, isr7, isr8, isr9, isr10, isr11, isr12, isr13,
            isr14, isr15, isr16, isr17, isr18, isr19, isr20, isr21, isr22, isr23, isr24, isr25,
            isr26, isr27, isr28, isr29, isr30, isr31,
        ];

        for (vector, handler) in exceptions.iter().enumerate() {
            let gate = if vector == 3 {
                IDT_GATE_TRAP
            } else {
                IDT_GATE_INTERRUPT
            };
            write_entry(
                idt_ptr,
                vector as u8,
                *handler as u64,
                KERNEL_CODE_SELECTOR,
                gate,
            );
        }

        let irqs: [unsafe extern "C" fn(); 16] = [
            irq0, irq1, irq2, irq3, irq4, irq5, irq6, irq7, irq8, irq9, irq10, irq11, irq12, irq13,
            irq14, irq15,
        ];

        for (i, handler) in irqs.iter().enumerate() {
            write_entry(
                idt_ptr,
                (32 + i) as u8,
                *handler as u64,
                KERNEL_CODE_SELECTOR,
                IDT_GATE_INTERRUPT,
            );
        }

        let ipis: [unsafe extern "C" fn(); IPI_VECTOR_COUNT as usize] =
            [apic_vec240, apic_vec241, apic_vec242, apic_vec243];
        for (i, handler) in ipis.iter().enumerate() {
            write_entry(
                idt_ptr,
                IPI_VECTOR_FIRST + i as u8,
                *handler as u64,
                KERNEL_CODE_SELECTOR,
                IDT_GATE_INTERRUPT,
            );
        }
        write_entry(
            idt_ptr,
            lapic::SPURIOUS_VECTOR,
            apic_vec255 as *const () as u64,
            KERNEL_CODE_SELECTOR,
            IDT_GATE_INTERRUPT,
        );

        IDTR.limit = (size_of::<IdtEntry>() * IDT_ENTRIES - 1) as u16;
        IDTR.base = idt_ptr as u64;

        lidt(ptr::addr_of!(IDTR));
    }
}

/// Load the shared IDT on the executing CPU (used by application processors
/// after the BSP has built it with [`init`]).
#[cfg(target_os = "none")]
pub fn load() {
    unsafe { lidt(ptr::addr_of!(IDTR)) };
}

#[cfg(not(target_os = "none"))]
pub fn load() {}

#[cfg(not(target_os = "none"))]
pub fn init() {
    // No-op for host builds/tests.
}

#[cfg(target_os = "none")]
#[no_mangle]
pub extern "C" fn isr_handler(frame: &mut InterruptFrame) -> u64 {
    interrupts::disable();

    let int_no = unsafe { ptr::read_unaligned(ptr::addr_of!(frame.int_no)) };
    let vector = int_no as usize;
    let isa_irq = irq::vector_to_irq(int_no);

    if let Some(line) = isa_irq {
        // Spurious legacy PIC IRQ7/IRQ15: no handler, no EOI (the helper
        // already acknowledges the master for a spurious IRQ15).
        if irq::is_spurious(line) {
            return 0;
        }
    }

    if vector == lapic::SPURIOUS_VECTOR as usize {
        // LAPIC spurious interrupts are never acknowledged.
        return 0;
    }

    if let Some(fast) = unsafe { FAST_HANDLERS[vector] } {
        fast(frame);
    }
    let lock_free = is_lock_free(vector as u8);
    if !lock_free {
        if let Some(enter) = unsafe { ENTRY_HOOK } {
            enter();
        }
        match unsafe { HANDLERS[vector] } {
            Some(handler) => handler(frame),
            None if int_no < 32 => default_exception_handler(frame),
            None => {}
        }
    }

    // Acknowledge centrally so a handler can never leave the controller
    // blocked (an unacknowledged LAPIC vector masks every vector at or below
    // its priority class, including the timer).
    if let Some(line) = isa_irq {
        irq::eoi(line);
    } else if is_ipi_vector(int_no) {
        lapic::eoi();
    }

    if lock_free {
        return 0;
    }
    if let Some(hook) = unsafe { POST_HANDLER_HOOK } {
        return hook(frame);
    }

    0
}

#[inline]
pub const fn is_ipi_vector(vector: u64) -> bool {
    vector >= IPI_VECTOR_FIRST as u64 && vector < (IPI_VECTOR_FIRST + IPI_VECTOR_COUNT) as u64
}

#[cfg(target_os = "none")]
fn default_exception_handler(_frame: &mut InterruptFrame) -> ! {
    loop {
        unsafe {
            asm!("cli; hlt", options(nomem, nostack, preserves_flags));
        }
    }
}

#[cfg(not(target_os = "none"))]
#[allow(dead_code)]
pub extern "C" fn isr_handler(_frame: &mut InterruptFrame) -> u64 {
    0
}

#[cfg(target_os = "none")]
unsafe fn lidt(idtr: *const Idtr) {
    asm!("lidt [{0}]", in(reg) idtr, options(nostack, preserves_flags));
}

#[cfg(target_os = "none")]
core::arch::global_asm!(
    r#"
.section .text
.extern isr_handler

.macro ISR_NOERR num
.global isr\num
isr\num:
    push 0
    push \num
    jmp isr_common_stub
.endm

.macro ISR_ERR num
.global isr\num
isr\num:
    push \num
    jmp isr_common_stub
.endm

.macro IRQ num, vec
.global irq\num
irq\num:
    push 0
    push \vec
    jmp isr_common_stub
.endm

ISR_NOERR 0
ISR_NOERR 1
ISR_NOERR 2
ISR_NOERR 3
ISR_NOERR 4
ISR_NOERR 5
ISR_NOERR 6
ISR_NOERR 7
ISR_ERR 8
ISR_NOERR 9
ISR_ERR 10
ISR_ERR 11
ISR_ERR 12
ISR_ERR 13
ISR_ERR 14
ISR_NOERR 15
ISR_NOERR 16
ISR_ERR 17
ISR_NOERR 18
ISR_NOERR 19
ISR_NOERR 20
ISR_ERR 21
ISR_NOERR 22
ISR_NOERR 23
ISR_NOERR 24
ISR_NOERR 25
ISR_NOERR 26
ISR_NOERR 27
ISR_NOERR 28
ISR_ERR 29
ISR_ERR 30
ISR_NOERR 31

ISR_NOERR 128

IRQ 0, 32
IRQ 1, 33
IRQ 2, 34
IRQ 3, 35
IRQ 4, 36
IRQ 5, 37
IRQ 6, 38
IRQ 7, 39
IRQ 8, 40
IRQ 9, 41
IRQ 10, 42
IRQ 11, 43
IRQ 12, 44
IRQ 13, 45
IRQ 14, 46
IRQ 15, 47

.macro APIC_VEC num
.global apic_vec\num
apic_vec\num:
    push 0
    push \num
    jmp isr_common_stub
.endm

APIC_VEC 240
APIC_VEC 241
APIC_VEC 242
APIC_VEC 243
APIC_VEC 255

isr_common_stub:
    // Coming from ring 3: switch GS to this CPU's per-CPU area.
    test byte ptr [rsp + 24], 3
    jz 1f
    swapgs
1:
    push rax
    push rbx
    push rcx
    push rdx
    push rsi
    push rdi
    push rbp
    push r8
    push r9
    push r10
    push r11
    push r12
    push r13
    push r14
    push r15

    mov rdi, rsp
    call isr_handler
    test rax, rax
    jz .no_switch
    mov rsp, rax
.no_switch:
.global arch_x86_64_frame_exit
arch_x86_64_frame_exit:
    // rsp = frame about to be restored (possibly another task's). Let the
    // kernel release its lock now that the old stack is no longer in use.
    mov rax, [rip + ARCH_X86_64_EXIT_HOOK]
    test rax, rax
    jz 2f
    mov rdi, rsp
    mov rbx, rsp
    and rsp, -16
    call rax
    mov rsp, rbx
2:
    pop r15
    pop r14
    pop r13
    pop r12
    pop r11
    pop r10
    pop r9
    pop r8
    pop rbp
    pop rdi
    pop rsi
    pop rdx
    pop rcx
    pop rbx
    pop rax

    add rsp, 16
    // Returning to ring 3: restore the user GS base.
    test byte ptr [rsp + 8], 3
    jz 3f
    swapgs
3:
    iretq
"#
);

#[cfg(target_os = "none")]
extern "C" {
    fn isr0();
    fn isr1();
    fn isr2();
    fn isr3();
    fn isr4();
    fn isr5();
    fn isr6();
    fn isr7();
    fn isr8();
    fn isr9();
    fn isr10();
    fn isr11();
    fn isr12();
    fn isr13();
    fn isr14();
    fn isr15();
    fn isr16();
    fn isr17();
    fn isr18();
    fn isr19();
    fn isr20();
    fn isr21();
    fn isr22();
    fn isr23();
    fn isr24();
    fn isr25();
    fn isr26();
    fn isr27();
    fn isr28();
    fn isr29();
    fn isr30();
    fn isr31();
    fn isr128();

    fn irq0();
    fn irq1();
    fn irq2();
    fn irq3();
    fn irq4();
    fn irq5();
    fn irq6();
    fn irq7();
    fn irq8();
    fn irq9();
    fn irq10();
    fn irq11();
    fn irq12();
    fn irq13();
    fn irq14();
    fn irq15();

    fn apic_vec240();
    fn apic_vec241();
    fn apic_vec242();
    fn apic_vec243();
    fn apic_vec255();
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::size_of;

    #[test]
    fn ipi_vector_range() {
        assert!(!is_ipi_vector(0xEF));
        assert!(is_ipi_vector(0xF0));
        assert!(is_ipi_vector(0xF3));
        assert!(!is_ipi_vector(0xF4));
        assert!(!is_ipi_vector(0xFF));
    }

    #[test]
    fn idt_struct_sizes() {
        assert_eq!(size_of::<IdtEntry>(), 16);
        assert_eq!(size_of::<Idtr>(), 10);
        assert_eq!(size_of::<InterruptFrame>(), 176);
    }

    #[test]
    fn idt_entry_fields_match_layout() {
        let mut entry = IdtEntry::missing();
        fill_entry(&mut entry, 0x1122_3344_5566_7788, 0x08, 0x8E);

        unsafe {
            assert_eq!(ptr::read_unaligned(ptr::addr_of!(entry.offset_low)), 0x7788);
            assert_eq!(ptr::read_unaligned(ptr::addr_of!(entry.offset_mid)), 0x5566);
            assert_eq!(
                ptr::read_unaligned(ptr::addr_of!(entry.offset_high)),
                0x1122_3344
            );
            assert_eq!(ptr::read_unaligned(ptr::addr_of!(entry.selector)), 0x08);
            assert_eq!(ptr::read_unaligned(ptr::addr_of!(entry.ist)), 0);
            assert_eq!(ptr::read_unaligned(ptr::addr_of!(entry.type_attr)), 0x8E);
            assert_eq!(ptr::read_unaligned(ptr::addr_of!(entry.reserved)), 0);
        }
    }
}
