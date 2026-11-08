#!/usr/bin/env bash
set -euo pipefail

# End-to-end peetwl compositor smoke: boots the compositor-first profile,
# injects real PS/2 input through the QEMU monitor, and verifies window
# contents, focus, move and close behaviour from screendumps and the log.
# Set WLCOMP_SHOTS=<dir> to keep PNG copies of the screendumps.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IMAGE="${ROOT_DIR}/build/peetos.img"
EXT2_IMG="${ROOT_DIR}/user/sample_ext2_musl.img"
LOG_FILE="$(mktemp)"
MONITOR_SOCK="$(mktemp -u /tmp/peetos-wlcomp-monitor.XXXXXX)"
QEMU_PID=""

source "${ROOT_DIR}/tests/rust_build_compat_env.bash"
setup_rust_build_compat "${ROOT_DIR}"

cleanup() {
  if [[ -n "${QEMU_PID}" ]]; then
    kill "${QEMU_PID}" 2>/dev/null || true
    wait "${QEMU_PID}" 2>/dev/null || true
  fi
  rm -f "${LOG_FILE}" "${MONITOR_SOCK}"
}
trap cleanup EXIT

make -C "${ROOT_DIR}" image PROFILE=release KERNEL_FEATURES="compositor-first musl-ext2-wlcomp-smoke"

qemu-system-x86_64 \
  -drive id=disk0,file="${IMAGE}",if=none,format=raw \
  -device ich9-ahci,id=ahci \
  -device ide-hd,drive=disk0,bus=ahci.0 \
  -drive id=disk1,file="${EXT2_IMG}",if=none,format=raw \
  -device ide-hd,drive=disk1,bus=ahci.1 \
  -serial file:"${LOG_FILE}" \
  -display none \
  -monitor unix:"${MONITOR_SOCK}",server,nowait \
  -no-reboot \
  -no-shutdown &
QEMU_PID=$!

for _ in $(seq 1 100); do
  [[ -S "${MONITOR_SOCK}" ]] && break
  sleep 0.1
done
if [[ ! -S "${MONITOR_SOCK}" ]]; then
  echo "QEMU monitor socket did not appear" >&2
  exit 1
fi

shots_args=()
if [[ -n "${WLCOMP_SHOTS:-}" ]]; then
  shots_args=(--shots "${WLCOMP_SHOTS}")
fi

set +e
python3 "${ROOT_DIR}/scripts/wlcomp_smoke_driver.py" \
  --log "${LOG_FILE}" --monitor "${MONITOR_SOCK}" "${shots_args[@]}"
driver_status=$?
set -e

for _ in $(seq 1 100); do
  grep -q '\[musl-ext2-wlcomp-smoke\] done' "${LOG_FILE}" && break
  sleep 0.2
done

grep -E '\[(peetwl|wl-demo:[A-Za-z]+)\] (ready|mapped|focus|close requested|unmapped|move end|all clients|pass|FAIL)' "${LOG_FILE}" || true

if [[ "${driver_status}" -ne 0 ]]; then
  echo "peetwl compositor smoke driver failed" >&2
  cat "${LOG_FILE}" >&2
  exit 1
fi
if ! grep -qE '\[peetwl\] drm flips=[1-9]' "${LOG_FILE}"; then
  echo "peetwl compositor smoke did not present through DRM page flips" >&2
  cat "${LOG_FILE}" >&2
  exit 1
fi
if ! grep -q '\[musl-ext2-wlcomp-smoke\] done' "${LOG_FILE}" \
  || grep -q '\[musl-ext2-wlcomp-smoke\] failures' "${LOG_FILE}"; then
  echo "peetwl compositor smoke payload did not finish cleanly" >&2
  cat "${LOG_FILE}" >&2
  exit 1
fi

echo "peetwl compositor smoke completed successfully."
