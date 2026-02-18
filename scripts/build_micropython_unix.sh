#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MICROPY_SRC="${ROOT_DIR}/micropython/micropython-src"
OUT="${ROOT_DIR}/micropython/out/micropython"
MUSL_CC="${MUSL_CC:-${ROOT_DIR}/musl/sysroot/bin/musl-gcc}"
VARIANT="minimal"
JOBS="$(nproc)"

usage() {
  cat <<'USAGE'
Usage: build_micropython_unix.sh [options]

Build a static MicroPython unix-port binary for ext2 staging.

Options:
  --micropython-src <dir>   MicroPython source directory (default: micropython/micropython-src)
  --out <path>              output binary path (default: micropython/out/micropython)
  --musl-cc <path>          musl compiler wrapper (default: musl/sysroot/bin/musl-gcc)
  --variant <name>          unix port variant (default: minimal)
  --jobs <n>                parallel jobs for make (default: nproc)
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --micropython-src)
      MICROPY_SRC="$2"
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
    --variant)
      VARIANT="$2"
      shift 2
      ;;
    --jobs)
      JOBS="$2"
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

if [[ ! -d "${MICROPY_SRC}" ]]; then
  echo "MicroPython source not found: ${MICROPY_SRC}" >&2
  echo "Run 'make micropython-fetch' first." >&2
  exit 1
fi

if [[ ! -x "${MUSL_CC}" ]]; then
  echo "musl compiler wrapper not found: ${MUSL_CC}" >&2
  echo "Run 'make musl' first so musl/sysroot/bin/musl-gcc is installed." >&2
  exit 1
fi

if [[ ! -f "${MICROPY_SRC}/ports/unix/Makefile" || ! -f "${MICROPY_SRC}/mpy-cross/Makefile" ]]; then
  echo "MicroPython source tree appears incomplete: ${MICROPY_SRC}" >&2
  exit 1
fi

make -C "${MICROPY_SRC}/mpy-cross" -j"${JOBS}"

make -C "${MICROPY_SRC}/ports/unix" clean >/dev/null 2>&1 || true
make -C "${MICROPY_SRC}/ports/unix" -j"${JOBS}" \
  VARIANT="${VARIANT}" \
  MICROPY_STANDALONE=1 \
  CC="${MUSL_CC}" \
  CFLAGS_EXTRA='-Os -fno-stack-protector -fno-omit-frame-pointer -ffunction-sections -fdata-sections' \
  LDFLAGS_EXTRA='-static -no-pie -Wl,-z,max-page-size=0x1000 -Wl,--gc-sections'

build_dir="build"
if [[ "${VARIANT}" != "standard" ]]; then
  build_dir="build-${VARIANT}"
fi

bin_path="${MICROPY_SRC}/ports/unix/${build_dir}/micropython"
if [[ ! -x "${bin_path}" ]]; then
  echo "Expected MicroPython binary not found: ${bin_path}" >&2
  exit 1
fi

mkdir -p "$(dirname "${OUT}")"
cp "${bin_path}" "${OUT}"
chmod 0755 "${OUT}"

if command -v readelf >/dev/null 2>&1; then
  if ! readelf -h "${OUT}" | rg -q 'Type:\s+EXEC'; then
    echo "Expected an ET_EXEC ELF output for micropython" >&2
    exit 1
  fi
fi

echo "Built ${OUT}"
