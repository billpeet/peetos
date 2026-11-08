#!/usr/bin/env bash
set -euo pipefail

# Build libwayland-server/libwayland-client (+ libffi) for PeetOS userspace
# with the musl toolchain, both as static archives and as shared libraries,
# plus a host wayland-scanner. The shared libraries link against musl's
# libc.so (scripts/build_musl_shared.sh) and are installed under their sonames
# (libwayland-server.so.0, ...) with a plain .so symlink for the linker.
#
# No meson/ninja/autotools needed: libwayland's core is a handful of C files
# compiled directly here, and libffi's release tarball ships a generated
# configure script.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WAYLAND_SRC="${ROOT_DIR}/wayland/wayland-src"
LIBFFI_SRC="${ROOT_DIR}/wayland/libffi-src"
PROTOCOLS_SRC="${ROOT_DIR}/wayland/wayland-protocols-src"
# wayland-protocols XML files (relative to PROTOCOLS_SRC) whose interface code
# is compiled into libwlprotocols.a, with client/server headers installed.
PROTOCOL_XMLS=(
  stable/xdg-shell/xdg-shell.xml
  unstable/xdg-decoration/xdg-decoration-unstable-v1.xml
)
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
  --protocols-src <dir> wayland-protocols tree (default: wayland/wayland-protocols-src)
  --out <dir>          install prefix (default: wayland/out)
  --musl-cc <path>     musl compiler wrapper (default: musl/sysroot/bin/musl-gcc)
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --wayland-src) WAYLAND_SRC="$2"; shift 2 ;;
    --libffi-src) LIBFFI_SRC="$2"; shift 2 ;;
    --protocols-src) PROTOCOLS_SRC="$2"; shift 2 ;;
    --out) OUT="$2"; shift 2 ;;
    --musl-cc) MUSL_CC="$2"; shift 2 ;;
    --help|-h) usage; exit 0 ;;
    *) echo "Unknown option: $1" >&2; usage >&2; exit 1 ;;
  esac
done

for dir in "${WAYLAND_SRC}" "${LIBFFI_SRC}" "${PROTOCOLS_SRC}"; do
  if [[ ! -d "${dir}" ]]; then
    echo "source tree not found: ${dir} (run 'make -C wayland fetch')" >&2
    exit 1
  fi
done
if [[ ! -x "${MUSL_CC}" ]]; then
  echo "musl compiler wrapper not found: ${MUSL_CC} (run 'make musl')" >&2
  exit 1
fi
MUSL_SHARED_LIB="${ROOT_DIR}/musl/shared/lib"
if [[ ! -f "${MUSL_SHARED_LIB}/libc.so" ]]; then
  echo "shared musl libc not found: ${MUSL_SHARED_LIB}/libc.so (run 'make -C musl shared')" >&2
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

# Shared libraries: link PIC objects against musl's libc.so. Installed under
# the soname, plus an unversioned symlink so -lfoo finds them at link time.
SHLIB_LDFLAGS=(-shared -L"${MUSL_SHARED_LIB}" -L"${OUT}/lib" -Wl,-z,max-page-size=0x1000 -Wl,--no-undefined)
link_shared() {
  local soname="$1"; shift
  local base="${soname%%.so*}.so"
  rm -f "${OUT}/lib/${soname}" "${OUT}/lib/${base}"
  "${MUSL_CC}" "${SHLIB_LDFLAGS[@]}" -Wl,-soname,"${soname}" -o "${OUT}/lib/${soname}" "$@"
  ln -s "${soname}" "${OUT}/lib/${base}"
}

# libffi's objects are already PIC (CFLAGS above); its libtool shared build is
# skipped in favour of linking the archive whole.
link_shared libffi.so.8 -Wl,--whole-archive "${OUT}/lib/libffi.a" -Wl,--no-whole-archive

# --- libwayland (static, musl) -----------------------------------------------
echo "[wayland] building libwayland-server / libwayland-client"
# -fvisibility=hidden as upstream's meson build: only WL_EXPORT symbols are
# exported from the shared libraries. -fPIC so the same objects serve both.
CFLAGS=(-O2 -std=gnu99 -fno-strict-aliasing -fPIC -fvisibility=hidden
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
link_shared libwayland-server.so.0 "${COMMON[@]}" \
  "${O}/wayland-server.o" "${O}/wayland-shm.o" "${O}/event-loop.o" -lffi
link_shared libwayland-client.so.0 "${COMMON[@]}" "${O}/wayland-client.o" -lffi

# libwayland-cursor: xcursor theme loading with built-in fallback cursors
# (cursor-data.h), so clients get a pointer image even with no theme installed.
CSRC="${WAYLAND_SRC}/cursor"
for f in wayland-cursor os-compatibility xcursor; do
  "${MUSL_CC}" "${CFLAGS[@]}" -I"${CSRC}" -DICONDIR='"/usr/share/icons"' -c "${CSRC}/${f}.c" -o "${O}/cursor-${f}.o"
done
CURSOR_OBJS=("${O}/cursor-wayland-cursor.o" "${O}/cursor-os-compatibility.o" "${O}/cursor-xcursor.o")
rm -f "${OUT}/lib/libwayland-cursor.a"
ar rcs "${OUT}/lib/libwayland-cursor.a" "${CURSOR_OBJS[@]}"
link_shared libwayland-cursor.so.0 "${CURSOR_OBJS[@]}" -lwayland-client

# --- wayland-protocols ---------------------------------------------------------
echo "[wayland] generating wayland-protocols code"
PROTO_OBJS=()
mkdir -p "${OUT}/share/wayland-protocols"
for rel in "${PROTOCOL_XMLS[@]}"; do
  xml="${PROTOCOLS_SRC}/${rel}"
  name="$(basename "${rel}" .xml)"
  "${SCANNER}" -s client-header "${xml}" "${OUT}/include/${name}-client-protocol.h"
  "${SCANNER}" -s server-header "${xml}" "${OUT}/include/${name}-server-protocol.h"
  "${SCANNER}" -s private-code "${xml}" "${BUILD}/gen/${name}-protocol.c"
  compile "${BUILD}/gen/${name}-protocol.c" "${O}/${name}-protocol.o"
  PROTO_OBJS+=("${O}/${name}-protocol.o")
  cp "${xml}" "${OUT}/share/wayland-protocols/"
done
# Protocol glue stays static-only: like upstream wayland-protocols, each
# program compiles in the interfaces it uses (private-code, hidden symbols).
rm -f "${OUT}/lib/libwlprotocols.a"
ar rcs "${OUT}/lib/libwlprotocols.a" "${PROTO_OBJS[@]}"

# --- headers -----------------------------------------------------------------
cp "${WAYLAND_SRC}/cursor/wayland-cursor.h" "${OUT}/include/"
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
