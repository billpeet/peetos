#!/usr/bin/env bash
set -euo pipefail

# Build the 2D toolkit libraries for PeetOS userspace (musl, shared) into the
# staged sysroot from scripts/build_render_stack.sh (wayland/out/stage):
#   zlib, libpng, pcre2 (configure), fribidi, GLib, cairo, pango (meson).
#
# GLib is built for pango (and later GTK); its gio, gobject and gmodule
# libraries are installed too. cairo has the image, PNG, PDF/PS/SVG and
# script backends plus freetype/fontconfig fonts; no X11. pango uses
# harfbuzz, fribidi, fontconfig/freetype and cairo.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WL="${ROOT_DIR}/wayland"
OUT="${WL}/out"
STAGE="${OUT}/stage"
CROSS="${OUT}/peetos-cross.ini"
BUILD="${OUT}/build/toolkit"
MUSL_CC="${MUSL_CC:-${ROOT_DIR}/musl/sysroot/bin/musl-gcc}"
MUSL_SHARED_LIB="${ROOT_DIR}/musl/shared/lib"
JOBS="${JOBS:-$(nproc 2>/dev/null || echo 2)}"

if [[ -d "${HOME}/.peetos-venv/bin" ]]; then
  PATH="${HOME}/.peetos-venv/bin:${PATH}"
fi
MESON="${MESON:-meson}"
for tool in "${MESON}" ninja pkg-config python3; do
  command -v "${tool}" >/dev/null 2>&1 || { echo "${tool} not found (see docs/rendering.md)" >&2; exit 1; }
done
if [[ ! -f "${CROSS}" || ! -f "${STAGE}/usr/lib/libharfbuzz.so.0" ]]; then
  echo "rendering stack not staged (run scripts/build_render_stack.sh first)" >&2
  exit 1
fi
for d in zlib libpng pcre2 glib fribidi cairo pango; do
  [[ -d "${WL}/${d}-src" ]] || { echo "${d} source not found (run 'make -C wayland fetch')" >&2; exit 1; }
done

rm -rf "${BUILD}"
mkdir -p "${BUILD}"

TARGET_CFLAGS="-O2 -fPIC"
TARGET_LDFLAGS="-L${MUSL_SHARED_LIB} -L${STAGE}/usr/lib -Wl,-rpath-link,${STAGE}/usr/lib -Wl,-z,max-page-size=0x1000"

run_logged() {
  local log="$1"
  shift
  "$@" > "${log}" 2>&1 || { tail -40 "${log}" >&2; exit 1; }
}

