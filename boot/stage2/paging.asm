; ===========================================================================
; Paging Setup for Long Mode - paging.asm
; ===========================================================================
;
; WHAT IS PAGING?
; ===============
; Paging is a memory management scheme that translates virtual addresses
; (what programs use) to physical addresses (actual memory locations).
;
; Benefits of paging:
;   - Memory isolation between processes
;   - Virtual memory (swap to disk)
;   - Memory protection (read-only, no-execute, etc.)
;   - Flexible memory layout
;
; LONG MODE PAGING REQUIREMENT:
; =============================
; In x86_64 Long Mode, paging is MANDATORY. You cannot run in Long Mode
; without paging enabled. This is unlike 32-bit Protected Mode where
; paging was optional.
;
; 4-LEVEL PAGING STRUCTURE:
; =========================
; Long Mode uses 4-level page tables to translate 48-bit virtual addresses
; to physical addresses. (The full 64-bit address space isn't used yet.)
;
; Virtual Address (48 bits used):
; ┌───────────────────────────────────────────────────────────────────────┐
; │ Sign extend │ PML4 idx  │ PDPT idx  │ PDT idx   │ PT idx    │ Offset  │
; │  (16 bits)  │ (9 bits)  │ (9 bits)  │ (9 bits)  │ (9 bits)  │(12 bits)│
; │   [63:48]   │  [47:39]  │  [38:30]  │  [29:21]  │  [20:12]  │ [11:0]  │
; └───────────────────────────────────────────────────────────────────────┘
;
; PAGE TABLE HIERARCHY:
; =====================
;
;  PML4 (Page Map Level 4)
;    │
;    ├─────────────────────────> PDPT (Page Directory Pointer Table)
;    │                             │
;    │                             ├───────────────> PDT (Page Directory Table)
;    │                             │                   │
;    │                             │                   ├─────────> PT (Page Table)
;    │                             │                   │             │
;    │                             │                   │             └─> Physical Page
;    │                             │                   │                  (4 KB)
;    │                             │                   │
;    │                             │                   └─────────> (Alternative)
;    │                             │                                 Large Page
;    │                             │                                 (2 MB)
;    │                             │
;    │                             └───────────────> (Alternative)
;    │                                                 Huge Page
;    │                                                 (1 GB)
;    └─────────────────────────> ...
;
; Each table has 512 entries (indexed by 9-bit field).
; Each entry is 8 bytes (64 bits).
; So each table is 512 * 8 = 4096 bytes = 4 KB.
;
; PAGE TABLE ENTRY FORMAT:
; ========================
; ┌────────────────────────────────────────────────────────────────────────┐
; │ Available │  NX  │ Available │ Address [51:12] │ AVL │ Flags │  P  │
; │  [63:52]  │ [63] │  [62:52]  │     [51:12]     │[11:9]│[8:1] │ [0] │
; └────────────────────────────────────────────────────────────────────────┘
;
; Key flags (bits 0-11):
;   Bit 0 (P)   - Present: Entry is valid
;   Bit 1 (R/W) - Read/Write: 0=read-only, 1=writable
;   Bit 2 (U/S) - User/Supervisor: 0=kernel only, 1=user accessible
;   Bit 3 (PWT) - Page Write-Through
;   Bit 4 (PCD) - Page Cache Disable
;   Bit 5 (A)   - Accessed: Set by CPU when page is accessed
;   Bit 6 (D)   - Dirty: Set by CPU when page is written (PT entries only)
;   Bit 7 (PS)  - Page Size: 1=large page (2MB in PDT, 1GB in PDPT)
;
; OUR INITIAL PAGING SETUP:
; =========================
; We identity-map the first 128 MB of physical memory using 2MB pages.
; Identity mapping means virtual address = physical address.
; This lets us continue executing code after enabling paging without
; needing to jump to a different address.
;
; For identity mapping (bootloader only, removed by kernel):
;   - 1 PML4 entry pointing to PDPT
;   - 1 PDPT entry pointing to PDT
;   - 64 PDT entries, each mapping a 2 MB page (using PS bit)
;   - 64 * 2 MB = 128 MB total mapped
;
; For higher-half kernel mapping (permanent):
;   - PML4[511] -> PDPT_HIGH -> PD_HIGH
;   - PD_HIGH[0] -> PT (4KB pages for first 2MB) - FIXES BSS CLEARING
;   - PD_HIGH[1-63] -> 2MB pages for remaining 126MB
;   - Total: 128 MB mapped at 0xFFFFFFFF80000000
;
; We use 4KB pages for the kernel's first 2MB to avoid write buffer
; coherency issues with 2MB pages that were preventing BSS clearing.
;
; ===========================================================================

[BITS 32]                       ; This code runs in 32-bit Protected Mode
                                ; (we've already switched by the time we run this)

; Include constants for page table addresses
%include "boot/include/constants.inc"

; ===========================================================================
; SUBROUTINE: setup_paging
; ===========================================================================
; Sets up 4-level paging with identity mapping of the first 128 MB.
;
; This must be called from 32-bit Protected Mode, before switching to
; Long Mode.
;
; Memory layout for page tables:
;   0x10000 - PML4 (4 KB)
;   0x11000 - PDPT (4 KB) - identity mapping
;   0x12000 - PDT  (4 KB) - identity mapping
;   0x13000 - PT   (4 KB) - higher-half kernel (4KB pages)
;   0x14000 - PDPT_HIGH (4 KB) - higher-half mapping
;   0x15000 - PD_HIGH (4 KB) - higher-half mapping
;   0x16000 - PDPT_FB (4 KB) - framebuffer (if VESA)
;   0x17000 - PD_FB (4 KB) - framebuffer (if VESA)
;   Total: 32 KB of page tables
;
; Clobbers:
;   EAX, ECX, EDI
; ===========================================================================

setup_paging:
    ; -----------------------------------------------------------------------
    ; STEP 1: Clear all page table memory
    ; -----------------------------------------------------------------------
    ; We need to zero out 32 KB (8 pages of 4 KB each).
    ; This ensures all entries start as "not present".
    ; Tables: PML4, PDPT, PDT, PT (identity) + PDPT_HIGH, PD_HIGH (higher-half)
    ;         + PDPT_FB, PD_FB (framebuffer)

    mov edi, PML4_ADDR          ; Starting address (0x10000)
    mov ecx, 8192               ; 32 KB / 4 = 8192 dwords to clear
                                ; (We use STOSD which writes 4 bytes at a time)
    xor eax, eax                ; EAX = 0 (value to store)
    cld                         ; Clear direction flag (increment EDI)
    rep stosd                   ; Store EAX at [EDI], repeat ECX times

    ; -----------------------------------------------------------------------
    ; STEP 2: Set up PML4 entry 0
    ; -----------------------------------------------------------------------
    ; PML4[0] should point to our PDPT.
    ; Entry format: [Address of PDPT (page-aligned)] | [Flags]
    ;
    ; We set Present (P) and Read/Write (R/W) flags.

    mov edi, PML4_ADDR          ; EDI = address of PML4 (0x10000)
    mov eax, PDPT_ADDR          ; EAX = address of PDPT (0x11000)
    or eax, PAGE_PRESENT | PAGE_WRITE  ; Set Present and Writable flags
    mov dword [edi], eax        ; Store low 32 bits of entry
    mov dword [edi + 4], 0      ; Store high 32 bits (zero for addresses < 4GB)

    ; -----------------------------------------------------------------------
    ; STEP 3: Set up PDPT entry 0
    ; -----------------------------------------------------------------------
    ; PDPT[0] should point to our PDT.

    mov edi, PDPT_ADDR          ; EDI = address of PDPT (0x11000)
    mov eax, PDT_ADDR           ; EAX = address of PDT (0x12000)
    or eax, PAGE_PRESENT | PAGE_WRITE
    mov dword [edi], eax
    mov dword [edi + 4], 0

    ; -----------------------------------------------------------------------
    ; STEP 4: Fill PDT with 2MB pages for identity mapping
    ; -----------------------------------------------------------------------
    ; We use 2 MB pages (PS bit set) to identity-map the first 128 MB.
    ; This is simpler than using 4 KB pages and covers more memory.
    ;
    ; PDT[0] maps 0x00000000 - 0x001FFFFF (2 MB)
    ; PDT[1] maps 0x00200000 - 0x003FFFFF (2 MB)
    ; ...
    ; PDT[63] maps 0x07E00000 - 0x07FFFFFF (2 MB)
    ;
    ; Total: 64 * 2MB = 128 MB identity-mapped

    mov edi, PDT_ADDR           ; EDI = address of PDT (0x12000)
    mov eax, PAGE_PRESENT | PAGE_WRITE | (1 << 7)  ; Flags + PS bit for 2MB page
    mov ecx, 64                 ; 64 entries = 128 MB

.fill_pdt:
    ; Store this entry
    mov dword [edi], eax        ; Low 32 bits (address + flags)
    mov dword [edi + 4], 0      ; High 32 bits (zero for < 4GB)

    add eax, 0x200000           ; Next 2 MB physical region
    add edi, 8                  ; Next entry (each entry is 8 bytes)
    loop .fill_pdt              ; Decrement ECX, loop if not zero

    ; -----------------------------------------------------------------------
    ; STEP 6: Set up Higher-Half Mapping
    ; -----------------------------------------------------------------------
    ; We map the kernel at 0xFFFFFFFF80000000 (canonical high address).
    ; This requires PML4[511] -> PDPT_HIGH -> PD_HIGH -> physical memory
    ;
    ; We use 2MB pages for simplicity:
    ;   0xFFFFFFFF80000000 - 0xFFFFFFFF801FFFFF -> Physical 0x00000000 - 0x001FFFFF
    ;
    ; The kernel is at physical 0x100000, so it will be at virtual
    ; 0xFFFFFFFF80100000.

    ; PML4[511] -> PDPT_HIGH
    mov edi, PML4_ADDR
    add edi, 511 * 8            ; Entry 511 (index * 8 bytes per entry)
    mov eax, PDPT_HIGH_ADDR
    or eax, PAGE_PRESENT | PAGE_WRITE
    mov dword [edi], eax
    mov dword [edi + 4], 0

    ; PDPT_HIGH[510] -> PD_HIGH
    ; (PDPT index 510 because bits 38:30 of 0xFFFFFFFF80000000 = 0x1FE = 510)
    mov edi, PDPT_HIGH_ADDR
    add edi, 510 * 8            ; Entry 510
    mov eax, PD_HIGH_ADDR
    or eax, PAGE_PRESENT | PAGE_WRITE
    mov dword [edi], eax
    mov dword [edi + 4], 0

    ; Map higher-half kernel using 4KB pages (not 2MB pages)
    ; PD_HIGH[0] -> PT (Page Table at 0x13000)
    ; PT[0..511] -> 512 x 4KB pages at physical 0x00000000 - 0x001FFFFF (2 MB)
    ;
    ; This fixes BSS clearing issues with 2MB pages and follows OSDev best practices.

    ; Set up PD_HIGH[0] to point to PT (not a 2MB page!)
    mov edi, PD_HIGH_ADDR
    mov eax, PT_ADDR            ; PT is at 0x13000
    or eax, PAGE_PRESENT | PAGE_WRITE  ; NOTE: NO PS bit! This points to a PT
    mov dword [edi], eax
    mov dword [edi + 4], 0

    ; Fill PT with 512 entries for 4KB pages
    ; This covers physical 0x0 - 0x1FFFFF (2 MB), which includes:
    ; - Kernel code/data at 0x100000
    ; - VGA text buffer at 0xB8000
    ; - BSS section ending around 0x1AA000
    mov edi, PT_ADDR            ; EDI = address of PT (0x13000)
    xor eax, eax                ; Start at physical address 0
    or eax, PAGE_PRESENT | PAGE_WRITE  ; Flags (NO PS bit for 4KB pages!)
    mov ecx, 512                ; 512 entries = 2 MB

.fill_pt:
    mov dword [edi], eax        ; Low 32 bits (address + flags)
    mov dword [edi + 4], 0      ; High 32 bits (zero for < 4GB)
    add eax, 0x1000             ; Next 4 KB page
    add edi, 8                  ; Next entry (8 bytes)
    loop .fill_pt

    ; Map additional 2MB pages for extended memory
    ; PD_HIGH[1..63] -> 2MB pages at physical 0x00200000 - 0x07FFFFFF (126 MB)
    ; Total with PD_HIGH[0]: 2MB + 126MB = 128MB mapped
    mov edi, PD_HIGH_ADDR
    add edi, 8                  ; Skip PD_HIGH[0], start at PD_HIGH[1]
    mov eax, 0x00200000         ; Start at physical 2MB
    or eax, PAGE_PRESENT | PAGE_WRITE | (1 << 7)  ; PS bit = 2MB page
    mov ecx, 63                 ; Map 63 x 2MB = 126 MB (for total 128 MB)

.fill_pd_high_large:
    mov dword [edi], eax
    mov dword [edi + 4], 0
    add eax, 0x200000           ; Next 2 MB
    add edi, 8                  ; Next entry
    loop .fill_pd_high_large

    ; -----------------------------------------------------------------------
    ; STEP 7: Set up Framebuffer Mapping (if VESA mode set)
    ; -----------------------------------------------------------------------
    ; If VESA mode was set, the framebuffer is at a high physical address
    ; (typically around 0xFD000000). We map this to higher-half virtual
    ; address 0xFFFFFFFF81000000 (instead of identity mapping) to avoid
    ; cache aliasing issues.
    ;
    ; Check if VESA was initialized by reading VESA_STATUS_ADDR

    ; Check if VESA is available
    mov al, [VESA_STATUS_ADDR]
    test al, al
    jz .skip_fb_mapping             ; No framebuffer, skip mapping

    ; Get framebuffer physical address
    mov eax, [VESA_STATUS_ADDR + 4]
    test eax, eax
    jz .skip_fb_mapping             ; No address, skip

    ; Save framebuffer base address
    mov [.fb_base], eax

    ; Map framebuffer to higher-half virtual address 0xFFFFFFFF88000000
    ; This uses PML4[511] -> PDPT_HIGH[510] -> PD_HIGH[64+]
    ; (PD_HIGH[0-63] are used for kernel physical memory mapping)
    ;
    ; We'll map 16 MB starting at PD_HIGH[64] to cover the framebuffer

    ; Use PD_HIGH for framebuffer mapping (entries 64-71 for 16MB)
    mov edi, PD_HIGH_ADDR
    add edi, 64 * 8                 ; Start at PD_HIGH[64] (8 bytes per entry)

    ; Get base address aligned to 2MB boundary
    mov eax, [.fb_base]
    and eax, 0xFFE00000             ; Align to 2MB
    or eax, PAGE_PRESENT | PAGE_WRITE | (1 << 7)  ; 2MB page flags

    mov ecx, 8                      ; Map 8 x 2MB = 16MB
.map_fb_pages:
    mov dword [edi], eax
    mov dword [edi + 4], 0
    add eax, 0x200000               ; Next 2MB
    add edi, 8                      ; Next entry
    loop .map_fb_pages

    jmp .fb_mapping_done

.fb_base: dd 0

.skip_fb_mapping:
.fb_mapping_done:

    ; -----------------------------------------------------------------------
    ; PAGING SETUP COMPLETE
    ; -----------------------------------------------------------------------
    ; At this point we have:
    ;
    ; Identity mapping (low memory - 128 MB using 2MB pages):
    ;   - PML4[0] -> PDPT at 0x11000
    ;   - PDPT[0] -> PDT at 0x12000
    ;   - PDT[0..63] -> 2MB pages at physical 0x00000000 - 0x07FFFFFF
    ;   - Virtual 0x00000000 - 0x07FFFFFF -> Physical 0x00000000 - 0x07FFFFFF
    ;   NOTE: This will be UNMAPPED by kernel/entry.asm to prevent cache aliasing!
    ;
    ; Higher-half mapping (kernel) - USING 4KB PAGES:
    ;   - PML4[511] -> PDPT_HIGH at 0x14000
    ;   - PDPT_HIGH[510] -> PD_HIGH at 0x15000
    ;   - PD_HIGH[0] -> PT at 0x13000 (Page Table for first 2MB)
    ;   - PT[0..511] -> 512 x 4KB pages at physical 0x00000000 - 0x001FFFFF (2 MB)
    ;   - PD_HIGH[1..63] -> 63 x 2MB pages at physical 0x00200000 - 0x07FFFFFF (126 MB)
    ;   - Virtual 0xFFFFFFFF80000000 - 0xFFFFFFFF87FFFFFF -> Physical 0x00000000 - 0x07FFFFFF
    ;   NOTE: First 2MB uses 4KB pages to fix BSS clearing issues!
    ;
    ; Framebuffer mapping (if VESA enabled, mapped to higher-half):
    ;   - PML4[511] -> PDPT_HIGH[510] -> PD_HIGH (same as kernel)
    ;   - PD_HIGH[64..71] -> 8 x 2MB pages for framebuffer physical address
    ;   - Virtual 0xFFFFFFFF88000000 - 0xFFFFFFFF88FFFFFF -> Physical fb_addr (16MB)
    ;
    ; VGA text buffer:
    ;   - Physical 0xB8000 is within the kernel mapping (first 2 MB, 4KB pages)
    ;   - Accessible at virtual 0xFFFFFFFF800B8000
    ;
    ; The kernel at physical 0x100000 is accessible at:
    ;   - 0x100000 (identity mapped, ONLY during bootloader, removed by kernel)
    ;   - 0xFFFFFFFF80100000 (higher-half, used by kernel)

    ret

; ===========================================================================
; SUBROUTINE: enable_paging
; ===========================================================================
; Enables paging by loading CR3 and setting CR0.PG.
;
; IMPORTANT: This should be called AFTER enabling Long Mode in EFER!
; The sequence is:
;   1. Set up page tables (setup_paging)
;   2. Enable PAE in CR4
;   3. Load CR3 with PML4 address
;   4. Enable Long Mode in EFER MSR
;   5. Enable Paging in CR0 (this function)
;
; When we set CR0.PG with EFER.LME already set, the CPU automatically
; enters Long Mode.
;
; Clobbers:
;   EAX
; ===========================================================================

enable_paging:
    ; Load CR3 with PML4 address
    ; CR3 holds the physical address of the top-level page table
    mov eax, PML4_ADDR
    mov cr3, eax

    ; Enable Paging (CR0.PG)
    mov eax, cr0
    or eax, CR0_PG              ; Set bit 31 (Paging)
    mov cr0, eax                ; This activates paging!
                                ; If EFER.LME was set, we're now in Long Mode

    ret

; ===========================================================================
; SUBROUTINE: enable_pae
; ===========================================================================
; Enables Physical Address Extension (PAE) in CR4.
;
; PAE is REQUIRED for Long Mode. It extends the page table entries from
; 32-bit to 64-bit, allowing physical addresses larger than 4 GB.
;
; Clobbers:
;   EAX
; ===========================================================================

enable_pae:
    mov eax, cr4
    or eax, CR4_PAE             ; Set bit 5 (PAE)
    mov cr4, eax
    ret

; ===========================================================================
; SUBROUTINE: enable_long_mode
; ===========================================================================
; Enables Long Mode by setting EFER.LME bit.
;
; EFER (Extended Feature Enable Register) is a Model-Specific Register (MSR).
; MSRs are accessed via special instructions: RDMSR and WRMSR.
;
; To read an MSR:
;   MOV ECX, <MSR number>
;   RDMSR
;   ; EDX:EAX now contains the MSR value
;
; To write an MSR:
;   MOV ECX, <MSR number>
;   ; Put new value in EDX:EAX
;   WRMSR
;
; EFER is MSR 0xC0000080.
; Bit 8 (LME) is the Long Mode Enable bit.
;
; Note: This just ENABLES Long Mode. It doesn't ACTIVATE it yet.
; Long Mode is activated when we enable paging (CR0.PG = 1).
;
; Clobbers:
;   EAX, ECX, EDX
; ===========================================================================

enable_long_mode:
    mov ecx, EFER_MSR           ; ECX = 0xC0000080 (EFER MSR number)
    rdmsr                       ; Read EFER into EDX:EAX

    or eax, EFER_LME            ; Set bit 8 (Long Mode Enable)
    or eax, EFER_NXE            ; Set bit 11 (No Execute Enable - required for NX page bit)
    wrmsr                       ; Write back to EFER

    ret
