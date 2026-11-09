#!/usr/bin/env bash
set -euo pipefail

# Build D-Bus for PeetOS userspace (musl, shared) into the staged sysroot
# (wayland/out/stage), on top of scripts/build_render_stack.sh (expat):
#   libdbus-1, dbus-daemon (session bus) and the dbus-send, dbus-monitor,
#   dbus-uuidgen, dbus-launch-less tools (meson).
#
# Only the session bus is used: no systemd, SELinux, AppArmor, audit, X11
# autolaunch or inotify (PeetOS has no inotify; the daemon then does not
# reload its configuration on change). GLib's GDBus talks to the daemon
# without libdbus. See docs/dbus.md.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WL="${ROOT_DIR}/wayland"
OUT="${WL}/out"
STAGE="${OUT}/stage"
CROSS="${OUT}/peetos-cross.ini"
BUILD="${OUT}/build/dbus"
JOBS="${JOBS:-$(nproc 2>/dev/null || echo 2)}"

if [[ -d "${HOME}/.peetos-venv/bin" ]]; then
  PATH="${HOME}/.peetos-venv/bin:${PATH}"
fi
MESON="${MESON:-meson}"
for tool in "${MESON}" ninja pkg-config; do
  command -v "${tool}" >/dev/null 2>&1 || { echo "${tool} not found on the build machine (see docs/dbus.md)" >&2; exit 1; }
done
if [[ ! -f "${STAGE}/usr/lib/libexpat.so.1" || ! -f "${CROSS}" ]]; then
  echo "expat not staged (run scripts/build_render_stack.sh first)" >&2
  exit 1
fi
[[ -d "${WL}/dbus-src" ]] || { echo "dbus source not found (run 'make -C wayland fetch')" >&2; exit 1; }

rm -rf "${BUILD}"
mkdir -p "${BUILD}"

run_logged() {
  local log="$1"
  shift
  "$@" > "${log}" 2>&1 || { tail -40 "${log}" >&2; exit 1; }
}

echo "[dbus] building dbus"
run_logged "${BUILD}/setup.log" "${MESON}" setup "${BUILD}/dbus" "${WL}/dbus-src" --cross-file "${CROSS}" \
  --prefix=/usr --sysconfdir=/etc --localstatedir=/var --libdir=lib \
  --buildtype=release -Ddefault_library=shared -Db_pie=true --wrap-mode=nodownload \
  -Dapparmor=disabled -Dlibaudit=disabled -Dselinux=disabled -Dsystemd=disabled \
  -Dx11_autolaunch=disabled -Dlaunchd=disabled -Dkqueue=disabled -Dinotify=disabled -Depoll=enabled \
  -Ddoxygen_docs=disabled -Dducktype_docs=disabled -Dxml_docs=disabled -Dqt_help=disabled \
  -Dmodular_tests=disabled -Dinstalled_tests=false -Duser_session=false \
  -Dmessage_bus=true -Dtools=true -Dtraditional_activation=true \
  -Dsession_socket_dir=/tmp -Druntime_dir=/run -Ddbus_user=root
run_logged "${BUILD}/build.log" ninja -C "${BUILD}/dbus" -j "${JOBS}"
DESTDIR="${STAGE}" run_logged "${BUILD}/install.log" "${MESON}" install -C "${BUILD}/dbus" --no-rebuild

echo "[dbus] staged: $(cd "${STAGE}/usr/lib" && ls | grep -E '^libdbus-1\.so\.[0-9]+$') $(cd "${STAGE}/usr/bin" && ls | grep -E '^dbus-(daemon|send|monitor|uuidgen)$' | tr '\n' ' ')"
