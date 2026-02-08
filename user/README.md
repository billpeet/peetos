# User Programs (Embedded)

User images embedded by the kernel:

- `user/hello.elf`: minimal static ELF64 baseline.
- `user/hello_dyn.elf`: PIE `ET_DYN` main image with `PT_INTERP` set to `/lib/ld-peetos.so`.
- `user/ld-peetos.so`: minimal interpreter stub that reads `AT_ENTRY` from `auxv` and jumps to the program entry point.
- `user/sample_ext2.img`: sample ext2 image (built from `user/ext2_root/`) mounted at `/mnt/ext2` with in-memory write support.

Regenerate the binary if needed:

```bash
make user
```

Regenerate only the ext2 sample image:

```bash
make ext2-image
```

These binaries are embedded in the kernel via `include_bytes!`, and `make image`
rebuilds them automatically.
