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
- Not SMP-safe; no locking or per-CPU freelists.
- Only reserves low 1MB and the kernel image range by default.

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
- `set_flags()`
- `create_address_space()`, `destroy_address_space()`
- `clone_user_space()` and `clear_user_space()`
- `switch()` and `invalidate_page()`

RAII support:
- `MappingGuard` unmaps a range on drop (intended for temporary mappings).

Limitations / Simplifications
- Only 4 KB mappings are created; large pages are detected but not allocated.
- Page table allocation is simple and not reference-counted; `destroy_address_space` only frees the PML4.
- No TLB shootdown across CPUs, PCID, or advanced address-space isolation features.

**Kernel Heap (kmalloc)**
`kernel/src/memory/heap.rs` is a simple first-fit allocator:
- Heap range: `0xFFFF_FFFF_9000_0000` to `+64MB`.
- Backed by PMM + VMM mappings via `heap_expand()`.
- Supports `kmalloc`, `kzalloc`, `kmalloc_aligned`, `krealloc`, and `kfree`.

Block layout:
- `BlockHeader` (magic, flags, size) followed by payload.
- Free list is sorted by address and coalesced on free.

Limitations / Simplifications
- Single global heap with no locking; not safe for concurrent allocations.
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
