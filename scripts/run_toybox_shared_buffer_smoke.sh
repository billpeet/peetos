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

make -C "${ROOT_DIR}" image PROFILE=release KERNEL_FEATURES=musl-ext2-shared-buffer-smoke

set +e
timeout 20s qemu-system-x86_64 \
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

if ! rg -q '\[musl-ext2-shared-buffer-smoke\] done' "${LOG_FILE}"; then
  echo "shared buffer smoke did not reach completion marker" >&2
  cat "${LOG_FILE}" >&2
  exit 1
fi

if rg -q '\[musl-ext2-shared-buffer-smoke\] failures' "${LOG_FILE}"; then
  echo "shared buffer smoke completed with failures" >&2
  cat "${LOG_FILE}" >&2
  exit 1
fi

if ! rg -q '\[shared-buffer\] pass' "${LOG_FILE}"; then
  echo "shared buffer smoke did not report userspace pass marker" >&2
  cat "${LOG_FILE}" >&2
  exit 1
fi

echo "shared buffer smoke completed successfully."
