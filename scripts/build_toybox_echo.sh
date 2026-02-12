#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="${ROOT_DIR}/user/toybox_echo.elf"
TOYBOX_DIR="${ROOT_DIR}/toybox"
DEFAULT_TOYBOX_SRC="${TOYBOX_DIR}/toybox-src"
TOYBOX_SRC="${DEFAULT_TOYBOX_SRC}"
MUSL_CC="${MUSL_CC:-${ROOT_DIR}/musl/sysroot/bin/musl-gcc}"

usage() {
  cat <<'USAGE'
Usage: build_toybox_echo.sh [--out <elf-file>] [--toybox-src <dir>] [--musl-cc <path>]

Build a minimal static Toybox binary configured for the `echo` applet.
Defaults:
  --out user/toybox_echo.elf
  --toybox-src toybox/toybox-src
  --musl-cc musl/sysroot/bin/musl-gcc
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --out)
      OUT="$2"
      shift 2
      ;;
    --toybox-src)
      TOYBOX_SRC="$2"
      shift 2
      ;;
    --musl-cc)
      MUSL_CC="$2"
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

if [[ ! -x "${MUSL_CC}" ]]; then
  echo "musl compiler wrapper not found: ${MUSL_CC}" >&2
  echo "Run 'make musl' first so musl/sysroot/bin/musl-gcc is installed." >&2
  exit 1
fi

if [[ ! -d "${TOYBOX_SRC}" ]]; then
  if [[ "${TOYBOX_SRC}" != "${DEFAULT_TOYBOX_SRC}" ]]; then
    echo "Toybox source not found: ${TOYBOX_SRC}" >&2
    exit 1
  fi
  echo "Toybox source not found at ${TOYBOX_SRC}; fetching pinned source"
  make -C "${TOYBOX_DIR}" fetch
fi

make -C "${TOYBOX_DIR}" echo MUSL_CC="${MUSL_CC}"

mkdir -p "$(dirname "${OUT}")"
cp "${TOYBOX_DIR}/out/echo" "${OUT}"
chmod 0755 "${OUT}"

if command -v readelf >/dev/null 2>&1; then
  if ! readelf -h "${OUT}" | rg -q "Type:\s+EXEC"; then
    echo "Expected an ET_EXEC ELF output for Toybox echo" >&2
    exit 1
  fi
fi

echo "Built ${OUT}"
