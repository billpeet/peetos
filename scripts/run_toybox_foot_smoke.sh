#!/usr/bin/env bash
set -euo pipefail

# End-to-end foot terminal smoke: boots the compositor-first profile,
# injects real PS/2 input through the QEMU monitor, and verifies window
# contents, focus, move and close behaviour from screendumps and the log.
# Set FOOT_SHOTS=<dir> to keep PNG copies of the screendumps.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IMAGE="${ROOT_DIR}/build/peetos.img"
EXT2_IMG="${ROOT_DIR}/user/sample_ext2_musl.img"
LOG_FILE="$(mktemp)"
MONITOR_SOCK="$(mktemp -u /tmp/peetos-foot-monitor.XXXXXX)"
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

make -C "${ROOT_DIR}" image PROFILE=release KERNEL_FEATURES="compositor-first musl-ext2-foot-smoke"

qemu-system-x86_64 \
  -m "${FOOT_SMOKE_MEM:-512M}" \
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
if [[ -n "${FOOT_SHOTS:-}" ]]; then
  shots_args=(--shots "${FOOT_SHOTS}")
fi

set +e
python3 "${ROOT_DIR}/scripts/foot_smoke_driver.py" \
  --log "${LOG_FILE}" --monitor "${MONITOR_SOCK}" "${shots_args[@]}"
driver_status=$?
set -e

for _ in $(seq 1 100); do
  grep -q '\[musl-ext2-foot-smoke\] done' "${LOG_FILE}" && break
  sleep 0.2
done

grep -E '\[(peetwl|foot-child)\] (output|ready|mapped|unmapped|all clients|pass|FAIL|started|prompt|got)|drm flips' "${LOG_FILE}" || true

if [[ "${driver_status}" -ne 0 ]]; then
  echo "foot terminal smoke driver failed" >&2
  cat "${LOG_FILE}" >&2
  exit 1
fi
if ! grep -qE '\[peetwl\] drm flips=[1-9]' "${LOG_FILE}"; then
  echo "foot terminal smoke did not present through DRM page flips" >&2
  cat "${LOG_FILE}" >&2
  exit 1
fi
if ! grep -q '\[musl-ext2-foot-smoke\] done' "${LOG_FILE}" \
  || grep -q '\[musl-ext2-foot-smoke\] failures' "${LOG_FILE}"; then
  echo "foot terminal smoke payload did not finish cleanly" >&2
  cat "${LOG_FILE}" >&2
  exit 1
fi

# Per-block scheduler tracing is opt-in (`sched-trace`); foot blocks on
# every keystroke and frame, so a default build must not log it.
if grep -q '\[SCHED\] Blocking task' "${LOG_FILE}"; then
  echo "scheduler block tracing is on in a default build (expected only with sched-trace)" >&2
  exit 1
fi

echo "foot terminal smoke completed successfully."
