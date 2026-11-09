#!/usr/bin/env bash
set -euo pipefail

# Build GTK 3 for PeetOS userspace (musl, shared, Wayland backend only) into
# the staged sysroot (wayland/out/stage), on top of scripts/build_render_stack.sh
# and scripts/build_cairo_pango.sh:
#   libwayland-egl (direct build from the libwayland source), libepoxy, ATK,
#   gdk-pixbuf, GTK 3.24 (meson), and the hicolor icon theme index.
#
# GTK's Wayland backend draws with cairo into wl_shm buffers, so no GL driver
# is needed; libepoxy is built for EGL but only loads libEGL if an
# application asks for GL. Build-machine tools: wayland-scanner (from
# build_wayland.sh) and GLib's glib-compile-resources/glib-compile-schemas
# (Ubuntu: libglib2.0-dev-bin), see docs/gtk.md.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WL="${ROOT_DIR}/wayland"
OUT="${WL}/out"
STAGE="${OUT}/stage"
CROSS="${OUT}/peetos-cross.ini"
BUILD="${OUT}/build/gtk"
MUSL_CC="${MUSL_CC:-${ROOT_DIR}/musl/sysroot/bin/musl-gcc}"
MUSL_SHARED_LIB="${ROOT_DIR}/musl/shared/lib"
JOBS="${JOBS:-$(nproc 2>/dev/null || echo 2)}"

if [[ -d "${HOME}/.peetos-venv/bin" ]]; then
  PATH="${HOME}/.peetos-venv/bin:${PATH}"
fi
MESON="${MESON:-meson}"
for tool in "${MESON}" ninja pkg-config python3 glib-compile-resources glib-compile-schemas; do
  command -v "${tool}" >/dev/null 2>&1 || { echo "${tool} not found on the build machine (see docs/gtk.md)" >&2; exit 1; }
done
if [[ ! -f "${STAGE}/usr/lib/libpangocairo-1.0.so.0" || ! -x "${OUT}/bin/wayland-scanner" ]]; then
  echo "cairo/pango not staged (run scripts/build_cairo_pango.sh first)" >&2
  exit 1
fi
for d in wayland egl-registry libepoxy atk gdk-pixbuf gtk hicolor-icon-theme; do
  [[ -d "${WL}/${d}-src" ]] || { echo "${d} source not found (run 'make -C wayland fetch')" >&2; exit 1; }
done

rm -rf "${BUILD}"
mkdir -p "${BUILD}"

run_logged() {
  local log="$1"
  shift
  "$@" > "${log}" 2>&1 || { tail -40 "${log}" >&2; exit 1; }
}

# Build-machine tools for meson's gnome module and GTK's protocol code: the
# host GLib code generators and resource/schema compilers, the staged GLib's
# Python generators (same version as the target library), and the host
# wayland-scanner.
NATIVE_PC="${BUILD}/native-pkgconfig"
mkdir -p "${NATIVE_PC}"
GLIB_VERSION="$(PKG_CONFIG_LIBDIR="${STAGE}/usr/lib/pkgconfig" pkg-config --modversion glib-2.0)"
cat > "${NATIVE_PC}/glib-2.0.pc" <<EOF
glib_genmarshal=${STAGE}/usr/bin/glib-genmarshal
gobject_query=/bin/false
glib_mkenums=${STAGE}/usr/bin/glib-mkenums

Name: GLib
Description: GLib code generators (build machine)
Version: ${GLIB_VERSION}
EOF
sed 's/^Name: GLib$/Name: GObject/' "${NATIVE_PC}/glib-2.0.pc" > "${NATIVE_PC}/gobject-2.0.pc"
cat > "${NATIVE_PC}/gio-2.0.pc" <<EOF
glib_compile_resources=$(command -v glib-compile-resources)
glib_compile_schemas=$(command -v glib-compile-schemas)
gdbus_codegen=${STAGE}/usr/bin/gdbus-codegen

Name: GIO
Description: GIO tools (build machine)
Version: ${GLIB_VERSION}
EOF
cat > "${NATIVE_PC}/wayland-scanner.pc" <<EOF
wayland_scanner=${OUT}/bin/wayland-scanner

Name: Wayland Scanner
Description: Wayland scanner (build machine)
Version: 1.23.1
EOF
export PATH="${OUT}/bin:${PATH}"

meson_build() {
  local name="$1" src="$2"
  shift 2
  echo "[gtk] building ${name}"
  run_logged "${BUILD}/${name}.setup.log" "${MESON}" setup "${BUILD}/${name}" "${src}" --cross-file "${CROSS}" \
    --prefix=/usr --sysconfdir=/etc --localstatedir=/var --libdir=lib \
    --buildtype=release -Ddefault_library=shared -Db_pie=true --wrap-mode=nodownload \
    -Dbuild.pkg_config_path="${NATIVE_PC}" "$@"
  run_logged "${BUILD}/${name}.build.log" ninja -C "${BUILD}/${name}" -j "${JOBS}"
  DESTDIR="${STAGE}" run_logged "${BUILD}/${name}.install.log" "${MESON}" install -C "${BUILD}/${name}" --no-rebuild
}

# --- libwayland-egl (one C file in libwayland's tree) ---------------------------
echo "[gtk] building libwayland-egl"
"${MUSL_CC}" -O2 -fPIC -shared -I"${WL}/wayland-src/egl" -I"${STAGE}/usr/include" -Wl,-soname,libwayland-egl.so.1 \
  -L"${MUSL_SHARED_LIB}" -Wl,-z,max-page-size=0x1000 \
  -o "${STAGE}/usr/lib/libwayland-egl.so.1" "${WL}/wayland-src/egl/wayland-egl.c"
