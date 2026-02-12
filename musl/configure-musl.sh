#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: configure-musl.sh [options]

Configure musl for PeetOS-oriented cross-compilation.

Options:
  --musl-dir <dir>       musl source directory (default: ./musl-src)
  --sysroot <dir>        install prefix/sysroot (default: ./sysroot)
  --target <triple>      musl target triple (default: x86_64-linux-musl)
  --cross-prefix <pref>  tool prefix, e.g. x86_64-peetos-
  --cc <path>            target C compiler
  --ar <path>            target archiver
  --ranlib <path>        target ranlib
  --cflags <flags>       extra CFLAGS
  --enable-shared        build shared libc in addition to static
  --dry-run              print configure command only
  --help                 show this help
USAGE
}

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MUSL_DIR="${SCRIPT_DIR}/musl-src"
SYSROOT="${SCRIPT_DIR}/sysroot"
TARGET="x86_64-linux-musl"
CROSS_PREFIX=""
CC_BIN=""
AR_BIN=""
RANLIB_BIN=""
CFLAGS_VALUE="-O2 -g -fno-stack-protector -fno-omit-frame-pointer"
DISABLE_SHARED=1
DRY_RUN=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --musl-dir)
      MUSL_DIR="$2"
      shift 2
      ;;
    --sysroot)
      SYSROOT="$2"
      shift 2
      ;;
    --target)
      TARGET="$2"
      shift 2
      ;;
    --cross-prefix)
      CROSS_PREFIX="$2"
      shift 2
      ;;
    --cc)
      CC_BIN="$2"
      shift 2
      ;;
    --ar)
      AR_BIN="$2"
      shift 2
      ;;
    --ranlib)
      RANLIB_BIN="$2"
      shift 2
      ;;
    --cflags)
      CFLAGS_VALUE="$2"
      shift 2
      ;;
    --enable-shared)
      DISABLE_SHARED=0
      shift
      ;;
    --dry-run)
      DRY_RUN=1
      shift
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

if [[ ! -d "${MUSL_DIR}" ]]; then
  echo "musl source directory not found: ${MUSL_DIR}" >&2
  echo "Run musl/fetch-musl.sh first" >&2
  exit 1
fi

MUSL_DIR="$(cd "${MUSL_DIR}" && pwd)"
mkdir -p "${SYSROOT}"
SYSROOT="$(cd "${SYSROOT}" && pwd)"

if [[ ! -x "${MUSL_DIR}/configure" ]]; then
  echo "musl configure script not found at ${MUSL_DIR}/configure" >&2
  echo "Run musl/fetch-musl.sh first" >&2
  exit 1
fi

if [[ -n "${CROSS_PREFIX}" ]]; then
  if [[ -z "${CC_BIN}" ]]; then
    CC_BIN="${CROSS_PREFIX}gcc"
  fi
  if [[ -z "${AR_BIN}" ]]; then
    AR_BIN="${CROSS_PREFIX}ar"
  fi
  if [[ -z "${RANLIB_BIN}" ]]; then
    RANLIB_BIN="${CROSS_PREFIX}ranlib"
  fi
fi

CC_BIN="${CC_BIN:-${CC:-gcc}}"
AR_BIN="${AR_BIN:-${AR:-ar}}"
RANLIB_BIN="${RANLIB_BIN:-${RANLIB:-ranlib}}"

CONFIGURE_CMD=(
  ./configure
  "--prefix=${SYSROOT}"
  "--target=${TARGET}"
  "CC=${CC_BIN}"
  "AR=${AR_BIN}"
  "RANLIB=${RANLIB_BIN}"
  "CFLAGS=${CFLAGS_VALUE}"
)

if [[ "${DISABLE_SHARED}" -eq 1 ]]; then
  CONFIGURE_CMD+=(--disable-shared)
fi

echo "Configuring musl"
echo "  source:  ${MUSL_DIR}"
echo "  sysroot: ${SYSROOT}"
echo "  target:  ${TARGET}"
echo "  CC:      ${CC_BIN}"
echo "  AR:      ${AR_BIN}"
echo "  RANLIB:  ${RANLIB_BIN}"

if [[ "${DRY_RUN}" -eq 1 ]]; then
  printf '  command: %q' "${CONFIGURE_CMD[@]}"
  printf '\n'
  exit 0
fi

(
  cd "${MUSL_DIR}"
  "${CONFIGURE_CMD[@]}"
)

echo "musl configure complete"
echo "Next: make -C ${MUSL_DIR} && make -C ${MUSL_DIR} install"
