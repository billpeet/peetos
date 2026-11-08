#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IMAGE="${ROOT_DIR}/build/peetos.img"
EXT2_IMG="${ROOT_DIR}/user/sample_ext2_musl.img"
LOG_FILE="$(mktemp)"
TIMEOUT_SECS="${DYNLINK_SMOKE_TIMEOUT_SECS:-90}"
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

make -C "${ROOT_DIR}" image PROFILE=release KERNEL_FEATURES=musl-ext2-dynlink-smoke

qemu-system-x86_64 \
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
  if grep -q '\[musl-ext2-dynlink-smoke\] done' "${LOG_FILE}"; then
    break
  fi
  if ! kill -0 "${QEMU_PID}" 2>/dev/null; then
    break
  fi
  sleep 0.5
done
sleep 0.5

if ! grep -q '\[musl-ext2-dynlink-smoke\] done' "${LOG_FILE}"; then
  echo "dynamic-linking smoke did not reach completion marker within ${TIMEOUT_SECS}s" >&2
  cat "${LOG_FILE}" >&2
  exit 1
fi

if grep -q '\[musl-ext2-dynlink-smoke\] failures' "${LOG_FILE}" || grep -q '\[dynlink\] FAIL' "${LOG_FILE}"; then
  echo "dynamic-linking smoke completed with failures" >&2
  cat "${LOG_FILE}" >&2
  exit 1
fi

for marker in \
  '\[dynlink\] libc.so stdio/malloc ok' \
  '\[dynlink\] TLS in shared library ok' \
  '\[dynlink\] dynamic TLS in dlopen.d plugin ok' \
  '\[dynlink\] new thread gets fresh library TLS ok' \
  '\[dynlink\] fork + exec of a dynamic program ok' \
  '\[dynlink\] 120 distinct mappings alongside shared libraries ok' \
  '\[dynlink\] mprotect split and restore ok' \
  '\[dynlink\] pass'; do
  if ! grep -q "${marker}" "${LOG_FILE}"; then
    echo "dynamic-linking smoke is missing marker: ${marker}" >&2
    cat "${LOG_FILE}" >&2
    exit 1
  fi
done

grep '\[dynlink\]' "${LOG_FILE}"
echo "dynamic-linking smoke completed successfully."
