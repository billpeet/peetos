#!/usr/bin/env bash
set -euo pipefail

# Build the desktop shell for PeetOS userspace (musl, shared) into the staged
# sysroot (wayland/out/stage), on top of GTK 3, wlroots/labwc and foot:
#   json-c and gtk-layer-shell (wlr-layer-shell surfaces for GTK 3), the
#   sfwbar panel (taskbar, application menu, clock), and the fuzzel
#   application launcher (fcft + pixman, like foot).
# See docs/desktop.md.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WL="${ROOT_DIR}/wayland"
OUT="${WL}/out"
STAGE="${OUT}/stage"
CROSS="${OUT}/peetos-cross.ini"
BUILD="${OUT}/build/desktop"
PATCHES="${WL}/peetos/patches"
MUSL_CC="${MUSL_CC:-${ROOT_DIR}/musl/sysroot/bin/musl-gcc}"
MUSL_SHARED_LIB="${ROOT_DIR}/musl/shared/lib"
JOBS="${JOBS:-$(nproc 2>/dev/null || echo 2)}"

if [[ -d "${HOME}/.peetos-venv/bin" ]]; then
  PATH="${HOME}/.peetos-venv/bin:${PATH}"
fi
MESON="${MESON:-meson}"
for tool in "${MESON}" ninja cmake pkg-config patch glib-compile-resources; do
  command -v "${tool}" >/dev/null 2>&1 || { echo "${tool} not found on the build machine (see docs/desktop.md)" >&2; exit 1; }
done
if [[ ! -f "${STAGE}/usr/lib/libgtk-3.so.0" || ! -x "${OUT}/bin/wayland-scanner" ]]; then
  echo "GTK 3 not staged (run scripts/build_gtk.sh first)" >&2
  exit 1
fi
if [[ ! -f "${STAGE}/usr/lib/libfcft.so.4" ]]; then
  echo "fcft not staged (run scripts/build_foot.sh first)" >&2
  exit 1
fi
for d in json-c gtk-layer-shell sfwbar fuzzel; do
  [[ -d "${WL}/${d}-src" ]] || { echo "${d} source not found (run 'make -C wayland fetch')" >&2; exit 1; }
done

rm -rf "${BUILD}"
mkdir -p "${BUILD}"

run_logged() {
  local log="$1"
  shift
  "$@" > "${log}" 2>&1 || { tail -40 "${log}" >&2; exit 1; }
}

# Build-machine pkg-config: the wayland-scanner from build_wayland.sh.
NATIVE_PC="${BUILD}/native-pkgconfig"
mkdir -p "${NATIVE_PC}"
cat > "${NATIVE_PC}/wayland-scanner.pc" <<EOF
prefix=${OUT}
bindir=\${prefix}/bin
wayland_scanner=\${bindir}/wayland-scanner

Name: Wayland Scanner
Description: Wayland scanner (build machine)
Version: 1.23.1
EOF
export PATH="${OUT}/bin:${PATH}"

# A pristine copy of the source with wayland/peetos/patches/<name>-*.patch.
prepare_src() {
  local name="$1" src="$2"
  rm -rf "${BUILD}/src-${name}"
  cp -a "${src}" "${BUILD}/src-${name}"
  local p
  for p in "${PATCHES}/${name}"-*.patch; do
    [[ -f "${p}" ]] || continue
    patch -d "${BUILD}/src-${name}" -p1 < "${p}" > "${BUILD}/${name}.patch.log" 2>&1 \
      || { cat "${BUILD}/${name}.patch.log" >&2; exit 1; }
  done
}

meson_build() {
  local name="$1" src="$2"
  shift 2
  echo "[desktop] building ${name}"
  prepare_src "${name}" "${src}"
  run_logged "${BUILD}/${name}.setup.log" "${MESON}" setup "${BUILD}/${name}" "${BUILD}/src-${name}" --cross-file "${CROSS}" \
    --prefix=/usr --sysconfdir=/etc --localstatedir=/var --libdir=lib \
    --buildtype=release -Ddefault_library=shared -Db_pie=true --wrap-mode=nodownload \
    -Dbuild.pkg_config_path="${NATIVE_PC}" "$@"
  run_logged "${BUILD}/${name}.build.log" ninja -C "${BUILD}/${name}" -j "${JOBS}"
  DESTDIR="${STAGE}" run_logged "${BUILD}/${name}.install.log" "${MESON}" install -C "${BUILD}/${name}" --no-rebuild
}

# --- json-c (cmake) -------------------------------------------------------------------
echo "[desktop] building json-c"
prepare_src json-c "${WL}/json-c-src"
run_logged "${BUILD}/json-c.setup.log" cmake -S "${BUILD}/src-json-c" -B "${BUILD}/json-c" -G Ninja \
  -DCMAKE_SYSTEM_NAME=Linux -DCMAKE_SYSTEM_PROCESSOR=x86_64 \
  -DCMAKE_C_COMPILER="${MUSL_CC}" -DCMAKE_C_FLAGS="-O2 -fPIC" \
  -DCMAKE_SHARED_LINKER_FLAGS="-L${MUSL_SHARED_LIB} -Wl,-z,max-page-size=0x1000" \
  -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX=/usr -DCMAKE_INSTALL_LIBDIR=lib \
  -DBUILD_SHARED_LIBS=ON -DBUILD_STATIC_LIBS=OFF -DBUILD_TESTING=OFF -DBUILD_APPS=OFF \
  -DDISABLE_WERROR=ON -DENABLE_THREADING=ON
run_logged "${BUILD}/json-c.build.log" ninja -C "${BUILD}/json-c" -j "${JOBS}"
DESTDIR="${STAGE}" run_logged "${BUILD}/json-c.install.log" cmake --install "${BUILD}/json-c"

# --- gtk-layer-shell --------------------------------------------------------------------
meson_build gtk-layer-shell "${WL}/gtk-layer-shell-src" \
  -Dintrospection=false -Dvapi=false -Dexamples=false -Ddocs=false -Dtests=false

# --- sfwbar: panel with a taskbar, application menu and clock ---------------------------
# No audio, network-manager, bluetooth or MPD modules (nothing to talk to),
# and no notification center: its widget is drawn from SVG, and there is no
# SVG loader.
meson_build sfwbar "${WL}/sfwbar-src" \
  -Dalsa=disabled -Dpulse=disabled -Dmpd=disabled -Dbluez=disabled -Diwd=disabled \
  -Dnm=disabled -Dnetwork=disabled -Dbsdctl=disabled -Dxkb=disabled \
  -Dappmenu=enabled -Dncenter=disabled -Dbuild-docs=disabled

# --- fuzzel: application launcher -------------------------------------------------------
meson_build fuzzel "${WL}/fuzzel-src" \
  -Denable-cairo=enabled -Dpng-backend=libpng -Dsvg-backend=nanosvg -Dsystem-nanosvg=disabled

echo "[desktop] staged: $(cd "${STAGE}/usr/lib" && ls | grep -E '^lib(json-c|gtk-layer-shell)\.so(\.[0-9]+)?$' | tr '\n' ' ')$(cd "${STAGE}/usr/bin" && ls | grep -E '^(sfwbar|fuzzel)$' | tr '\n' ' ')"
