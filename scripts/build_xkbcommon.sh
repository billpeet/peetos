#!/usr/bin/env bash
set -euo pipefail

# Build static libxkbcommon for PeetOS userspace (musl), stage a verified
# xkeyboard-config subset, and install both into wayland/out.
#
# No meson/ninja: the library is a fixed list of C files plus a bison-generated
# parser. The keymap data subset comes from the host's installed
# xkeyboard-config (XKB_DATA_SRC, default /usr/share/X11/xkb) and is verified
# by compiling every staged layout with a host build of the same library.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
XKB_SRC="${ROOT_DIR}/wayland/libxkbcommon-src"
OUT="${ROOT_DIR}/wayland/out"
MUSL_CC="${MUSL_CC:-${ROOT_DIR}/musl/sysroot/bin/musl-gcc}"
HOST_CC="${HOST_CC:-cc}"
XKB_DATA_SRC="${XKB_DATA_SRC:-/usr/share/X11/xkb}"
# Layouts (layout[:variant]) and options whose data is staged and verified.
XKB_LAYOUTS="${XKB_LAYOUTS:-us,gb,de,de:nodeadkeys,fr,us:dvorak}"
XKB_OPTIONS="${XKB_OPTIONS:-ctrl:nocaps,caps:escape,compose:ralt}"

usage() {
  cat <<'USAGE'
Usage: build_xkbcommon.sh [options]

Options:
  --xkbcommon-src <dir>  libxkbcommon source tree (default: wayland/libxkbcommon-src)
  --out <dir>            install prefix (default: wayland/out)
  --musl-cc <path>       musl compiler wrapper (default: musl/sysroot/bin/musl-gcc)

Environment: XKB_DATA_SRC, XKB_LAYOUTS, XKB_OPTIONS (see script header).
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --xkbcommon-src) XKB_SRC="$2"; shift 2 ;;
    --out) OUT="$2"; shift 2 ;;
    --musl-cc) MUSL_CC="$2"; shift 2 ;;
    --help|-h) usage; exit 0 ;;
    *) echo "Unknown option: $1" >&2; usage >&2; exit 1 ;;
  esac
done

if [[ ! -d "${XKB_SRC}" ]]; then
  echo "libxkbcommon source not found: ${XKB_SRC} (run 'make -C wayland fetch')" >&2
  exit 1
fi
for tool in bison python3; do
  if ! command -v "${tool}" >/dev/null 2>&1; then
    echo "${tool} not found (needed to build libxkbcommon)" >&2
    exit 1
  fi
done
if [[ ! -f "${XKB_DATA_SRC}/rules/evdev" ]]; then
  echo "xkeyboard-config data not found at ${XKB_DATA_SRC} (install xkb-data, or set XKB_DATA_SRC)" >&2
  exit 1
fi

BUILD="${OUT}/build/xkbcommon"
rm -rf "${BUILD}"
mkdir -p "${BUILD}/gen" "${BUILD}/obj-musl" "${BUILD}/obj-host" "${OUT}/include/xkbcommon" "${OUT}/lib"

cp "${ROOT_DIR}/wayland/peetos/xkbcommon-config.h" "${BUILD}/gen/config.h"
bison --defines="${BUILD}/gen/parser.h" -o "${BUILD}/gen/parser.c" -p _xkbcommon_ \
  "${XKB_SRC}/src/xkbcomp/parser.y"

# libxkbcommon_sources from meson.build (C files only).
SOURCES=(
  src/compose/parser.c src/compose/paths.c src/compose/state.c src/compose/table.c
  src/xkbcomp/action.c src/xkbcomp/ast-build.c src/xkbcomp/compat.c src/xkbcomp/expr.c
  src/xkbcomp/include.c src/xkbcomp/keycodes.c src/xkbcomp/keymap.c src/xkbcomp/keymap-dump.c
  src/xkbcomp/keywords.c src/xkbcomp/rules.c src/xkbcomp/scanner.c src/xkbcomp/symbols.c
  src/xkbcomp/types.c src/xkbcomp/vmod.c src/xkbcomp/xkbcomp.c
  src/atom.c src/context.c src/context-priv.c src/keysym.c src/keysym-utf.c src/keymap.c
  src/keymap-priv.c src/state.c src/text.c src/utf8.c src/utils.c
)
# The generated parser.h must come from -I gen (compose/ has its own parser.h,
# found first via its directory).
CFLAGS=(-O2 -std=gnu11 -fno-strict-aliasing -fvisibility=hidden
  -I"${BUILD}/gen" -I"${XKB_SRC}/src" -I"${XKB_SRC}/include" -I"${XKB_SRC}")

build_lib() {
  local cc="$1" objdir="$2" lib="$3"
  local objs=()
  for src in "${SOURCES[@]}" GEN; do
    local path obj
    if [[ "${src}" == GEN ]]; then
      path="${BUILD}/gen/parser.c"
      obj="${objdir}/xkbcomp-parser.o"
    else
      path="${XKB_SRC}/${src}"
      obj="${objdir}/$(echo "${src}" | tr '/' '-' | sed 's/\.c$/.o/')"
    fi
    "${cc}" "${CFLAGS[@]}" -c "${path}" -o "${obj}"
    objs+=("${obj}")
  done
  rm -f "${lib}"
  ar rcs "${lib}" "${objs[@]}"
}

echo "[xkbcommon] building libxkbcommon (musl)"
build_lib "${MUSL_CC}" "${BUILD}/obj-musl" "${OUT}/lib/libxkbcommon.a"
cp "${XKB_SRC}"/include/xkbcommon/xkbcommon.h \
   "${XKB_SRC}"/include/xkbcommon/xkbcommon-keysyms.h \
   "${XKB_SRC}"/include/xkbcommon/xkbcommon-names.h \
   "${XKB_SRC}"/include/xkbcommon/xkbcommon-compat.h \
   "${XKB_SRC}"/include/xkbcommon/xkbcommon-compose.h \
   "${OUT}/include/xkbcommon/"

echo "[xkbcommon] building host copy for data verification"
build_lib "${HOST_CC}" "${BUILD}/obj-host" "${BUILD}/libxkbcommon-host.a"
"${HOST_CC}" -O2 -I"${XKB_SRC}/include" "${ROOT_DIR}/wayland/peetos/xkb-check.c" \
  "${BUILD}/libxkbcommon-host.a" -o "${BUILD}/xkb-check"

echo "[xkbcommon] staging xkeyboard-config subset from ${XKB_DATA_SRC}"
DATA_OUT="${OUT}/share/X11/xkb"
python3 "${ROOT_DIR}/scripts/xkb_data_subset.py" --src "${XKB_DATA_SRC}" --out "${DATA_OUT}" \
  --layouts "${XKB_LAYOUTS}" --options "${XKB_OPTIONS}"
"${BUILD}/xkb-check" "${DATA_OUT}" "${XKB_LAYOUTS}"
"${BUILD}/xkb-check" "${DATA_OUT}" "us" "${XKB_OPTIONS}"

echo "[xkbcommon] installed libxkbcommon 1.7.0 and xkb data into ${OUT}"
