#!/usr/bin/env bash
set -euo pipefail

# Build static libwayland-server/libwayland-client (+ libffi) for PeetOS
# userspace with the musl toolchain, plus a host wayland-scanner.
#
# No meson/ninja/autotools needed: libwayland's core is a handful of C files
# compiled directly here, and libffi's release tarball ships a generated
# configure script.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WAYLAND_SRC="${ROOT_DIR}/wayland/wayland-src"
LIBFFI_SRC="${ROOT_DIR}/wayland/libffi-src"
OUT="${ROOT_DIR}/wayland/out"
MUSL_CC="${MUSL_CC:-${ROOT_DIR}/musl/sysroot/bin/musl-gcc}"
HOST_CC="${HOST_CC:-cc}"
JOBS="${JOBS:-$(nproc 2>/dev/null || echo 2)}"

usage() {
  cat <<'USAGE'
Usage: build_wayland.sh [options]

Options:
  --wayland-src <dir>  libwayland source tree (default: wayland/wayland-src)
  --libffi-src <dir>   libffi source tree (default: wayland/libffi-src)
  --out <dir>          install prefix (default: wayland/out)
  --musl-cc <path>     musl compiler wrapper (default: musl/sysroot/bin/musl-gcc)
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --wayland-src) WAYLAND_SRC="$2"; shift 2 ;;
    --libffi-src) LIBFFI_SRC="$2"; shift 2 ;;
    --out) OUT="$2"; shift 2 ;;
    --musl-cc) MUSL_CC="$2"; shift 2 ;;
    --help|-h) usage; exit 0 ;;
    *) echo "Unknown option: $1" >&2; usage >&2; exit 1 ;;
  esac
done

for dir in "${WAYLAND_SRC}" "${LIBFFI_SRC}"; do
  if [[ ! -d "${dir}" ]]; then
    echo "source tree not found: ${dir} (run 'make -C wayland fetch')" >&2
    exit 1
  fi
done
if [[ ! -x "${MUSL_CC}" ]]; then
  echo "musl compiler wrapper not found: ${MUSL_CC} (run 'make musl')" >&2
  exit 1
fi
if ! echo '#include <expat.h>' | "${HOST_CC}" -E -x c - >/dev/null 2>&1; then
  echo "host expat headers not found (install libexpat1-dev) - needed for wayland-scanner" >&2
  exit 1
fi

BUILD="${OUT}/build"
rm -rf "${BUILD}"
mkdir -p "${BUILD}/host" "${BUILD}/gen" "${BUILD}/obj" "${BUILD}/libffi" \
  "${OUT}/bin" "${OUT}/include" "${OUT}/lib" "${OUT}/share/wayland"

WSRC="${WAYLAND_SRC}/src"
VERSION="$(sed -n "s/^[[:space:]]*version: '\([0-9.]*\)'.*/\1/p" "${WAYLAND_SRC}/meson.build" | head -n 1)"
IFS=. read -r V_MAJOR V_MINOR V_MICRO <<<"${VERSION}"

# --- generated headers shared by scanner and libraries ---------------------
cp "${ROOT_DIR}/wayland/peetos/config.h" "${WAYLAND_SRC}/config.h"
sed -e "s/@WAYLAND_VERSION@/${VERSION}/" \
    -e "s/@WAYLAND_VERSION_MAJOR@/${V_MAJOR}/" \
    -e "s/@WAYLAND_VERSION_MINOR@/${V_MINOR}/" \
    -e "s/@WAYLAND_VERSION_MICRO@/${V_MICRO}/" \
    "${WSRC}/wayland-version.h.in" > "${BUILD}/gen/wayland-version.h"

# --- host wayland-scanner ----------------------------------------------------
echo "[wayland] building host wayland-scanner ${VERSION}"
python3 "${WSRC}/embed.py" "${WAYLAND_SRC}/protocol/wayland.dtd" wayland_dtd \
  > "${BUILD}/host/wayland.dtd.h"
"${HOST_CC}" -O2 -std=gnu99 -include "${WAYLAND_SRC}/config.h" \
  -I"${BUILD}/host" -I"${BUILD}/gen" -I"${WSRC}" -I"${WAYLAND_SRC}/protocol" \
  "${WSRC}/scanner.c" "${WSRC}/wayland-util.c" \
  -lexpat -o "${OUT}/bin/wayland-scanner"
