#!/usr/bin/env bash
set -euo pipefail

# End-to-end desktop smoke: boots the desktop-smoke profile, whose init runs
# peet-session (a D-Bus session bus, labwc, and the sfwbar panel from
# labwc's autostart), drives it with real PS/2 key presses, mouse moves and
# clicks through the QEMU monitor, and checks the screen and the session bus
# (see scripts/desktop_smoke_driver.py and docs/desktop.md).
# Set DESKTOP_SHOTS=<dir> to keep PNG copies of the screendumps.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IMAGE="${ROOT_DIR}/build/peetos.img"
EXT2_IMG="${ROOT_DIR}/user/sample_ext2_musl.img"
LOG_FILE="$(mktemp)"
MONITOR_SOCK="$(mktemp -u /tmp/peetos-desktop-monitor.XXXXXX)"
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
  KERNEL_FEATURES="compositor-first musl-ext2-desktop-smoke ${DESKTOP_SMOKE_EXTRA_FEATURES:-}"

qemu-system-x86_64 \
  -m "${DESKTOP_SMOKE_MEM:-1G}" \
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
if [[ -n "${DESKTOP_SHOTS:-}" ]]; then
  shots_args=(--shots "${DESKTOP_SHOTS}")
fi

set +e
python3 "${ROOT_DIR}/scripts/desktop_smoke_driver.py" \
  --log "${LOG_FILE}" --monitor "${MONITOR_SOCK}" "${shots_args[@]}"
driver_status=$?
set -e

for _ in $(seq 1 150); do
  grep -q '\[musl-ext2-desktop-smoke\] done' "${LOG_FILE}" && break
  sleep 0.2
done

grep -aE '\[(desktop-driver|desktop-child)\]|musl-ext2-desktop-smoke|ERROR' "${LOG_FILE}" || true

if [[ "${driver_status}" -ne 0 ]]; then
  echo "desktop smoke driver failed" >&2
  cat "${LOG_FILE}" >&2
  exit 1
fi
if ! grep -q '\[musl-ext2-desktop-smoke\] done' "${LOG_FILE}" \
  || grep -q '\[musl-ext2-desktop-smoke\] failures' "${LOG_FILE}"; then
  echo "desktop smoke payload did not finish cleanly" >&2
  cat "${LOG_FILE}" >&2
  exit 1
fi
if grep -q "Unhandled syscall" "${LOG_FILE}"; then
  echo "desktop smoke hit an unhandled syscall:" >&2
  grep "Unhandled syscall" "${LOG_FILE}" | sort | uniq -c >&2
  exit 1
fi
if grep -q "terminated after page fault" "${LOG_FILE}"; then
  echo "a process crashed during the desktop smoke:" >&2
  grep "terminated after page fault" "${LOG_FILE}" >&2
  exit 1
fi
# A client that broke the Wayland protocol (labwc disconnects it). Passed
# file descriptors arriving apart from their request showed up this way.
if grep -q "error in client communication" "${LOG_FILE}"; then
  echo "labwc dropped a Wayland client during the desktop smoke:" >&2
  grep -a -B1 "error in client communication" "${LOG_FILE}" >&2
  exit 1
fi
# wlroots/labwc errors, except the expected ones: the simpledrm-like
# connector has no EDID, and libinput's lateness warnings ("your system is
# too slow", and its notice that it stops repeating them) under TCG
# emulation with a 10 ms timer tick.
unexpected="$(grep -a '\[ERROR\]' "${LOG_FILE}" \
  | grep -v -e 'Failed to parse EDID' -e 'your system is too slow' -e 'log rate limit exceeded' || true)"
if [[ -n "${unexpected}" ]]; then
  echo "desktop smoke logged unexpected errors:" >&2
  echo "${unexpected}" >&2
  exit 1
fi

echo "desktop smoke completed successfully."