autotools_build() {
  local name="$1" src="$2"
  shift 2
  echo "[toolkit] building ${name}"
  mkdir -p "${BUILD}/${name}"
  (
    cd "${BUILD}/${name}"
    run_logged configure.log env CC="${MUSL_CC}" CFLAGS="${TARGET_CFLAGS}" \
      CPPFLAGS="-I${STAGE}/usr/include" LDFLAGS="${TARGET_LDFLAGS}" \
      PKG_CONFIG_LIBDIR="${STAGE}/usr/lib/pkgconfig" PKG_CONFIG_SYSROOT_DIR="${STAGE}" \
      "${src}/configure" --build="$(cc -dumpmachine)" --host=x86_64-linux-musl \
      --prefix=/usr --enable-shared --disable-static "$@"
    run_logged make.log make -j"${JOBS}"
    run_logged install.log make install DESTDIR="${STAGE}"
  )
  # libtool .la files carry absolute /usr paths that confuse later links.
  rm -f "${STAGE}"/usr/lib/*.la
}

meson_build() {
  local name="$1" src="$2"
  shift 2
  echo "[toolkit] building ${name}"
  run_logged "${BUILD}/${name}.setup.log" "${MESON}" setup "${BUILD}/${name}" "${src}" --cross-file "${CROSS}" \
    --prefix=/usr --sysconfdir=/etc --localstatedir=/var --libdir=lib \
    --buildtype=release -Ddefault_library=shared -Db_pie=true --wrap-mode=nodownload "$@"
  run_logged "${BUILD}/${name}.build.log" ninja -C "${BUILD}/${name}" -j "${JOBS}"
  DESTDIR="${STAGE}" run_logged "${BUILD}/${name}.install.log" "${MESON}" install -C "${BUILD}/${name}" --no-rebuild
}

# --- zlib (its own configure script, in-tree build) -------------------------------
echo "[toolkit] building zlib"
cp -a "${WL}/zlib-src" "${BUILD}/zlib"
(
  cd "${BUILD}/zlib"
  run_logged configure.log env CC="${MUSL_CC}" CFLAGS="${TARGET_CFLAGS}" LDFLAGS="${TARGET_LDFLAGS}" \
    ./configure --prefix=/usr --shared
  run_logged make.log make -j"${JOBS}" libz.so.1.3.1 libz.a
  run_logged install.log make install DESTDIR="${STAGE}"
)
rm -f "${STAGE}/usr/lib/libz.a"

autotools_build libpng "${WL}/libpng-src" --disable-tools --without-binconfigs
autotools_build pcre2 "${WL}/pcre2-src" \
  --enable-pcre2-8 --disable-pcre2-16 --disable-pcre2-32 --disable-jit \
  --disable-pcre2grep-libz --disable-pcre2grep-libbz2 --disable-pcre2test-libreadline

meson_build fribidi "${WL}/fribidi-src" -Ddocs=false -Dbin=false -Dtests=false

# GLib's code generators (glib-mkenums, glib-genmarshal, ...) are Python
# scripts installed into the stage; meson projects that use them (pango) look
# them up as build-machine programs through glib-2.0's pkg-config file, so a
# native pkg-config directory points at the staged copies.
meson_build glib "${WL}/glib-src" \
  -Dtests=false -Dinstalled_tests=false -Dnls=disabled -Dintrospection=disabled \
  -Dselinux=disabled -Dxattr=false -Dlibmount=disabled -Dman-pages=disabled \
  -Ddocumentation=false -Dsysprof=disabled -Dlibelf=disabled -Ddtrace=disabled \
  -Dsystemtap=disabled -Dglib_debug=disabled

NATIVE_PC="${BUILD}/native-pkgconfig"
mkdir -p "${NATIVE_PC}"
cat > "${NATIVE_PC}/glib-2.0.pc" <<EOF
bindir=${STAGE}/usr/bin
glib_genmarshal=\${bindir}/glib-genmarshal
gobject_query=\${bindir}/gobject-query
glib_mkenums=\${bindir}/glib-mkenums
glib_valgrind_suppressions=

Name: GLib
Description: GLib code generators (build machine, from the PeetOS stage)
Version: $(PKG_CONFIG_LIBDIR="${STAGE}/usr/lib/pkgconfig" pkg-config --modversion glib-2.0)
EOF
cp "${NATIVE_PC}/glib-2.0.pc" "${NATIVE_PC}/gobject-2.0.pc"
sed -i 's/^Name: GLib$/Name: GObject/' "${NATIVE_PC}/gobject-2.0.pc"
cat > "${NATIVE_PC}/gio-2.0.pc" <<EOF
bindir=${STAGE}/usr/bin
glib_compile_resources=/bin/false
glib_compile_schemas=/bin/false
gdbus_codegen=\${bindir}/gdbus-codegen

Name: GIO
Description: GIO code generators (build machine, from the PeetOS stage)
Version: $(PKG_CONFIG_LIBDIR="${STAGE}/usr/lib/pkgconfig" pkg-config --modversion gio-2.0)
EOF

meson_build cairo "${WL}/cairo-src" \
  -Dtests=disabled -Dxlib=disabled -Dxcb=disabled -Dxlib-xcb=disabled -Dquartz=disabled \
  -Ddwrite=disabled -Dgtk2-utils=disabled -Dspectre=disabled -Dsymbol-lookup=disabled \
  -Dtee=disabled -Dpng=enabled -Dzlib=enabled -Dfreetype=enabled -Dfontconfig=enabled -Dglib=enabled

meson_build pango "${WL}/pango-src" \
  -Dbuild.pkg_config_path="${NATIVE_PC}" \
  -Dintrospection=disabled -Ddocumentation=false -Dbuild-testsuite=false -Dbuild-examples=false \
  -Dlibthai=disabled -Dxft=disabled -Dsysprof=disabled -Dcairo=enabled -Dfontconfig=enabled \
  -Dfreetype=enabled

echo "[toolkit] staged: $(cd "${STAGE}/usr/lib" && ls | grep -E '^lib(z|png16|pcre2-8|fribidi|glib-2\.0|gobject-2\.0|gio-2\.0|cairo|pango[a-z0-9]*-1\.0)\.so\.[0-9]+$' | tr '\n' ' ')"
