#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LAUNCHER="${ROOT_DIR}/user/musl_execve_ext2_smoke.c"
RUNNER="${ROOT_DIR}/scripts/run_toybox_next_smoke.sh"
KERNEL_CARGO="${ROOT_DIR}/kernel/Cargo.toml"
SYSCALL_RS="${ROOT_DIR}/kernel/src/syscall.rs"
MAKEFILE="${ROOT_DIR}/Makefile"

if [[ ! -f "${LAUNCHER}" ]]; then
  echo "musl ext2 command smoke launcher missing" >&2
  exit 1
fi

if ! grep -q '"/mnt/ext2/bin/toybox"' "${LAUNCHER}"; then
  echo "musl ext2 command smoke launcher is not targeting /mnt/ext2/bin/toybox" >&2
  exit 1
fi

if ! grep -q '"sh"' "${LAUNCHER}"; then
  echo "musl ext2 command smoke launcher is missing toybox sh argv0" >&2
  exit 1
fi

if ! grep -q '"bin/next_smoke.sh"' "${LAUNCHER}"; then
  echo "musl ext2 command smoke launcher is missing bin/next_smoke.sh script path" >&2
  exit 1
fi

for token in 'toybox cp' 'toybox mv' 'toybox ln' 'toybox touch' 'toybox ping' 'next_smoke.sh' 'musl-ext2-cmd-smoke] failures'; do
  if ! grep -q "${token}" "${ROOT_DIR}/scripts/build_ext2_musl_execve.sh"; then
    echo "ext2 smoke script payload is missing ${token}" >&2
    exit 1
  fi
done

if ! grep -q 'musl-ext2-cmd-smoke' "${KERNEL_CARGO}"; then
  echo "kernel feature musl-ext2-cmd-smoke missing" >&2
  exit 1
fi

if ! grep -q 'log_stub_enosys' "${SYSCALL_RS}"; then
  echo "syscall stub ENOSYS logging helper missing" >&2
  exit 1
fi

if ! grep -q 'run-shell-smoke' "${MAKEFILE}"; then
  echo "Makefile run-shell-smoke target missing" >&2
  exit 1
fi

if [[ ! -x "${RUNNER}" ]]; then
  echo "toybox next smoke runner is missing or not executable" >&2
  exit 1
fi
