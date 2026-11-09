#!/usr/bin/env bash
set -euo pipefail

# Host-side checks for the C++ cross toolchain (docs/cxx.md):
# - the toolchain scripts parse and refuse to run without their inputs;
# - when the toolchain is built, a static C++ program using exceptions,
#   threads and std::format runs on the (Linux) build machine, and a dynamic
#   build gets the musl loader and the shared C++ runtime as dependencies.
# The in-QEMU runtime test is tests/musl_ext2_cxx_smoke.sh.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CXX="${ROOT_DIR}/toolchain/out/bin/x86_64-linux-musl-g++"
TMP_DIR="$(mktemp -d)"
trap 'rm -rf "${TMP_DIR}"' EXIT

for script in toolchain/fetch-toolchain.sh toolchain/build-toolchain.sh scripts/build_cxx_smoke.sh \
  scripts/run_toybox_cxx_smoke.sh; do
  bash -n "${ROOT_DIR}/${script}"
done

# build-toolchain.sh must stop with a clear message when the GCC sources are
# missing (run from a scratch copy so the real toolchain/ is untouched).
mkdir -p "${TMP_DIR}/repo/toolchain"
cp "${ROOT_DIR}/toolchain/build-toolchain.sh" "${TMP_DIR}/repo/toolchain/"
if out="$("${TMP_DIR}/repo/toolchain/build-toolchain.sh" 2>&1)"; then
  echo "build-toolchain.sh succeeded without GCC sources" >&2
  exit 1
fi
if ! grep -qE "GCC sources not found|not found on the build machine" <<< "${out}"; then
  echo "build-toolchain.sh failed without a useful message: ${out}" >&2
  exit 1
fi

if [[ ! -x "${CXX}" ]]; then
  echo "C++ toolchain not built (make -C toolchain); skipping compile checks"
  exit 0
fi

cat > "${TMP_DIR}/probe.cpp" <<'EOS'
#include <atomic>
#include <format>
#include <iostream>
#include <stdexcept>
#include <thread>
#include <vector>

static int thrower(int v) {
    if (v > 2) throw std::runtime_error(std::format("v={}", v));
    return v;
}

int main() {
    std::atomic<int> sum{0};
    std::vector<std::thread> threads;
    for (int t = 0; t < 4; ++t)
        threads.emplace_back([&] { for (int i = 0; i < 1000; ++i) sum.fetch_add(1); });
    for (auto &th : threads) th.join();
    std::string caught;
    try {
        thrower(3);
    } catch (const std::exception &e) {
        caught = e.what();
    }
    std::cout << "sum=" << sum << " caught=" << caught << std::endl;
    return (sum == 4000 && caught == "v=3") ? 0 : 1;
}
EOS

"${CXX}" -std=gnu++20 -O2 -static -o "${TMP_DIR}/probe-static" "${TMP_DIR}/probe.cpp"
if [[ "$(uname -s)" == "Linux" && "$(uname -m)" == "x86_64" ]]; then
  got="$("${TMP_DIR}/probe-static")"
  if [[ "${got}" != "sum=4000 caught=v=3" ]]; then
    echo "static C++ probe printed '${got}'" >&2
    exit 1
  fi
fi

"${CXX}" -std=gnu++20 -O2 -o "${TMP_DIR}/probe-dyn" "${TMP_DIR}/probe.cpp"
interp="$(readelf -l "${TMP_DIR}/probe-dyn" | sed -n 's/.*Requesting program interpreter: \(.*\)]/\1/p')"
if [[ "${interp}" != "/lib/ld-musl-x86_64.so.1" ]]; then
  echo "dynamic C++ probe has interpreter '${interp}'" >&2
  exit 1
fi
if ! readelf -h "${TMP_DIR}/probe-dyn" | grep -q 'DYN (Position-Independent Executable'; then
  echo "dynamic C++ probe is not a PIE" >&2
  exit 1
fi
for need in libstdc++.so.6 libgcc_s.so.1 libc.so; do
  if ! readelf -d "${TMP_DIR}/probe-dyn" | grep -qF "Shared library: [${need}]"; then
    echo "dynamic C++ probe does not need ${need}" >&2
    exit 1
  fi
done

echo "C++ toolchain checks passed"
