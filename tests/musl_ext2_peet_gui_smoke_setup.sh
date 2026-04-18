#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LAUNCHER="${ROOT_DIR}/user/musl_execve_ext2_peet_gui_smoke.c"
HELPER_SRC="${ROOT_DIR}/user/peet_gui_compositor_smoke.c"
RUNNER="${ROOT_DIR}/scripts/run_toybox_peet_gui_smoke.sh"
EXT2_BUILDER="${ROOT_DIR}/scripts/build_ext2_musl_execve.sh"
KERNEL_CARGO="${ROOT_DIR}/kernel/Cargo.toml"
MAKEFILE="${ROOT_DIR}/Makefile"

if [[ ! -f "${LAUNCHER}" ]]; then
  echo "musl ext2 peet-gui smoke launcher missing" >&2
  exit 1
fi

for token in '"/mnt/ext2/bin/toybox"' '"sh"' '"bin/peet_gui_smoke.sh"' 'PEETOS_DISPLAY_MODE=compositor-first' 'PEETOS_FB=/dev/fb0'; do
  if ! grep -q "${token}" "${LAUNCHER}"; then
    echo "musl ext2 peet-gui launcher is missing ${token}" >&2
    exit 1
  fi
done

if [[ ! -f "${HELPER_SRC}" ]]; then
  echo "peet-gui compositor smoke helper source missing" >&2
  exit 1
fi

for token in 'queue_push' 'queue_pop' 'MSG_ATTACH' 'MSG_DAMAGE' 'MSG_COMMIT' 'MSG_FOCUS' 'MSG_KEY' 'MSG_DETACH' 'damage-validated=1' 'overlap-top=%d' '/dev/fb0' '/dev/input/event0' '/dev/input/event1' 'focus-switched=%d overlap-top=%d' 'client%u detached' 'disconnect redraw validation failed' '[peet-gui] waiting-for-focus-input' '[peet-gui] pass'; do
  if ! grep -F -q "${token}" "${HELPER_SRC}"; then
    echo "peet-gui compositor helper is missing ${token}" >&2
    exit 1
  fi
done

for token in 'peet_gui_smoke.sh' 'bin/peet_gui_compositor_smoke' 'musl-ext2-peet-gui-smoke] done' 'musl-ext2-peet-gui-smoke] failures'; do
  if ! grep -q "${token}" "${EXT2_BUILDER}"; then
    echo "ext2 peet-gui smoke payload is missing ${token}" >&2
    exit 1
  fi
done

if ! grep -q 'musl-ext2-peet-gui-smoke' "${KERNEL_CARGO}"; then
  echo "kernel feature musl-ext2-peet-gui-smoke missing" >&2
  exit 1
fi

for token in 'run-peet-gui-smoke' 'user-musl-ext2-peet-gui-smoke' 'musl_execve_ext2_peet_gui_smoke.elf' 'peet_gui_compositor_smoke.elf'; do
  if ! grep -q "${token}" "${MAKEFILE}"; then
    echo "Makefile peet-gui smoke support is missing ${token}" >&2
    exit 1
  fi
done

if [[ ! -x "${RUNNER}" ]]; then
  echo "toybox peet-gui smoke runner is missing or not executable" >&2
  exit 1
fi

for token in 'mouse_button 1' 'sendkey a' 'sendkey c' 'client2 detached' 'disconnect=2'; do
  if ! grep -F -q "${token}" "${RUNNER}"; then
    echo "peet-gui smoke runner is missing ${token}" >&2
    exit 1
  fi
done

echo "musl ext2 peet-gui smoke setup looks good."
