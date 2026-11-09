#!/usr/bin/env bash
set -euo pipefail

# D-Bus smoke: a session dbus-daemon, libdbus (dbus-send) and GDBus (gdbus)
# clients authenticating over AF_UNIX (SO_PEERCRED / SCM_CREDENTIALS), bus
# activation of xfconfd, and xfconf settings written to disk and read back.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IMAGE="${ROOT_DIR}/build/peetos.img"
EXT2_IMG="${ROOT_DIR}/user/sample_ext2_musl.img"
LOG_FILE="$(mktemp)"
TIMEOUT_SECS="${DBUS_SMOKE_TIMEOUT_SECS:-150}"
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

make -C "${ROOT_DIR}" image PROFILE=release KERNEL_FEATURES="musl-ext2-dbus-smoke ${DBUS_SMOKE_EXTRA_FEATURES:-}"

qemu-system-x86_64 \
  -m "${DBUS_SMOKE_MEM:-256M}" \
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

deadline=$((SECONDS + TIMEOUT_SECS))
while (( SECONDS < deadline )); do
  if grep -q '\[musl-ext2-dbus-smoke\] done' "${LOG_FILE}"; then
    break
  fi
  if ! kill -0 "${QEMU_PID}" 2>/dev/null; then
    break
  fi
  sleep 0.5
done
sleep 0.5

grep -aE '\[dbus-smoke\]|musl-ext2-dbus-smoke|dbus-daemon|xfconf|GLib' "${LOG_FILE}" | head -60 || true

if ! grep -q '\[musl-ext2-dbus-smoke\] done' "${LOG_FILE}"; then
  echo "D-Bus smoke did not reach completion marker within ${TIMEOUT_SECS}s" >&2
  cat "${LOG_FILE}" >&2
  exit 1
fi

if grep -q '\[musl-ext2-dbus-smoke\] failures' "${LOG_FILE}" || grep -qF '[dbus-smoke] FAIL' "${LOG_FILE}"; then
  echo "D-Bus smoke completed with failures" >&2
  cat "${LOG_FILE}" >&2
  exit 1
fi

if grep -q 'Unhandled syscall' "${LOG_FILE}"; then
  echo "D-Bus smoke hit an unhandled syscall:" >&2
  grep 'Unhandled syscall' "${LOG_FILE}" | sort | uniq -c >&2
  exit 1
fi

for marker in \
  '[dbus-smoke] dbus-daemon started' \
  '[dbus-smoke] dbus-send ListNames ok' \
  "[dbus-smoke] gdbus call sees the daemon's pid ok" \
  '[dbus-smoke] gdbus GetConnectionUnixUser ok' \
  '[dbus-smoke] xfconf-query set (xfconfd activated) ok' \
  '[dbus-smoke] org.xfce.Xfconf is on the bus ok' \
  '[dbus-smoke] xfconfd saved the channel ok' \
  '[dbus-smoke] re-activated xfconfd reads the saved value ok'; do
  if ! grep -qF "${marker}" "${LOG_FILE}"; then
    echo "D-Bus smoke is missing marker: ${marker}" >&2
    cat "${LOG_FILE}" >&2
    exit 1
  fi
done

echo "D-Bus smoke completed successfully."
