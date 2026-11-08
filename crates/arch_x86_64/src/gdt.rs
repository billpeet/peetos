#[cfg(target_os = "none")]
use core::arch::asm;
#[cfg(target_os = "none")]
use core::mem::size_of;
#[cfg(any(target_os = "none", test))]
use core::ptr;

pub const GDT_ENTRIES: usize = 7;

pub const GDT_NULL: u16 = 0x00;
pub const GDT_KERNEL_CS: u16 = 0x08;
pub const GDT_KERNEL_DS: u16 = 0x10;
pub const GDT_USER_DS: u16 = 0x18;
pub const GDT_USER_CS: u16 = 0x20;
pub const GDT_TSS: u16 = 0x28;

pub const KERNEL_CS: u16 = GDT_KERNEL_CS;
pub const KERNEL_DS: u16 = GDT_KERNEL_DS;
pub const USER_DS: u16 = GDT_USER_DS | 0x3;
pub const USER_CS: u16 = GDT_USER_CS | 0x3;
pub const TSS_SEL: u16 = GDT_TSS;

pub const GDT_ACCESS_PRESENT: u8 = 1 << 7;
pub const GDT_ACCESS_DPL_RING0: u8 = 0 << 5;
pub const GDT_ACCESS_DPL_RING3: u8 = 3 << 5;
pub const GDT_ACCESS_CODE_DATA: u8 = 1 << 4;
pub const GDT_ACCESS_EXECUTABLE: u8 = 1 << 3;
pub const GDT_ACCESS_RW: u8 = 1 << 1;

pub const GDT_ACCESS_KERNEL_CODE: u8 = GDT_ACCESS_PRESENT
    | GDT_ACCESS_DPL_RING0
    | GDT_ACCESS_CODE_DATA
    | GDT_ACCESS_EXECUTABLE
    | GDT_ACCESS_RW;
pub const GDT_ACCESS_KERNEL_DATA: u8 =
    GDT_ACCESS_PRESENT | GDT_ACCESS_DPL_RING0 | GDT_ACCESS_CODE_DATA | GDT_ACCESS_RW;
pub const GDT_ACCESS_USER_CODE: u8 = GDT_ACCESS_PRESENT
    | GDT_ACCESS_DPL_RING3
    | GDT_ACCESS_CODE_DATA
    | GDT_ACCESS_EXECUTABLE
    | GDT_ACCESS_RW;
pub const GDT_ACCESS_USER_DATA: u8 =
    GDT_ACCESS_PRESENT | GDT_ACCESS_DPL_RING3 | GDT_ACCESS_CODE_DATA | GDT_ACCESS_RW;

pub const GDT_ACCESS_TSS: u8 = GDT_ACCESS_PRESENT | GDT_ACCESS_DPL_RING0 | 0x09;

pub const GDT_FLAG_GRANULARITY: u8 = 1 << 7;
pub const GDT_FLAG_32BIT: u8 = 1 << 6;
pub const GDT_FLAG_64BIT: u8 = 1 << 5;

#[cfg(any(target_os = "none", test))]
#[repr(C, packed)]
#[derive(Clone, Copy)]
struct GdtEntry {
    limit_low: u16,
    base_low: u16,
    base_mid: u8,
    access: u8,
    flags_limit: u8,
    base_high: u8,
}

#[cfg(any(target_os = "none", test))]
impl GdtEntry {
    const fn zero() -> Self {
        Self {
            limit_low: 0,
            base_low: 0,
            base_mid: 0,
            access: 0,
            flags_limit: 0,
            base_high: 0,
        }
    }
}

#[cfg(any(target_os = "none", test))]
#[repr(C, packed)]
#[derive(Clone, Copy)]
struct GdtTssEntry {
    limit_low: u16,
    base_low: u16,
    base_mid: u8,
    access: u8,
    flags_limit: u8,
    base_mid2: u8,
    base_high: u32,
    reserved: u32,
}

#[cfg(any(target_os = "none", test))]
#[repr(C, packed)]
#[derive(Clone, Copy)]
struct Gdtr {
    limit: u16,
    base: u64,
}

#[cfg(target_os = "none")]
static mut GDT: [GdtEntry; GDT_ENTRIES] = [GdtEntry::zero(); GDT_ENTRIES];
#[cfg(target_os = "none")]
static mut GDTR: Gdtr = Gdtr { limit: 0, base: 0 };

#[cfg(any(target_os = "none", test))]
fn fill_entry(entry: &mut GdtEntry, base: u32, limit: u32, access: u8, flags: u8) {
    entry.limit_low = (limit & 0xFFFF) as u16;
    entry.base_low = (base & 0xFFFF) as u16;
    entry.base_mid = ((base >> 16) & 0xFF) as u8;
    entry.access = access;
    entry.flags_limit = ((limit >> 16) & 0x0F) as u8 | (flags & 0xF0);
    entry.base_high = ((base >> 24) & 0xFF) as u8;
}

#[cfg(target_os = "none")]
unsafe fn write_entry(
    gdt_ptr: *mut GdtEntry,
    index: usize,
    base: u32,
    limit: u32,
    access: u8,
    flags: u8,
) {
    let mut entry = GdtEntry::zero();
    fill_entry(&mut entry, base, limit, access, flags);
    ptr::write(gdt_ptr.add(index), entry);
}

