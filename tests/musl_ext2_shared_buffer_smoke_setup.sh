#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LAUNCHER="${ROOT_DIR}/user/musl_execve_ext2_shared_buffer_smoke.c"
HELPER_SRC="${ROOT_DIR}/user/shared_buffer_rights_smoke.c"
RUNNER="${ROOT_DIR}/scripts/run_toybox_shared_buffer_smoke.sh"
KERNEL_CARGO="${ROOT_DIR}/kernel/Cargo.toml"
MAKEFILE="${ROOT_DIR}/Makefile"
EXT2_BUILDER="${ROOT_DIR}/scripts/build_ext2_musl_execve.sh"

if [[ ! -f "${LAUNCHER}" ]]; then
  echo "musl ext2 shared-buffer smoke launcher missing" >&2
  exit 1
fi

for token in '"/mnt/ext2/bin/toybox"' '"sh"' '"bin/shared_buffer_smoke.sh"'; do
  if ! grep -F -q "${token}" "${LAUNCHER}"; then
    echo "musl ext2 shared-buffer launcher is missing ${token}" >&2
    exit 1
  fi
done

if [[ ! -f "${HELPER_SRC}" ]]; then
  echo "userspace shared-buffer smoke helper source missing" >&2
  exit 1
fi

for token in 'SYS_memfd_create' 'SCM_RIGHTS' 'socketpair' 'mmap' 'ftruncate' 'fork' '[shared-buffer] pass'; do
  if ! grep -F -q "${token}" "${HELPER_SRC}"; then
    echo "userspace shared-buffer helper is missing ${token}" >&2
    exit 1
  fi
done

for token in 'shared_buffer_smoke.sh' 'bin/shared_buffer_rights_smoke' 'musl-ext2-shared-buffer-smoke] done' 'musl-ext2-shared-buffer-smoke] failures'; do
  if ! grep -F -q "${token}" "${EXT2_BUILDER}"; then
    echo "ext2 shared-buffer smoke payload is missing ${token}" >&2
    exit 1
  fi
done

if ! grep -q 'musl-ext2-shared-buffer-smoke' "${KERNEL_CARGO}"; then
  echo "kernel feature musl-ext2-shared-buffer-smoke missing" >&2
  exit 1
fi

for token in 'run-shared-buffer-smoke' 'user-musl-ext2-shared-buffer-smoke' 'musl_execve_ext2_shared_buffer_smoke.elf' 'shared_buffer_rights_smoke.elf'; do
  if ! grep -q "${token}" "${MAKEFILE}"; then
    echo "Makefile is missing ${token}" >&2
    exit 1
  fi
done

if [[ ! -x "${RUNNER}" ]]; then
  echo "toybox shared-buffer smoke runner is missing or not executable" >&2
  exit 1
fi