ln -sfn libwayland-egl.so.1 "${STAGE}/usr/lib/libwayland-egl.so"
cp "${WL}/wayland-src/egl/wayland-egl.h" "${WL}/wayland-src/egl/wayland-egl-core.h" \
  "${WL}/wayland-src/egl/wayland-egl-backend.h" "${STAGE}/usr/include/"
cat > "${STAGE}/usr/lib/pkgconfig/wayland-egl.pc" <<'EOF'
prefix=/usr
libdir=${prefix}/lib
includedir=${prefix}/include

Name: wayland-egl
Description: Frontend wayland-egl library (PeetOS build)
Version: 18.1.0
Requires: wayland-client
Libs: -L${libdir} -lwayland-egl
Cflags: -I${includedir}
EOF

# --- Khronos EGL headers ---------------------------------------------------------------
# libepoxy's EGL dispatch and GTK's Wayland GL code include them; there is no
# libEGL yet. PeetOS has no X11, so EGL_NO_X11 is the default (otherwise
# eglplatform.h pulls in <X11/Xlib.h> on Linux).
mkdir -p "${STAGE}/usr/include/EGL" "${STAGE}/usr/include/KHR"
cp "${WL}/egl-registry-src/api/EGL/egl.h" "${WL}/egl-registry-src/api/EGL/eglext.h" "${STAGE}/usr/include/EGL/"
cp "${WL}/egl-registry-src/api/KHR/khrplatform.h" "${STAGE}/usr/include/KHR/"
{
  printf '/* PeetOS: no X11, so default to the generic native types. */\n'
  printf '#ifndef EGL_NO_X11\n#define EGL_NO_X11\n#endif\n'
  cat "${WL}/egl-registry-src/api/EGL/eglplatform.h"
} > "${STAGE}/usr/include/EGL/eglplatform.h"
# libepoxy takes the headers from dependency('egl') (compile flags only; it
# loads libEGL at run time), so describe them with a headers-only egl.pc.
cat > "${STAGE}/usr/lib/pkgconfig/egl.pc" <<'EOF'
prefix=/usr
includedir=${prefix}/include

Name: egl
Description: Khronos EGL headers only (PeetOS has no libEGL yet)
Version: 1.5
Cflags: -I${includedir}
EOF

meson_build libepoxy "${WL}/libepoxy-src" -Degl=yes -Dglx=no -Dx11=false -Dtests=false -Ddocs=false

meson_build atk "${WL}/atk-src" -Dintrospection=false -Ddocs=false

# All loaders built in: no loaders.cache to generate (that needs running a
# target program) and no module directory at run time.
meson_build gdk-pixbuf "${WL}/gdk-pixbuf-src" \
  -Dpng=enabled -Djpeg=disabled -Dtiff=disabled -Dgif=enabled -Dothers=disabled \
  -Dbuiltin_loaders=all -Dgio_sniffing=false -Dintrospection=disabled -Dman=false \
  -Dgtk_doc=false -Ddocs=false -Dtests=false -Dinstalled_tests=false -Drelocatable=false

# GTK's resource bundle converts its icons with gdk-pixbuf-pixdata
# (glib-compile-resources' to-pixdata preprocessing). Run the staged PeetOS
# build on the build machine through musl's dynamic loader, which works as a
# command: ld-musl --library-path <dirs> <program> <args>. Same version as
# the library, and no host gdk-pixbuf needed.
TOOLS="${BUILD}/tools"
mkdir -p "${TOOLS}"
cat > "${TOOLS}/gdk-pixbuf-pixdata" <<EOF
#!/bin/sh
exec "${MUSL_SHARED_LIB}/libc.so" --library-path "${STAGE}/usr/lib:${MUSL_SHARED_LIB}" \\
  "${STAGE}/usr/bin/gdk-pixbuf-pixdata" "\$@"
EOF
chmod +x "${TOOLS}/gdk-pixbuf-pixdata"
export GDK_PIXBUF_PIXDATA="${TOOLS}/gdk-pixbuf-pixdata"

meson_build gtk "${WL}/gtk-src" \
  -Dx11_backend=false -Dwayland_backend=true -Dbroadway_backend=false \
  -Dprint_backends=file -Dcolord=no -Dintrospection=false -Ddemos=false -Dexamples=false \
  -Dtests=false -Dinstalled_tests=false -Dgtk_doc=false -Dman=false -Dbuiltin_immodules=yes \
  -Dprofiler=false -Dtracker3=false -Dcloudproviders=false -Dxinerama=no

# --- icon theme index and GSettings schemas ------------------------------------------
mkdir -p "${STAGE}/usr/share/icons/hicolor"
cp "${WL}/hicolor-icon-theme-src/index.theme" "${STAGE}/usr/share/icons/hicolor/"
# GTK aborts when a GSettings schema it needs (file chooser, color chooser) is
# missing, so compile the installed schemas with the build machine's tool.
glib-compile-schemas "${STAGE}/usr/share/glib-2.0/schemas"

echo "[gtk] staged: $(cd "${STAGE}/usr/lib" && ls | grep -E '^lib(wayland-egl|epoxy|atk-1\.0|gdk_pixbuf-2\.0|gdk-3|gtk-3|gailutil-3)\.so\.[0-9]+$' | tr '\n' ' ')"
