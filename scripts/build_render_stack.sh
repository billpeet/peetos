#!/usr/bin/env bash
set -euo pipefail

# Build the software rendering stack for PeetOS userspace (musl, shared
# libraries) into a staged sysroot at wayland/out/stage:
#   expat (configure), freetype, fontconfig, pixman (meson), DejaVu fonts.
#
# Everything installs with prefix /usr and sysconfdir /etc, so the paths
# fontconfig bakes in (/etc/fonts, /usr/share/fonts) are right on the target.
# The libraries built earlier (libwayland, libffi, libxkbcommon, libdrm, from
# wayland/out) are copied into the stage with generated pkg-config files, so
# meson projects find everything through one sysroot.
#
# Needs meson + ninja and gperf on the build machine (PATH, or MESON/NINJA/
# GPERF_DIR); see docs/rendering.md.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WL="${ROOT_DIR}/wayland"
OUT="${WL}/out"
STAGE="${OUT}/stage"
BUILD="${OUT}/build/render"
MUSL_CC="${MUSL_CC:-${ROOT_DIR}/musl/sysroot/bin/musl-gcc}"
MUSL_SHARED_LIB="${ROOT_DIR}/musl/shared/lib"
JOBS="${JOBS:-$(nproc 2>/dev/null || echo 2)}"

if [[ -d "${HOME}/.peetos-venv/bin" ]]; then
  PATH="${HOME}/.peetos-venv/bin:${PATH}"
fi
MESON="${MESON:-meson}"
for tool in "${MESON}" ninja gperf pkg-config; do
  if ! command -v "${tool}" >/dev/null 2>&1; then
    echo "${tool} not found (see docs/rendering.md for the build prerequisites)" >&2
    exit 1
  fi
done
for d in pixman expat freetype fontconfig dejavu wayland-protocols; do
  if [[ ! -d "${WL}/${d}-src" ]]; then
    echo "${d} source not found (run 'make -C wayland fetch')" >&2
    exit 1
  fi
done
if [[ ! -f "${OUT}/lib/libwayland-client.so.0" || ! -f "${OUT}/lib/libdrm.so.2" ]]; then
  echo "libwayland/libdrm not built yet (run build_wayland.sh, build_xkbcommon.sh, build_libdrm.sh first)" >&2
  exit 1
fi

rm -rf "${STAGE}" "${BUILD}"
mkdir -p "${STAGE}/usr/lib/pkgconfig" "${STAGE}/usr/include" "${STAGE}/usr/share/pkgconfig" "${BUILD}"

# --- seed the stage with the libraries built earlier --------------------------
cp -a "${OUT}/lib/." "${STAGE}/usr/lib/"
cp -a "${OUT}/include/." "${STAGE}/usr/include/"
mkdir -p "${STAGE}/usr/share/wayland-protocols"
for sub in stable staging unstable; do
  cp -a "${WL}/wayland-protocols-src/${sub}" "${STAGE}/usr/share/wayland-protocols/"
done
mkdir -p "${STAGE}/usr/share/wayland"
cp "${OUT}/share/wayland/wayland.xml" "${STAGE}/usr/share/wayland/"

pc() {
  local name="$1" version="$2" libs="$3" extra_cflags="$4" requires="$5"
  cat > "${STAGE}/usr/lib/pkgconfig/${name}.pc" <<EOF
prefix=/usr
libdir=\${prefix}/lib
includedir=\${prefix}/include

Name: ${name}
Description: ${name} (PeetOS build)
Version: ${version}
Requires.private: ${requires}
Libs: -L\${libdir} ${libs}
Cflags: -I\${includedir} ${extra_cflags}
EOF
}
pc libffi 3.4.6 "-lffi" "" ""
pc wayland-client 1.23.1 "-lwayland-client" "" "libffi"
pc wayland-server 1.23.1 "-lwayland-server" "" "libffi"
pc xkbcommon 1.7.0 "-lxkbcommon" "" ""
pc libdrm 2.4.123 "-ldrm" "-I\${includedir}/libdrm" ""
cat > "${STAGE}/usr/share/pkgconfig/wayland-protocols.pc" <<'EOF'
prefix=/usr
datarootdir=${prefix}/share
pkgdatadir=${pc_sysrootdir}${datarootdir}/wayland-protocols

