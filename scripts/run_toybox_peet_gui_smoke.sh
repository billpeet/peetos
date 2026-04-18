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

make -C "${ROOT_DIR}" image PROFILE=release KERNEL_FEATURES="compositor-first musl-ext2-peet-gui-smoke"

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
  if rg -q '\[peet-gui\] waiting-for-focus-input' "${LOG_FILE}"; then
    break
  fi
  if ! kill -0 "${QEMU_PID}" 2>/dev/null; then
    echo "QEMU exited before peet-gui compositor became ready" >&2
    cat "${LOG_FILE}" >&2
    exit 1
  fi
  sleep 0.1
done

if ! rg -q '\[peet-gui\] waiting-for-focus-input' "${LOG_FILE}"; then
  echo "peet-gui compositor did not reach input-ready marker" >&2
  cat "${LOG_FILE}" >&2
  exit 1
fi

(
  sleep 0.2
  {
    printf 'sendkey a\n'
    sleep 0.2
    printf 'mouse_move 12 0\n'
    sleep 0.2
    printf 'mouse_button 1\n'
    sleep 0.2
    printf 'mouse_button 0\n'
    sleep 0.2
    printf 'sendkey c\n'
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
  '\[GUI\] Display owner: compositor-first' \
  '\[peet-gui\] client1 attached' \
  '\[peet-gui\] client2 attached' \
  '\[peet-gui\] overlap-top=1' \
  '\[peet-gui\] damage-validated=1' \
  '\[peet-gui-client1\] received-key=30' \
  '\[peet-gui-client2\] received-key=46' \
  '\[peet-gui-client2\] disconnect-requested=1' \
  '\[peet-gui\] client2 detached' \
  '\[peet-gui\] focus-switched=2 overlap-top=2' \
  '\[peet-gui\] observed done client1=1 client2=1 pointer=1 disconnect=2' \
  '\[peet-gui\] pass' \
  '\[musl-ext2-peet-gui-smoke\] done'
do
  if ! rg -q "${token}" "${LOG_FILE}"; then
    echo "peet-gui smoke log missing ${token}" >&2
    cat "${LOG_FILE}" >&2
    exit 1
  fi
done

if rg -q '\[musl-ext2-peet-gui-smoke\] failures' "${LOG_FILE}"; then
  echo "peet-gui smoke completed with failures" >&2
  cat "${LOG_FILE}" >&2
  exit 1
fi

echo "peet-gui compositor smoke completed successfully."
