use core::cmp;
use core::mem::size_of;
use core::ptr;

use arch_x86_64::addr::{PhysAddr, VirtAddr, USER_SPACE_END, PAGE_SIZE};

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
const ET_DYN: u16 = 3;
const EM_X86_64: u16 = 62;
const EV_CURRENT: u32 = 1;

const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;
const PT_PHDR: u32 = 6;
const PT_INTERP: u32 = 3;

const PF_X: u32 = 0x1;
const PF_W: u32 = 0x2;

const ELF_INTERP_MAX: usize = 256;

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

    pub phdr_addr: u64,
    pub phdr_count: u16,
    pub phdr_size: u16,
    pub base_addr: u64,
    pub interp_base: u64,
    pub interp_path: [u8; ELF_INTERP_MAX],
    pub is_dynamic: bool,
}

fn align_down(value: u64, align: u64) -> u64 {
    value & !(align - 1)
}

fn align_up_checked(value: u64, align: u64) -> Option<u64> {
    value.checked_add(align - 1).map(|v| v & !(align - 1))
}

fn phdr_bounds_ok(data_len: usize, offset: u64, size: u64) -> bool {
    match offset.checked_add(size) {
        Some(end) => end <= data_len as u64,
        None => false,
    }
}

fn read_ehdr(data: &[u8]) -> Option<Elf64Ehdr> {
    if data.len() < size_of::<Elf64Ehdr>() {
        return None;
    }

    let ptr = data.as_ptr() as *const Elf64Ehdr;
    Some(unsafe { ptr::read_unaligned(ptr) })
}

fn read_phdr(data: &[u8], offset: usize) -> Option<Elf64Phdr> {
    if offset.checked_add(size_of::<Elf64Phdr>())? > data.len() {
        return None;
    }

    let ptr = unsafe { data.as_ptr().add(offset) as *const Elf64Phdr };
    Some(unsafe { ptr::read_unaligned(ptr) })
}

fn check_ident(ehdr: &Elf64Ehdr) -> bool {
    let ident = ehdr.e_ident;
    let magic = u32::from_le_bytes([
        ident[EI_MAG0],
        ident[EI_MAG1],
        ident[EI_MAG2],
        ident[EI_MAG3],
    ]);
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

    true
}

fn phdr_table_bytes(ehdr: &Elf64Ehdr) -> Option<usize> {
    (ehdr.e_phnum as usize).checked_mul(ehdr.e_phentsize as usize)
}

fn valid_load_alignment(phdr: &Elf64Phdr) -> bool {
    if phdr.p_align == 0 {
        return true;
    }

    if !phdr.p_align.is_power_of_two() {
        return false;
    }

    if phdr.p_align == 1 {
        return true;
    }

    (phdr.p_vaddr & (phdr.p_align - 1)) == (phdr.p_offset & (phdr.p_align - 1))
}

fn validate_load_segment_layout(data: &[u8], phdr: &Elf64Phdr) -> bool {
    if phdr.p_filesz > phdr.p_memsz {
        elf_log!("[ELF] Invalid: segment filesz > memsz");
        return false;
    }

    if !phdr_bounds_ok(data.len(), phdr.p_offset, phdr.p_filesz) {
        elf_log!("[ELF] Invalid: segment out of bounds");
        return false;
    }

    if !valid_load_alignment(phdr) {
        elf_log!("[ELF] Invalid: bad segment alignment");
        return false;
    }

    if (phdr.p_flags & PF_W) != 0 && (phdr.p_flags & PF_X) != 0 {
        elf_log!("[ELF] Invalid: RWX segment not supported");
        return false;
    }

    true
}

