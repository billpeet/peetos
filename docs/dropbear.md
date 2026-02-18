# Dropbear Bootstrap

This document tracks the Rust-port Stage 9 Dropbear bootstrap setup.

## Scope

- Fetch pinned Dropbear source into `dropbear/dropbear-src`.
- Build static Dropbear binaries (`dropbear`, `dropbearkey`, `dbclient`, `scp`)
  using the musl sysroot toolchain.
- Stage `dropbear` and `dropbearkey` into ext2 for userspace SSH bring-up
  smoke validation.

## Commands

```bash
make dropbear-fetch
make dropbear
make dropbear-info
```

Dropbear smoke path (boots ext2 launcher and runs staged script checks with
`/mnt/ext2/bin/dropbear` and `/mnt/ext2/bin/dropbearkey`):

```bash
scripts/run_toybox_dropbear_smoke.sh
# or:
make run-dropbear-smoke
```

## Output

- Source tree: `dropbear/dropbear-src`
- Built binaries: `dropbear/out/dropbearmulti`, `dropbear/out/dropbear`,
  `dropbear/out/dropbearkey`, `dropbear/out/dbclient`, `dropbear/out/scp`
- Copied user staging artifacts:
  - `user/dropbear`
  - `user/dropbearkey`
- Staged runtime payload (via `scripts/build_ext2_musl_execve.sh`):
  - `/mnt/ext2/bin/dropbear`
  - `/mnt/ext2/bin/dropbearkey`
  - `/mnt/ext2/etc/dropbear/dropbear_ed25519_host_key`
  - `/mnt/ext2/bin/dropbear_smoke.sh`

## Limitations / Simplifications

- The current integration validates tool bring-up (`dropbear -V`,
  `dropbearkey -h`), staged host-key usage, and daemonized launch in
  smoke (`dropbear ... -p 2222`, plus a same-port bind probe to confirm the
  daemon is holding the listener).
- Authentication/forwarding options are intentionally restricted in
  `dropbear/peetos/localoptions.h` to match current kernel/runtime maturity.
- Full SSH auth/session validation (server/client login flow) remains pending.