#[cfg(target_os = "none")]
pub fn init() {
    unsafe {
        let gdt_ptr = ptr::addr_of_mut!(GDT) as *mut GdtEntry;
        ptr::write_bytes(gdt_ptr, 0, GDT_ENTRIES);

        write_entry(gdt_ptr, 0, 0, 0, 0, 0);

        write_entry(
            gdt_ptr,
            1,
            0,
            0xFFFFF,
            GDT_ACCESS_KERNEL_CODE,
            GDT_FLAG_GRANULARITY | GDT_FLAG_64BIT,
        );

        write_entry(
            gdt_ptr,
            2,
            0,
            0xFFFFF,
            GDT_ACCESS_KERNEL_DATA,
            GDT_FLAG_GRANULARITY | GDT_FLAG_32BIT,
        );

        write_entry(
            gdt_ptr,
            3,
            0,
            0xFFFFF,
            GDT_ACCESS_USER_DATA,
            GDT_FLAG_GRANULARITY | GDT_FLAG_32BIT,
        );

        write_entry(
            gdt_ptr,
            4,
            0,
            0xFFFFF,
            GDT_ACCESS_USER_CODE,
            GDT_FLAG_GRANULARITY | GDT_FLAG_64BIT,
        );

        write_entry(gdt_ptr, 5, 0, 0x67, GDT_ACCESS_TSS, 0);
        ptr::write(gdt_ptr.add(6), GdtEntry::zero());

        GDTR.limit = (size_of::<GdtEntry>() * GDT_ENTRIES - 1) as u16;
        GDTR.base = gdt_ptr as u64;

        load_gdt(ptr::addr_of!(GDTR));
    }
}

#[cfg(not(target_os = "none"))]
pub fn init() {
    // No-op for host builds/tests.
}

#[cfg(target_os = "none")]
pub fn set_tss_base(base: u64) {
    unsafe {
        let tss_desc = (ptr::addr_of_mut!(GDT) as *mut GdtEntry).add(5) as *mut GdtTssEntry;
        (*tss_desc).base_low = (base & 0xFFFF) as u16;
        (*tss_desc).base_mid = ((base >> 16) & 0xFF) as u8;
        (*tss_desc).base_mid2 = ((base >> 24) & 0xFF) as u8;
        (*tss_desc).base_high = ((base >> 32) & 0xFFFFFFFF) as u32;
        (*tss_desc).reserved = 0;
    }
}

#[cfg(not(target_os = "none"))]
pub fn set_tss_base(_base: u64) {
    // No-op for host builds/tests.
}

/// Load the shared GDT on the executing CPU and reload segment registers.
/// Used by application processors after the BSP has built it with [`init`].
/// The TSS is not loaded: the single TSS descriptor belongs to the BSP.
#[cfg(target_os = "none")]
pub fn load() {
    unsafe { load_gdt(ptr::addr_of!(GDTR)) };
}

#[cfg(not(target_os = "none"))]
pub fn load() {}

#[cfg(target_os = "none")]
pub fn load_tss() {
    unsafe {
        asm!(
            "ltr ax",
            in("ax") TSS_SEL,
            options(nostack, preserves_flags)
        );
    }
}

#[cfg(not(target_os = "none"))]
pub fn load_tss() {
    // No-op for host builds/tests.
}

#[cfg(target_os = "none")]
unsafe fn load_gdt(gdtr: *const Gdtr) {
    asm!(
        "lgdt [{0}]",
        "mov ax, {1}",
        "mov ds, ax",
        "mov es, ax",
        "mov fs, ax",
        "mov gs, ax",
        "mov ss, ax",
        "push {2}",
        "lea rax, [rip + 2f]",
        "push rax",
        "retfq",
        "2:",
        in(reg) gdtr,
        const KERNEL_DS,
        const KERNEL_CS,
        lateout("rax") _,
        options(preserves_flags)
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::size_of;

    #[test]
    fn gdt_struct_sizes() {
        assert_eq!(size_of::<GdtEntry>(), 8);
        assert_eq!(size_of::<GdtTssEntry>(), 16);
        assert_eq!(size_of::<Gdtr>(), 10);
    }

    #[test]
    fn gdt_entry_fields_match_layout() {
        let mut entry = GdtEntry::zero();
        fill_entry(&mut entry, 0x00FF_EE11, 0x12345, 0x9A, 0xA0);

        unsafe {
            assert_eq!(ptr::read_unaligned(ptr::addr_of!(entry.limit_low)), 0x2345);
            assert_eq!(ptr::read_unaligned(ptr::addr_of!(entry.base_low)), 0xEE11);
            assert_eq!(ptr::read_unaligned(ptr::addr_of!(entry.base_mid)), 0xFF);
            assert_eq!(ptr::read_unaligned(ptr::addr_of!(entry.access)), 0x9A);
            assert_eq!(ptr::read_unaligned(ptr::addr_of!(entry.flags_limit)), 0xA1);
            assert_eq!(ptr::read_unaligned(ptr::addr_of!(entry.base_high)), 0x00);
        }
    }
}
