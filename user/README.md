# User Programs (Embedded)

`user/hello.elf` is a minimal static ELF64 binary used for early user-mode bring-up. It invokes `int 0x80` with Linux x86_64 `SYS_EXIT` (`60`).

Regenerate the binary if needed:

```bash
make user
```

This binary is embedded in the kernel via `include_bytes!`, and `make image`
rebuilds it automatically.