fn parse_interp_path<'a>(data: &'a [u8], phdr: &Elf64Phdr) -> Option<&'a [u8]> {
    if phdr.p_filesz < 2 {
        elf_log!("[ELF] Invalid: PT_INTERP too short");
        return None;
    }

    if phdr.p_filesz as usize > ELF_INTERP_MAX {
        elf_log!("[ELF] Invalid: PT_INTERP path too long");
        return None;
    }

    if !phdr_bounds_ok(data.len(), phdr.p_offset, phdr.p_filesz) {
        elf_log!("[ELF] Invalid: PT_INTERP out of bounds");
        return None;
    }

    let start = phdr.p_offset as usize;
    let end = start + phdr.p_filesz as usize;
    let raw = &data[start..end];

    if raw[raw.len() - 1] != 0 {
        elf_log!("[ELF] Invalid: PT_INTERP not NUL terminated");
        return None;
    }

    let path = &raw[..raw.len() - 1];
    if path.is_empty() {
        elf_log!("[ELF] Invalid: PT_INTERP empty path");
        return None;
    }

    if path.contains(&0) {
        elf_log!("[ELF] Invalid: PT_INTERP contains interior NUL");
        return None;
    }

    Some(path)
}

fn validate_program_headers(data: &[u8], ehdr: &Elf64Ehdr) -> bool {
    let mut interp_count = 0usize;

    for i in 0..ehdr.e_phnum {
        let offset = ehdr.e_phoff as usize + (i as usize * ehdr.e_phentsize as usize);
        let phdr = match read_phdr(data, offset) {
            Some(phdr) => phdr,
            None => {
                elf_log!("[ELF] Invalid: malformed program header");
                return false;
            }
        };

        match phdr.p_type {
            PT_LOAD => {
                if !validate_load_segment_layout(data, &phdr) {
                    return false;
                }
            }
            PT_INTERP => {
                interp_count += 1;
                if interp_count > 1 {
                    elf_log!("[ELF] Invalid: multiple PT_INTERP segments");
                    return false;
                }
                if parse_interp_path(data, &phdr).is_none() {
                    return false;
                }
            }
            _ => {}
        }
    }

    true
}

fn entry_point_in_exec_segment(data: &[u8], ehdr: &Elf64Ehdr, load_bias: u64) -> bool {
    let entry = match ehdr.e_entry.checked_add(load_bias) {
        Some(addr) => addr,
        None => return false,
    };

    for i in 0..ehdr.e_phnum {
        let offset = ehdr.e_phoff as usize + (i as usize * ehdr.e_phentsize as usize);
        let phdr = match read_phdr(data, offset) {
            Some(phdr) => phdr,
            None => return false,
        };

        if phdr.p_type != PT_LOAD || phdr.p_memsz == 0 || (phdr.p_flags & PF_X) == 0 {
            continue;
        }

        let seg_start = match phdr.p_vaddr.checked_add(load_bias) {
            Some(addr) => addr,
            None => return false,
        };
        let seg_end = match phdr
            .p_vaddr
            .checked_add(phdr.p_memsz)
            .and_then(|v| v.checked_add(load_bias))
        {
            Some(addr) => addr,
            None => return false,
        };

        if entry >= seg_start && entry < seg_end {
            let entry_end = match entry.checked_add(1) {
                Some(end) => end,
                None => return false,
            };
            return is_user_range(entry, entry_end);
        }
    }

    false
}

