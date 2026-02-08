# Subsystem Crate Stubs

This document summarizes subsystem crates that exist as placeholders for later phases.

**Where They Live**
- `crates/drivers`
- `crates/fs`
- `crates/net`
- `crates/syscall`
- `crates/userabi`

**Current State**
Each crate is `no_std` and currently contains only a placeholder module comment. They exist to reserve the workspace layout and will be populated as phases 4–9 are implemented.

Limitations / Simplifications
- No runtime functionality is provided yet.
- No public APIs are stabilized; the crates are not wired into the kernel build.
