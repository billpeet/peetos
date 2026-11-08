# Memory Management

This document summarizes the current memory management implementation: typed addresses, PMM, VMM, and the kernel heap.

**Where It Lives**
- `crates/arch_x86_64/src/addr.rs`
- `kernel/src/memory/pmm.rs`
- `kernel/src/memory/vmm.rs`
- `kernel/src/memory/heap.rs`
- `kernel/src/memory/mod.rs`
- `kernel/src/main.rs`

**Typed Addresses**
`crates/arch_x86_64/src/addr.rs` provides:
- `PhysAddr` and `VirtAddr` newtypes with alignment helpers.
- Constants for `PAGE_SIZE` and the higher-half base (`0xFFFF_FFFF_8000_0000`).
- Conversions between physical and higher-half virtual addresses.

These types are used throughout PMM/VMM/heap to avoid mixing address spaces.

Limitations / Simplifications
- `PhysAddr`/`VirtAddr` do not validate canonicality or bounds; they are lightweight wrappers only.
- Physical-to-virtual conversion assumes a fixed higher-half direct mapping.

**Physical Memory Manager (PMM)**
`kernel/src/memory/pmm.rs` implements a bitmap allocator with a 128 MB cap:
- Bitmap size: 128MB / 4KB = 32768 pages -> 4096-byte bitmap.
- E820 memory map is read from the bootloader at:
  - Entries: `0xFFFF_FFFF_8000_0500`
  - Count: `0xFFFF_FFFF_8000_07F0`
- Usable regions are marked free, then the kernel range and low 1MB are reserved.
- Allocation uses a search hint for faster next-fit allocation.

Public API:
- `init()`
- `alloc_page()`, `free_page()`
- `alloc_pages()`, `free_pages()` for contiguous ranges
- `mark_used()` for reserved pages
- `get_stats()` and `is_managed()`

Limitations / Simplifications
- Hard cap of 128 MB managed memory; higher RAM is ignored.
- E820 is read from fixed bootloader-provided addresses without additional validation.
- Protected by its own interrupt-safe spinlock (`PMM_LOCK`), taken at every public entry point. It is independent of the big kernel lock. There are no per-CPU freelists.
- Only reserves low 1MB and the kernel image range by default. SMP bring-up relies on that reservation: it reuses physical `0x8000-0xBFFF` for the AP trampoline and its transitional page tables (see `docs/smp.md`).

**Virtual Memory Manager (VMM)**
`kernel/src/memory/vmm.rs` manages 4-level x86_64 page tables:
- Reads CR3 to discover the current PML4.
- Assumes page tables are accessible via higher-half physical mapping.
- Supports mapping and unmapping 4KB pages, with defensive handling for 2MB/1GB large pages.
- Provides per-address-space helpers (`*_in`) and address space creation/cloning/clearing.

Key functions:
- `init()` reads CR3 and sets internal state.
- `map_page()`, `unmap_page()`
- `map_range()`, `unmap_range()`
- `get_physical()`, `get_physical_in()` and `is_mapped()`
- `get_pte_in()` (raw 4 KiB leaf entry, used by `mremap` to move pages with their flags)
- `set_flags()`
- `create_address_space()`, `destroy_address_space()`
- `clone_user_space()` (copy-on-write fork clone with a skip predicate) and `clear_user_space()`
- `resolve_cow_in()` breaks copy-on-write sharing for one page; `cow_share_pte()` / `cow_adjust_flags()` are the pure PTE rules (see `docs/process_model.md`)
- `switch()` and `invalidate_page()`

User `mremap`:
- `mremap(old, old_size, new_size, flags)` (`kernel/src/syscall/vm.rs`) shrinks in place, grows in place when the following range is free, or with `MREMAP_MAYMOVE` relocates the mapping.
- Relocation moves the present page-table entries instead of copying (`vmm::move_page_in`, one `VMM_LOCK` hold per page so other CPUs never see a page mapped twice or missing), so anonymous memory, page-cache pins (tracked by physical address), and dirty state follow the mapping; if a page-table allocation fails midway, already-moved pages are put back.
- Growing a shared writable file mapping re-validates the file window, and growth is counted against the per-task mapping budget.

User shared anonymous mappings:
- `mmap(MAP_SHARED | MAP_ANONYMOUS)` is backed by a hidden tmpfs file created in the memfd storage directory and unlinked at once (`mmap_shared_anon_backing` in `kernel/src/syscall/vm.rs`). Its pages live in the shared VM page cache, so a `fork`ed child sees the same memory.
- File-backed mappings pin their inode: `vm_sync_inode_pins` keeps one fs open reference per inode that any task maps, so the hidden file (or any regular file unlinked while mapped) is freed exactly when its last mapping goes away.

RAII support:
- `MappingGuard` unmaps a range on drop (intended for temporary mappings).

