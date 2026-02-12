#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SRC="${ROOT_DIR}/user/musl_echo.c"
OUT="${ROOT_DIR}/user/musl_echo.elf"
MUSL_CC="${MUSL_CC:-${ROOT_DIR}/musl/sysroot/bin/musl-gcc}"

usage() {
  cat <<'USAGE'
Usage: build_musl_echo.sh [--src <c-file>] [--out <elf-file>]

Build a small static musl user binary.
Defaults:
  --src user/musl_echo.c
  --out user/musl_echo.elf
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --src)
      SRC="$2"
      shift 2
      ;;
    --out)
      OUT="$2"
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

if [[ ! -f "${SRC}" ]]; then
  echo "musl source not found: ${SRC}" >&2
  exit 1
fi

if [[ ! -x "${MUSL_CC}" ]]; then
  echo "musl compiler wrapper not found: ${MUSL_CC}" >&2
  echo "Run 'make musl' first so musl/sysroot/bin/musl-gcc is installed." >&2
  exit 1
fi

"${MUSL_CC}" \
  -static \
  -Os \
  -s \
  -ffunction-sections \
  -fdata-sections \
  -fno-unwind-tables \
  -fno-asynchronous-unwind-tables \
  -fno-stack-protector \
  -fno-omit-frame-pointer \
  -Wl,--gc-sections \
  -no-pie \
  -Wl,-z,max-page-size=0x1000 \
  -o "${OUT}" \
  "${SRC}"

if command -v readelf >/dev/null 2>&1; then
  if ! readelf -h "${OUT}" | rg -q "Type:\s+EXEC"; then
    echo "Expected an ET_EXEC ELF output for musl smoke binary" >&2
    exit 1
  fi
fi

chmod 0755 "${OUT}"
echo "Built ${OUT}"
