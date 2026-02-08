# ELF Loading

This document describes the ELF loader used for the Phase 4 user-mode bring-up. The loader accepts an embedded ELF64 image and maps its loadable segments into a fresh user address space.

**Where It Lives**
- `kernel/src/elf.rs`
- `kernel/src/user.rs`
- `user/hello.asm`
- `user/hello.elf`
- `scripts/build_user.sh`

**Supported Format**
The loader supports ELF64 for `x86_64` little-endian. `elf::load()` accepts `ET_EXEC` binaries, while `elf::load_at()` accepts both `ET_EXEC` and `ET_DYN` (PIE/shared objects) and applies a load bias.

**Load Flow**
1. `user::spawn_init()` creates a new address space (`vmm::create_address_space()`).
2. `elf::load()` validates the ELF header and program headers.
3. For each `PT_LOAD` segment:
   - The virtual address range is aligned to 4KB pages.
   - Physical pages are allocated via `pmm::alloc_page()` and mapped with `vmm::map_page_in()`.
   - Pages are zeroed, then segment bytes are copied from the ELF image into the mapped pages.
4. The loader records program header metadata (PHDR address/count/size), detects `PT_INTERP` for dynamic binaries, and returns the entry point (`e_entry`) plus the top of the loaded image (`program_break`) for future heap work.
5. `user::spawn_init()` sets up a 32KB user stack and builds an `InterruptFrame` to enter ring 3 at the ELF entry point.

Limitations / Simplifications
- `elf::load_at()` accepts PIE/`ET_DYN`, but relocations and `PT_DYNAMIC` parsing are not implemented.
- `PT_INTERP` is detected and stored, but the interpreter is not loaded yet.
- No dynamic linking or symbol resolution.
- Segment permissions set `PTE_NO_EXECUTE` for non-executable segments, but NX is not enabled in EFER yet.
- No W^X enforcement or SMEP/SMAP integration.
- User address range checks are minimal; there is no ASLR.
- `program_break` is reported but not wired into a `brk`/heap syscall yet.
- The embedded ELF is rebuilt automatically via the Makefile, but `scripts/build_user.sh` is still manual.