fn validate_common(data: &[u8], allow_dyn: bool) -> Option<Elf64Ehdr> {
    let ehdr = match read_ehdr(data) {
        Some(header) => header,
        None => {
            elf_log!("[ELF] Invalid: too small");
            return None;
        }
    };

    if !check_ident(&ehdr) {
        return None;
    }

    if !allow_dyn && ehdr.e_type != ET_EXEC {
        elf_log!("[ELF] Invalid: not ET_EXEC");
        return None;
    }

    if allow_dyn && ehdr.e_type != ET_EXEC && ehdr.e_type != ET_DYN {
        elf_log!("[ELF] Invalid: not ET_EXEC or ET_DYN");
        return None;
    }

    if ehdr.e_machine != EM_X86_64 {
        elf_log!("[ELF] Invalid: not x86_64");
        return None;
    }

    if ehdr.e_version != EV_CURRENT {
        elf_log!("[ELF] Invalid: unsupported ELF version");
        return None;
    }

    if ehdr.e_ehsize as usize != size_of::<Elf64Ehdr>() {
        elf_log!("[ELF] Invalid: ELF header size mismatch");
        return None;
    }

    if ehdr.e_phentsize as usize != size_of::<Elf64Phdr>() {
        elf_log!("[ELF] Invalid: phdr size mismatch");
        return None;
    }

    let phdr_bytes = match phdr_table_bytes(&ehdr) {
        Some(bytes) => bytes,
        None => {
            elf_log!("[ELF] Invalid: phdr size overflow");
            return None;
        }
    };

    if !phdr_bounds_ok(data.len(), ehdr.e_phoff, phdr_bytes as u64) {
        elf_log!("[ELF] Invalid: phdr out of bounds");
        return None;
    }

    if !validate_program_headers(data, &ehdr) {
        return None;
    }

    Some(ehdr)
}

fn init_result() -> ElfLoadResult {
    ElfLoadResult {
        entry_point: 0,
        program_break: 0,
        success: false,
        phdr_addr: 0,
        phdr_count: 0,
        phdr_size: 0,
        base_addr: 0,
        interp_base: 0,
        interp_path: [0u8; ELF_INTERP_MAX],
        is_dynamic: false,
    }
}

fn is_user_range(start: u64, end: u64) -> bool {
    start < end && end <= USER_SPACE_END
}

pub fn validate(data: &[u8]) -> bool {
    validate_common(data, false).is_some()
}

pub fn validate_dynamic(data: &[u8]) -> bool {
    validate_common(data, true).is_some()
}

pub fn is_dynamic(data: &[u8]) -> bool {
    let ehdr = match validate_common(data, true) {
        Some(header) => header,
        None => return false,
    };

    let phdr_bytes = match phdr_table_bytes(&ehdr) {
        Some(bytes) => bytes,
        None => return false,
    };
    if !phdr_bounds_ok(data.len(), ehdr.e_phoff, phdr_bytes as u64) {
        return false;
    }

    if ehdr.e_type == ET_DYN {
        return true;
    }

    for i in 0..ehdr.e_phnum {
        let offset = ehdr.e_phoff as usize + (i as usize * ehdr.e_phentsize as usize);
        let phdr = match read_phdr(data, offset) {
            Some(phdr) => phdr,
            None => return false,
        };
        if phdr.p_type == PT_INTERP || phdr.p_type == PT_DYNAMIC {
            return true;
        }
    }

    false
}

pub fn get_interp(data: &[u8], path_out: &mut [u8; ELF_INTERP_MAX]) -> bool {
    let ehdr = match validate_common(data, true) {
        Some(header) => header,
        None => return false,
    };

    let phdr_bytes = match phdr_table_bytes(&ehdr) {
        Some(bytes) => bytes,
        None => return false,
    };
    if !phdr_bounds_ok(data.len(), ehdr.e_phoff, phdr_bytes as u64) {
        return false;
    }

    for byte in path_out.iter_mut() {
        *byte = 0;
    }

    let mut found = false;

    for i in 0..ehdr.e_phnum {
        let offset = ehdr.e_phoff as usize + (i as usize * ehdr.e_phentsize as usize);
        let phdr = match read_phdr(data, offset) {
            Some(phdr) => phdr,
            None => return false,
        };

        if phdr.p_type != PT_INTERP {
            continue;
        }

        if found {
            return false;
        }

        let path = match parse_interp_path(data, &phdr) {
            Some(path) => path,
            None => return false,
        };

        if path.len() >= ELF_INTERP_MAX {
            return false;
        }

        path_out[..path.len()].copy_from_slice(path);
        path_out[path.len()] = 0;
        found = true;
    }

    found
}

