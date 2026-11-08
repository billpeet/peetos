#!/usr/bin/env bash
set -euo pipefail

# Boot the musl-ext2-wayland-tests profile and require every selected
# upstream libwayland test binary (scripts/build_wayland_tests.sh --list)
# to report PASS.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IMAGE="${ROOT_DIR}/build/peetos.img"
EXT2_IMG="${ROOT_DIR}/user/sample_ext2_musl.img"
LOG_FILE="$(mktemp)"
TIMEOUT_SECS="${WAYLAND_TESTS_TIMEOUT_SECS:-600}"
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

make -C "${ROOT_DIR}" image PROFILE=release KERNEL_FEATURES=musl-ext2-wayland-tests

qemu-system-x86_64 \
  -m 512M \
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
  if grep -q '\[musl-ext2-wayland-tests\] done' "${LOG_FILE}"; then
    break
  fi
  if ! kill -0 "${QEMU_PID}" 2>/dev/null; then
    break
  fi
  sleep 0.5
done
sleep 0.5

if ! grep -q '\[musl-ext2-wayland-tests\] done' "${LOG_FILE}"; then
  echo "upstream libwayland tests did not reach completion marker within ${TIMEOUT_SECS}s" >&2
  cat "${LOG_FILE}" >&2
  exit 1
fi

status=0
if grep -q '\[musl-ext2-wayland-tests\] failures' "${LOG_FILE}" || grep -q '\[wl-tests\] FAIL' "${LOG_FILE}"; then
  echo "upstream libwayland tests completed with failures" >&2
  status=1
fi

while read -r name; do
  if ! grep -q "\[wl-tests\] PASS ${name}\$" <(tr -d '\r' < "${LOG_FILE}"); then
    echo "upstream libwayland test did not pass: ${name}" >&2
    status=1
  fi
done < <("${ROOT_DIR}/scripts/build_wayland_tests.sh" --list)

if [[ "${status}" -ne 0 ]]; then
  cat "${LOG_FILE}" >&2
  exit 1
fi

# Per-test summaries from upstream's runner ("N tests, N pass, 0 fail").
grep -E '\[wl-tests\] PASS|tests, [0-9]+ pass, [0-9]+ fail' "${LOG_FILE}"
echo "upstream libwayland tests completed successfully."
