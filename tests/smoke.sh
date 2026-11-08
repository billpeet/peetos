#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IMAGE="${ROOT_DIR}/build/peetos.img"
LOG_FILE="$(mktemp)"

source "${ROOT_DIR}/tests/rust_build_compat_env.bash"
setup_rust_build_compat "${ROOT_DIR}"

cleanup() {
  rm -f "${LOG_FILE}"
}
trap cleanup EXIT

make -C "${ROOT_DIR}" image PROFILE=release KERNEL_FEATURES=test-exit

# Boot once as a uniprocessor and once with four CPUs so both the APIC
# uniprocessor path and AP bring-up are covered.
run_qemu() {
  local smp="$1"
  set +e
  timeout 15s qemu-system-x86_64 \
    -smp "${smp}" \
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
    echo "QEMU (-smp ${smp}) did not exit within timeout"
    exit 1
  fi
  # isa-debug-exit reports (code << 1) | 1; 33 is qemu::exit_success().
  if [[ "${status}" -ne 33 ]]; then
    echo "QEMU (-smp ${smp}) exited with status ${status}"
    cat "${LOG_FILE}"
    exit 1
  fi

  grep -q "PeetOS Rust kernel: serial online" "${LOG_FILE}"
  grep -q "\[PCI\] Enumerated" "${LOG_FILE}"
  grep -q "\[STORAGE\] Detected" "${LOG_FILE}"
  grep -q "\[BLK\] Inventory" "${LOG_FILE}"
  grep -q "\[APIC\] IOAPIC id=" "${LOG_FILE}"
  grep -q "PIC disabled" "${LOG_FILE}"
  grep -q "\[TEST\] Timer advanced [0-9]* tick(s) via IOAPIC" "${LOG_FILE}"
}

run_qemu 1
grep -q "\[SMP\] 1 CPU online (uniprocessor)" "${LOG_FILE}"

run_qemu 4
grep -q "\[SMP\] 4 of 4 CPU(s) online" "${LOG_FILE}"
grep -q "\[SMP\] IPI self-test: 3/3 AP(s) responded" "${LOG_FILE}"
