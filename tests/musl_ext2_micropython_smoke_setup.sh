#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LAUNCHER="${ROOT_DIR}/user/musl_execve_ext2_micropython_smoke.c"
RUNNER="${ROOT_DIR}/scripts/run_toybox_micropython_smoke.sh"
KERNEL_CARGO="${ROOT_DIR}/kernel/Cargo.toml"
MAKEFILE="${ROOT_DIR}/Makefile"
EXT2_BUILDER="${ROOT_DIR}/scripts/build_ext2_musl_execve.sh"

if [[ ! -f "${LAUNCHER}" ]]; then
  echo "musl ext2 micropython smoke launcher missing" >&2
  exit 1
fi

if ! grep -q '"/mnt/ext2/bin/toybox"' "${LAUNCHER}"; then
  echo "musl ext2 micropython smoke launcher is not targeting /mnt/ext2/bin/toybox" >&2
  exit 1
fi

if ! grep -q '"sh"' "${LAUNCHER}"; then
  echo "musl ext2 micropython smoke launcher is missing toybox sh argv0" >&2
  exit 1
fi

if ! grep -q '"bin/micropython_smoke.sh"' "${LAUNCHER}"; then
  echo "musl ext2 micropython smoke launcher is missing bin/micropython_smoke.sh script path" >&2
  exit 1
fi

for token in 'bin/micropython' 'test_hello.py' 'test_math.py' 'micropython_smoke.sh' 'musl-ext2-micropython-smoke] done' 'musl-ext2-micropython-smoke] failures'; do
  if ! grep -q "${token}" "${EXT2_BUILDER}"; then
    echo "ext2 micropython smoke payload is missing ${token}" >&2
    exit 1
  fi
done

if ! grep -q 'musl-ext2-micropython-smoke' "${KERNEL_CARGO}"; then
  echo "kernel feature musl-ext2-micropython-smoke missing" >&2
  exit 1
fi

for token in 'run-micropython-smoke' 'user-musl-ext2-micropython-smoke' 'micropython-fetch' 'micropython-clean'; do
  if ! grep -q "${token}" "${MAKEFILE}"; then
    echo "Makefile token missing: ${token}" >&2
    exit 1
  fi
done

if [[ ! -x "${RUNNER}" ]]; then
  echo "micropython smoke runner is missing or not executable" >&2
  exit 1
fi
