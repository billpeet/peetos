#!/usr/bin/env bash
set -euo pipefail

# Build a dynamically linked musl user program: a PIE whose PT_INTERP is
# musl's loader (/lib/ld-musl-x86_64.so.1), linked against musl's libc.so
# (scripts/build_musl_shared.sh) and any shared libraries named after `--`.
# The static counterpart is scripts/build_musl_echo.sh.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SRC=""
OUT=""
EXTRA_ARGS=()
MUSL_CC="${MUSL_CC:-${ROOT_DIR}/musl/sysroot/bin/musl-gcc}"
SHARED_LIB_DIR="${ROOT_DIR}/musl/shared/lib"

usage() {
  cat <<'USAGE'
Usage: build_musl_dyn.sh --src <c-file> --out <elf-file> [-- <extra cc args>]

Build a dynamically linked (PIE) musl user binary. Arguments after `--` are
passed to the compiler after the source file (include paths, -L/-l flags,
static archives).
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --src) SRC="$2"; shift 2 ;;
    --out) OUT="$2"; shift 2 ;;
    --) shift; EXTRA_ARGS=("$@"); break ;;
    --help|-h) usage; exit 0 ;;
    *) echo "Unknown option: $1" >&2; usage >&2; exit 1 ;;
  esac
done

if [[ -z "${SRC}" || -z "${OUT}" || ! -f "${SRC}" ]]; then
  usage >&2
  exit 1
fi
if [[ ! -x "${MUSL_CC}" ]]; then
  echo "musl compiler wrapper not found: ${MUSL_CC} (run 'make musl')" >&2
  exit 1
fi
if [[ ! -f "${SHARED_LIB_DIR}/libc.so" ]]; then
  echo "shared musl libc not found: ${SHARED_LIB_DIR}/libc.so (run 'make -C musl shared')" >&2
  exit 1
fi

# -L musl/shared/lib first so -lc resolves to libc.so rather than the
# sysroot's libc.a.
"${MUSL_CC}" \
  -Os \
  -s \
  -fPIE \
  -pie \
  -fno-stack-protector \
  -fno-omit-frame-pointer \
  -Wl,-z,max-page-size=0x1000 \
  -Wl,--as-needed \
  -L"${SHARED_LIB_DIR}" \
  -o "${OUT}" \
  "${SRC}" \
  "${EXTRA_ARGS[@]}"

if command -v readelf >/dev/null 2>&1; then
  interp="$(readelf -l "${OUT}" | sed -n 's/.*Requesting program interpreter: \(.*\)]/\1/p')"
  if [[ "${interp}" != "/lib/ld-musl-x86_64.so.1" ]]; then
    echo "${OUT}: unexpected interpreter '${interp}'" >&2
    exit 1
  fi
fi

chmod 0755 "${OUT}"
echo "Built ${OUT} (dynamic)"
