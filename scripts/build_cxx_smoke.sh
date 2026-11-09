#!/usr/bin/env bash
set -euo pipefail

# Build the C++ runtime smoke test with the PeetOS cross g++
# (toolchain/build-toolchain.sh): a dynamically linked PIE, a C++ shared
# library it links against, and a C++ plugin it loads with dlopen().
#
# Outputs (staged into the ext2 image by build_ext2_musl_execve.sh):
#   user/cxx_smoke.elf            PIE, PT_INTERP /lib/ld-musl-x86_64.so.1,
#                                 NEEDED libstdc++.so.6, libgcc_s.so.1,
#                                 libpeetcxx.so
#   user/libpeetcxx.so            NEEDED by cxx_smoke
#   user/libpeetcxxplugin.so      loaded at run time with dlopen()

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CXX="${PEETOS_CXX:-${ROOT_DIR}/toolchain/out/bin/x86_64-linux-musl-g++}"
SRC="${ROOT_DIR}/user"
OUT="${ROOT_DIR}/user"

if [[ ! -x "${CXX}" ]]; then
  echo "PeetOS g++ not found at ${CXX} (run 'make -C toolchain')" >&2
  exit 1
fi

CXXFLAGS=(-std=gnu++20 -O2 -g0 -Wall -Wextra -I"${SRC}")
LDFLAGS=(-Wl,-z,max-page-size=0x1000 -L"${OUT}")

"${CXX}" "${CXXFLAGS[@]}" -fPIC -shared -Wl,-soname,libpeetcxx.so \
  -o "${OUT}/libpeetcxx.so" "${SRC}/cxx/libpeetcxx.cpp" "${LDFLAGS[@]}"
"${CXX}" "${CXXFLAGS[@]}" -fPIC -shared -Wl,-soname,libpeetcxxplugin.so \
  -o "${OUT}/libpeetcxxplugin.so" "${SRC}/cxx/plugin.cpp" "${LDFLAGS[@]}" -lpeetcxx
"${CXX}" "${CXXFLAGS[@]}" -fPIE -pie \
  -o "${OUT}/cxx_smoke.elf" "${SRC}/cxx_smoke.cpp" "${LDFLAGS[@]}" -lpeetcxx

if command -v readelf >/dev/null 2>&1; then
  interp="$(readelf -l "${OUT}/cxx_smoke.elf" | sed -n 's/.*Requesting program interpreter: \(.*\)]/\1/p')"
  if [[ "${interp}" != "/lib/ld-musl-x86_64.so.1" ]]; then
    echo "cxx_smoke has unexpected interpreter '${interp}'" >&2
    exit 1
  fi
  for need in libstdc++.so.6 libgcc_s.so.1 libc.so libpeetcxx.so; do
    if ! readelf -d "${OUT}/cxx_smoke.elf" | grep -qF "Shared library: [${need}]"; then
      echo "cxx_smoke is not dynamically linked against ${need}" >&2
      exit 1
    fi
  done
fi
chmod 0755 "${OUT}/cxx_smoke.elf"
echo "Built cxx_smoke.elf, libpeetcxx.so, libpeetcxxplugin.so"
