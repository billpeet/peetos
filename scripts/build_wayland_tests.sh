#!/usr/bin/env bash
set -euo pipefail

# Build a subset of upstream libwayland's own test suite (wayland-src/tests)
# as static musl binaries for PeetOS.
#
# Each upstream test file becomes one binary linked against the shared
# test-runner (test-runner.c, test-helpers.c, test-compositor.c), exactly as
# libwayland's meson build does. The fetched sources are never edited:
# PeetOS-specific build configuration lives in wayland/peetos/tests-config.h,
# which is force-included ahead of upstream's config.h.
#
# Output (default wayland/out/tests/):
#   <name>-test           one binary per selected test
#   exec-fd-leak-checker  helper exec'd by tests via $TEST_BUILD_DIR
#   TESTS                 newline-separated list of selected test names

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WAYLAND_SRC="${ROOT_DIR}/wayland/wayland-src"
WAYLAND_OUT="${ROOT_DIR}/wayland/out"
OUT=""
MUSL_CC="${MUSL_CC:-${ROOT_DIR}/musl/sysroot/bin/musl-gcc}"

# Tests run inside PeetOS. Keep in sync with docs/wayland.md ("Upstream test
# suite") which also lists the skipped tests and why.
TESTS=(
  array-test
  client-test
  compositor-introspection-test
  connection-test
  display-test
  enum-validator-test
  event-loop-test
  fixed-test
  headers-test
  interface-test
  list-test
  map-test
  message-test
  newsignal-test
  protocol-logger-test
  proxy-test
  queue-test
  resources-test
  sanity-test
  signal-test
  socket-test
)

usage() {
  cat <<'USAGE'
Usage: build_wayland_tests.sh [options]

Options:
  --wayland-src <dir>  libwayland source tree (default: wayland/wayland-src)
  --wayland-out <dir>  libwayland install dir from build_wayland.sh (default: wayland/out)
  --out <dir>          output directory (default: <wayland-out>/tests)
  --musl-cc <path>     musl compiler wrapper (default: musl/sysroot/bin/musl-gcc)
  --list               print the selected test names and exit
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --wayland-src) WAYLAND_SRC="$2"; shift 2 ;;
    --wayland-out) WAYLAND_OUT="$2"; shift 2 ;;
    --out) OUT="$2"; shift 2 ;;
    --musl-cc) MUSL_CC="$2"; shift 2 ;;
    --list) printf '%s\n' "${TESTS[@]}"; exit 0 ;;
    --help|-h) usage; exit 0 ;;
    *) echo "Unknown option: $1" >&2; usage >&2; exit 1 ;;
  esac
done
OUT="${OUT:-${WAYLAND_OUT}/tests}"

TSRC="${WAYLAND_SRC}/tests"
WSRC="${WAYLAND_SRC}/src"
GEN="${WAYLAND_OUT}/build/gen"
SCANNER="${WAYLAND_OUT}/bin/wayland-scanner"
for f in "${TSRC}/test-runner.c" "${SCANNER}" "${GEN}/wayland-version.h" \
         "${WAYLAND_OUT}/lib/libwayland-server.a"; do
  if [[ ! -e "${f}" ]]; then
    echo "missing ${f} (run 'make -C wayland' first)" >&2
    exit 1
  fi
done
if [[ ! -x "${MUSL_CC}" ]]; then
  echo "musl compiler wrapper not found: ${MUSL_CC}" >&2
  exit 1
fi

BUILD="${OUT}/build"
rm -rf "${OUT}"
mkdir -p "${BUILD}/gen" "${BUILD}/obj"

# --- generated test protocol ------------------------------------------------
XML="${WAYLAND_SRC}/protocol/tests.xml"
"${SCANNER}" -s server-header "${XML}" "${BUILD}/gen/tests-server-protocol.h"
"${SCANNER}" -s client-header "${XML}" "${BUILD}/gen/tests-client-protocol.h"
"${SCANNER}" -s public-code "${XML}" "${BUILD}/gen/tests-protocol.c"

CFLAGS=(-O2 -g0 -std=gnu99 -fno-strict-aliasing -UNDEBUG
  -include "${ROOT_DIR}/wayland/peetos/tests-config.h"
  -I"${BUILD}/gen" -I"${GEN}" -I"${WAYLAND_SRC}" -I"${WSRC}" -I"${TSRC}"
  -I"${WAYLAND_OUT}/include")
LIBS=("${WAYLAND_OUT}/lib/libwayland-server.a"
  "${WAYLAND_OUT}/lib/libwayland-client.a"
  "${WAYLAND_OUT}/lib/libffi.a" -lm)

O="${BUILD}/obj"
cc_obj() {
  "${MUSL_CC}" "${CFLAGS[@]}" -c "$1" -o "$2"
}

echo "[wayland-tests] building test runner"
cc_obj "${TSRC}/test-runner.c" "${O}/test-runner.o"
cc_obj "${TSRC}/test-helpers.c" "${O}/test-helpers.o"
cc_obj "${TSRC}/test-compositor.c" "${O}/test-compositor.o"
cc_obj "${BUILD}/gen/tests-protocol.c" "${O}/tests-protocol.o"
# An archive, as in meson: exec-fd-leak-checker has its own main() and must
# not pull in test-runner.o.
ar rcs "${O}/libtest-runner.a" "${O}/test-runner.o" "${O}/test-helpers.o" "${O}/test-compositor.o"
RUNNER=("${O}/libtest-runner.a")

"${MUSL_CC}" -static -s "${CFLAGS[@]}" "${TSRC}/exec-fd-leak-checker.c" \
  "${RUNNER[@]}" "${LIBS[@]}" -o "${OUT}/exec-fd-leak-checker"

for t in "${TESTS[@]}"; do
  echo "[wayland-tests] building ${t}"
  srcs=("${TSRC}/${t}.c")
  case "${t}" in
    display-test) srcs+=("${O}/tests-protocol.o") ;;
    # Needs wl_priv_* internals, so meson compiles wayland-server.c into it.
    # The object wins over the archive member at static link time.
    newsignal-test) srcs+=("${WSRC}/wayland-server.c") ;;
    headers-test) srcs+=("${TSRC}/headers-protocol-test.c" "${TSRC}/headers-protocol-core-test.c") ;;
  esac
  "${MUSL_CC}" -static -s "${CFLAGS[@]}" "${srcs[@]}" "${RUNNER[@]}" "${LIBS[@]}" \
    -o "${OUT}/${t}"
done

printf '%s\n' "${TESTS[@]}" > "${OUT}/TESTS"
rm -rf "${BUILD}"
echo "[wayland-tests] built ${#TESTS[@]} tests into ${OUT}"
