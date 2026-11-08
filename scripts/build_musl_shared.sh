#!/usr/bin/env bash
set -euo pipefail

# Build musl's shared libc (which is also its dynamic loader,
# ld-musl-x86_64.so.1) out of tree from musl/musl-src into musl/shared.
#
# The default musl sysroot (musl/sysroot) stays static-only; this produces
# musl/shared/lib/libc.so for dynamically linked PeetOS programs. Programs are
# still compiled with musl/sysroot's musl-gcc and headers (same musl version);
# linking with `-L musl/shared/lib` (before the sysroot's libc.a) and without
# `-static` produces a PIE that uses /lib/ld-musl-x86_64.so.1 at run time.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MUSL_SRC="${MUSL_SRC:-${ROOT_DIR}/musl/musl-src}"
OUT="${ROOT_DIR}/musl/shared"
BUILD="${ROOT_DIR}/musl/build-shared"
CC_BIN="${CC:-gcc}"
JOBS="${JOBS:-$(nproc 2>/dev/null || echo 2)}"

if [[ ! -x "${MUSL_SRC}/configure" ]]; then
  echo "musl source not found at ${MUSL_SRC} (run 'make -C musl fetch')" >&2
  exit 1
fi

rm -rf "${BUILD}"
mkdir -p "${BUILD}"
(
  cd "${BUILD}"
  "${MUSL_SRC}/configure" --prefix="${OUT}" --target=x86_64-linux-musl \
    --enable-shared --disable-static CC="${CC_BIN}" > configure.log 2>&1 \
    || { tail -30 configure.log >&2; exit 1; }
  make -j"${JOBS}" lib/libc.so > make.log 2>&1 || { tail -30 make.log >&2; exit 1; }
)
mkdir -p "${OUT}/lib"
cp "${BUILD}/lib/libc.so" "${OUT}/lib/libc.so"
echo "[musl-shared] built ${OUT}/lib/libc.so ($(stat -c %s "${OUT}/lib/libc.so") bytes)"
