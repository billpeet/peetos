#!/usr/bin/env bash
set -euo pipefail

# Build wlroots and the labwc compositor for PeetOS userspace (musl, shared)
# into the staged sysroot (wayland/out/stage), on top of the rendering stack,
# cairo/pango and GLib:
#   libevdev, mtdev, libudev-zero (libudev on /sys, no udevd), libinput,
#   libseat (seatd's library: built-in seat and noop backends, no daemon),
#   hwdata's pnp.ids, libdisplay-info, wlroots 0.18 (DRM + libinput backends,
#   pixman renderer, no Xwayland), libxml2 and labwc 0.8.
# See docs/labwc.md.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WL="${ROOT_DIR}/wayland"
OUT="${WL}/out"
STAGE="${OUT}/stage"
CROSS="${OUT}/peetos-cross.ini"
BUILD="${OUT}/build/wlroots"
PATCHES="${WL}/peetos/patches"
MUSL_CC="${MUSL_CC:-${ROOT_DIR}/musl/sysroot/bin/musl-gcc}"
MUSL_SHARED_LIB="${ROOT_DIR}/musl/shared/lib"
JOBS="${JOBS:-$(nproc 2>/dev/null || echo 2)}"

if [[ -d "${HOME}/.peetos-venv/bin" ]]; then
  PATH="${HOME}/.peetos-venv/bin:${PATH}"
fi
MESON="${MESON:-meson}"
for tool in "${MESON}" ninja pkg-config python3 patch; do
  command -v "${tool}" >/dev/null 2>&1 || { echo "${tool} not found on the build machine (see docs/labwc.md)" >&2; exit 1; }
done
if [[ ! -f "${STAGE}/usr/lib/libpangocairo-1.0.so.0" || ! -x "${OUT}/bin/wayland-scanner" ]]; then
  echo "cairo/pango not staged (run scripts/build_cairo_pango.sh first)" >&2
  exit 1
fi
for d in libevdev mtdev libudev-zero libinput seatd hwdata libdisplay-info wlroots libxml2 labwc; do
  [[ -d "${WL}/${d}-src" ]] || { echo "${d} source not found (run 'make -C wayland fetch')" >&2; exit 1; }
done

rm -rf "${BUILD}"
mkdir -p "${BUILD}"

run_logged() {
  local log="$1"
  shift
  "$@" > "${log}" 2>&1 || { tail -40 "${log}" >&2; exit 1; }
}

# Build-machine pkg-config: wayland-scanner and hwdata's data directory.
NATIVE_PC="${BUILD}/native-pkgconfig"
mkdir -p "${NATIVE_PC}"
cat > "${NATIVE_PC}/wayland-scanner.pc" <<EOF
wayland_scanner=${OUT}/bin/wayland-scanner

Name: Wayland Scanner
Description: Wayland scanner (build machine)
Version: 1.23.1
EOF
cat > "${NATIVE_PC}/hwdata.pc" <<EOF
pkgdatadir=${STAGE}/usr/share/hwdata

Name: hwdata
Description: Hardware identification databases (staged copy)
Version: 0.390
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
  echo "[wlroots] building ${name}"
  prepare_src "${name}" "${src}"
  run_logged "${BUILD}/${name}.setup.log" "${MESON}" setup "${BUILD}/${name}" "${BUILD}/src-${name}" --cross-file "${CROSS}" \
    --prefix=/usr --sysconfdir=/etc --localstatedir=/var --libdir=lib \
    --buildtype=release -Ddefault_library=shared -Db_pie=true --wrap-mode=nodownload \
    -Dbuild.pkg_config_path="${NATIVE_PC}" "$@"
  run_logged "${BUILD}/${name}.build.log" ninja -C "${BUILD}/${name}" -j "${JOBS}"
  DESTDIR="${STAGE}" run_logged "${BUILD}/${name}.install.log" "${MESON}" install -C "${BUILD}/${name}" --no-rebuild
}

TARGET_LDFLAGS="-L${MUSL_SHARED_LIB} -L${STAGE}/usr/lib -Wl,-rpath-link,${STAGE}/usr/lib -Wl,-z,max-page-size=0x1000"

# --- input stack --------------------------------------------------------------------
meson_build libevdev "${WL}/libevdev-src" -Dtests=disabled -Ddocumentation=disabled

