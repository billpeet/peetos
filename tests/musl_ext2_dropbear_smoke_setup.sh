#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LAUNCHER="${ROOT_DIR}/user/musl_execve_ext2_dropbear_smoke.c"
RUNNER="${ROOT_DIR}/scripts/run_toybox_dropbear_smoke.sh"
EXT2_SCRIPT="${ROOT_DIR}/scripts/build_ext2_musl_execve.sh"
KERNEL_CARGO="${ROOT_DIR}/kernel/Cargo.toml"
MAKEFILE="${ROOT_DIR}/Makefile"

if [[ ! -f "${LAUNCHER}" ]]; then
  echo "musl ext2 dropbear smoke launcher missing" >&2
  exit 1
fi

if ! grep -q '"/mnt/ext2/bin/toybox"' "${LAUNCHER}"; then
  echo "musl ext2 dropbear smoke launcher is not targeting /mnt/ext2/bin/toybox" >&2
  exit 1
fi

if ! grep -q '(char \*)"sh"' "${LAUNCHER}"; then
  echo "musl ext2 dropbear smoke launcher is missing toybox sh argv0" >&2
  exit 1
fi

if ! grep -q '"bin/dropbear_smoke.sh"' "${LAUNCHER}"; then
  echo "musl ext2 dropbear smoke launcher is missing bin/dropbear_smoke.sh script path" >&2
  exit 1
fi

for token in 'bin/dropbear' 'bin/dropbearkey' 'dropbear_smoke.sh' 'dropbear_ed25519_host_key' 'musl-ext2-dropbear-smoke] daemon running' 'musl-ext2-dropbear-smoke] done' 'musl-ext2-dropbear-smoke] failures'; do
  if ! grep -q "${token}" "${EXT2_SCRIPT}"; then
    echo "ext2 dropbear smoke payload is missing ${token}" >&2
    exit 1
  fi
done

if ! grep -q 'musl-ext2-dropbear-smoke' "${KERNEL_CARGO}"; then
  echo "kernel feature musl-ext2-dropbear-smoke missing" >&2
  exit 1
fi

for token in 'run-dropbear-smoke' 'user-musl-ext2-dropbear-smoke' 'dropbear-fetch' 'dropbear-clean' 'USER_DROPBEAR_HOSTKEY'; do
  if ! grep -q "${token}" "${MAKEFILE}"; then
    echo "Makefile is missing ${token}" >&2
    exit 1
  fi
done

if [[ ! -x "${RUNNER}" ]]; then
  echo "dropbear smoke runner is missing or not executable" >&2
  exit 1
fi
