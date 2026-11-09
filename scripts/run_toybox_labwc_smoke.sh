#!/usr/bin/env bash
set -euo pipefail

# End-to-end labwc smoke: boots the labwc profile (labwc on DRM through the
# wlroots DRM and libinput backends, foot as its startup client), drives it
# with real PS/2 key presses, mouse moves and clicks through the QEMU
# monitor, and checks the screen (see scripts/labwc_smoke_driver.py).
# Set LABWC_SHOTS=<dir> to keep PNG copies of the screendumps.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IMAGE="${ROOT_DIR}/build/peetos.img"
EXT2_IMG="${ROOT_DIR}/user/sample_ext2_musl.img"
LOG_FILE="$(mktemp)"
MONITOR_SOCK="$(mktemp -u /tmp/peetos-labwc-monitor.XXXXXX)"
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

make -C "${ROOT_DIR}" image PROFILE=release \
  KERNEL_FEATURES="compositor-first musl-ext2-labwc-smoke ${LABWC_SMOKE_EXTRA_FEATURES:-}"

qemu-system-x86_64 \
  -m "${LABWC_SMOKE_MEM:-768M}" \
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
if [[ -n "${LABWC_SHOTS:-}" ]]; then
  shots_args=(--shots "${LABWC_SHOTS}")
fi

set +e
python3 "${ROOT_DIR}/scripts/labwc_smoke_driver.py" \
  --log "${LOG_FILE}" --monitor "${MONITOR_SOCK}" "${shots_args[@]}"
driver_status=$?
set -e

for _ in $(seq 1 150); do
  grep -q '\[musl-ext2-labwc-smoke\] done' "${LOG_FILE}" && break
  sleep 0.2
done

grep -aE '\[(labwc-driver|labwc-child)\]|musl-ext2-labwc-smoke|Modesetting with|Adding .*\[|ERROR' "${LOG_FILE}" || true

if [[ "${driver_status}" -ne 0 ]]; then
  echo "labwc smoke driver failed" >&2
  cat "${LOG_FILE}" >&2
  exit 1
fi
if ! grep -q '\[musl-ext2-labwc-smoke\] done' "${LOG_FILE}" \
  || grep -q '\[musl-ext2-labwc-smoke\] failures' "${LOG_FILE}"; then
  echo "labwc smoke payload did not finish cleanly" >&2
  cat "${LOG_FILE}" >&2
  exit 1
fi
if grep -q "Unhandled syscall" "${LOG_FILE}"; then
  echo "labwc smoke hit an unhandled syscall:" >&2
  grep "Unhandled syscall" "${LOG_FILE}" | sort | uniq -c >&2
  exit 1
fi
if grep -q "terminated after page fault" "${LOG_FILE}"; then
  echo "a process crashed during the labwc smoke:" >&2
  grep "terminated after page fault" "${LOG_FILE}" >&2
  exit 1
fi
# wlroots/labwc errors, except the expected ones: the simpledrm-like
# connector has no EDID, and libinput's lateness warnings ("your system is
# too slow") under TCG emulation with a 10 ms timer tick.
unexpected="$(grep -a '\[ERROR\]' "${LOG_FILE}" \
  | grep -v -e 'Failed to parse EDID' -e 'your system is too slow' || true)"
if [[ -n "${unexpected}" ]]; then
  echo "labwc smoke logged unexpected errors:" >&2
  echo "${unexpected}" >&2
  exit 1
fi

echo "labwc smoke completed successfully."
