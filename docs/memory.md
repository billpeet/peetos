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
- Relocation moves the present page-table entries instead of copying, so anonymous memory, page-cache pins (tracked by physical address), and dirty state follow the mapping; if a page-table allocation fails midway, already-moved pages are put back.
- Growing a shared writable file mapping re-validates the file window, and growth is counted against the per-task mapping budget.

RAII support:
- `MappingGuard` unmaps a range on drop (intended for temporary mappings).

Limitations / Simplifications
- Only 4 KB mappings are created; large pages are detected but not allocated.
- Page table allocation is simple and not reference-counted; `destroy_address_space` only frees the PML4. Data frames carry a 16-bit PMM share count for copy-on-write; page-table frames are never shared.
- TLB shootdown is coarse. Removing or downgrading a present mapping sends an IPI that makes every CPU using that address space (or every CPU, for kernel addresses) flush its whole TLB. Shootdowns are not batched across pages, and there is no PCID or other advanced address-space isolation.
- `mremap` requires the old range to lie inside one existing mapping, does not support `MREMAP_FIXED` or `MREMAP_DONTUNMAP`, and refuses to resize framebuffer mappings.

**Kernel Heap (kmalloc)**
`kernel/src/memory/heap.rs` is a simple first-fit allocator:
- Heap range: `0xFFFF_FFFF_9000_0000` to `+64MB`.
- Backed by PMM + VMM mappings via `heap_expand()`.
- Supports `kmalloc`, `kzalloc`, `kmalloc_aligned`, `krealloc`, and `kfree`.

Block layout:
- `BlockHeader` (magic, flags, size) followed by payload.
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
- Shared-buffer bring-up now includes `memfd_create` tmpfs-backed fd objects for
  userspace sharing workflows, plus baseline writable shared `mmap`
  reliability for compositor/client buffer exchange; per-task VMA mapping
  coverage now has an explicit hardening cap (currently 8192 pages / 32 MiB
  virtual range) while broader VM policy work remains tracked separately in the
  GUI roadmap.
