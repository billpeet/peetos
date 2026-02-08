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
2. `elf::load()` validates the ELF header and program headers (`e_version`, header sizes, `PT_LOAD` bounds/alignment rules, malformed/duplicate `PT_INTERP`, and rejects RWX load segments).
3. For each `PT_LOAD` segment:
   - The virtual address range is aligned to 4KB pages.
   - Physical pages are allocated via `pmm::alloc_page()` and mapped with `vmm::map_page_in()`.
   - Pages are zeroed, then segment bytes are copied from the ELF image into the mapped pages.
4. Before committing load success, the loader verifies the final entry point is inside an executable user `PT_LOAD` range.
5. The loader records program header metadata (PHDR address/count/size), detects dynamic metadata (`ET_DYN`, `PT_INTERP`, `PT_DYNAMIC`), and returns the entry point (`e_entry`) plus the top of the loaded image (`program_break`) for future heap work.
6. `user::spawn_init()` sets up a 32KB user stack, writes a Linux-style initial stack image (`argc`, `argv`, `envp`, `auxv` with `AT_PHDR`/`AT_PHENT`/`AT_PHNUM`/`AT_PAGESZ`/`AT_BASE`/`AT_ENTRY`), and builds an `InterruptFrame` to enter ring 3 at the ELF entry point.

Limitations / Simplifications
- `elf::load_at()` accepts PIE/`ET_DYN`, but relocations and `PT_DYNAMIC` entry parsing are not implemented.
- `PT_INTERP` is detected and stored, but the interpreter is not loaded yet.
- No dynamic linking or symbol resolution.
- Segment permissions set `PTE_NO_EXECUTE`, and `EFER.NXE` is enabled during syscall/MSR setup.
- Segment-level W^X is enforced by rejecting `PF_W | PF_X` `PT_LOAD` segments, but there is no SMEP/SMAP integration.
- User address checks are stricter than the initial baseline (checked arithmetic + entrypoint-in-exec-range checks), but there is still no ASLR.
- `program_break` is now consumed by per-task `brk` state, but the full userspace allocator contract (`mmap`, overcommit, guard pages) is not implemented yet.
- Initial `argv`/`envp` values are currently fixed for the embedded init task; there is no `execve` path to supply caller-provided vectors yet.
- Load failure rollback is still minimal; pages mapped before a late failure are not fully unwound.
- The embedded ELF is rebuilt automatically via the Makefile; `scripts/build_user.sh` remains a manual helper.