fn load_segments(
    data: &[u8],
    ehdr: &Elf64Ehdr,
    pml4: PhysAddr,
    load_bias: u64,
) -> Option<(u64, u64, u64, bool)> {
    let mut highest_addr = 0u64;
    let mut phdr_addr = 0u64;
    let mut found_pt_phdr = false;

    for i in 0..ehdr.e_phnum {
        let offset = ehdr.e_phoff as usize + (i as usize * ehdr.e_phentsize as usize);
        let phdr = read_phdr(data, offset)?;

        if phdr.p_type == PT_PHDR {
            phdr_addr = match phdr.p_vaddr.checked_add(load_bias) {
                Some(addr) => addr,
                None => {
                    elf_log!("[ELF] ERROR: PT_PHDR address overflow");
                    return None;
                }
            };
            found_pt_phdr = true;
            continue;
        }

        if phdr.p_type != PT_LOAD || phdr.p_memsz == 0 {
            continue;
        }

        if !validate_load_segment_layout(data, &phdr) {
            return None;
        }

        let segment_end = match phdr.p_vaddr.checked_add(phdr.p_memsz) {
            Some(end) => end,
            None => {
                elf_log!("[ELF] ERROR: segment address overflow");
                return None;
            }
        };

        let seg_vaddr = match phdr.p_vaddr.checked_add(load_bias) {
            Some(addr) => addr,
            None => {
                elf_log!("[ELF] ERROR: segment vaddr overflow");
                return None;
            }
        };
        let seg_end = match segment_end.checked_add(load_bias) {
            Some(addr) => addr,
            None => {
                elf_log!("[ELF] ERROR: segment end overflow");
                return None;
            }
        };

        let vaddr_start = align_down(seg_vaddr, PAGE_SIZE);
        let vaddr_end = match align_up_checked(seg_end, PAGE_SIZE) {
            Some(addr) => addr,
            None => {
                elf_log!("[ELF] ERROR: segment alignment overflow");
                return None;
            }
        };
        if !is_user_range(vaddr_start, vaddr_end) {
            elf_log!("[ELF] ERROR: segment outside user range");
            return None;
        }

        let num_pages = (vaddr_end - vaddr_start) / PAGE_SIZE;

        let mut page_flags = vmm::PTE_PRESENT | vmm::PTE_USER;
        if (phdr.p_flags & PF_W) != 0 {
            page_flags |= vmm::PTE_WRITE;
        }
        if (phdr.p_flags & PF_X) == 0 {
            page_flags |= vmm::PTE_NO_EXECUTE;
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
                return None;
            }

            unsafe {
                ptr::write_bytes(phys.to_virt().as_u64() as *mut u8, 0, PAGE_SIZE as usize);
            }

            if vmm::map_page_in(pml4, virt, phys, page_flags) != 0 {
                elf_log!("[ELF] ERROR: failed to map page");
                pmm::free_page(phys);
                return None;
            }
        }

        let mut bytes_remaining = phdr.p_filesz;
        let mut src_offset = phdr.p_offset as usize;
        let mut dest_vaddr = seg_vaddr;

        while bytes_remaining > 0 {
            let page_vaddr = dest_vaddr & !0xFFF;
            let page_offset = (dest_vaddr & 0xFFF) as usize;
            let bytes_in_page =
                cmp::min(bytes_remaining as usize, PAGE_SIZE as usize - page_offset);

            let phys = match vmm::get_physical_in(pml4, VirtAddr::new(page_vaddr)) {
                Some(phys) => phys,
                None => {
                    elf_log!("[ELF] ERROR: page not mapped during copy");
                    return None;
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

    if !found_pt_phdr {
        for i in 0..ehdr.e_phnum {
            let offset = ehdr.e_phoff as usize + (i as usize * ehdr.e_phentsize as usize);
            let phdr = read_phdr(data, offset)?;
            if phdr.p_type != PT_LOAD || phdr.p_filesz == 0 {
                continue;
            }

            let phdr_off = ehdr.e_phoff;
            let phdr_end = match phdr.p_offset.checked_add(phdr.p_filesz) {
                Some(end) => end,
                None => {
                    elf_log!("[ELF] ERROR: phdr segment bounds overflow");
                    return None;
                }
            };
            if phdr_off >= phdr.p_offset && phdr_off < phdr_end {
                let base = match phdr.p_vaddr.checked_add(load_bias) {
                    Some(addr) => addr,
                    None => {
                        elf_log!("[ELF] ERROR: phdr address overflow");
                        return None;
                    }
                };
                phdr_addr = base + (phdr_off - phdr.p_offset);
                break;
            }
        }
    }

    Some((highest_addr, phdr_addr, load_bias, found_pt_phdr))
}

pub fn load(data: &[u8], pml4: PhysAddr) -> ElfLoadResult {
    let mut result = init_result();

    let ehdr = match validate_common(data, false) {
        Some(header) => header,
        None => return result,
    };

    result.phdr_count = ehdr.e_phnum;
    result.phdr_size = ehdr.e_phentsize;
    result.base_addr = 0;
    result.interp_base = 0;
    result.is_dynamic = is_dynamic(data);
    let _ = get_interp(data, &mut result.interp_path);

    if !entry_point_in_exec_segment(data, &ehdr, 0) {
        elf_log!("[ELF] ERROR: entry point not in executable user segment");
        return result;
    }

    let (highest_addr, phdr_addr, load_bias, _) = match load_segments(data, &ehdr, pml4, 0) {
        Some(values) => values,
        None => return result,
    };

    result.phdr_addr = phdr_addr;
    result.entry_point = match ehdr.e_entry.checked_add(load_bias) {
        Some(entry) => entry,
        None => return result,
    };
    let break_raw = match highest_addr.checked_add(load_bias) {
        Some(addr) => addr,
        None => return result,
    };
    result.program_break = match align_up_checked(break_raw, PAGE_SIZE) {
        Some(addr) => addr,
        None => return result,
    };
    result.success = true;

    result
}

pub fn load_at(data: &[u8], pml4: PhysAddr, base_addr: u64) -> ElfLoadResult {
    let mut result = init_result();

    let ehdr = match validate_common(data, true) {
        Some(header) => header,
        None => return result,
    };

    let mut lowest_addr = u64::MAX;
    let mut highest_addr = 0u64;

    for i in 0..ehdr.e_phnum {
        let offset = ehdr.e_phoff as usize + (i as usize * ehdr.e_phentsize as usize);
        let phdr = match read_phdr(data, offset) {
            Some(phdr) => phdr,
            None => return result,
        };
        if phdr.p_type != PT_LOAD || phdr.p_memsz == 0 {
            continue;
        }

        if phdr.p_vaddr < lowest_addr {
            lowest_addr = phdr.p_vaddr;
        }
        let segment_end = match phdr.p_vaddr.checked_add(phdr.p_memsz) {
            Some(end) => end,
            None => return result,
        };
        if segment_end > highest_addr {
            highest_addr = segment_end;
        }
    }

    if lowest_addr == u64::MAX {
        elf_log!("[ELF] ERROR: no loadable segments");
        return result;
    }

    if (base_addr & (PAGE_SIZE - 1)) != 0 {
        elf_log!("[ELF] ERROR: base address is not page aligned");
        return result;
    }

    let aligned_lowest = align_down(lowest_addr, PAGE_SIZE);
    if base_addr < aligned_lowest {
        elf_log!("[ELF] ERROR: base address below lowest segment");
        return result;
    }
    let load_bias = base_addr - aligned_lowest;

    if !entry_point_in_exec_segment(data, &ehdr, load_bias) {
        elf_log!("[ELF] ERROR: entry point not in executable user segment");
        return result;
    }

    result.phdr_count = ehdr.e_phnum;
    result.phdr_size = ehdr.e_phentsize;
    result.base_addr = base_addr;
    result.interp_base = 0;
    result.is_dynamic = is_dynamic(data);
    let _ = get_interp(data, &mut result.interp_path);

    let (seg_highest, phdr_addr, _, _) = match load_segments(data, &ehdr, pml4, load_bias) {
        Some(values) => values,
        None => return result,
    };

    result.phdr_addr = phdr_addr;
    result.entry_point = match ehdr.e_entry.checked_add(load_bias) {
        Some(entry) => entry,
        None => return result,
    };
    let break_raw = match seg_highest.checked_add(load_bias) {
        Some(addr) => addr,
        None => return result,
    };
    result.program_break = match align_up_checked(break_raw, PAGE_SIZE) {
        Some(addr) => addr,
        None => return result,
    };
    result.success = true;

    result
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn write_struct<T: Copy>(out: &mut [u8], value: &T) {
        let size = size_of::<T>();
        unsafe {
            ptr::copy_nonoverlapping(value as *const T as *const u8, out.as_mut_ptr(), size);
        }
    }

    fn build_image(mut header: Elf64Ehdr, phdrs: &[Elf64Phdr]) -> Vec<u8> {
        header.e_phoff = size_of::<Elf64Ehdr>() as u64;
        header.e_phnum = phdrs.len() as u16;

        let phdr_start = size_of::<Elf64Ehdr>();
        let phdr_bytes = phdrs.len() * size_of::<Elf64Phdr>();
        let mut total_size = phdr_start + phdr_bytes;

        for phdr in phdrs {
            let seg_end = phdr.p_offset.saturating_add(phdr.p_filesz) as usize;
            if seg_end > total_size {
                total_size = seg_end;
            }
        }

        let mut bytes = vec![0u8; total_size];
        write_struct(&mut bytes[..size_of::<Elf64Ehdr>()], &header);
        for (idx, phdr) in phdrs.iter().enumerate() {
            let off = phdr_start + (idx * size_of::<Elf64Phdr>());
            write_struct(&mut bytes[off..off + size_of::<Elf64Phdr>()], phdr);
        }
        bytes
    }

    #[test]
    fn elf_header_size_matches() {
        assert_eq!(size_of::<Elf64Ehdr>(), 64);
        assert_eq!(size_of::<Elf64Phdr>(), 56);
    }

    #[test]
    fn validate_rejects_short_buffer() {
        assert!(!validate(&[0u8; 8]));
    }

    #[test]
    fn validate_accepts_minimal_header() {
        let header = minimal_header();
        let mut bytes = vec![0u8; size_of::<Elf64Ehdr>()];
        write_struct(&mut bytes, &header);
        assert!(validate(&bytes));
    }

    #[test]
    fn validate_rejects_bad_class() {
        let mut header = minimal_header();
        header.e_ident[EI_CLASS] = 1;
        let mut bytes = vec![0u8; size_of::<Elf64Ehdr>()];
        write_struct(&mut bytes, &header);
        assert!(!validate(&bytes));
    }

    #[test]
    fn validate_rejects_bad_header_size() {
        let mut header = minimal_header();
        header.e_ehsize = 0;
        let mut bytes = vec![0u8; size_of::<Elf64Ehdr>()];
        write_struct(&mut bytes, &header);
        assert!(!validate(&bytes));
    }

    #[test]
    fn validate_dynamic_accepts_et_dyn() {
        let mut header = minimal_header();
        header.e_type = ET_DYN;
        let mut bytes = vec![0u8; size_of::<Elf64Ehdr>()];
        write_struct(&mut bytes, &header);
        assert!(validate_dynamic(&bytes));
    }

    #[test]
    fn is_dynamic_detects_et_dyn_without_interp() {
        let mut header = minimal_header();
        header.e_type = ET_DYN;
        let mut bytes = vec![0u8; size_of::<Elf64Ehdr>()];
        write_struct(&mut bytes, &header);
        assert!(is_dynamic(&bytes));
    }

    #[test]
    fn validate_dynamic_rejects_rwx_load_segment() {
        let header = minimal_header();
        let phdr = Elf64Phdr {
            p_type: PT_LOAD,
            p_flags: PF_W | PF_X,
            p_offset: 0x1000,
            p_vaddr: 0x400000,
            p_paddr: 0,
            p_filesz: 4,
            p_memsz: 4,
            p_align: 0x1000,
        };
        let bytes = build_image(header, &[phdr]);
        assert!(!validate_dynamic(&bytes));
    }

    #[test]
    fn validate_dynamic_rejects_bad_load_alignment() {
        let header = minimal_header();
        let phdr = Elf64Phdr {
            p_type: PT_LOAD,
            p_flags: PF_X,
            p_offset: 0x2000,
            p_vaddr: 0x400123,
            p_paddr: 0,
            p_filesz: 4,
            p_memsz: 4,
            p_align: 0x1000,
        };
        let bytes = build_image(header, &[phdr]);
        assert!(!validate_dynamic(&bytes));
    }

    #[test]
    fn interp_detection_and_copy() {
        let mut header = minimal_header();
        header.e_phnum = 1;
        header.e_phoff = size_of::<Elf64Ehdr>() as u64;

        let interp_len = b"/lib/ld.so\0".len() as u64;
        let phdr = Elf64Phdr {
            p_type: PT_INTERP,
            p_flags: 0,
            p_offset: (size_of::<Elf64Ehdr>() + size_of::<Elf64Phdr>()) as u64,
            p_vaddr: 0,
            p_paddr: 0,
            p_filesz: interp_len,
            p_memsz: interp_len,
            p_align: 1,
        };

        let mut bytes =
            vec![0u8; size_of::<Elf64Ehdr>() + size_of::<Elf64Phdr>() + interp_len as usize];
        write_struct(&mut bytes[..size_of::<Elf64Ehdr>()], &header);
        write_struct(
            &mut bytes[size_of::<Elf64Ehdr>()..size_of::<Elf64Ehdr>() + size_of::<Elf64Phdr>()],
            &phdr,
        );
        bytes[size_of::<Elf64Ehdr>() + size_of::<Elf64Phdr>()..].copy_from_slice(b"/lib/ld.so\0");

        assert!(is_dynamic(&bytes));
        let mut path = [0u8; ELF_INTERP_MAX];
        assert!(get_interp(&bytes, &mut path));
        let nul_pos = path.iter().position(|b| *b == 0).unwrap_or(path.len());
        let path_str = core::str::from_utf8(&path[..nul_pos]).unwrap();
        assert_eq!(path_str, "/lib/ld.so");
    }

    #[test]
    fn interp_rejects_non_nul_terminated_path() {
        let header = minimal_header();
        let interp_bytes = b"/lib/ld.so";
        let phdr = Elf64Phdr {
            p_type: PT_INTERP,
            p_flags: 0,
            p_offset: (size_of::<Elf64Ehdr>() + size_of::<Elf64Phdr>()) as u64,
            p_vaddr: 0,
            p_paddr: 0,
            p_filesz: interp_bytes.len() as u64,
            p_memsz: interp_bytes.len() as u64,
            p_align: 1,
        };
        let mut bytes = build_image(header, &[phdr]);
        let start = phdr.p_offset as usize;
        bytes[start..start + interp_bytes.len()].copy_from_slice(interp_bytes);

        let mut path = [0u8; ELF_INTERP_MAX];
        assert!(!get_interp(&bytes, &mut path));
    }
}
