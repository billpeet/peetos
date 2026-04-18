#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LAUNCHER="${ROOT_DIR}/user/musl_execve_ext2_peet_shell_demo.c"
DEMO="${ROOT_DIR}/user/peet_shell_demo.c"
KERNEL_CARGO="${ROOT_DIR}/kernel/Cargo.toml"
RUNNER="${ROOT_DIR}/scripts/run_toybox_peet_shell_demo_smoke.sh"
MAKEFILE="${ROOT_DIR}/Makefile"

if [[ ! -f "${LAUNCHER}" ]]; then
  echo "musl ext2 peet shell demo launcher missing" >&2
  exit 1
fi

for token in 'bin/peet_shell_demo.sh' 'PEETOS_DISPLAY_MODE=compositor-first' 'PEETOS_INPUT_POINTER=/dev/input/event1'; do
  if ! grep -q "${token}" "${LAUNCHER}"; then
    echo "musl ext2 peet shell demo launcher is missing ${token}" >&2
    exit 1
  fi
done

if [[ ! -f "${DEMO}" ]]; then
  echo "peet shell demo source missing" >&2
  exit 1
fi

for token in '[peet-shell] ready' '[peet-shell-files] ready' 'socketpair' 'pipe' 'fork' 'waitpid' 'terminal-shell-spawned' 'terminal-command=' 'terminal-observed-pwd=' 'PEETOS_FB' 'ROTATE THEME' 'surface-attach' 'surface-raised' 'TERMINAL' 'FILES' 'INSPECTOR'; do
  if ! grep -F -q "${token}" "${DEMO}"; then
    echo "peet shell demo source is missing ${token}" >&2
    exit 1
  fi
done

if ! grep -q 'musl-ext2-peet-shell-demo' "${KERNEL_CARGO}"; then
  echo "kernel feature musl-ext2-peet-shell-demo missing" >&2
  exit 1
fi

for token in 'run-peet-shell-demo' 'run-peet-shell-demo-smoke' 'user-musl-ext2-peet-shell-demo'; do
  if ! grep -q "${token}" "${MAKEFILE}"; then
    echo "Makefile peet shell demo support is missing ${token}" >&2
    exit 1
  fi
done

if [[ ! -x "${RUNNER}" ]]; then
  echo "peet shell demo smoke runner is missing or not executable" >&2
  exit 1
fi

echo "musl ext2 peet shell demo setup looks good."
