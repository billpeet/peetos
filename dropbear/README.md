# Dropbear Setup for PeetOS (Rust Port)

This directory bootstraps Dropbear source and builds static Dropbear tools for
PeetOS userspace staging.

## Quick Start

From repo root:

```bash
make dropbear-fetch
make dropbear
```

## Layout

- `dropbear/fetch-dropbear.sh`: downloads/extracts a pinned Dropbear release
  into `dropbear/dropbear-src`.
- `dropbear/Makefile`: wraps fetch/build/clean/info.
- `dropbear/peetos/localoptions.h`: PeetOS-specific Dropbear feature toggles for
  initial bring-up.
- `dropbear/dropbear-src` (generated): upstream Dropbear source tree.
- `dropbear/out/` (generated): built static binaries (`dropbearmulti`,
  `dropbear`, `dropbearkey`, `dbclient`, `scp`).

## Limitations / Simplifications

- Initial staged integration focuses on baseline daemon/key tooling bring-up,
  not full SSH feature parity.
- Password auth and forwarding features are disabled in `localoptions.h` to keep
  dependencies aligned with current kernel/runtime support.
- Smoke validation is intentionally narrow and does not yet prove complete
  interactive SSH session behavior.
