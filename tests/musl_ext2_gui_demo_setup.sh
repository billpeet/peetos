#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LAUNCHER="${ROOT_DIR}/user/musl_execve_ext2_gui_demo.c"
RUNNER="${ROOT_DIR}/scripts/run_toybox_gui_demo.sh"
MAKEFILE="${ROOT_DIR}/Makefile"
EXT2_BUILDER="${ROOT_DIR}/scripts/build_ext2_musl_execve.sh"
KERNEL_CARGO="${ROOT_DIR}/kernel/Cargo.toml"

if [[ ! -f "${LAUNCHER}" ]]; then
  echo "musl ext2 gui demo launcher missing" >&2
  exit 1
fi

for token in '"/mnt/ext2/bin/toybox"' '"sh"' '"bin/gui_demo.sh"' 'musl-ext2-gui-demo'; do
  if ! grep -F -q "${token}" "${LAUNCHER}"; then
    echo "musl ext2 gui demo launcher is missing ${token}" >&2
    exit 1
  fi
done

for token in 'gui_demo.sh' 'bin/gui_compositor_smoke demo' 'musl-ext2-gui-demo] done' 'musl-ext2-gui-demo] failures'; do
  if ! grep -F -q "${token}" "${EXT2_BUILDER}"; then
    echo "ext2 gui demo payload is missing ${token}" >&2
    exit 1
  fi
done

for token in 'musl-ext2-gui-demo' 'run-gui-demo' 'user-musl-ext2-gui-demo' 'musl_execve_ext2_gui_demo.elf'; do
  if ! grep -q "${token}" "${MAKEFILE}" && ! grep -q "${token}" "${KERNEL_CARGO}"; then
    echo "build integration is missing ${token}" >&2
    exit 1
  fi
done

if [[ ! -x "${RUNNER}" ]]; then
  echo "toybox gui demo runner is missing or not executable" >&2
  exit 1
fi

for token in 'musl-ext2-gui-demo compositor-first' 'qemu-system-x86_64' 'sample_ext2_musl.img'; do
  if ! grep -q "${token}" "${RUNNER}"; then
    echo "toybox gui demo runner is missing ${token}" >&2
    exit 1
  fi
done
