# AGENTS.md

This repo is the primary Rust implementation of the PeetOS kernel.
`../peetos_c` is now a historical reference for behavior comparisons, not the
main project target.

**Basics**
- Target: `x86_64` bare metal, higher-half kernel.
- Entry: `kernel/arch/x86_64/entry.asm` jumps to Rust `kmain()` in `kernel/src/main.rs`.
- Linker: `linker/kernel.ld` (reused from C).
- Rust is `no_std` with `panic=abort`.
- Build is driven by `Makefile` (`make image`, `make run`).
- `.cargo/config.toml` sets the default target to `x86_64-unknown-none`.

**Testing**
- Use the repo test harness: `make test`.
- `make test` runs:
  - Host unit tests (`tests/unit.sh`), currently for serial formatting.
  - QEMU smoke test (`tests/smoke.sh`) using `isa-debug-exit` so QEMU exits automatically.
- The smoke test builds with `KERNEL_FEATURES=test-exit` so the kernel can exit QEMU; do not bake test-only behavior into default builds.

**Best Practices**
- Always add tests for new functionality:
  - Prefer host unit tests when logic can be isolated.
  - Add/extend QEMU smoke or integration tests as needed.
- Always update docs for new features:
  - `README.md` for user-facing behavior and how to run things.
  - `docs/PORTING_PLAN.md` for status and roadmap tracking.
- Keep subsystem docs in `docs/` up to date. Each section must include a "Limitations / Simplifications" subsection describing known gaps, shortcuts, or future hardening needs.
- Keep the default build clean and minimal; gate test-only behavior behind features.
- Preserve the existing boot flow and linker setup unless there is a strong reason to change it.

**Historical Reference**
- `../peetos_c/docs/ROADMAP.md` and `../peetos_c/kernel/src` can still be used to compare legacy behavior when useful.
