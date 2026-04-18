#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LAUNCHER="${ROOT_DIR}/user/musl_execve_ext2_gui_smoke.c"
HELPER_SRC="${ROOT_DIR}/user/gui_compositor_smoke.c"
RUNNER="${ROOT_DIR}/scripts/run_toybox_gui_smoke.sh"
KERNEL_CARGO="${ROOT_DIR}/kernel/Cargo.toml"
MAKEFILE="${ROOT_DIR}/Makefile"
EXT2_BUILDER="${ROOT_DIR}/scripts/build_ext2_musl_execve.sh"

if [[ ! -f "${LAUNCHER}" ]]; then
  echo "musl ext2 gui smoke launcher missing" >&2
  exit 1
fi

for token in '"/mnt/ext2/bin/toybox"' '"sh"' '"bin/gui_smoke.sh"' 'PEETOS_DISPLAY_MODE=compositor-first' 'PEETOS_FB=/dev/fb0'; do
  if ! grep -F -q "${token}" "${LAUNCHER}"; then
    echo "musl ext2 gui launcher is missing ${token}" >&2
    exit 1
  fi
done

if [[ ! -f "${HELPER_SRC}" ]]; then
  echo "userspace gui smoke helper source missing" >&2
  exit 1
fi

for token in 'FBIOGETINFO' '/dev/fb0' '/dev/input/event0' '/dev/input/event1' 'mmap' 'epoll_create1' 'KEY_A' 'REL_X' '[gui-smoke] waiting-for-input' '[gui-smoke] pass'; do
  if ! grep -F -q "${token}" "${HELPER_SRC}"; then
    echo "userspace gui helper is missing ${token}" >&2
    exit 1
  fi
done

for token in 'gui_smoke.sh' 'bin/gui_compositor_smoke' 'musl-ext2-gui-smoke] done' 'musl-ext2-gui-smoke] failures'; do
  if ! grep -F -q "${token}" "${EXT2_BUILDER}"; then
    echo "ext2 gui smoke payload is missing ${token}" >&2
    exit 1
  fi
done

if ! grep -q 'musl-ext2-gui-smoke' "${KERNEL_CARGO}"; then
  echo "kernel feature musl-ext2-gui-smoke missing" >&2
  exit 1
fi

for token in 'run-gui-smoke' 'user-musl-ext2-gui-smoke' 'musl_execve_ext2_gui_smoke.elf' 'gui_compositor_smoke.elf'; do
  if ! grep -q "${token}" "${MAKEFILE}"; then
    echo "Makefile is missing ${token}" >&2
    exit 1
  fi
done

for token in 'sendkey a' 'mouse_move 12 0' 'mouse_button 1' 'observed keyboard=1 pointer=1'; do
  if ! grep -F -q "${token}" "${RUNNER}"; then
    echo "gui smoke runner is missing ${token}" >&2
    exit 1
  fi
done

if [[ ! -x "${RUNNER}" ]]; then
  echo "toybox gui smoke runner is missing or not executable" >&2
  exit 1
fi
