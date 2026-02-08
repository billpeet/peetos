use core::cmp;
use core::mem::size_of;
use core::ptr;

use arch_x86_64::addr::{PhysAddr, VirtAddr, PAGE_SIZE};

use crate::memory::{pmm, vmm};
#[cfg(not(test))]
macro_rules! elf_log {
    ($($arg:tt)*) => {
        crate::serial_println!($($arg)*);
    };
}

#[cfg(test)]
macro_rules! elf_log {
    ($($arg:tt)*) => {{
        let _ = core::format_args!($($arg)*);
    }};
}

const ELF_MAGIC: u32 = 0x464C457F; // "\x7FELF" little-endian

const EI_MAG0: usize = 0;
const EI_MAG1: usize = 1;
const EI_MAG2: usize = 2;
const EI_MAG3: usize = 3;
const EI_CLASS: usize = 4;
const EI_DATA: usize = 5;

const ELFCLASS64: u8 = 2;
const ELFDATA2LSB: u8 = 1;

const ET_EXEC: u16 = 2;
const EM_X86_64: u16 = 62;

const PT_LOAD: u32 = 1;

const PF_X: u32 = 0x1;
const PF_W: u32 = 0x2;

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct Elf64Ehdr {
    e_ident: [u8; 16],
    e_type: u16,
    e_machine: u16,
    e_version: u32,
    e_entry: u64,
    e_phoff: u64,
    e_shoff: u64,
    e_flags: u32,
    e_ehsize: u16,
    e_phentsize: u16,
    e_phnum: u16,
    e_shentsize: u16,
    e_shnum: u16,
    e_shstrndx: u16,
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct Elf64Phdr {
    p_type: u32,
    p_flags: u32,
    p_offset: u64,
    p_vaddr: u64,
    p_paddr: u64,
    p_filesz: u64,
    p_memsz: u64,
    p_align: u64,
}

pub struct ElfLoadResult {
    pub entry_point: u64,
    pub program_break: u64,
    pub success: bool,
}

fn align_down(value: u64, align: u64) -> u64 {
    value & !(align - 1)
}

fn align_up(value: u64, align: u64) -> u64 {
    (value + align - 1) & !(align - 1)
}

fn phdr_bounds_ok(data_len: usize, offset: u64, size: u64) -> bool {
    match offset.checked_add(size) {
        Some(end) => end <= data_len as u64,
        None => false,
    }
}

pub fn validate(data: &[u8]) -> bool {
    if data.len() < size_of::<Elf64Ehdr>() {
        elf_log!("[ELF] Invalid: too small");
        return false;
    }

    let ehdr = unsafe { &*(data.as_ptr() as *const Elf64Ehdr) };
    let ident = ehdr.e_ident;

    let magic = u32::from_le_bytes([ident[EI_MAG0], ident[EI_MAG1], ident[EI_MAG2], ident[EI_MAG3]]);
    if magic != ELF_MAGIC {
        elf_log!("[ELF] Invalid: bad magic");
        return false;
    }

    if ident[EI_CLASS] != ELFCLASS64 {
        elf_log!("[ELF] Invalid: not ELF64");
        return false;
    }

    if ident[EI_DATA] != ELFDATA2LSB {
        elf_log!("[ELF] Invalid: not little-endian");
        return false;
    }

    if ehdr.e_type != ET_EXEC {
        elf_log!("[ELF] Invalid: not ET_EXEC");
        return false;
    }

    if ehdr.e_machine != EM_X86_64 {
        elf_log!("[ELF] Invalid: not x86_64");
        return false;
    }

    if ehdr.e_phentsize as usize != size_of::<Elf64Phdr>() {
        elf_log!("[ELF] Invalid: phdr size mismatch");
        return false;
    }

    let phdr_bytes = (ehdr.e_phnum as usize)
        .checked_mul(ehdr.e_phentsize as usize)
        .unwrap_or(usize::MAX);
    if phdr_bytes == usize::MAX {
        elf_log!("[ELF] Invalid: phdr size overflow");
        return false;
    }

    if !phdr_bounds_ok(data.len(), ehdr.e_phoff, phdr_bytes as u64) {
        elf_log!("[ELF] Invalid: phdr out of bounds");
        return false;
    }

    true
}

pub fn load(data: &[u8], pml4: PhysAddr) -> ElfLoadResult {
    let mut result = ElfLoadResult {
        entry_point: 0,
        program_break: 0,
        success: false,
    };

    if !validate(data) {
        return result;
    }

    let ehdr = unsafe { &*(data.as_ptr() as *const Elf64Ehdr) };
    let phdrs = unsafe {
        core::slice::from_raw_parts(
            data.as_ptr().add(ehdr.e_phoff as usize) as *const Elf64Phdr,
            ehdr.e_phnum as usize,
        )
    };

    let mut highest_addr = 0u64;

    for phdr in phdrs.iter() {
        if phdr.p_type != PT_LOAD || phdr.p_memsz == 0 {
            continue;
        }

        if phdr.p_filesz > phdr.p_memsz {
            elf_log!("[ELF] ERROR: segment filesz > memsz");
            return result;
        }

        if !phdr_bounds_ok(data.len(), phdr.p_offset, phdr.p_filesz) {
            elf_log!("[ELF] ERROR: segment out of bounds");
            return result;
        }

        let segment_end = match phdr.p_vaddr.checked_add(phdr.p_memsz) {
            Some(end) => end,
            None => {
                elf_log!("[ELF] ERROR: segment address overflow");
                return result;
            }
        };

        let vaddr_start = align_down(phdr.p_vaddr, PAGE_SIZE);
        let vaddr_end = align_up(segment_end, PAGE_SIZE);
        let num_pages = (vaddr_end - vaddr_start) / PAGE_SIZE;

        let mut page_flags = vmm::PTE_PRESENT | vmm::PTE_USER;
        if (phdr.p_flags & PF_W) != 0 {
            page_flags |= vmm::PTE_WRITE;
        }
        if (phdr.p_flags & PF_X) == 0 {
            // NX is not enabled yet; keep pages executable for now.
        }

        for page in 0..num_pages {
            let vaddr = vaddr_start + page * PAGE_SIZE;
            let virt = VirtAddr::new(vaddr);
            if vmm::get_physical_in(pml4, virt).is_some() {
                continue;
            }

            let phys = pmm::alloc_page();
            if phys.as_u64() == 0 {
                elf_log!("[ELF] ERROR: failed to allocate page");
                return result;
            }

            unsafe {
                ptr::write_bytes(phys.to_virt().as_u64() as *mut u8, 0, PAGE_SIZE as usize);
            }

            if vmm::map_page_in(pml4, virt, phys, page_flags) != 0 {
                elf_log!("[ELF] ERROR: failed to map page");
                pmm::free_page(phys);
                return result;
            }
        }

        let mut bytes_remaining = phdr.p_filesz;
        let mut src_offset = phdr.p_offset as usize;
        let mut dest_vaddr = phdr.p_vaddr;

        while bytes_remaining > 0 {
            let page_vaddr = dest_vaddr & !0xFFF;
            let page_offset = (dest_vaddr & 0xFFF) as usize;
            let bytes_in_page = cmp::min(bytes_remaining as usize, PAGE_SIZE as usize - page_offset);

            let phys = match vmm::get_physical_in(pml4, VirtAddr::new(page_vaddr)) {
                Some(phys) => phys,
                None => {
                    elf_log!("[ELF] ERROR: page not mapped during copy");
                    return result;
                }
            };

            let dest_phys = phys.as_u64() + page_offset as u64;
            let dest_ptr = PhysAddr::new(dest_phys).to_virt().as_u64() as *mut u8;
            let src_ptr = unsafe { data.as_ptr().add(src_offset) };

            unsafe {
                ptr::copy_nonoverlapping(src_ptr, dest_ptr, bytes_in_page);
            }

            src_offset += bytes_in_page;
            dest_vaddr += bytes_in_page as u64;
            bytes_remaining -= bytes_in_page as u64;
        }

        if segment_end > highest_addr {
            highest_addr = segment_end;
        }
    }

    result.entry_point = ehdr.e_entry;
    result.program_break = align_up(highest_addr, PAGE_SIZE);
    result.success = true;

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elf_header_size_matches() {
        assert_eq!(size_of::<Elf64Ehdr>(), 64);
        assert_eq!(size_of::<Elf64Phdr>(), 56);
    }

    #[test]
    fn validate_rejects_short_buffer() {
        assert!(!validate(&[0u8; 8]));
    }

    fn minimal_header() -> Elf64Ehdr {
        let mut ident = [0u8; 16];
        ident[EI_MAG0] = 0x7F;
        ident[EI_MAG1] = b'E';
        ident[EI_MAG2] = b'L';
        ident[EI_MAG3] = b'F';
        ident[EI_CLASS] = ELFCLASS64;
        ident[EI_DATA] = ELFDATA2LSB;

        Elf64Ehdr {
            e_ident: ident,
            e_type: ET_EXEC,
            e_machine: EM_X86_64,
            e_version: 1,
            e_entry: 0x400000,
            e_phoff: size_of::<Elf64Ehdr>() as u64,
            e_shoff: 0,
            e_flags: 0,
            e_ehsize: size_of::<Elf64Ehdr>() as u16,
            e_phentsize: size_of::<Elf64Phdr>() as u16,
            e_phnum: 0,
            e_shentsize: 0,
            e_shnum: 0,
            e_shstrndx: 0,
        }
    }

    #[test]
    fn validate_accepts_minimal_header() {
        let header = minimal_header();
        let mut bytes = vec![0u8; size_of::<Elf64Ehdr>()];
        unsafe {
            ptr::copy_nonoverlapping(
                &header as *const Elf64Ehdr as *const u8,
                bytes.as_mut_ptr(),
                bytes.len(),
            );
        }
        assert!(validate(&bytes));
    }

    #[test]
    fn validate_rejects_bad_class() {
        let mut header = minimal_header();
        header.e_ident[EI_CLASS] = 1;
        let mut bytes = vec![0u8; size_of::<Elf64Ehdr>()];
        unsafe {
            ptr::copy_nonoverlapping(
                &header as *const Elf64Ehdr as *const u8,
                bytes.as_mut_ptr(),
                bytes.len(),
            );
        }
        assert!(!validate(&bytes));
    }
}
