#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IMAGE="${ROOT_DIR}/build/peetos.img"
EXT2_IMG="${ROOT_DIR}/user/sample_ext2_musl.img"
LOG_FILE="$(mktemp)"

source "${ROOT_DIR}/tests/rust_build_compat_env.bash"
setup_rust_build_compat "${ROOT_DIR}"

cleanup() {
  rm -f "${LOG_FILE}"
}
trap cleanup EXIT

make -C "${ROOT_DIR}" image PROFILE=release KERNEL_FEATURES=musl-ext2-cmd-smoke

set +e
timeout 12s qemu-system-x86_64 \
  -drive id=disk0,file="${IMAGE}",if=none,format=raw \
  -device ich9-ahci,id=ahci \
  -device ide-hd,drive=disk0,bus=ahci.0 \
  -drive id=disk1,file="${EXT2_IMG}",if=none,format=raw \
  -device ide-hd,drive=disk1,bus=ahci.1 \
  -serial stdio \
  -display none \
  -no-reboot \
  -no-shutdown \
  > "${LOG_FILE}" 2>&1
status=$?
set -e

if [[ "${status}" -ne 0 && "${status}" -ne 124 ]]; then
  echo "QEMU exited with unexpected status ${status}" >&2
  cat "${LOG_FILE}" >&2
  exit 1
fi

if ! rg -q '\[musl-ext2-cmd-smoke\] done' "${LOG_FILE}"; then
  echo "Smoke command sequence did not reach completion marker" >&2
  cat "${LOG_FILE}" >&2
  exit 1
fi

if ! rg -q '\[GUI\] Display owner: console' "${LOG_FILE}"; then
  echo "Kernel did not remain in console-first display ownership for shell smoke" >&2
  cat "${LOG_FILE}" >&2
  exit 1
fi

if rg -q '\[musl-ext2-cmd-smoke\] failures' "${LOG_FILE}"; then
  echo "Smoke command sequence completed with command failures (syscall gaps still expected)."
fi

echo "Smoke sequence completed. ENOSYS syscall stubs observed:"
stubs="$(rg -o '\[SYSCALL\] [a-z0-9_]+ -> -ENOSYS' "${LOG_FILE}" | sort -u || true)"
if [[ -n "${stubs}" ]]; then
  printf '%s\n' "${stubs}"
else
  echo "(none)"
fi

unknown="$(rg '\[SYSCALL\] Unhandled syscall' "${LOG_FILE}" || true)"
if [[ -n "${unknown}" ]]; then
  echo
  echo "Unhandled syscall logs observed:"
  printf '%s\n' "${unknown}"
fi
