#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DROPBEAR_SRC="${ROOT_DIR}/dropbear/dropbear-src"
OUT_DIR="${ROOT_DIR}/dropbear/out"
MUSL_CC="${MUSL_CC:-${ROOT_DIR}/musl/sysroot/bin/musl-gcc}"
JOBS="$(nproc)"
PROGRAMS="dropbear dropbearkey dbclient scp"
LOCALOPTIONS_TEMPLATE="${ROOT_DIR}/dropbear/peetos/localoptions.h"

usage() {
  cat <<'USAGE'
Usage: build_dropbear_multi.sh [options]

Build static Dropbear multi-binary artifacts for ext2 staging.

Options:
  --dropbear-src <dir>   Dropbear source directory (default: dropbear/dropbear-src)
  --out-dir <dir>        output directory (default: dropbear/out)
  --musl-cc <path>       musl compiler wrapper (default: musl/sysroot/bin/musl-gcc)
  --jobs <n>             parallel build jobs (default: nproc)
  --programs <list>      program list for MULTI build (default: "dropbear dropbearkey dbclient scp")
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --dropbear-src)
      DROPBEAR_SRC="$2"
      shift 2
      ;;
    --out-dir)
      OUT_DIR="$2"
      shift 2
      ;;
    --musl-cc)
      MUSL_CC="$2"
      shift 2
      ;;
    --jobs)
      JOBS="$2"
      shift 2
      ;;
    --programs)
      PROGRAMS="$2"
      shift 2
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      echo "Unknown option: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

if [[ ! -d "${DROPBEAR_SRC}" ]]; then
  echo "Dropbear source not found: ${DROPBEAR_SRC}" >&2
  echo "Run 'make dropbear-fetch' first." >&2
  exit 1
fi

if [[ ! -x "${MUSL_CC}" ]]; then
  echo "musl compiler wrapper not found: ${MUSL_CC}" >&2
  echo "Run 'make musl' first so musl/sysroot/bin/musl-gcc is installed." >&2
  exit 1
fi

if [[ ! -x "${DROPBEAR_SRC}/configure" ]]; then
  echo "Dropbear source tree appears incomplete: missing configure script" >&2
  exit 1
fi

if [[ ! -f "${LOCALOPTIONS_TEMPLATE}" ]]; then
  echo "Dropbear PeetOS localoptions missing: ${LOCALOPTIONS_TEMPLATE}" >&2
  exit 1
fi

CPPFLAGS="-I${ROOT_DIR}/musl/sysroot/include"
CFLAGS="-Os -static -fno-stack-protector -fno-pic -no-pie -ffunction-sections -fdata-sections"
LDFLAGS="-static -no-pie -Wl,--gc-sections"
LIBS="-lc -lgcc"
CONFIGURE_FLAGS=(
  --host=x86_64-linux-musl
  --enable-static
  --enable-bundled-libtom
  --disable-harden
  --disable-zlib
  --disable-syslog
  --disable-shadow
  --disable-lastlog
  --disable-utmp
  --disable-utmpx
  --disable-wtmp
  --disable-wtmpx
  --disable-loginfunc
  --disable-pututline
  --disable-pututxline
)

cp "${LOCALOPTIONS_TEMPLATE}" "${DROPBEAR_SRC}/localoptions.h"

if [[ -f "${DROPBEAR_SRC}/Makefile" ]]; then
  make -C "${DROPBEAR_SRC}" distclean >/dev/null 2>&1 || true
fi

(
  cd "${DROPBEAR_SRC}"
  CC="${MUSL_CC}" \
  AR="ar" \
  RANLIB="ranlib" \
  CPPFLAGS="${CPPFLAGS}" \
  CFLAGS="${CFLAGS}" \
  LDFLAGS="${LDFLAGS}" \
  LIBS="${LIBS}" \
  LTM_CFLAGS="${CPPFLAGS} ${CFLAGS}" \
  ac_cv_func_crypt=no \
  ac_cv_lib_crypt_crypt=no \
  ./configure "${CONFIGURE_FLAGS[@]}"
)

make -C "${DROPBEAR_SRC}" -j "${JOBS}" PROGRAMS="${PROGRAMS}" MULTI=1 SCPPROGRESS=1

mkdir -p "${OUT_DIR}"
cp "${DROPBEAR_SRC}/dropbearmulti" "${OUT_DIR}/dropbearmulti"
chmod 0755 "${OUT_DIR}/dropbearmulti"

for prog in ${PROGRAMS}; do
  cp "${DROPBEAR_SRC}/dropbearmulti" "${OUT_DIR}/${prog}"
  chmod 0755 "${OUT_DIR}/${prog}"
done

echo "Built Dropbear binaries under ${OUT_DIR}"
