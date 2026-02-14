#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IMAGE="${ROOT_DIR}/build/peetos.img"
LOG_FILE="$(mktemp)"

cleanup() {
  rm -f "${LOG_FILE}"
}
trap cleanup EXIT

make -C "${ROOT_DIR}" image PROFILE=release KERNEL_FEATURES=test-exit

set +e
timeout 5s qemu-system-x86_64 \
  -drive format=raw,file="${IMAGE}" \
  -serial stdio \
  -display none \
  -no-reboot \
  -no-shutdown \
  -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
  > "${LOG_FILE}" 2>&1
status=$?
set -e

# If QEMU doesn't exit via the debug-exit device, we treat it as a failure.
if [[ "${status}" -eq 124 ]]; then
  echo "QEMU did not exit within timeout"
  exit 1
fi

grep -q "PeetOS Rust kernel: serial online" "${LOG_FILE}"
grep -q "\\[PCI\\] Enumerated" "${LOG_FILE}"
grep -q "\\[STORAGE\\] Detected" "${LOG_FILE}"
grep -q "\\[BLK\\] Inventory" "${LOG_FILE}"
