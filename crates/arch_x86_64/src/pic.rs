#[cfg(target_os = "none")]
use crate::port::{inb, outb};

pub const PIC1_COMMAND: u16 = 0x20;
pub const PIC1_DATA: u16 = 0x21;
pub const PIC2_COMMAND: u16 = 0xA0;
pub const PIC2_DATA: u16 = 0xA1;

pub const PIC_EOI: u8 = 0x20;

pub const ICW1_INIT: u8 = 0x10;
pub const ICW1_ICW4: u8 = 0x01;
pub const ICW4_8086: u8 = 0x01;

pub const OCW3_READ_IRR: u8 = 0x0A;
pub const OCW3_READ_ISR: u8 = 0x0B;

pub const PIC1_OFFSET: u8 = 0x20;
pub const PIC2_OFFSET: u8 = 0x28;

pub const IRQ_TIMER: u8 = 0;
pub const IRQ_KEYBOARD: u8 = 1;
pub const IRQ_CASCADE: u8 = 2;
pub const IRQ_COM2: u8 = 3;
pub const IRQ_COM1: u8 = 4;
pub const IRQ_LPT2: u8 = 5;
pub const IRQ_FLOPPY: u8 = 6;
pub const IRQ_LPT1: u8 = 7;
pub const IRQ_RTC: u8 = 8;
pub const IRQ_ACPI: u8 = 9;
pub const IRQ_AVAILABLE1: u8 = 10;
pub const IRQ_AVAILABLE2: u8 = 11;
pub const IRQ_MOUSE: u8 = 12;
pub const IRQ_FPU: u8 = 13;
pub const IRQ_ATA1: u8 = 14;
pub const IRQ_ATA2: u8 = 15;

#[inline]
pub const fn irq_to_vector(irq: u8) -> u8 {
    if irq < 8 {
        PIC1_OFFSET + irq
    } else {
        PIC2_OFFSET + (irq - 8)
    }
}

#[cfg(target_os = "none")]
#[inline]
fn io_wait() {
    unsafe { outb(0x80, 0) };
}

#[cfg(target_os = "none")]
pub fn init() {
    unsafe {
        let _ = inb(PIC1_DATA);
        let _ = inb(PIC2_DATA);

        outb(PIC1_COMMAND, ICW1_INIT | ICW1_ICW4);
        io_wait();
        outb(PIC2_COMMAND, ICW1_INIT | ICW1_ICW4);
        io_wait();

        outb(PIC1_DATA, PIC1_OFFSET);
        io_wait();
        outb(PIC2_DATA, PIC2_OFFSET);
        io_wait();

        outb(PIC1_DATA, 0x04);
        io_wait();
        outb(PIC2_DATA, 0x02);
        io_wait();

        outb(PIC1_DATA, ICW4_8086);
        io_wait();
        outb(PIC2_DATA, ICW4_8086);
        io_wait();

        outb(PIC1_DATA, 0xFB); // mask all except cascade
        outb(PIC2_DATA, 0xFF); // mask all on slave
    }
}

#[cfg(not(target_os = "none"))]
pub fn init() {}

#[cfg(target_os = "none")]
pub fn send_eoi(irq: u8) {
    unsafe {
        if irq >= 8 {
            outb(PIC2_COMMAND, PIC_EOI);
        }
        outb(PIC1_COMMAND, PIC_EOI);
    }
}

#[cfg(not(target_os = "none"))]
pub fn send_eoi(_irq: u8) {}

#[cfg(target_os = "none")]
pub fn mask_irq(irq: u8) {
    let (port, line) = if irq < 8 {
        (PIC1_DATA, irq)
    } else {
        (PIC2_DATA, irq - 8)
    };
    unsafe {
        let value = inb(port) | (1 << line);
        outb(port, value);
    }
}

#[cfg(not(target_os = "none"))]
pub fn mask_irq(_irq: u8) {}

#[cfg(target_os = "none")]
pub fn unmask_irq(irq: u8) {
    let (port, line) = if irq < 8 {
        (PIC1_DATA, irq)
    } else {
        (PIC2_DATA, irq - 8)
    };
    unsafe {
        let value = inb(port) & !(1 << line);
        outb(port, value);
    }
}

#[cfg(not(target_os = "none"))]
pub fn unmask_irq(_irq: u8) {}

#[cfg(target_os = "none")]
pub fn get_mask() -> u16 {
    unsafe { (inb(PIC1_DATA) as u16) | ((inb(PIC2_DATA) as u16) << 8) }
}

#[cfg(not(target_os = "none"))]
pub fn get_mask() -> u16 {
    0xFFFF
}

#[cfg(target_os = "none")]
pub fn set_mask(mask: u16) {
    unsafe {
        outb(PIC1_DATA, (mask & 0xFF) as u8);
        outb(PIC2_DATA, ((mask >> 8) & 0xFF) as u8);
    }
}

#[cfg(not(target_os = "none"))]
pub fn set_mask(_mask: u16) {}

#[cfg(target_os = "none")]
pub fn disable() {
    unsafe {
        outb(PIC1_DATA, 0xFF);
        outb(PIC2_DATA, 0xFF);
    }
}

#[cfg(not(target_os = "none"))]
pub fn disable() {}

#[cfg(target_os = "none")]
fn read_isr(pic: u8) -> u8 {
    let port = if pic == 0 { PIC1_COMMAND } else { PIC2_COMMAND };
    unsafe {
        outb(port, OCW3_READ_ISR);
        inb(port)
    }
}

#[cfg(target_os = "none")]
pub fn is_spurious(irq: u8) -> bool {
    if irq == 7 {
        (read_isr(0) & 0x80) == 0
    } else if irq == 15 {
        if (read_isr(1) & 0x80) == 0 {
            unsafe { outb(PIC1_COMMAND, PIC_EOI) };
            return true;
        }
        false
    } else {
        false
    }
}

#[cfg(not(target_os = "none"))]
pub fn is_spurious(_irq: u8) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn irq_to_vector_maps_correctly() {
        assert_eq!(irq_to_vector(0), 0x20);
        assert_eq!(irq_to_vector(7), 0x27);
        assert_eq!(irq_to_vector(8), 0x28);
        assert_eq!(irq_to_vector(15), 0x2F);
    }
}
