use core::mem::size_of;

use crate::gdt;

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct Tss {
    _reserved0: u32,
    rsp0: u64,
    rsp1: u64,
    rsp2: u64,
    _reserved1: u64,
    ist1: u64,
    ist2: u64,
    ist3: u64,
    ist4: u64,
    ist5: u64,
    ist6: u64,
    ist7: u64,
    _reserved2: u64,
    _reserved3: u16,
    iopb_offset: u16,
}

impl Tss {
    const fn zero() -> Self {
        Self {
            _reserved0: 0,
            rsp0: 0,
            rsp1: 0,
            rsp2: 0,
            _reserved1: 0,
            ist1: 0,
            ist2: 0,
            ist3: 0,
            ist4: 0,
            ist5: 0,
            ist6: 0,
            ist7: 0,
            _reserved2: 0,
            _reserved3: 0,
            iopb_offset: size_of::<Tss>() as u16,
        }
    }
}

static mut TSS: Tss = Tss::zero();

pub fn init() {
    let tss_addr = &raw const TSS as *const Tss as u64;
    gdt::set_tss_base(tss_addr);
    gdt::load_tss();
}

pub fn set_rsp0(rsp0: u64) {
    unsafe {
        TSS.rsp0 = rsp0;
    }
}

#[cfg(test)]
mod tests {
    use super::Tss;
    use core::mem::size_of;

    #[test]
    fn tss_size_is_expected() {
        assert_eq!(size_of::<Tss>(), 104);
    }
}
