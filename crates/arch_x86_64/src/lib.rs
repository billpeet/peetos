#![no_std]

#[cfg(test)]
extern crate std;

pub mod acpi;
pub mod addr;
pub mod fpu;
pub mod gdt;
pub mod idt;
pub mod interrupts;
pub mod ioapic;
pub mod irq;
pub mod lapic;
pub mod msr;
pub mod percpu;
pub mod pic;
pub mod pit;
pub mod port;
pub mod syscall;
pub mod tss;
