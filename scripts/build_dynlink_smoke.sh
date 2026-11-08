#!/usr/bin/env bash
set -euo pipefail

# Build the dynamic-linking smoke test: two shared libraries and a dynamically
# linked PIE that uses them, all against musl's shared libc
# (scripts/build_musl_shared.sh).
#
# Outputs (staged into the ext2 image by build_ext2_musl_execve.sh):
#   user/dynlink_smoke.elf   PIE, PT_INTERP /lib/ld-musl-x86_64.so.1
#   user/libpeetdyn.so       NEEDED by dynlink_smoke
#   user/libpeetplugin.so    loaded at run time with dlopen()

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MUSL_CC="${MUSL_CC:-${ROOT_DIR}/musl/sysroot/bin/musl-gcc}"
SHARED_LIB_DIR="${ROOT_DIR}/musl/shared/lib"
SRC="${ROOT_DIR}/user"
OUT="${ROOT_DIR}/user"

if [[ ! -f "${SHARED_LIB_DIR}/libc.so" ]]; then
  echo "shared musl libc not found: ${SHARED_LIB_DIR}/libc.so (run 'make -C musl shared')" >&2
  exit 1
fi

# -L musl/shared/lib comes before the sysroot's libc.a in the search order, so
# these link against libc.so (and the loader) instead of static libc.
LINK=(-L"${SHARED_LIB_DIR}" -L"${OUT}")
CFLAGS=(-O2 -g0 -Wall -Wextra)

"${MUSL_CC}" "${CFLAGS[@]}" -fPIC -shared -Wl,-soname,libpeetdyn.so \
  -o "${OUT}/libpeetdyn.so" "${SRC}/dynlink/libpeetdyn.c" "${LINK[@]}"
"${MUSL_CC}" "${CFLAGS[@]}" -fPIC -shared -Wl,-soname,libpeetplugin.so \
  -o "${OUT}/libpeetplugin.so" "${SRC}/dynlink/plugin.c" "${LINK[@]}" -lpeetdyn
"${MUSL_CC}" "${CFLAGS[@]}" -fPIE -pie -Wl,-z,max-page-size=0x1000 \
  -o "${OUT}/dynlink_smoke.elf" "${SRC}/dynlink_smoke.c" "${LINK[@]}" -lpeetdyn

if command -v readelf >/dev/null 2>&1; then
  interp="$(readelf -l "${OUT}/dynlink_smoke.elf" | sed -n 's/.*Requesting program interpreter: \(.*\)]/\1/p')"
  if [[ "${interp}" != "/lib/ld-musl-x86_64.so.1" ]]; then
    echo "dynlink_smoke has unexpected interpreter '${interp}'" >&2
    exit 1
  fi
  for need in libc.so libpeetdyn.so; do
    if ! readelf -d "${OUT}/dynlink_smoke.elf" | grep -q "Shared library: \[${need}\]"; then
      echo "dynlink_smoke is not dynamically linked against ${need}" >&2
      exit 1
    fi
  done
fi
echo "Built dynlink_smoke.elf, libpeetdyn.so, libpeetplugin.so"
