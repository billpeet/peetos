pub const PAGE_SIZE: u64 = 4096;
pub const PAGE_SHIFT: u8 = 12;
pub const PAGE_OFFSET_MASK: u64 = PAGE_SIZE - 1;

pub const KERNEL_VIRT_BASE: u64 = 0xFFFF_FFFF_8000_0000;
/// Base of the kernel's direct map of all physical RAM (PML4 slot 256, the
/// start of the kernel half). `PhysAddr::to_virt` points here. Built by
/// `vmm::init`; the boot mapping at `KERNEL_VIRT_BASE` only covers 128 MiB.
pub const DIRECT_MAP_BASE: u64 = 0xFFFF_8000_0000_0000;
/// Exclusive end of user space (lower canonical half). Every user range
/// check must stay below this: the kernel half, including the direct map,
/// is shared by all address spaces.
pub const USER_SPACE_END: u64 = 0x0000_8000_0000_0000;

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
        VirtAddr(self.0 + DIRECT_MAP_BASE)
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

    /// Physical address behind a kernel virtual address in either linear
    /// mapping: the kernel image / boot window at `KERNEL_VIRT_BASE`, or the
    /// direct map at `DIRECT_MAP_BASE` (what `PhysAddr::to_virt` returns).
    pub const fn to_phys(self) -> PhysAddr {
        if self.0 >= KERNEL_VIRT_BASE {
            PhysAddr(self.0 - KERNEL_VIRT_BASE)
        } else {
            PhysAddr(self.0 - DIRECT_MAP_BASE)
        }
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
    fn direct_map_and_kernel_window_round_trip() {
        // to_virt goes through the direct map, beyond the old 128 MiB window.
        let high = PhysAddr::new(0x1_2345_6000);
        assert_eq!(high.to_virt().as_u64(), DIRECT_MAP_BASE + 0x1_2345_6000);
        assert_eq!(high.to_virt().to_phys(), high);
        // Kernel image / boot window addresses still translate.
        let kernel = VirtAddr::new(KERNEL_VIRT_BASE + 0x10_0000);
        assert_eq!(kernel.to_phys().as_u64(), 0x10_0000);
        // The direct map is kernel-half, above all user space.
        assert!(DIRECT_MAP_BASE >= USER_SPACE_END);
    }

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