echo "[wlroots] building mtdev"
prepare_src mtdev "${WL}/mtdev-src"
(
  cd "${BUILD}/src-mtdev"
  run_logged "${BUILD}/mtdev.configure.log" env CC="${MUSL_CC}" CFLAGS="-O2 -fPIC" \
    CPPFLAGS="-isystem ${STAGE}/usr/include" LDFLAGS="${TARGET_LDFLAGS}" \
    ./configure --build="$(cc -dumpmachine)" --host=x86_64-linux-musl --prefix=/usr \
    --enable-shared --disable-static
  run_logged "${BUILD}/mtdev.make.log" make -j"${JOBS}" -C src
  run_logged "${BUILD}/mtdev.install.log" make -C src install DESTDIR="${STAGE}"
  install -m 0644 mtdev.pc "${STAGE}/usr/lib/pkgconfig/mtdev.pc"
)
rm -f "${STAGE}"/usr/lib/libmtdev.la

# libudev-zero: libudev over /sys and /dev, no udevd (the kernel provides a
# static /sys, kernel/src/sysfs.rs). Its monitor needs <linux/netlink.h>,
# kept in a private include directory (see that header).
echo "[wlroots] building libudev-zero"
prepare_src libudev-zero "${WL}/libudev-zero-src"
run_logged "${BUILD}/libudev-zero.make.log" make -C "${BUILD}/src-libudev-zero" -j"${JOBS}" \
  CC="${MUSL_CC}" CFLAGS="-O2 -fPIC -I${WL}/peetos/libudev-zero-include -isystem ${STAGE}/usr/include" \
  LDFLAGS="-L${MUSL_SHARED_LIB}" PREFIX=/usr
run_logged "${BUILD}/libudev-zero.install.log" make -C "${BUILD}/src-libudev-zero" install \
  PREFIX=/usr DESTDIR="${STAGE}"
rm -f "${STAGE}/usr/lib/libudev.a"

meson_build libinput "${WL}/libinput-src" \
  -Dlibwacom=false -Ddebug-gui=false -Dtests=false -Ddocumentation=false -Dinstall-tests=false \
  -Dzshcompletiondir=no -Dudev-dir=/usr/lib/udev

# libseat, used with its noop backend (LIBSEAT_BACKEND=noop): devices are
# opened directly, which needs root, and there are no VTs to switch. The
# seatd client backend is built because libseat needs one "real" backend;
# there is no seatd daemon. logind and the built-in (in-process seatd)
# backends need systemd or VT/KD support.
meson_build seatd "${WL}/seatd-src" \
  -Dlibseat-logind=disabled -Dlibseat-seatd=enabled -Dlibseat-builtin=disabled \
  -Dserver=disabled -Dexamples=disabled -Dman-pages=disabled

# --- display info ---------------------------------------------------------------------
echo "[wlroots] staging hwdata pnp.ids"
mkdir -p "${STAGE}/usr/share/hwdata"
cp "${WL}/hwdata-src/pnp.ids" "${STAGE}/usr/share/hwdata/pnp.ids"

meson_build libdisplay-info "${WL}/libdisplay-info-src"

# --- wlroots and labwc ------------------------------------------------------------------
meson_build wlroots "${WL}/wlroots-src" \
  -Dxwayland=disabled -Dexamples=false -Drenderers=[] -Dbackends=drm,libinput -Dallocators=[] \
  -Dsession=enabled -Dcolor-management=disabled -Dlibliftoff=disabled -Dxcb-errors=disabled

meson_build libxml2 "${WL}/libxml2-src" \
  -Dpython=false -Dicu=disabled -Dlzma=disabled -Dzlib=disabled -Dhistory=false \
  -Dreadline=false -Dmodules=disabled -Dhttp=false -Dftp=false
# libxml2 2.13's meson build writes "Cflags: -I${includedir}", but the
# headers are in include/libxml2 (fixed upstream in 2.14).
sed -i 's|^Cflags: -I${includedir}$|Cflags: -I${includedir}/libxml2|' "${STAGE}/usr/lib/pkgconfig/libxml-2.0.pc"

meson_build labwc "${WL}/labwc-src" \
  -Dxwayland=disabled -Dsvg=disabled -Dicon=disabled -Dnls=disabled -Dman-pages=disabled -Dtest=disabled

echo "[wlroots] staged: $(cd "${STAGE}/usr/lib" && ls | grep -E '^lib(evdev|mtdev|udev|input|seat|display-info|wlroots-0\.18|xml2)\.so(\.[0-9]+)?$' | tr '\n' ' ') labwc=$( [[ -x ${STAGE}/usr/bin/labwc ]] && echo yes || echo no)"
