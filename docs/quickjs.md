# quickjs Bootstrap

This document tracks the Rust-port Stage 9 quickjs bootstrap setup.

## Scope

- Fetch pinned quickjs source into `quickjs/quickjs-src`.
- Build a static `qjs` interpreter with the musl sysroot toolchain.
- Stage `qjs` plus smoke scripts into ext2 for user-space runtime validation.

## Commands

```bash
make quickjs-fetch
make quickjs
make quickjs-info
```

QuickJS smoke path (boots ext2 launcher and runs staged scripts with `/bin/qjs`):

```bash
scripts/run_toybox_quickjs_smoke.sh
# or:
make run-quickjs-smoke
```

## Output

- Source tree: `quickjs/quickjs-src`
- Built interpreter: `quickjs/out/qjs` and copied to `user/qjs`
- Staged runtime payload (via `scripts/build_ext2_musl_execve.sh`):
  - `/mnt/ext2/bin/qjs`
  - `/mnt/ext2/tmp/test_hello.js`
  - `/mnt/ext2/tmp/test_math.js`
  - `/mnt/ext2/bin/quickjs_smoke.sh`

## Limitations / Simplifications

- Current integration builds/stages `qjs` only; `qjsc` and full upstream REPL bytecode generation are not wired.
- Worker API support is disabled in `quickjs-libc.c` for this path (depends on pthread/atomics runtime behavior not currently staged).
- Smoke validation is intentionally narrow (`hello` + math scripts) and is not a full ECMAScript conformance run.
