#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LAUNCHER="${ROOT_DIR}/user/musl_execve_ext2_epoll_smoke.c"
HELPER_SRC="${ROOT_DIR}/user/epoll_compositor_smoke.c"
RUNNER="${ROOT_DIR}/scripts/run_toybox_epoll_smoke.sh"
KERNEL_CARGO="${ROOT_DIR}/kernel/Cargo.toml"
MAKEFILE="${ROOT_DIR}/Makefile"
EXT2_BUILDER="${ROOT_DIR}/scripts/build_ext2_musl_execve.sh"

if [[ ! -f "${LAUNCHER}" ]]; then
  echo "musl ext2 epoll smoke launcher missing" >&2
  exit 1
fi

for token in '"/mnt/ext2/bin/toybox"' '"sh"' '"bin/epoll_smoke.sh"'; do
  if ! grep -F -q "${token}" "${LAUNCHER}"; then
    echo "musl ext2 epoll launcher is missing ${token}" >&2
    exit 1
  fi
done

if [[ ! -f "${HELPER_SRC}" ]]; then
  echo "userspace epoll compositor smoke helper source missing" >&2
  exit 1
fi

for token in 'epoll_create1' 'epoll_ctl' 'epoll_wait' 'socketpair' 'fork' '[epoll-smoke] pass'; do
  if ! grep -F -q "${token}" "${HELPER_SRC}"; then
    echo "userspace epoll helper is missing ${token}" >&2
    exit 1
  fi
done

for token in 'epoll_smoke.sh' 'bin/epoll_compositor_smoke' 'musl-ext2-epoll-smoke] done' 'musl-ext2-epoll-smoke] failures'; do
  if ! grep -F -q "${token}" "${EXT2_BUILDER}"; then
    echo "ext2 epoll smoke payload is missing ${token}" >&2
    exit 1
  fi
done

if ! grep -q 'musl-ext2-epoll-smoke' "${KERNEL_CARGO}"; then
  echo "kernel feature musl-ext2-epoll-smoke missing" >&2
  exit 1
fi

for token in 'run-epoll-smoke' 'user-musl-ext2-epoll-smoke' 'musl_execve_ext2_epoll_smoke.elf' 'epoll_compositor_smoke.elf'; do
  if ! grep -q "${token}" "${MAKEFILE}"; then
    echo "Makefile is missing ${token}" >&2
    exit 1
  fi
done

if [[ ! -x "${RUNNER}" ]]; then
  echo "toybox epoll smoke runner is missing or not executable" >&2
  exit 1
fi
