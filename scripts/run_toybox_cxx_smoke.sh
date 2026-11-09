#!/usr/bin/env bash
set -euo pipefail

# C++ runtime smoke: boots the musl-ext2-cxx-smoke profile, which runs
# bin/cxx_smoke (libstdc++ + libgcc_s on musl: exceptions across shared
# objects, RTTI, iostreams, threads, thread_local, dlopen()ed C++ plugins).
# Boots with 2 CPUs by default so the thread checks run in parallel.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IMAGE="${ROOT_DIR}/build/peetos.img"
EXT2_IMG="${ROOT_DIR}/user/sample_ext2_musl.img"
LOG_FILE="$(mktemp)"
TIMEOUT_SECS="${CXX_SMOKE_TIMEOUT_SECS:-90}"
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

make -C "${ROOT_DIR}" image PROFILE=release KERNEL_FEATURES=musl-ext2-cxx-smoke

qemu-system-x86_64 \
  -m "${CXX_SMOKE_MEM:-256M}" \
  -smp "${CXX_SMOKE_SMP:-2}" \
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
  if grep -q '\[musl-ext2-cxx-smoke\] done' "${LOG_FILE}"; then
    break
  fi
  if ! kill -0 "${QEMU_PID}" 2>/dev/null; then
    break
  fi
  sleep 0.5
done
sleep 0.5

if ! grep -q '\[musl-ext2-cxx-smoke\] done' "${LOG_FILE}"; then
  echo "C++ runtime smoke did not reach completion marker within ${TIMEOUT_SECS}s" >&2
  cat "${LOG_FILE}" >&2
  exit 1
fi

if grep -q '\[musl-ext2-cxx-smoke\] failures' "${LOG_FILE}" || grep -q '\[cxx\] FAIL' "${LOG_FILE}"; then
  echo "C++ runtime smoke completed with failures" >&2
  cat "${LOG_FILE}" >&2
  exit 1
fi

for marker in \
  '[cxx] executable static constructor ran before main ok' \
  '[cxx] shared library static constructor ran once ok' \
  '[cxx] unwinding runs destructors in every frame ok' \
  '[cxx] exception_ptr rethrow ok' \
  '[cxx] exception thrown in a shared library caught by its type ok' \
  '[cxx] failed allocation throws std::bad_alloc ok' \
  '[cxx] dynamic_cast ok' \
  '[cxx] virtual call into a shared library object ok' \
  '[cxx] ostringstream formatting ok' \
  '[cxx] std::format ok' \
  '[cxx] std::thread with atomics and std::mutex ok' \
  '[cxx] thread_local storage is per thread ok' \
  '[cxx] thread_local destructors run at thread exit ok' \
  '[cxx] function-local static initialized once under contention ok' \
  '[cxx] condition_variable handoff ok' \
  '[cxx] std::async result and exception propagation ok' \
  '[cxx] steady_clock and sleep_for ok' \
  '[cxx] plugin static constructor ran on dlopen ok' \
  '[cxx] exception thrown in a dlopen()ed plugin caught by its type ok' \
  '[cxx] dlclose C++ plugin ok' \
  '[cxx] std::filesystem directory iteration ok' \
  '[cxx] pass' \
  '[cxx] static destructor ran'; do
  if ! grep -qF "${marker}" "${LOG_FILE}"; then
    echo "C++ runtime smoke is missing marker: ${marker}" >&2
    cat "${LOG_FILE}" >&2
    exit 1
  fi
done

grep -F '[SMP]' "${LOG_FILE}" || true
grep -F '[cxx]' "${LOG_FILE}"
echo "C++ runtime smoke completed successfully."
