# micropython Bootstrap

This document tracks the Rust-port Stage 9 micropython bootstrap setup.

## Scope

- Fetch pinned MicroPython source into `micropython/micropython-src`.
- Build a static `micropython` interpreter from `ports/unix` using the musl
  sysroot toolchain.
- Stage `micropython` plus smoke scripts into ext2 for userspace runtime
  validation.

## Commands

```bash
make micropython-fetch
make micropython
make micropython-info
```

MicroPython smoke path (boots ext2 launcher and runs staged scripts with
`/mnt/ext2/bin/micropython`):

```bash
scripts/run_toybox_micropython_smoke.sh
# or:
make run-micropython-smoke
```

## Output

- Source tree: `micropython/micropython-src`
- Built interpreter: `micropython/out/micropython` and copied to
  `user/micropython`
- Staged runtime payload (via `scripts/build_ext2_musl_execve.sh`):
  - `/mnt/ext2/bin/micropython`
  - `/mnt/ext2/tmp/test_hello.py`
  - `/mnt/ext2/tmp/test_math.py`
  - `/mnt/ext2/bin/micropython_smoke.sh`

## Limitations / Simplifications

- The current integration uses `ports/unix` with `VARIANT=minimal` for initial
  bring-up.
- Smoke validation is intentionally narrow (`hello` + math scripts) and is not a
  full MicroPython conformance suite.
- Current smoke scripts intentionally use integer-only expressions; this minimal
  variant should not be treated as full floating-point language coverage yet.
- Optional modules requiring broader runtime support (for example networking,
  SSL, threading) are not validated by this smoke path yet.
