pub const PAGE_SIZE: u64 = 4096;
pub const PAGE_SHIFT: u8 = 12;
pub const PAGE_OFFSET_MASK: u64 = PAGE_SIZE - 1;

pub const KERNEL_VIRT_BASE: u64 = 0xFFFF_FFFF_8000_0000;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct PhysAddr(u64);

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct VirtAddr(u64);

impl PhysAddr {
    pub const fn new(addr: u64) -> Self {
        Self(addr)
    }

    pub const fn as_u64(self) -> u64 {
        self.0
    }

    pub const fn is_aligned(self) -> bool {
        (self.0 & PAGE_OFFSET_MASK) == 0
    }

    pub const fn align_down(self) -> Self {
        Self(self.0 & !PAGE_OFFSET_MASK)
    }

    pub const fn align_up(self) -> Self {
        let mask = PAGE_OFFSET_MASK;
        if self.0 & mask == 0 {
            self
        } else {
            Self((self.0 + mask) & !mask)
        }
    }

    pub const fn page_offset(self) -> u64 {
        self.0 & PAGE_OFFSET_MASK
    }

    pub const fn to_pfn(self) -> u64 {
        self.0 >> PAGE_SHIFT
    }

    pub const fn to_virt(self) -> VirtAddr {
        VirtAddr(self.0 + KERNEL_VIRT_BASE)
    }
}

impl VirtAddr {
    pub const fn new(addr: u64) -> Self {
        Self(addr)
    }

    pub const fn as_u64(self) -> u64 {
        self.0
    }

    pub const fn is_aligned(self) -> bool {
        (self.0 & PAGE_OFFSET_MASK) == 0
    }

    pub const fn align_down(self) -> Self {
        Self(self.0 & !PAGE_OFFSET_MASK)
    }

    pub const fn align_up(self) -> Self {
        let mask = PAGE_OFFSET_MASK;
        if self.0 & mask == 0 {
            self
        } else {
            Self((self.0 + mask) & !mask)
        }
    }

    pub const fn page_offset(self) -> u64 {
        self.0 & PAGE_OFFSET_MASK
    }

    pub const fn to_phys(self) -> PhysAddr {
        PhysAddr(self.0 - KERNEL_VIRT_BASE)
    }
}

impl core::ops::Add<u64> for PhysAddr {
    type Output = Self;

    fn add(self, rhs: u64) -> Self::Output {
        Self(self.0 + rhs)
    }
}

impl core::ops::Add<u64> for VirtAddr {
    type Output = Self;

    fn add(self, rhs: u64) -> Self::Output {
        Self(self.0 + rhs)
    }
}

impl core::ops::Sub<u64> for PhysAddr {
    type Output = Self;

    fn sub(self, rhs: u64) -> Self::Output {
        Self(self.0 - rhs)
    }
}

impl core::ops::Sub<u64> for VirtAddr {
    type Output = Self;

    fn sub(self, rhs: u64) -> Self::Output {
        Self(self.0 - rhs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phys_align_helpers() {
        let addr = PhysAddr::new(0x1234);
        assert_eq!(addr.align_down().as_u64(), 0x1000);
        assert_eq!(addr.align_up().as_u64(), 0x2000);
        assert_eq!(addr.page_offset(), 0x234);
    }

    #[test]
    fn virt_align_helpers() {
        let addr = VirtAddr::new(0xFFFF_FFFF_8000_1234);
        assert_eq!(addr.align_down().as_u64(), 0xFFFF_FFFF_8000_1000);
        assert_eq!(addr.align_up().as_u64(), 0xFFFF_FFFF_8000_2000);
        assert_eq!(addr.page_offset(), 0x234);
    }

    #[test]
    fn phys_virt_conversion_roundtrip() {
        let phys = PhysAddr::new(0x0010_4000);
        let virt = phys.to_virt();
        assert_eq!(virt.to_phys(), phys);
    }
}
