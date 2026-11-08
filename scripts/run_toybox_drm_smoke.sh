#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IMAGE="${ROOT_DIR}/build/peetos.img"
EXT2_IMG="${ROOT_DIR}/user/sample_ext2_musl.img"
LOG_FILE="$(mktemp)"
TIMEOUT_SECS="${DRM_SMOKE_TIMEOUT_SECS:-90}"
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

make -C "${ROOT_DIR}" image PROFILE=release KERNEL_FEATURES=musl-ext2-drm-smoke

qemu-system-x86_64 \
  -m "${DRM_SMOKE_MEM:-256M}" \
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
  if grep -q '\[musl-ext2-drm-smoke\] done' "${LOG_FILE}"; then
    break
  fi
  if ! kill -0 "${QEMU_PID}" 2>/dev/null; then
    break
  fi
  sleep 0.5
done
sleep 0.5

if ! grep -q '\[musl-ext2-drm-smoke\] done' "${LOG_FILE}"; then
  echo "DRM/KMS smoke did not reach completion marker within ${TIMEOUT_SECS}s" >&2
  cat "${LOG_FILE}" >&2
  exit 1
fi

if grep -q '\[musl-ext2-drm-smoke\] failures' "${LOG_FILE}" || grep -q '\[drm\] FAIL' "${LOG_FILE}"; then
  echo "DRM/KMS smoke completed with failures" >&2
  cat "${LOG_FILE}" >&2
  exit 1
fi

for marker in \
  '\[drm\] drmGetVersion reports simpledrm ok' \
  '\[drm\] connector connected with one mode ok' \
  '\[drm\] plane type property is primary ok' \
  '\[drm\] buffer A is on screen ok' \
  '\[drm\] flip completion event via poll + drmHandleEvent ok' \
  '\[drm\] buffer B is on screen after the flip ok' \
  '\[drm\] drmModeDirtyFB flushes the clip ok' \
  '\[drm\] exported buffer maps with the same contents ok' \
  '\[drm\] non-master modeset is EACCES ok' \
  '\[drm\] pass'; do
  if ! grep -qF "${marker//\\/}" "${LOG_FILE}"; then
    echo "DRM/KMS smoke is missing marker: ${marker}" >&2
    cat "${LOG_FILE}" >&2
    exit 1
  fi
done

grep '\[drm\]' "${LOG_FILE}"
echo "DRM/KMS smoke completed successfully."
