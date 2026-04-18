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
  if [[ -n "${QEMU_PID}" ]] && kill -0 "${QEMU_PID}" 2>/dev/null; then
    kill "${QEMU_PID}" 2>/dev/null || true
    for _ in $(seq 1 20); do
      if ! kill -0 "${QEMU_PID}" 2>/dev/null; then
        break
      fi
      sleep 0.1
    done
    if kill -0 "${QEMU_PID}" 2>/dev/null; then
      kill -9 "${QEMU_PID}" 2>/dev/null || true
    fi
    wait "${QEMU_PID}" 2>/dev/null || true
  fi
  rm -f "${LOG_FILE}" "${MONITOR_SOCK}"
}
trap cleanup EXIT

make -C "${ROOT_DIR}" image PROFILE=release KERNEL_FEATURES="compositor-first musl-ext2-peet-shell-demo"

qemu-system-x86_64 \
  -drive id=disk0,file="${IMAGE}",if=none,format=raw \
  -device ich9-ahci,id=ahci \
  -device ide-hd,drive=disk0,bus=ahci.0 \
  -drive id=disk1,file="${EXT2_IMG}",if=none,format=raw \
  -device ide-hd,drive=disk1,bus=ahci.1 \
  -serial stdio \
  -display gtk \
  -monitor unix:"${MONITOR_SOCK}",server,nowait \
  -no-reboot \
  -no-shutdown \
  > "${LOG_FILE}" 2>&1 &
QEMU_PID=$!

for _ in $(seq 1 80); do
  if [[ -S "${MONITOR_SOCK}" ]]; then
    break
  fi
  sleep 0.1
done

if [[ ! -S "${MONITOR_SOCK}" ]]; then
  echo "QEMU monitor socket did not appear" >&2
  cat "${LOG_FILE}" >&2
  exit 1
fi

for _ in $(seq 1 120); do
  if rg -q '\[peet-shell\] ready' "${LOG_FILE}"; then
    break
  fi
  if ! kill -0 "${QEMU_PID}" 2>/dev/null; then
    echo "QEMU exited before visual shell became ready" >&2
    cat "${LOG_FILE}" >&2
    exit 1
  fi
  sleep 0.1
done

if ! rg -q '\[peet-shell\] ready' "${LOG_FILE}"; then
  echo "visual shell did not reach ready marker" >&2
  cat "${LOG_FILE}" >&2
  exit 1
fi

(
  sleep 0.2
  {
    printf 'sendkey tab\n'
    sleep 0.2
    printf 'sendkey ret\n'
    sleep 0.2
    printf 'sendkey t\n'
    sleep 0.2
    printf 'sendkey tab\n'
    sleep 0.2
    printf 'sendkey tab\n'
    sleep 0.2
    printf 'sendkey p\n'
    sleep 0.2
    printf 'sendkey w\n'
    sleep 0.2
    printf 'sendkey d\n'
    sleep 0.2
    printf 'sendkey ret\n'
    sleep 0.2
    printf 'quit\n'
  } | nc -U -q 0 "${MONITOR_SOCK}"
) >/dev/null 2>&1 &

wait "${QEMU_PID}"
status=$?
QEMU_PID=""

if [[ "${status}" -ne 0 ]]; then
  echo "QEMU exited with unexpected status ${status}" >&2
  cat "${LOG_FILE}" >&2
  exit 1
fi

for token in \
  '\[musl-ext2-peet-shell-demo\] launching visual shell scaffold' \
  '\[peet-shell-files\] ready' \
  '\[peet-shell-files\] selected=' \
  '\[peet-shell\] surface-attach=TERMINAL' \
  '\[peet-shell\] surface-attach=FILES' \
  '\[peet-shell\] surface-attach=INSPECTOR' \
  '\[peet-shell\] ready' \
  '\[peet-shell\] terminal-shell-spawned pid=' \
  '\[peet-shell\] focus=FILES source=keyboard' \
  '\[peet-shell\] surface-raised=FILES source=keyboard' \
  '\[peet-shell\] theme=' \
  '\[peet-shell\] focus=INSPECTOR source=keyboard' \
  '\[peet-shell\] surface-raised=INSPECTOR source=keyboard' \
  '\[peet-shell\] focus=TERMINAL source=keyboard' \
  '\[peet-shell\] surface-raised=TERMINAL source=keyboard' \
  '\[peet-shell\] terminal-command=pwd' \
  '\[peet-shell\] terminal-observed-pwd=/mnt/ext2'
do
  if ! rg -q "${token}" "${LOG_FILE}"; then
    echo "visual shell smoke log missing ${token}" >&2
    cat "${LOG_FILE}" >&2
    exit 1
  fi
done

echo "peet shell demo smoke completed successfully."
