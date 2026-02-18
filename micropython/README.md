# MicroPython Setup for PeetOS (Rust Port)

This directory bootstraps MicroPython source and builds a static `micropython`
interpreter for PeetOS userspace staging.

## Quick Start

From repo root:

```bash
make micropython-fetch
make micropython
```

## Layout

- `micropython/fetch-micropython.sh`: downloads/extracts a pinned MicroPython
  release into `micropython/micropython-src`.
- `micropython/Makefile`: wraps fetch/build/clean/info.
- `micropython/test_hello.py`, `micropython/test_math.py`: runtime smoke scripts
  staged into ext2 as `/mnt/ext2/tmp/test_hello.py` and
  `/mnt/ext2/tmp/test_math.py` when present.
- `micropython/micropython-src` (generated): upstream MicroPython source tree.
- `micropython/out/micropython` (generated): built static interpreter.

## Limitations / Simplifications

- The current integration uses `ports/unix` with `VARIANT=minimal` for bring-up,
  not the full-featured Unix variant.
- Smoke validation is intentionally narrow (hello + math scripts) and is not a
  full MicroPython language/module compliance suite.
- Networking and threading modules are out of scope for this initial staged
  integration.
