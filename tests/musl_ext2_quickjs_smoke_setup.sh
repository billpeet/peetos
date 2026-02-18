#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LAUNCHER="${ROOT_DIR}/user/musl_execve_ext2_quickjs_smoke.c"
RUNNER="${ROOT_DIR}/scripts/run_toybox_quickjs_smoke.sh"
KERNEL_CARGO="${ROOT_DIR}/kernel/Cargo.toml"
MAKEFILE="${ROOT_DIR}/Makefile"
EXT2_BUILDER="${ROOT_DIR}/scripts/build_ext2_musl_execve.sh"

if [[ ! -f "${LAUNCHER}" ]]; then
  echo "musl ext2 quickjs smoke launcher missing" >&2
  exit 1
fi

if ! grep -q '"/mnt/ext2/bin/toybox"' "${LAUNCHER}"; then
  echo "musl ext2 quickjs smoke launcher is not targeting /mnt/ext2/bin/toybox" >&2
  exit 1
fi

if ! grep -q '"sh"' "${LAUNCHER}"; then
  echo "musl ext2 quickjs smoke launcher is missing toybox sh argv0" >&2
  exit 1
fi

if ! grep -q '"bin/quickjs_smoke.sh"' "${LAUNCHER}"; then
  echo "musl ext2 quickjs smoke launcher is missing bin/quickjs_smoke.sh script path" >&2
  exit 1
fi

for token in 'bin/qjs' 'test_hello.js' 'test_math.js' 'quickjs_smoke.sh' 'musl-ext2-quickjs-smoke] done' 'musl-ext2-quickjs-smoke] failures'; do
  if ! grep -q "${token}" "${EXT2_BUILDER}"; then
    echo "ext2 quickjs smoke payload is missing ${token}" >&2
    exit 1
  fi
done

if ! grep -q 'musl-ext2-quickjs-smoke' "${KERNEL_CARGO}"; then
  echo "kernel feature musl-ext2-quickjs-smoke missing" >&2
  exit 1
fi

for token in 'run-quickjs-smoke' 'user-musl-ext2-quickjs-smoke' 'quickjs-fetch' 'quickjs-clean'; do
  if ! grep -q "${token}" "${MAKEFILE}"; then
    echo "Makefile token missing: ${token}" >&2
    exit 1
  fi
done

if [[ ! -x "${RUNNER}" ]]; then
  echo "quickjs smoke runner is missing or not executable" >&2
  exit 1
fi