Limitations / Simplifications
- Only 4 KB mappings are created; large pages are detected but not allocated.
- Page table allocation is simple and not reference-counted; `destroy_address_space` only frees the PML4. Data frames carry a 16-bit PMM share count for copy-on-write; page-table frames are never shared.
- Page-table updates are serialised by `VMM_LOCK`, an interrupt-safe spinlock taken at the public entry points. Internal calls use `*_locked` variants. `switch` and the getters take no lock. Lock order is heap, then VMM, then PMM.
- TLB shootdown is coarse. Removing or downgrading a present mapping sends an IPI that makes every CPU using that address space (or every CPU, for kernel addresses) flush its whole TLB. Shootdowns are not batched across pages, and there is no PCID or other advanced address-space isolation.
- Each task has 256 region slots. Inserting a region merges it in place with adjacent regions that have the same protection, sharing, and backing (contiguous offsets for file mappings), as Linux merges VMAs, so allocators that map many small adjacent areas (musl's mallocng, for example while libxkbcommon compiles a keymap) use few slots. Before merging existed, such programs ran out of slots and `mmap` returned `ENOMEM`. `mprotect` also re-merges the pieces it splits once their protections match again (as after musl's RELRO), and a split that would need more slots than are free fails before the table changes.
- Shared writable file mappings may cover the partial page at the end of the file (the window is checked against the file size rounded up to a page), matching Linux.
- A table is 8 KiB, half the 16 KiB kernel stack, so it is never copied onto the stack. `mmap`, `mremap` and `munmap` build the new table in a heap copy (`RegionScratch`, about 8 KiB of kmalloc per call) and commit it once every step has succeeded. Everything else borrows `TASK_VM_MAPPINGS` in place (`task_regions`, `region_table_copy`). An earlier attempt at 128 slots, still with stack copies, overflowed the stack in `mmap`.
- The tables are a static 512 KiB array (64 tasks x 256 slots), zero-initialised so it sits in `.bss` rather than in the kernel image. Threads share their leader's table, so the thread slots' tables go unused.
- `mremap` requires the old range to lie inside one existing mapping, does not support `MREMAP_FIXED` or `MREMAP_DONTUNMAP`, and refuses to resize framebuffer mappings.
- `MAP_SHARED | MAP_ANONYMOUS` regions are backed by a hidden, unlinked tmpfs inode whose contents live in a shmem object (up to 64 MiB, the same storage as memfd). Each region uses one VFS node and one of the 64 shmem objects until it is unmapped everywhere.
- Inode pins are recomputed by scanning every task's region table (64 tasks x 256 regions, 16384 entries) after each `mmap`/`munmap`/teardown. That is linear; a per-inode mapping count would avoid it if it shows up in profiles.

**Kernel Heap (kmalloc)**
`kernel/src/memory/heap.rs` is a simple first-fit allocator:
- Heap range: `0xFFFF_FFFF_9000_0000` to `+64MB`.
- Backed by PMM + VMM mappings via `heap_expand()`.
- Supports `kmalloc`, `kzalloc`, `kmalloc_aligned`, `krealloc`, and `kfree`.

Block layout:
- `BlockHeader` (magic, flags, size) followed by payload.
- `kmalloc_aligned` over-allocates a normal block and returns an aligned
  pointer inside it. The 16 bytes in front of that pointer hold a tag header
  (`KMALLOC_ALIGNED_MAGIC`, with the original block pointer in `size`) so
  `kfree`/`krealloc` can tell it from a real header. Stale bytes from earlier
  allocations must never be read as a header: they used to cause bogus
  "double free" reports and leaks when AHCI DMA buffers were freed.
- Free list is sorted by address and coalesced on free.

Limitations / Simplifications
- Single global heap protected by its own interrupt-safe spinlock (`HEAP_LOCK`, taken by `kmalloc`/`kfree`/`get_stats`). Lock order is heap, then PMM. Heap expansion maps pages through the VMM, which still relies on the big kernel lock.
- Fixed 64 MB heap range with no guard pages or shrinking.
- First-fit allocator only; no slabs, per-size caches, or fragmentation mitigation beyond coalescing.

**Initialization Order**
1. `pmm::init()`
2. `vmm::init()`
3. `heap::init()`

This order is enforced in `kernel/src/main.rs`.

**Status Notes**
- PMM and VMM are functional but still conservative about large pages.
- Heap is suitable for early kernel allocations; there is no slab/buddy layer yet.
- memfd contents live in page-backed shmem objects (`kernel/src/shmem.rs`): up to 64 MiB per object in lazily allocated pages tracked through index pages. Each user mapping of a shmem page takes a PMM share reference (`pmm::page_share`) that the normal unmap/exit paths drop with `pmm::free_page`, so mapped pages outlive the object.
- Shared-buffer bring-up now includes `memfd_create` fd objects for
  userspace sharing workflows, plus baseline writable shared `mmap`
  reliability for compositor/client buffer exchange; per-task VMA mapping
  coverage now has an explicit hardening cap (currently 8192 pages / 32 MiB
  virtual range) while broader VM policy work remains tracked separately in the
  GUI roadmap.