SCANNER="${OUT}/bin/wayland-scanner"

# --- protocol code -----------------------------------------------------------
XML="${WAYLAND_SRC}/protocol/wayland.xml"
"${SCANNER}" -s public-code "${XML}" "${BUILD}/gen/wayland-protocol.c"
"${SCANNER}" -s server-header "${XML}" "${BUILD}/gen/wayland-server-protocol.h"
"${SCANNER}" -s server-header -c "${XML}" "${BUILD}/gen/wayland-server-protocol-core.h"
"${SCANNER}" -s client-header "${XML}" "${BUILD}/gen/wayland-client-protocol.h"
"${SCANNER}" -s client-header -c "${XML}" "${BUILD}/gen/wayland-client-protocol-core.h"

# --- libffi (static, musl) ---------------------------------------------------
echo "[wayland] building libffi"
FFI_PREFIX="${BUILD}/libffi-install"
(
  cd "${BUILD}/libffi"
  CC="${MUSL_CC}" CFLAGS="-O2 -fPIC" "${LIBFFI_SRC}/configure" \
    --build="$("${HOST_CC}" -dumpmachine)" --host=x86_64-linux-musl \
    --prefix="${FFI_PREFIX}" \
    --disable-shared --enable-static \
    --disable-docs --disable-multi-os-directory \
    --disable-exec-static-tramp \
    > configure.log 2>&1 || { tail -40 configure.log >&2; exit 1; }
  make -j"${JOBS}" > make.log 2>&1 || { tail -40 make.log >&2; exit 1; }
  make install > install.log 2>&1
)
cp "${FFI_PREFIX}/include/ffi.h" "${FFI_PREFIX}/include/ffitarget.h" "${OUT}/include/"
cp "${FFI_PREFIX}/lib/libffi.a" "${OUT}/lib/"

# --- libwayland (static, musl) -----------------------------------------------
echo "[wayland] building libwayland-server / libwayland-client"
CFLAGS=(-O2 -std=gnu99 -fno-strict-aliasing
  -I"${WAYLAND_SRC}" -I"${BUILD}/gen" -I"${WSRC}" -I"${OUT}/include")

compile() {
  local src="$1" obj="$2"
  "${MUSL_CC}" "${CFLAGS[@]}" -c "${src}" -o "${obj}"
}

O="${BUILD}/obj"
compile "${WSRC}/wayland-util.c" "${O}/wayland-util.o"
compile "${WSRC}/wayland-os.c" "${O}/wayland-os.o"
compile "${WSRC}/connection.c" "${O}/connection.o"
compile "${BUILD}/gen/wayland-protocol.c" "${O}/wayland-protocol.o"
compile "${WSRC}/wayland-server.c" "${O}/wayland-server.o"
compile "${WSRC}/wayland-shm.c" "${O}/wayland-shm.o"
compile "${WSRC}/event-loop.c" "${O}/event-loop.o"
compile "${WSRC}/wayland-client.c" "${O}/wayland-client.o"

COMMON=("${O}/wayland-util.o" "${O}/wayland-os.o" "${O}/connection.o" "${O}/wayland-protocol.o")
rm -f "${OUT}/lib/libwayland-server.a" "${OUT}/lib/libwayland-client.a"
ar rcs "${OUT}/lib/libwayland-server.a" "${COMMON[@]}" \
  "${O}/wayland-server.o" "${O}/wayland-shm.o" "${O}/event-loop.o"
ar rcs "${OUT}/lib/libwayland-client.a" "${COMMON[@]}" "${O}/wayland-client.o"

# --- headers -----------------------------------------------------------------
cp "${WSRC}/wayland-util.h" \
   "${WSRC}/wayland-client.h" "${WSRC}/wayland-client-core.h" \
   "${WSRC}/wayland-server.h" "${WSRC}/wayland-server-core.h" \
   "${BUILD}/gen/wayland-version.h" \
   "${BUILD}/gen/wayland-client-protocol.h" \
   "${BUILD}/gen/wayland-server-protocol.h" \
   "${OUT}/include/"
cp "${XML}" "${OUT}/share/wayland/wayland.xml"

echo "[wayland] installed libwayland ${VERSION} into ${OUT}"
ls -1 "${OUT}/lib"
