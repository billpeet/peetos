# ELF Loading

This document describes the ELF loader used for user-mode bring-up. The loader accepts an embedded ELF64 image and maps its loadable segments into a fresh user address space.

**Where It Lives**
- `kernel/src/elf.rs`
- `kernel/src/user.rs`
- `user/hello.asm`
- `user/hello_dyn.asm`
- `user/ld_peetos.asm`
- `user/hello.elf`
- `user/hello_dyn.elf`
- `user/ld-peetos.so`
- `scripts/build_user.sh`
- `scripts/build_musl_shared.sh`, `scripts/build_dynlink_smoke.sh`
- `user/dynlink_smoke.c`, `user/dynlink/`

**Supported Format**
The loader supports ELF64 for `x86_64` little-endian. `elf::load()` accepts `ET_EXEC` binaries, while `elf::load_at()` accepts both `ET_EXEC` and `ET_DYN` (PIE/shared objects) and applies a load bias.

**Load Flow**
1. `user::spawn_init()` creates a new address space (`vmm::create_address_space()`).
2. The main image is validated and loaded (`elf::load_at()` for PIE `ET_DYN` init image, or `elf::load()` for `ET_EXEC` fallback). Validation covers header sizes, `PT_LOAD` bounds/alignment rules, malformed/duplicate `PT_INTERP`, and rejects RWX load segments.
3. For each `PT_LOAD` segment:
   - The virtual address range is aligned to 4KB pages.
   - Physical pages are allocated via `pmm::alloc_page()` and mapped with `vmm::map_page_in()`.
   - Pages are zeroed, then segment bytes are copied from the ELF image into the mapped pages.
4. If the main image has `PT_INTERP`, the interpreter path is resolved and loaded with `elf::load_at()` at a fixed user base (`/lib/ld-peetos.so` is embedded for bootstrap; `execve` also supports VFS-backed interpreter lookup).
5. Before committing load success, the loader verifies each final entry point is inside an executable user `PT_LOAD` range.
6. The loader records program header metadata (PHDR address/count/size), detects dynamic metadata (`ET_DYN`, `PT_INTERP`, `PT_DYNAMIC`), and returns the entry point (`e_entry`) plus the top of the loaded image (`program_break`) for future heap work.
7. `user::spawn_init()` sets up a 32KB user stack, writes a Linux-style initial stack image (`argc`, `argv`, `envp`, `auxv` with `AT_PHDR`/`AT_PHENT`/`AT_PHNUM`/`AT_PAGESZ`/`AT_BASE`/`AT_ENTRY` plus baseline identity/random entries like `AT_UID`/`AT_EUID`/`AT_GID`/`AT_EGID`/`AT_SECURE`/`AT_RANDOM`/`AT_EXECFN`), and builds an `InterruptFrame` that starts at the interpreter entry (when present) or the main entry.
8. `execve` now reuses the same loader/stack path to replace the current task image from a VFS file and apply caller-provided `argv`/`envp`.

Limitations / Simplifications
- The kernel never processes relocations or `PT_DYNAMIC` itself; that is the interpreter's job (see "Dynamic Linking" below).
- `PT_INTERP` is loaded from the embedded bootstrap image or, for `execve`, from the VFS (up to 2 MiB, read whole into a kernel buffer). There is still no shebang-style script resolution.
- Segment permissions set `PTE_NO_EXECUTE`, and `EFER.NXE` is enabled during syscall/MSR setup.
- Segment-level W^X is enforced by rejecting `PF_W | PF_X` `PT_LOAD` segments, but there is no SMEP/SMAP integration.
- User address checks are stricter than the initial baseline (checked arithmetic + entrypoint-in-exec-range checks), but there is still no ASLR.
- `program_break` is consumed by per-task `brk` state, and baseline `mmap`/`mprotect`/`munmap` syscalls are now available for dynamic-loader bring-up; advanced VM policy (overcommit/accounting, VMA metadata, guard-page policy) is still pending.
- Initial `argv`/`envp` for bootstrap remain fixed, but `execve` now accepts caller-provided vectors (with bounded argument/environment counts and string lengths).
- Load failure rollback is still minimal; pages mapped before a late failure are not fully unwound.
- The embedded ELF is rebuilt automatically via the Makefile; `scripts/build_user.sh` remains a manual helper.

## Dynamic Linking

Dynamically linked programs run through musl's own dynamic loader. musl's
shared `libc.so` is also its loader; it is installed in the ext2 image as
`/lib/ld-musl-x86_64.so.1`, the interpreter path `musl-gcc` already writes
into `PT_INTERP`.

**How it works**
1. `execve` loads the program (PIE `ET_DYN` at `0x400000`) and its
   `PT_INTERP` loader (at `0x7000_0000`) eagerly, and passes `AT_PHDR`,
   `AT_ENTRY`, `AT_BASE` and the other auxv entries on the initial stack.
2. The loader relocates itself and the program, then opens `NEEDED` libraries
   from its default path (`/lib:/usr/local/lib:/usr/lib`) and maps them with
   ordinary file-backed `mmap` calls.
3. It applies RELRO with `mprotect(PROT_READ)` to each module, including the
   program and itself. Those two live below `MMAP_BASE_ADDR` and are not in the
   VM region table, so `mprotect` handles that low, execve-loaded range by
   updating the page-table entries directly (`mprotect_target()` in
   `kernel/src/syscall/vm.rs`).
4. Static TLS for startup modules and dynamically allocated TLS for `dlopen`ed
   modules are set up in user space (`arch_prctl(ARCH_SET_FS)` and
   `__tls_get_addr`); no other kernel support is needed.

**Building**
- `make -C musl shared` (or `scripts/build_musl_shared.sh`) builds musl's
  `libc.so` out of tree into `musl/shared/lib/` without touching the static
  sysroot.
- Link a dynamic program with `musl-gcc -L musl/shared/lib ... -fPIE -pie`;
  the extra `-L` makes the linker pick `libc.so` over the sysroot's `libc.a`.
- The ext2 builder stages `/lib/ld-musl-x86_64.so.1` whenever
  `musl/shared/lib/libc.so` exists.

**Testing**
`make run-dynlink-smoke` (`tests/musl_ext2_dynlink_smoke.sh`, part of
`make test`) runs `dynlink_smoke`, a dynamically linked PIE. It checks
`libc.so` stdio and malloc, then a `NEEDED` library (`libpeetdyn.so`: its
constructor, functions, exported data and a TLS variable). It then checks a
`dlopen`ed plugin (`libpeetplugin.so`), which depends on `libpeetdyn.so` and has
its own dynamic TLS, plus the `dlerror` path for a missing library. Finally it
checks fresh TLS copies in a new pthread and a dynamic program that forks and
`execve`s itself. The libraries live in `/usr/lib`.

Limitations / Simplifications
- The ported libraries (libwayland, libffi, libxkbcommon) and the staged
  programs are still linked statically; only the smoke test links dynamically
  so far.
- `mprotect` on the execve-loaded range changes page-table entries only. It
  needs every page to be present (the range is mapped eagerly, but untouched
  `brk` pages are not) and keeps no per-range protection record.
- Each shared library takes several VM region slots (one per segment, merged
  where adjacent and compatible) out of the 32 per task, which limits how many
  libraries a process can load.
- There is no ASLR: the program, the loader and `mmap` placement are all
  deterministic.
- Libraries are read through file-backed private mappings. Pages are not
  shared between processes that load the same library.
