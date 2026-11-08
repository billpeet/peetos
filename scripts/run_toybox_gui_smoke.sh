#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IMAGE="${ROOT_DIR}/build/peetos.img"
EXT2_IMG="${ROOT_DIR}/user/sample_ext2_musl.img"
LOG_FILE="$(mktemp)"
MONITOR_SOCK="$(mktemp -u)"
QEMU_PID=""

source "${ROOT_DIR}/tests/rust_build_compat_env.bash"
setup_rust_build_compat "${ROOT_DIR}"

cleanup() {
  rm -f "${LOG_FILE}"
  rm -f "${MONITOR_SOCK}"
  if [[ -n "${QEMU_PID}" ]] && kill -0 "${QEMU_PID}" 2>/dev/null; then
    kill "${QEMU_PID}" 2>/dev/null || true
    wait "${QEMU_PID}" 2>/dev/null || true
  fi
}
trap cleanup EXIT

make -C "${ROOT_DIR}" image PROFILE=release KERNEL_FEATURES="compositor-first musl-ext2-gui-smoke"

qemu-system-x86_64 \
  -drive id=disk0,file="${IMAGE}",if=none,format=raw \
  -device ich9-ahci,id=ahci \
  -device ide-hd,drive=disk0,bus=ahci.0 \
  -drive id=disk1,file="${EXT2_IMG}",if=none,format=raw \
  -device ide-hd,drive=disk1,bus=ahci.1 \
  -monitor "unix:${MONITOR_SOCK},server,nowait" \
  -serial stdio \
  -display none \
  -no-reboot \
  -no-shutdown \
  > "${LOG_FILE}" 2>&1 &
QEMU_PID=$!

deadline=$((SECONDS + 20))
monitor_ready=0
guest_ready=0
input_injected=0

while (( SECONDS < deadline )); do
  if [[ "${monitor_ready}" -eq 0 && -S "${MONITOR_SOCK}" ]]; then
    monitor_ready=1
  fi

  if [[ "${guest_ready}" -eq 0 ]] && rg -q '\[gui-smoke\] waiting-for-input' "${LOG_FILE}"; then
    guest_ready=1
  fi

  if [[ "${input_injected}" -eq 0 && "${monitor_ready}" -eq 1 && "${guest_ready}" -eq 1 ]]; then
    {
      printf 'sendkey a\n'
      printf 'mouse_move 12 0\n'
      printf 'mouse_button 1\n'
      printf 'mouse_button 0\n'
    } | nc -U -q 0 "${MONITOR_SOCK}" >/dev/null
    input_injected=1
  fi

  # Wait for the init script's completion marker, which follows the
  # userspace pass marker; stopping at "pass" races the final output.
  if rg -q '\[musl-ext2-gui-smoke\] done' "${LOG_FILE}"; then
    break
  fi

  if ! kill -0 "${QEMU_PID}" 2>/dev/null; then
    break
  fi

  sleep 0.1
done

if kill -0 "${QEMU_PID}" 2>/dev/null; then
  kill "${QEMU_PID}" 2>/dev/null || true
  wait "${QEMU_PID}" 2>/dev/null || true
fi
QEMU_PID=""

if ! rg -q '\[GUI\] Display owner: compositor-first' "${LOG_FILE}"; then
  echo "kernel did not enter compositor-first ownership mode" >&2
  cat "${LOG_FILE}" >&2
  exit 1
fi

if ! rg -q '\[musl-ext2-gui-smoke\] done' "${LOG_FILE}"; then
  echo "gui smoke did not reach completion marker" >&2
  cat "${LOG_FILE}" >&2
  exit 1
fi

if rg -q '\[musl-ext2-gui-smoke\] failures' "${LOG_FILE}"; then
  echo "gui smoke completed with failures" >&2
  cat "${LOG_FILE}" >&2
  exit 1
fi

if ! rg -q '\[gui-smoke\] observed keyboard=1 pointer=1' "${LOG_FILE}"; then
  echo "gui compositor smoke did not observe injected keyboard and pointer events" >&2
  cat "${LOG_FILE}" >&2
  exit 1
fi

if ! rg -q '\[gui-smoke\] pass' "${LOG_FILE}"; then
  echo "gui compositor smoke did not report userspace pass marker" >&2
  cat "${LOG_FILE}" >&2
  exit 1
fi

echo "gui compositor smoke completed successfully."
