#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LAUNCHER="${ROOT_DIR}/user/musl_execve_ext2_unix_rights_smoke.c"
HELPER_SRC="${ROOT_DIR}/user/unix_rights_smoke.c"
RUNNER="${ROOT_DIR}/scripts/run_toybox_unix_rights_smoke.sh"
KERNEL_CARGO="${ROOT_DIR}/kernel/Cargo.toml"
MAKEFILE="${ROOT_DIR}/Makefile"
EXT2_BUILDER="${ROOT_DIR}/scripts/build_ext2_musl_execve.sh"

if [[ ! -f "${LAUNCHER}" ]]; then
  echo "musl ext2 unix-rights smoke launcher missing" >&2
  exit 1
fi

if ! grep -q '"/mnt/ext2/bin/toybox"' "${LAUNCHER}"; then
  echo "musl ext2 unix-rights launcher is not targeting /mnt/ext2/bin/toybox" >&2
  exit 1
fi

if ! grep -q '"sh"' "${LAUNCHER}"; then
  echo "musl ext2 unix-rights launcher is missing toybox sh argv0" >&2
  exit 1
fi

if ! grep -q '"bin/unix_rights_smoke.sh"' "${LAUNCHER}"; then
  echo "musl ext2 unix-rights launcher is missing bin/unix_rights_smoke.sh script path" >&2
  exit 1
fi

if [[ ! -f "${HELPER_SRC}" ]]; then
  echo "userspace unix-rights smoke helper source missing" >&2
  exit 1
fi

for token in 'sendmsg' 'recvmsg' 'SCM_RIGHTS' 'socketpair' 'MSG_CTRUNC' 'fork' '[unix-rights] pass'; do
  if ! grep -F -q "${token}" "${HELPER_SRC}"; then
    echo "userspace unix-rights helper is missing ${token}" >&2
    exit 1
  fi
done

for token in 'unix_rights_smoke.sh' 'bin/unix_rights_smoke' 'musl-ext2-unix-rights-smoke] done' 'musl-ext2-unix-rights-smoke] failures'; do
  if ! grep -F -q "${token}" "${EXT2_BUILDER}"; then
    echo "ext2 unix-rights smoke payload is missing ${token}" >&2
    exit 1
  fi
done

if ! grep -q 'musl-ext2-unix-rights-smoke' "${KERNEL_CARGO}"; then
  echo "kernel feature musl-ext2-unix-rights-smoke missing" >&2
  exit 1
fi

for token in 'run-unix-rights-smoke' 'user-musl-ext2-unix-rights-smoke' 'musl_execve_ext2_unix_rights_smoke.elf' 'unix_rights_smoke.elf'; do
  if ! grep -q "${token}" "${MAKEFILE}"; then
    echo "Makefile is missing ${token}" >&2
    exit 1
  fi
done

if [[ ! -x "${RUNNER}" ]]; then
  echo "toybox unix-rights smoke runner is missing or not executable" >&2
  exit 1
fi
