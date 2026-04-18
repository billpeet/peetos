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

make -C "${ROOT_DIR}" image PROFILE=release KERNEL_FEATURES=musl-ext2-console-first-smoke

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

if ! rg -q '\[GUI\] Display owner: console' "${LOG_FILE}"; then
  echo "kernel did not stay in console-first ownership mode" >&2
  cat "${LOG_FILE}" >&2
  exit 1
fi

if rg -q '\[GUI\] Display owner: compositor-first' "${LOG_FILE}"; then
  echo "console-first smoke unexpectedly entered compositor-first mode" >&2
  cat "${LOG_FILE}" >&2
  exit 1
fi

if ! rg -q '\[musl-ext2-console-first-smoke\] done' "${LOG_FILE}"; then
  echo "console-first shell smoke did not reach completion marker" >&2
  cat "${LOG_FILE}" >&2
  exit 1
fi

if rg -q '\[musl-ext2-console-first-smoke\] failures' "${LOG_FILE}"; then
  echo "console-first shell smoke completed with command failures" >&2
  cat "${LOG_FILE}" >&2
  exit 1
fi

echo "console-first shell smoke completed successfully."
