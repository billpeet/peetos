#!/usr/bin/env bash
set -euo pipefail

# Build libdrm's core library (xf86drm / xf86drmMode) for PeetOS userspace:
# static libdrm.a and shared libdrm.so.2 against musl, plus headers, into
# wayland/out. No meson needed: the core is five C files and one generated
# table; the configuration lives in wayland/peetos/libdrm-config.h.
#
# The musl sysroot has no Linux uapi headers, so tiny <linux/types.h> and
# <asm/ioctl.h> shims (wayland/peetos/uapi) are installed alongside drm.h.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SRC="${ROOT_DIR}/wayland/libdrm-src"
OUT="${ROOT_DIR}/wayland/out"
MUSL_CC="${MUSL_CC:-${ROOT_DIR}/musl/sysroot/bin/musl-gcc}"
MUSL_SHARED_LIB="${ROOT_DIR}/musl/shared/lib"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --libdrm-src) SRC="$2"; shift 2 ;;
    --out) OUT="$2"; shift 2 ;;
    --musl-cc) MUSL_CC="$2"; shift 2 ;;
    *) echo "Unknown option: $1" >&2; exit 1 ;;
  esac
done

if [[ ! -d "${SRC}" ]]; then
  echo "libdrm source not found: ${SRC} (run 'make -C wayland fetch')" >&2
  exit 1
fi
if [[ ! -f "${MUSL_SHARED_LIB}/libc.so" ]]; then
  echo "shared musl libc not found (run 'make -C musl shared')" >&2
  exit 1
fi

BUILD="${OUT}/build/libdrm"
rm -rf "${BUILD}"
mkdir -p "${BUILD}/gen" "${BUILD}/obj" "${OUT}/lib" "${OUT}/include/libdrm" \
  "${OUT}/include/linux" "${OUT}/include/asm"

# Linux uapi headers musl lacks: small shims plus vendored input-event-codes.h.
cp -r "${ROOT_DIR}/wayland/peetos/uapi/." "${OUT}/include/"
cp "${ROOT_DIR}/wayland/peetos/libdrm-config.h" "${BUILD}/gen/config.h"
python3 "${SRC}/gen_table_fourcc.py" "${SRC}/include/drm/drm_fourcc.h" \
  "${BUILD}/gen/generated_static_table_fourcc.h"

CFLAGS=(-O2 -std=gnu99 -fPIC -fvisibility=hidden -include "${BUILD}/gen/config.h"
  -I"${BUILD}/gen" -I"${SRC}" -I"${SRC}/include/drm" -I"${OUT}/include")
OBJS=()
for f in xf86drm xf86drmHash xf86drmRandom xf86drmSL xf86drmMode; do
  "${MUSL_CC}" "${CFLAGS[@]}" -c "${SRC}/${f}.c" -o "${BUILD}/obj/${f}.o"
  OBJS+=("${BUILD}/obj/${f}.o")
done

rm -f "${OUT}/lib/libdrm.a" "${OUT}/lib/libdrm.so.2" "${OUT}/lib/libdrm.so"
ar rcs "${OUT}/lib/libdrm.a" "${OBJS[@]}"
"${MUSL_CC}" -shared -L"${MUSL_SHARED_LIB}" -Wl,-z,max-page-size=0x1000 -Wl,--no-undefined \
  -Wl,-soname,libdrm.so.2 -o "${OUT}/lib/libdrm.so.2" "${OBJS[@]}"
ln -s libdrm.so.2 "${OUT}/lib/libdrm.so"

cp "${SRC}/xf86drm.h" "${SRC}/xf86drmMode.h" "${OUT}/include/"
cp "${SRC}"/include/drm/drm.h "${SRC}"/include/drm/drm_mode.h "${SRC}"/include/drm/drm_fourcc.h \
  "${OUT}/include/libdrm/"

echo "[libdrm] installed libdrm into ${OUT}"