Name: Wayland Protocols
Description: Wayland protocol files
Version: 1.38
EOF

# --- meson cross file ------------------------------------------------------------
CROSS="${OUT}/peetos-cross.ini"
cat > "${CROSS}" <<EOF
[binaries]
c = '${MUSL_CC}'
ar = 'ar'
strip = 'strip'
pkg-config = 'pkg-config'

[host_machine]
system = 'linux'
cpu_family = 'x86_64'
cpu = 'x86_64'
endian = 'little'

[properties]
sys_root = '${STAGE}'
pkg_config_libdir = '${STAGE}/usr/lib/pkgconfig:${STAGE}/usr/share/pkgconfig'

[built-in options]
c_link_args = ['-L${MUSL_SHARED_LIB}', '-L${STAGE}/usr/lib', '-Wl,-rpath-link,${STAGE}/usr/lib', '-Wl,-z,max-page-size=0x1000']
EOF

meson_build() {
  local name="$1" src="$2"
  shift 2
  echo "[render] building ${name}"
  "${MESON}" setup "${BUILD}/${name}" "${src}" --cross-file "${CROSS}" \
    --prefix=/usr --sysconfdir=/etc --localstatedir=/var --libdir=lib \
    --buildtype=release -Ddefault_library=shared -Db_pie=true "$@" \
    > "${BUILD}/${name}.setup.log" 2>&1 || { tail -40 "${BUILD}/${name}.setup.log" >&2; exit 1; }
  ninja -C "${BUILD}/${name}" -j "${JOBS}" > "${BUILD}/${name}.build.log" 2>&1 \
    || { tail -40 "${BUILD}/${name}.build.log" >&2; exit 1; }
  DESTDIR="${STAGE}" "${MESON}" install -C "${BUILD}/${name}" --no-rebuild \
    > "${BUILD}/${name}.install.log" 2>&1 || { tail -20 "${BUILD}/${name}.install.log" >&2; exit 1; }
}

# --- expat (autotools release tarball) -------------------------------------------
echo "[render] building expat"
mkdir -p "${BUILD}/expat"
(
  cd "${BUILD}/expat"
  CC="${MUSL_CC}" CFLAGS="-O2 -fPIC" LDFLAGS="-L${MUSL_SHARED_LIB} -Wl,-z,max-page-size=0x1000" \
    "${WL}/expat-src/configure" --build="$(cc -dumpmachine)" --host=x86_64-linux-musl \
    --prefix=/usr --enable-shared --disable-static \
    --without-docbook --without-examples --without-tests --without-xmlwf \
    > configure.log 2>&1 || { tail -40 configure.log >&2; exit 1; }
  make -j"${JOBS}" > make.log 2>&1 || { tail -40 make.log >&2; exit 1; }
  make install DESTDIR="${STAGE}" > install.log 2>&1
)
# libtool .la files carry absolute /usr paths that confuse later links.
rm -f "${STAGE}"/usr/lib/*.la

meson_build freetype "${WL}/freetype-src" \
  -Dbrotli=disabled -Dbzip2=disabled -Dharfbuzz=disabled -Dpng=disabled -Dzlib=none -Dtests=disabled

meson_build fontconfig "${WL}/fontconfig-src" \
  -Ddoc=disabled -Dnls=disabled -Dtests=disabled -Dtools=enabled -Dcache-build=disabled \
  -Ddefault-fonts-dirs=/usr/share/fonts -Dadditional-fonts-dirs=[] -Dcache-dir=/var/cache/fontconfig

meson_build pixman "${WL}/pixman-src" \
  -Dtests=disabled -Ddemos=disabled -Dgtk=disabled -Dlibpng=disabled -Dopenmp=disabled

# --- fonts ------------------------------------------------------------------------
mkdir -p "${STAGE}/usr/share/fonts/dejavu"
cp "${WL}/dejavu-src/ttf/DejaVuSansMono.ttf" "${WL}/dejavu-src/ttf/DejaVuSans.ttf" \
  "${WL}/dejavu-src/LICENSE" "${STAGE}/usr/share/fonts/dejavu/"

echo "[render] staged sysroot at ${STAGE}:"
ls "${STAGE}/usr/lib" | grep -E '\.so\.[0-9]+$' | tr '\n' ' '
echo
