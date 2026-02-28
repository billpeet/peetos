# QuickJS Setup for PeetOS

This directory bootstraps QuickJS source and builds a static `qjs` binary for
PeetOS userspace staging.

## Quick Start

From repo root:

```bash
make quickjs-fetch
make quickjs
```

## Layout

- `quickjs/fetch-quickjs.sh`: downloads/extracts a pinned QuickJS release into
  `quickjs/quickjs-src`.
- `quickjs/Makefile`: wraps fetch/build/clean/info.
- `quickjs/peetos/Makefile.peetos`: PeetOS-oriented static `qjs` build file.
- `quickjs/peetos/repl_stub.c`: REPL symbol stub for initial `qjs` linking.
- `quickjs/test_hello.js`, `quickjs/test_math.js`: runtime smoke scripts staged
  into ext2 as `/mnt/ext2/tmp/test_hello.js` and `/mnt/ext2/tmp/test_math.js`
  when present.
- `quickjs/quickjs-src` (generated): upstream QuickJS source tree.
- `quickjs/out/qjs` (generated): built static QuickJS interpreter.

## Limitations / Simplifications

- Current integration builds only `qjs`; `qjsc` and full upstream REPL bytecode
  generation are not wired yet.
- Worker API support is disabled in `quickjs-libc.c` for this target path
  (depends on pthreads/atomics runtime behavior not currently staged here).
- The repo test harness validates fetch/build glue behavior, not a full network
  QuickJS build, to avoid external dependency assumptions.
