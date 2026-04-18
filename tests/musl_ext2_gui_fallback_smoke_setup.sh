#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUNNER="${ROOT_DIR}/scripts/run_toybox_gui_fallback_smoke.sh"
MAKEFILE="${ROOT_DIR}/Makefile"
if [[ ! -x "${RUNNER}" ]]; then
  echo "gui fallback smoke runner is missing or not executable" >&2
  exit 1
fi

for token in \
  'KERNEL_FEATURES="compositor-first musl-ext2-gui-smoke"' \
  'Compositor-owned init exited' \
  'framebuffer ownership returned to console' \
  'gui compositor fallback smoke completed successfully'; do
  if ! grep -F -q "${token}" "${RUNNER}"; then
    echo "gui fallback smoke runner is missing ${token}" >&2
    exit 1
  fi
done

if ! grep -q 'run-gui-fallback-smoke' "${MAKEFILE}"; then
  echo "Makefile run-gui-fallback-smoke target missing" >&2
  exit 1
fi
