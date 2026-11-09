#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IMAGE="${ROOT_DIR}/build/peetos.img"
EXT2_IMG="${ROOT_DIR}/user/sample_ext2_musl.img"
LOG_FILE="$(mktemp)"
TIMEOUT_SECS="${FPU_SMOKE_TIMEOUT_SECS:-90}"
QEMU_PID=""

source "${ROOT_DIR}/tests/rust_build_compat_env.bash"
setup_rust_build_compat "${ROOT_DIR}"

cleanup() {
  if [[ -n "${QEMU_PID}" ]]; then
    kill "${QEMU_PID}" 2>/dev/null || true
    wait "${QEMU_PID}" 2>/dev/null || true
  fi
  rm -f "${LOG_FILE}"
}
trap cleanup EXIT

make -C "${ROOT_DIR}" image PROFILE=release KERNEL_FEATURES=musl-ext2-fpu-smoke

qemu-system-x86_64 \
  -drive id=disk0,file="${IMAGE}",if=none,format=raw \
  -device ich9-ahci,id=ahci \
  -device ide-hd,drive=disk0,bus=ahci.0 \
  -drive id=disk1,file="${EXT2_IMG}",if=none,format=raw \
  -device ide-hd,drive=disk1,bus=ahci.1 \
  -serial stdio \
  -display none \
  -no-reboot \
  -no-shutdown \
  > "${LOG_FILE}" 2>&1 &
QEMU_PID=$!

# Stop as soon as the payload script reports completion instead of always
# waiting out the full timeout.
deadline=$((SECONDS + TIMEOUT_SECS))
while (( SECONDS < deadline )); do
  if grep -q '\[musl-ext2-fpu-smoke\] done' "${LOG_FILE}"; then
    break
  fi
  if ! kill -0 "${QEMU_PID}" 2>/dev/null; then
    break
  fi
  sleep 0.5
done
sleep 0.5

if ! grep -q '\[musl-ext2-fpu-smoke\] done' "${LOG_FILE}"; then
  echo "FPU context smoke did not reach completion marker within ${TIMEOUT_SECS}s" >&2
  cat "${LOG_FILE}" >&2
  exit 1
fi

if grep -q '\[musl-ext2-fpu-smoke\] failures' "${LOG_FILE}" || grep -q '\[fpu-smoke\] FAIL' "${LOG_FILE}"; then
  echo "FPU context smoke completed with failures" >&2
  cat "${LOG_FILE}" >&2
  exit 1
fi

for marker in '\[fpu-smoke\] preempt ok' '\[fpu-smoke\] signal ok' '\[fpu-smoke\] exec ok' '\[fpu-smoke\] direction flag ok' '\[fpu-smoke\] pass'; do
  if ! grep -q "${marker}" "${LOG_FILE}"; then
    echo "FPU context smoke is missing marker: ${marker}" >&2
    cat "${LOG_FILE}" >&2
    exit 1
  fi
done

grep '\[fpu-smoke\]' "${LOG_FILE}"
echo "FPU context smoke completed successfully."
