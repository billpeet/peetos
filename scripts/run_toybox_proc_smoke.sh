#!/usr/bin/env bash
# QEMU smoke test for the process model: copy-on-write fork and pthreads.
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

make -C "${ROOT_DIR}" image PROFILE=release KERNEL_FEATURES=musl-ext2-proc-smoke

set +e
timeout 40s qemu-system-x86_64 \
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

fail() {
  echo "$1" >&2
  cat "${LOG_FILE}" >&2
  exit 1
}

if [[ "${status}" -ne 0 && "${status}" -ne 124 ]]; then
  fail "QEMU exited with unexpected status ${status}"
fi

if ! grep -q '\[musl-ext2-proc-smoke\] done' "${LOG_FILE}"; then
  fail "process-model smoke did not reach completion marker"
fi

if grep -q '\[musl-ext2-proc-smoke\] failures' "${LOG_FILE}"; then
  fail "process-model smoke completed with failures"
fi

if ! grep -q '\[proc-smoke\] pass' "${LOG_FILE}"; then
  fail "process-model smoke did not report userspace pass marker"
fi

# fork must share private pages copy-on-write rather than copying them.
if ! grep -Eq '\[VMM\] Fork: [1-9][0-9]* pages shared copy-on-write, 0 copied' "${LOG_FILE}"; then
  fail "fork did not share pages copy-on-write"
fi

echo "process-model smoke completed successfully."
