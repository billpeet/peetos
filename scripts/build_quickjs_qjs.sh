#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
QUICKJS_SRC="${ROOT_DIR}/quickjs/quickjs-src"
OUT="${ROOT_DIR}/quickjs/out/qjs"
MUSL_CC="${MUSL_CC:-${ROOT_DIR}/musl/sysroot/bin/musl-gcc}"
QUICKJS_VERSION="2024-01-13"
CONFIG_VERSION="${QUICKJS_VERSION}-peetos"
MAKEFILE_TEMPLATE="${ROOT_DIR}/quickjs/peetos/Makefile.peetos"
REPL_STUB_TEMPLATE="${ROOT_DIR}/quickjs/peetos/repl_stub.c"

usage() {
  cat <<'USAGE'
Usage: build_quickjs_qjs.sh [options]

Build a static QuickJS qjs interpreter for ext2 staging.

Options:
  --quickjs-src <dir>      QuickJS source directory (default: quickjs/quickjs-src)
  --out <path>             output binary path (default: quickjs/out/qjs)
  --musl-cc <path>         musl compiler wrapper (default: musl/sysroot/bin/musl-gcc)
  --version <ver>          QuickJS version label (default: 2024-01-13)
  --config-version <ver>   CONFIG_VERSION passed to build (default: <version>-peetos)
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --quickjs-src)
      QUICKJS_SRC="$2"
      shift 2
      ;;
    --out)
      OUT="$2"
      shift 2
      ;;
    --musl-cc)
      MUSL_CC="$2"
      shift 2
      ;;
    --version)
      QUICKJS_VERSION="$2"
      CONFIG_VERSION="${QUICKJS_VERSION}-peetos"
      shift 2
      ;;
    --config-version)
      CONFIG_VERSION="$2"
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

if [[ ! -d "${QUICKJS_SRC}" ]]; then
  echo "QuickJS source not found: ${QUICKJS_SRC}" >&2
  echo "Run 'make quickjs-fetch' first." >&2
  exit 1
fi

if [[ ! -x "${MUSL_CC}" ]]; then
  echo "musl compiler wrapper not found: ${MUSL_CC}" >&2
  echo "Run 'make musl' first so musl/sysroot/bin/musl-gcc is installed." >&2
  exit 1
fi

if [[ ! -f "${QUICKJS_SRC}/quickjs-libc.c" || ! -f "${QUICKJS_SRC}/qjs.c" ]]; then
  echo "QuickJS source tree appears incomplete: ${QUICKJS_SRC}" >&2
  exit 1
fi

if [[ ! -f "${MAKEFILE_TEMPLATE}" || ! -f "${REPL_STUB_TEMPLATE}" ]]; then
  echo "QuickJS PeetOS templates missing under quickjs/peetos" >&2
  exit 1
fi

# Disable Worker API in quickjs-libc.c (depends on pthread/atomics runtime).
if ! rg -q 'disabled for PeetOS' "${QUICKJS_SRC}/quickjs-libc.c"; then
  perl -0pi -e 's@/\* enable the os\.Worker API\. IT relies on POSIX threads \*/\n#define USE_WORKER@/* enable the os.Worker API. IT relies on POSIX threads */\n#if 0 /* !defined(_WIN32) - disabled for PeetOS */\n#define USE_WORKER\n#endif@' \
    "${QUICKJS_SRC}/quickjs-libc.c"
fi

cp "${MAKEFILE_TEMPLATE}" "${QUICKJS_SRC}/Makefile.peetos"
cp "${REPL_STUB_TEMPLATE}" "${QUICKJS_SRC}/repl_stub.c"

make -C "${QUICKJS_SRC}" -f Makefile.peetos clean >/dev/null 2>&1 || true
make -C "${QUICKJS_SRC}" -f Makefile.peetos MUSL_CC="${MUSL_CC}" CONFIG_VERSION="${CONFIG_VERSION}"

mkdir -p "$(dirname "${OUT}")"
cp "${QUICKJS_SRC}/qjs" "${OUT}"
chmod 0755 "${OUT}"

if command -v readelf >/dev/null 2>&1; then
  if ! readelf -h "${OUT}" | rg -q 'Type:\s+EXEC'; then
    echo "Expected an ET_EXEC ELF output for qjs" >&2
    exit 1
  fi
fi

echo "Built ${OUT}"
