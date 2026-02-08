# ELF Loading

This document describes the minimal ELF loader used for the Phase 4 user-mode bring-up. The loader accepts an embedded ELF64 image and maps its loadable segments into a fresh user address space.

**Where It Lives**
- `kernel/src/elf.rs`
- `kernel/src/user.rs`
- `user/hello.asm`
- `user/hello.elf`
- `scripts/build_user.sh`

**Supported Format**
The loader only supports static ELF64 executables built as `ET_EXEC` for `x86_64` little-endian. It validates the ELF header and program header table before loading.

**Load Flow**
1. `user::spawn_init()` creates a new address space (`vmm::create_address_space()`).
2. `elf::load()` validates the ELF header and program headers.
3. For each `PT_LOAD` segment:
   - The virtual address range is aligned to 4KB pages.
   - Physical pages are allocated via `pmm::alloc_page()` and mapped with `vmm::map_page_in()`.
   - Pages are zeroed, then segment bytes are copied from the ELF image into the mapped pages.
4. The loader returns the entry point (`e_entry`) and the top of the loaded image (`program_break`) for future heap work.
5. `user::spawn_init()` sets up a 32KB user stack and builds an `InterruptFrame` to enter ring 3 at the ELF entry point.

Limitations / Simplifications
- Only `ET_EXEC` ELF64 images are accepted (no PIE/`ET_DYN`).
- Only `PT_LOAD` segments are handled; `PT_INTERP`, `PT_DYNAMIC`, `PT_TLS`, and relocations are ignored.
- No dynamic linking or symbol resolution.
- Segment permissions are minimal; `PF_X` does not affect page attributes (NX is not enabled).
- No W^X enforcement or SMEP/SMAP integration.
- No validation that segment virtual addresses stay within a safe user range.
- `program_break` is reported but not wired into a `brk`/heap syscall yet.
- The embedded ELF is rebuilt manually via `scripts/build_user.sh`; no build integration in the Makefile yet.
