#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LAUNCHER="${ROOT_DIR}/user/musl_execve_ext2_ping_smoke.c"
RUNNER="${ROOT_DIR}/scripts/run_toybox_ping_smoke.sh"
KERNEL_CARGO="${ROOT_DIR}/kernel/Cargo.toml"
MAKEFILE="${ROOT_DIR}/Makefile"
EXT2_BUILDER="${ROOT_DIR}/scripts/build_ext2_musl_execve.sh"

if [[ ! -f "${LAUNCHER}" ]]; then
  echo "musl ext2 ping smoke launcher missing" >&2
  exit 1
fi

if ! grep -q '"/mnt/ext2/bin/toybox"' "${LAUNCHER}"; then
  echo "musl ext2 ping smoke launcher is not targeting /mnt/ext2/bin/toybox" >&2
  exit 1
fi

if ! grep -q '"sh"' "${LAUNCHER}"; then
  echo "musl ext2 ping smoke launcher is missing toybox sh argv0" >&2
  exit 1
fi

if ! grep -q '"bin/ping_smoke.sh"' "${LAUNCHER}"; then
  echo "musl ext2 ping smoke launcher is missing bin/ping_smoke.sh script path" >&2
  exit 1
fi

for token in 'ping_smoke.sh' 'toybox ping' 'musl-ext2-ping-smoke] done' 'musl-ext2-ping-smoke] failures'; do
  if ! grep -q "${token}" "${EXT2_BUILDER}"; then
    echo "ext2 ping smoke payload is missing ${token}" >&2
    exit 1
  fi
done

if ! grep -q 'musl-ext2-ping-smoke' "${KERNEL_CARGO}"; then
  echo "kernel feature musl-ext2-ping-smoke missing" >&2
  exit 1
fi

if ! grep -q 'run-ping-smoke' "${MAKEFILE}"; then
  echo "Makefile run-ping-smoke target missing" >&2
  exit 1
fi

if [[ ! -x "${RUNNER}" ]]; then
  echo "toybox ping smoke runner is missing or not executable" >&2
  exit 1
fi
