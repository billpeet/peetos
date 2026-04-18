#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUNNER="${ROOT_DIR}/scripts/run_toybox_console_first_smoke.sh"
MAKEFILE="${ROOT_DIR}/Makefile"
if [[ ! -x "${RUNNER}" ]]; then
  echo "console-first smoke runner is missing or not executable" >&2
  exit 1
fi

for token in 'KERNEL_FEATURES=musl-ext2-console-first-smoke' 'Display owner: console' 'compositor-first' 'console-first shell smoke completed successfully'; do
  if ! grep -F -q "${token}" "${RUNNER}"; then
    echo "console-first smoke runner is missing ${token}" >&2
    exit 1
  fi
done

if ! grep -q 'run-console-first-smoke' "${MAKEFILE}"; then
  echo "Makefile run-console-first-smoke target missing" >&2
  exit 1
fi
