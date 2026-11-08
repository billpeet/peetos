#!/usr/bin/env bash
set -euo pipefail

# Build tllist, fcft and the foot terminal for PeetOS (musl, shared) with
# meson against the staged sysroot from scripts/build_render_stack.sh, and
# install them into it (wayland/out/stage).
#
# fcft is built without harfbuzz/utf8proc (no text shaping yet: that needs a
# C++ runtime) and without SVG glyphs; foot without terminfo, docs, themes,
# utf8proc grapheme clustering or utmp.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WL="${ROOT_DIR}/wayland"
OUT="${WL}/out"
STAGE="${OUT}/stage"
CROSS="${OUT}/peetos-cross.ini"
BUILD="${OUT}/build/foot"
JOBS="${JOBS:-$(nproc 2>/dev/null || echo 2)}"

if [[ -d "${HOME}/.peetos-venv/bin" ]]; then
  PATH="${HOME}/.peetos-venv/bin:${PATH}"
fi
MESON="${MESON:-meson}"
for tool in "${MESON}" ninja; do
  command -v "${tool}" >/dev/null 2>&1 || { echo "${tool} not found (see docs/rendering.md)" >&2; exit 1; }
done
if [[ ! -f "${CROSS}" || ! -f "${STAGE}/usr/lib/libfontconfig.so.1" ]]; then
  echo "rendering stack not staged (run scripts/build_render_stack.sh first)" >&2
  exit 1
fi
for d in tllist fcft foot; do
  [[ -d "${WL}/${d}-src" ]] || { echo "${d} source not found (run 'make -C wayland fetch')" >&2; exit 1; }
done

rm -rf "${BUILD}"
mkdir -p "${BUILD}/native-pkgconfig"

# foot looks up wayland-scanner as a native (build machine) dependency; point
# meson's build-machine pkg-config at the host scanner from build_wayland.sh.
cat > "${BUILD}/native-pkgconfig/wayland-scanner.pc" <<EOF
prefix=${OUT}
bindir=\${prefix}/bin
wayland_scanner=\${bindir}/wayland-scanner

Name: Wayland Scanner
Description: Wayland scanner (host build for PeetOS)
Version: 1.23.1
EOF

meson_build() {
  local name="$1" src="$2"
  shift 2
  echo "[foot] building ${name}"
  "${MESON}" setup "${BUILD}/${name}" "${src}" --cross-file "${CROSS}" \
    --prefix=/usr --sysconfdir=/etc --localstatedir=/var --libdir=lib \
    --buildtype=release -Ddefault_library=shared -Db_pie=true \
    -Dbuild.pkg_config_path="${BUILD}/native-pkgconfig" "$@" \
    > "${BUILD}/${name}.setup.log" 2>&1 || { tail -40 "${BUILD}/${name}.setup.log" >&2; exit 1; }
  ninja -C "${BUILD}/${name}" -j "${JOBS}" > "${BUILD}/${name}.build.log" 2>&1 \
    || { tail -40 "${BUILD}/${name}.build.log" >&2; exit 1; }
  DESTDIR="${STAGE}" "${MESON}" install -C "${BUILD}/${name}" --no-rebuild \
    > "${BUILD}/${name}.install.log" 2>&1 || { tail -20 "${BUILD}/${name}.install.log" >&2; exit 1; }
}

meson_build tllist "${WL}/tllist-src"
meson_build fcft "${WL}/fcft-src" \
  -Dsvg-backend=none -Dgrapheme-shaping=disabled -Drun-shaping=disabled \
  -Ddocs=disabled -Dexamples=false -Dtest-text-shaping=false
meson_build foot "${WL}/foot-src" \
  -Ddocs=disabled -Dthemes=false -Dgrapheme-clustering=disabled -Dtests=false \
  -Dterminfo=disabled -Dutmp-backend=none

echo "[foot] installed: $(ls "${STAGE}/usr/bin" | grep -E '^foot' | tr '\n' ' ')"
