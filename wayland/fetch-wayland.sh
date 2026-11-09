#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: fetch-wayland.sh [options]

Download, verify, and extract the pinned source releases of the PeetOS
graphics userland into <name>-src/ directories: libwayland, libffi,
wayland-protocols, libxkbcommon and libdrm, plus the software rendering stack
(pixman, expat, freetype, fontconfig, harfbuzz, utf8proc), the DejaVu
fonts, the 2D toolkit libraries (zlib, libpng, pcre2, GLib, fribidi, cairo,
pango), the GTK 3 stack (libepoxy, ATK, gdk-pixbuf, GTK, the hicolor icon
theme), the Xfce libraries and Thunar, D-Bus, the wlroots/labwc stack (libevdev, mtdev,
libudev-zero, libinput, seatd, hwdata, libdisplay-info, wlroots, libxml2,
labwc), and tllist, fcft and the foot terminal.

Options:
  --output-dir <dir>   destination root (default: script dir)
  --force              remove existing source directories first
  --help               show this help
USAGE
}

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OUTPUT_DIR="${SCRIPT_DIR}"
FORCE=0

WAYLAND_VERSION="1.23.1"
WAYLAND_URL="https://gitlab.freedesktop.org/wayland/wayland/-/releases/${WAYLAND_VERSION}/downloads/wayland-${WAYLAND_VERSION}.tar.xz"
WAYLAND_SHA256="864fb2a8399e2d0ec39d56e9d9b753c093775beadc6022ce81f441929a81e5ed"

LIBFFI_VERSION="3.4.6"
LIBFFI_URL="https://github.com/libffi/libffi/releases/download/v${LIBFFI_VERSION}/libffi-${LIBFFI_VERSION}.tar.gz"
LIBFFI_SHA256="b0dea9df23c863a7a50e825440f3ebffabd65df1497108e5d437747843895a4e"

PROTOCOLS_VERSION="1.38"
PROTOCOLS_URL="https://gitlab.freedesktop.org/wayland/wayland-protocols/-/releases/${PROTOCOLS_VERSION}/downloads/wayland-protocols-${PROTOCOLS_VERSION}.tar.xz"
PROTOCOLS_SHA256="ff17292c05159d2b20ce6cacfe42d7e31a28198fa1429a769b03af7c38581dbe"

XKBCOMMON_VERSION="1.7.0"
XKBCOMMON_URL="https://xkbcommon.org/download/libxkbcommon-${XKBCOMMON_VERSION}.tar.xz"
XKBCOMMON_SHA256="65782f0a10a4b455af9c6baab7040e2f537520caa2ec2092805cdfd36863b247"

LIBDRM_VERSION="2.4.123"
LIBDRM_URL="https://dri.freedesktop.org/libdrm/libdrm-${LIBDRM_VERSION}.tar.xz"
LIBDRM_SHA256="a2b98567a149a74b0f50e91e825f9c0315d86e7be9b74394dae8b298caadb79e"

PIXMAN_VERSION="0.43.4"
PIXMAN_URL="https://cairographics.org/releases/pixman-${PIXMAN_VERSION}.tar.gz"
PIXMAN_SHA256="a0624db90180c7ddb79fc7a9151093dc37c646d8c38d3f232f767cf64b85a226"

EXPAT_VERSION="2.6.3"
EXPAT_URL="https://github.com/libexpat/libexpat/releases/download/R_2_6_3/expat-${EXPAT_VERSION}.tar.xz"
EXPAT_SHA256="274db254a6979bde5aad404763a704956940e465843f2a9bd9ed7af22e2c0efc"

FREETYPE_VERSION="2.13.3"
FREETYPE_URL="https://download.savannah.gnu.org/releases/freetype/freetype-${FREETYPE_VERSION}.tar.xz"
FREETYPE_SHA256="0550350666d427c74daeb85d5ac7bb353acba5f76956395995311a9c6f063289"

FONTCONFIG_VERSION="2.15.0"
FONTCONFIG_URL="https://www.freedesktop.org/software/fontconfig/release/fontconfig-${FONTCONFIG_VERSION}.tar.xz"
FONTCONFIG_SHA256="63a0658d0e06e0fa886106452b58ef04f21f58202ea02a94c39de0d3335d7c0e"

DEJAVU_VERSION="2.37"
DEJAVU_URL="https://github.com/dejavu-fonts/dejavu-fonts/releases/download/version_2_37/dejavu-fonts-ttf-${DEJAVU_VERSION}.tar.bz2"
DEJAVU_SHA256="fa9ca4d13871dd122f61258a80d01751d603b4d3ee14095d65453b4e846e17d7"

TLLIST_VERSION="1.1.0"
TLLIST_URL="https://codeberg.org/dnkl/tllist/archive/${TLLIST_VERSION}.tar.gz"
TLLIST_SHA256="0e7b7094a02550dd80b7243bcffc3671550b0f1d8ba625e4dff52517827d5d23"

FCFT_VERSION="3.1.8"
FCFT_URL="https://codeberg.org/dnkl/fcft/archive/${FCFT_VERSION}.tar.gz"
FCFT_SHA256="7000c018597e5a0a9435d924320e63f1c45654fa83bf3ffe150b174131158dbc"

HARFBUZZ_VERSION="10.1.0"
HARFBUZZ_URL="https://github.com/harfbuzz/harfbuzz/releases/download/${HARFBUZZ_VERSION}/harfbuzz-${HARFBUZZ_VERSION}.tar.xz"
HARFBUZZ_SHA256="6ce3520f2d089a33cef0fc48321334b8e0b72141f6a763719aaaecd2779ecb82"

UTF8PROC_VERSION="2.9.0"
UTF8PROC_URL="https://github.com/JuliaStrings/utf8proc/archive/refs/tags/v${UTF8PROC_VERSION}.tar.gz"
UTF8PROC_SHA256="18c1626e9fc5a2e192311e36b3010bfc698078f692888940f1fa150547abb0c1"

ZLIB_VERSION="1.3.1"
ZLIB_URL="https://zlib.net/fossils/zlib-${ZLIB_VERSION}.tar.gz"
ZLIB_SHA256="9a93b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72df23"

LIBPNG_VERSION="1.6.44"
LIBPNG_URL="https://download.sourceforge.net/libpng/libpng-${LIBPNG_VERSION}.tar.xz"
LIBPNG_SHA256="60c4da1d5b7f0aa8d158da48e8f8afa9773c1c8baa5d21974df61f1886b8ce8e"

PCRE2_VERSION="10.44"
PCRE2_URL="https://github.com/PCRE2Project/pcre2/releases/download/pcre2-${PCRE2_VERSION}/pcre2-${PCRE2_VERSION}.tar.bz2"
PCRE2_SHA256="d34f02e113cf7193a1ebf2770d3ac527088d485d4e047ed10e5d217c6ef5de96"

GLIB_VERSION="2.82.2"
GLIB_URL="https://download.gnome.org/sources/glib/2.82/glib-${GLIB_VERSION}.tar.xz"
GLIB_SHA256="ab45f5a323048b1659ee0fbda5cecd94b099ab3e4b9abf26ae06aeb3e781fd63"

FRIBIDI_VERSION="1.0.16"
FRIBIDI_URL="https://github.com/fribidi/fribidi/releases/download/v${FRIBIDI_VERSION}/fribidi-${FRIBIDI_VERSION}.tar.xz"
FRIBIDI_SHA256="1b1cde5b235d40479e91be2f0e88a309e3214c8ab470ec8a2744d82a5a9ea05c"

CAIRO_VERSION="1.18.2"
CAIRO_URL="https://cairographics.org/releases/cairo-${CAIRO_VERSION}.tar.xz"
CAIRO_SHA256="a62b9bb42425e844cc3d6ddde043ff39dbabedd1542eba57a2eb79f85889d45a"

PANGO_VERSION="1.54.0"
PANGO_URL="https://download.gnome.org/sources/pango/1.54/pango-${PANGO_VERSION}.tar.xz"
PANGO_SHA256="8a9eed75021ee734d7fc0fdf3a65c3bba51dfefe4ae51a9b414a60c70b2d1ed8"

# Khronos EGL headers (EGL/*.h, KHR/khrplatform.h): no releases, so a commit.
EGL_REGISTRY_COMMIT="5961a7fe64cf8a126890ced6f13d69e0a1e1b83e"
EGL_REGISTRY_URL="https://github.com/KhronosGroup/EGL-Registry/archive/${EGL_REGISTRY_COMMIT}.tar.gz"
EGL_REGISTRY_SHA256="b360a7542cb51a74ad87f9a423903c1a88d9739167744da9397eb7190ebe83d0"

LIBEPOXY_VERSION="1.5.10"
LIBEPOXY_URL="https://github.com/anholt/libepoxy/archive/refs/tags/${LIBEPOXY_VERSION}.tar.gz"
LIBEPOXY_SHA256="a7ced37f4102b745ac86d6a70a9da399cc139ff168ba6b8002b4d8d43c900c15"

ATK_VERSION="2.38.0"
ATK_URL="https://download.gnome.org/sources/atk/2.38/atk-${ATK_VERSION}.tar.xz"
ATK_SHA256="ac4de2a4ef4bd5665052952fe169657e65e895c5057dffb3c2a810f6191a0c36"

GDK_PIXBUF_VERSION="2.42.12"
GDK_PIXBUF_URL="https://download.gnome.org/sources/gdk-pixbuf/2.42/gdk-pixbuf-${GDK_PIXBUF_VERSION}.tar.xz"
GDK_PIXBUF_SHA256="b9505b3445b9a7e48ced34760c3bcb73e966df3ac94c95a148cb669ab748e3c7"

GTK3_VERSION="3.24.43"
GTK3_URL="https://download.gnome.org/sources/gtk%2B/3.24/gtk%2B-${GTK3_VERSION}.tar.xz"
GTK3_SHA256="7e04f0648515034b806b74ae5d774d87cffb1a2a96c468cb5be476d51bf2f3c7"

HICOLOR_VERSION="0.18"
HICOLOR_URL="https://icon-theme.freedesktop.org/releases/hicolor-icon-theme-${HICOLOR_VERSION}.tar.xz"
HICOLOR_SHA256="db0e50a80aa3bf64bb45cbca5cf9f75efd9348cf2ac690b907435238c3cf81d7"

XFCE_URL="https://archive.xfce.org/src/xfce"

LIBXFCE4UTIL_VERSION="4.20.0"
LIBXFCE4UTIL_URL="${XFCE_URL}/libxfce4util/4.20/libxfce4util-${LIBXFCE4UTIL_VERSION}.tar.bz2"
LIBXFCE4UTIL_SHA256="21493f9c9995a282823db93839f6b9f06ae31edb094191ba9acf04d932a2b592"

XFCONF_VERSION="4.20.0"
XFCONF_URL="${XFCE_URL}/xfconf/4.20/xfconf-${XFCONF_VERSION}.tar.bz2"
XFCONF_SHA256="8bc43c60f1716b13cf35fc899e2a36ea9c6cdc3478a8f051220eef0f53567efd"

LIBXFCE4UI_VERSION="4.20.0"
LIBXFCE4UI_URL="${XFCE_URL}/libxfce4ui/4.20/libxfce4ui-${LIBXFCE4UI_VERSION}.tar.bz2"
LIBXFCE4UI_SHA256="75e8996984f20375aadecd5c16f5147c211ed0bd26d7861ab0257561eb76eaee"

EXO_VERSION="4.20.0"
EXO_URL="${XFCE_URL}/exo/4.20/exo-${EXO_VERSION}.tar.bz2"
EXO_SHA256="4277f799245f1efde01cd917fd538ba6b12cf91c9f8a73fe2035fd5456ec078d"

THUNAR_VERSION="4.20.0"
THUNAR_URL="${XFCE_URL}/thunar/4.20/thunar-${THUNAR_VERSION}.tar.bz2"
THUNAR_SHA256="27731a76f3aecf3752b1ca35afad89e264c52244f70083d933507dd4a17548b0"

ADWAITA_LEGACY_VERSION="46.2"
ADWAITA_LEGACY_URL="https://download.gnome.org/sources/adwaita-icon-theme-legacy/46/adwaita-icon-theme-legacy-${ADWAITA_LEGACY_VERSION}.tar.xz"
ADWAITA_LEGACY_SHA256="548480f58589a54b72d18833b755b15ffbd567e3187249d74e2e1f8f99f22fb4"

SHARED_MIME_INFO_VERSION="2.4"
SHARED_MIME_INFO_URL="https://gitlab.freedesktop.org/xdg/shared-mime-info/-/archive/${SHARED_MIME_INFO_VERSION}/shared-mime-info-${SHARED_MIME_INFO_VERSION}.tar.gz"
SHARED_MIME_INFO_SHA256="531291d0387eb94e16e775d7e73788d06d2b2fdd8cd2ac6b6b15287593b6a2de"

DBUS_VERSION="1.16.2"
DBUS_URL="https://dbus.freedesktop.org/releases/dbus/dbus-${DBUS_VERSION}.tar.xz"
DBUS_SHA256="0ba2a1a4b16afe7bceb2c07e9ce99a8c2c3508e5dec290dbb643384bd6beb7e2"

# wlroots + labwc stack (scripts/build_wlroots.sh).
LIBEVDEV_VERSION="1.13.3"
LIBEVDEV_URL="https://gitlab.freedesktop.org/libevdev/libevdev/-/archive/libevdev-${LIBEVDEV_VERSION}/libevdev-libevdev-${LIBEVDEV_VERSION}.tar.gz"
LIBEVDEV_SHA256="39e431563f0b35e4794b3af3f61bc05f81ef5d25f10238af9768ddf4589abef7"

MTDEV_VERSION="1.1.7"
MTDEV_URL="https://bitmath.org/code/mtdev/mtdev-${MTDEV_VERSION}.tar.bz2"
MTDEV_SHA256="a107adad2101fecac54ac7f9f0e0a0dd155d954193da55c2340c97f2ff1d814e"

LIBUDEV_ZERO_VERSION="1.0.3"
LIBUDEV_ZERO_URL="https://github.com/illiliti/libudev-zero/archive/refs/tags/${LIBUDEV_ZERO_VERSION}.tar.gz"
LIBUDEV_ZERO_SHA256="0bd89b657d62d019598e6c7ed726ff8fed80e8ba092a83b484d66afb80b77da5"

LIBINPUT_VERSION="1.27.1"
LIBINPUT_URL="https://gitlab.freedesktop.org/libinput/libinput/-/archive/${LIBINPUT_VERSION}/libinput-${LIBINPUT_VERSION}.tar.gz"
LIBINPUT_SHA256="f6d623dd8230db337a6457645ebca96b9d4788a56385463bb14b8174910dfe23"

SEATD_VERSION="0.9.1"
SEATD_URL="https://git.sr.ht/~kennylevinsen/seatd/archive/${SEATD_VERSION}.tar.gz"
SEATD_SHA256="819979c922a0be258aed133d93920bce6a3d3565a60588d6d372ce9db2712cd3"

HWDATA_VERSION="0.390"
HWDATA_URL="https://github.com/vcrhonek/hwdata/archive/refs/tags/v${HWDATA_VERSION}.tar.gz"
HWDATA_SHA256="f10684093d2a7780de8a96d3dd8a2dd544ed6136dd359197750c42bb08ce526f"

LIBDISPLAY_INFO_VERSION="0.2.0"
LIBDISPLAY_INFO_URL="https://gitlab.freedesktop.org/emersion/libdisplay-info/-/releases/${LIBDISPLAY_INFO_VERSION}/downloads/libdisplay-info-${LIBDISPLAY_INFO_VERSION}.tar.xz"
LIBDISPLAY_INFO_SHA256="5a2f002a16f42dd3540c8846f80a90b8f4bdcd067a94b9d2087bc2feae974176"

WLROOTS_VERSION="0.18.2"
WLROOTS_URL="https://gitlab.freedesktop.org/wlroots/wlroots/-/releases/${WLROOTS_VERSION}/downloads/wlroots-${WLROOTS_VERSION}.tar.gz"
WLROOTS_SHA256="cf776c169169f279808d9eabc6583f484338dcd454c966a285ff178c88c105d4"

LIBXML2_VERSION="2.13.5"
LIBXML2_URL="https://download.gnome.org/sources/libxml2/2.13/libxml2-${LIBXML2_VERSION}.tar.xz"
LIBXML2_SHA256="74fc163217a3964257d3be39af943e08861263c4231f9ef5b496b6f6d4c7b2b6"

LABWC_VERSION="0.8.4"
LABWC_URL="https://github.com/labwc/labwc/archive/refs/tags/${LABWC_VERSION}.tar.gz"
LABWC_SHA256="2d3ded90f78efb5060f7057ea802c78a79dc9b2e82ae7a2ad902af957b8b9797"

FOOT_VERSION="1.18.1"
FOOT_URL="https://codeberg.org/dnkl/foot/archive/${FOOT_VERSION}.tar.gz"
FOOT_SHA256="f1438298117b1346405f175e8a052ba9828a610e061971cc8dde9153939344c5"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --output-dir)
      OUTPUT_DIR="$2"
      shift 2
      ;;
    --force)
      FORCE=1
      shift
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      echo "Unknown option: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

mkdir -p "${OUTPUT_DIR}"
OUTPUT_DIR="$(cd "${OUTPUT_DIR}" && pwd)"

fetch_one() {
  local name="$1" url="$2" sha="$3" dest="$4" archive_name="$5"
  local archive="${OUTPUT_DIR}/${archive_name}"

  if [[ -d "${dest}" && "${FORCE}" -eq 0 ]]; then
    echo "${name} source already present at ${dest}"
    return 0
  fi
  rm -rf "${dest}"

  if [[ ! -f "${archive}" ]]; then
    echo "Downloading ${name} from ${url}"
    curl -fsSL -o "${archive}.tmp" "${url}"
    mv "${archive}.tmp" "${archive}"
  fi

  local got
  got="$(sha256sum "${archive}" | awk '{print $1}')"
  if [[ "${got}" != "${sha}" ]]; then
    echo "${name} checksum mismatch: expected ${sha}, got ${got}" >&2
    rm -f "${archive}"
    exit 1
  fi

  local tmp
  tmp="$(mktemp -d "${OUTPUT_DIR}/.extract.XXXXXX")"
  tar -xf "${archive}" -C "${tmp}"
  local top
  top="$(find "${tmp}" -mindepth 1 -maxdepth 1 -type d | head -n 1)"
  mv "${top}" "${dest}"
  rm -rf "${tmp}"
  echo "${name} ready at ${dest}"
}

fetch_one "libwayland ${WAYLAND_VERSION}" "${WAYLAND_URL}" "${WAYLAND_SHA256}" \
  "${OUTPUT_DIR}/wayland-src" "wayland-${WAYLAND_VERSION}.tar.xz"
fetch_one "libffi ${LIBFFI_VERSION}" "${LIBFFI_URL}" "${LIBFFI_SHA256}" \
  "${OUTPUT_DIR}/libffi-src" "libffi-${LIBFFI_VERSION}.tar.gz"
fetch_one "wayland-protocols ${PROTOCOLS_VERSION}" "${PROTOCOLS_URL}" "${PROTOCOLS_SHA256}" \
  "${OUTPUT_DIR}/wayland-protocols-src" "wayland-protocols-${PROTOCOLS_VERSION}.tar.xz"
fetch_one "libxkbcommon ${XKBCOMMON_VERSION}" "${XKBCOMMON_URL}" "${XKBCOMMON_SHA256}" \
  "${OUTPUT_DIR}/libxkbcommon-src" "libxkbcommon-${XKBCOMMON_VERSION}.tar.xz"
fetch_one "libdrm ${LIBDRM_VERSION}" "${LIBDRM_URL}" "${LIBDRM_SHA256}" \
  "${OUTPUT_DIR}/libdrm-src" "libdrm-${LIBDRM_VERSION}.tar.xz"
fetch_one "pixman ${PIXMAN_VERSION}" "${PIXMAN_URL}" "${PIXMAN_SHA256}" \
  "${OUTPUT_DIR}/pixman-src" "pixman-${PIXMAN_VERSION}.tar.gz"
fetch_one "expat ${EXPAT_VERSION}" "${EXPAT_URL}" "${EXPAT_SHA256}" \
  "${OUTPUT_DIR}/expat-src" "expat-${EXPAT_VERSION}.tar.xz"
fetch_one "freetype ${FREETYPE_VERSION}" "${FREETYPE_URL}" "${FREETYPE_SHA256}" \
  "${OUTPUT_DIR}/freetype-src" "freetype-${FREETYPE_VERSION}.tar.xz"
fetch_one "fontconfig ${FONTCONFIG_VERSION}" "${FONTCONFIG_URL}" "${FONTCONFIG_SHA256}" \
  "${OUTPUT_DIR}/fontconfig-src" "fontconfig-${FONTCONFIG_VERSION}.tar.xz"
fetch_one "DejaVu fonts ${DEJAVU_VERSION}" "${DEJAVU_URL}" "${DEJAVU_SHA256}" \
  "${OUTPUT_DIR}/dejavu-src" "dejavu-fonts-ttf-${DEJAVU_VERSION}.tar.bz2"
fetch_one "tllist ${TLLIST_VERSION}" "${TLLIST_URL}" "${TLLIST_SHA256}" \
  "${OUTPUT_DIR}/tllist-src" "tllist-${TLLIST_VERSION}.tar.gz"
fetch_one "fcft ${FCFT_VERSION}" "${FCFT_URL}" "${FCFT_SHA256}" \
  "${OUTPUT_DIR}/fcft-src" "fcft-${FCFT_VERSION}.tar.gz"
fetch_one "harfbuzz ${HARFBUZZ_VERSION}" "${HARFBUZZ_URL}" "${HARFBUZZ_SHA256}"   "${OUTPUT_DIR}/harfbuzz-src" "harfbuzz-${HARFBUZZ_VERSION}.tar.xz"
fetch_one "utf8proc ${UTF8PROC_VERSION}" "${UTF8PROC_URL}" "${UTF8PROC_SHA256}"   "${OUTPUT_DIR}/utf8proc-src" "utf8proc-${UTF8PROC_VERSION}.tar.gz"
fetch_one "zlib ${ZLIB_VERSION}" "${ZLIB_URL}" "${ZLIB_SHA256}" \
  "${OUTPUT_DIR}/zlib-src" "zlib-${ZLIB_VERSION}.tar.gz"
fetch_one "libpng ${LIBPNG_VERSION}" "${LIBPNG_URL}" "${LIBPNG_SHA256}" \
  "${OUTPUT_DIR}/libpng-src" "libpng-${LIBPNG_VERSION}.tar.xz"
fetch_one "pcre2 ${PCRE2_VERSION}" "${PCRE2_URL}" "${PCRE2_SHA256}" \
  "${OUTPUT_DIR}/pcre2-src" "pcre2-${PCRE2_VERSION}.tar.bz2"
fetch_one "GLib ${GLIB_VERSION}" "${GLIB_URL}" "${GLIB_SHA256}" \
  "${OUTPUT_DIR}/glib-src" "glib-${GLIB_VERSION}.tar.xz"
fetch_one "fribidi ${FRIBIDI_VERSION}" "${FRIBIDI_URL}" "${FRIBIDI_SHA256}" \
  "${OUTPUT_DIR}/fribidi-src" "fribidi-${FRIBIDI_VERSION}.tar.xz"
fetch_one "cairo ${CAIRO_VERSION}" "${CAIRO_URL}" "${CAIRO_SHA256}" \
  "${OUTPUT_DIR}/cairo-src" "cairo-${CAIRO_VERSION}.tar.xz"
fetch_one "pango ${PANGO_VERSION}" "${PANGO_URL}" "${PANGO_SHA256}" \
  "${OUTPUT_DIR}/pango-src" "pango-${PANGO_VERSION}.tar.xz"
fetch_one "EGL-Registry ${EGL_REGISTRY_COMMIT:0:12}" "${EGL_REGISTRY_URL}" "${EGL_REGISTRY_SHA256}" \
  "${OUTPUT_DIR}/egl-registry-src" "egl-registry-${EGL_REGISTRY_COMMIT}.tar.gz"
fetch_one "libepoxy ${LIBEPOXY_VERSION}" "${LIBEPOXY_URL}" "${LIBEPOXY_SHA256}" \
  "${OUTPUT_DIR}/libepoxy-src" "libepoxy-${LIBEPOXY_VERSION}.tar.gz"
fetch_one "ATK ${ATK_VERSION}" "${ATK_URL}" "${ATK_SHA256}" \
  "${OUTPUT_DIR}/atk-src" "atk-${ATK_VERSION}.tar.xz"
fetch_one "gdk-pixbuf ${GDK_PIXBUF_VERSION}" "${GDK_PIXBUF_URL}" "${GDK_PIXBUF_SHA256}" \
  "${OUTPUT_DIR}/gdk-pixbuf-src" "gdk-pixbuf-${GDK_PIXBUF_VERSION}.tar.xz"
fetch_one "GTK ${GTK3_VERSION}" "${GTK3_URL}" "${GTK3_SHA256}" \
  "${OUTPUT_DIR}/gtk-src" "gtk+-${GTK3_VERSION}.tar.xz"
fetch_one "hicolor-icon-theme ${HICOLOR_VERSION}" "${HICOLOR_URL}" "${HICOLOR_SHA256}" \
  "${OUTPUT_DIR}/hicolor-icon-theme-src" "hicolor-icon-theme-${HICOLOR_VERSION}.tar.xz"
fetch_one "libxfce4util ${LIBXFCE4UTIL_VERSION}" "${LIBXFCE4UTIL_URL}" "${LIBXFCE4UTIL_SHA256}" \
  "${OUTPUT_DIR}/libxfce4util-src" "libxfce4util-${LIBXFCE4UTIL_VERSION}.tar.bz2"
fetch_one "xfconf ${XFCONF_VERSION}" "${XFCONF_URL}" "${XFCONF_SHA256}" \
  "${OUTPUT_DIR}/xfconf-src" "xfconf-${XFCONF_VERSION}.tar.bz2"
fetch_one "libxfce4ui ${LIBXFCE4UI_VERSION}" "${LIBXFCE4UI_URL}" "${LIBXFCE4UI_SHA256}" \
  "${OUTPUT_DIR}/libxfce4ui-src" "libxfce4ui-${LIBXFCE4UI_VERSION}.tar.bz2"
fetch_one "exo ${EXO_VERSION}" "${EXO_URL}" "${EXO_SHA256}" \
  "${OUTPUT_DIR}/exo-src" "exo-${EXO_VERSION}.tar.bz2"
fetch_one "Thunar ${THUNAR_VERSION}" "${THUNAR_URL}" "${THUNAR_SHA256}" \
  "${OUTPUT_DIR}/thunar-src" "thunar-${THUNAR_VERSION}.tar.bz2"
fetch_one "adwaita-icon-theme-legacy ${ADWAITA_LEGACY_VERSION}" "${ADWAITA_LEGACY_URL}" "${ADWAITA_LEGACY_SHA256}" \
  "${OUTPUT_DIR}/adwaita-icon-theme-legacy-src" "adwaita-icon-theme-legacy-${ADWAITA_LEGACY_VERSION}.tar.xz"
fetch_one "shared-mime-info ${SHARED_MIME_INFO_VERSION}" "${SHARED_MIME_INFO_URL}" "${SHARED_MIME_INFO_SHA256}" \
  "${OUTPUT_DIR}/shared-mime-info-src" "shared-mime-info-${SHARED_MIME_INFO_VERSION}.tar.gz"
fetch_one "dbus ${DBUS_VERSION}" "${DBUS_URL}" "${DBUS_SHA256}"   "${OUTPUT_DIR}/dbus-src" "dbus-${DBUS_VERSION}.tar.xz"
fetch_one "libevdev ${LIBEVDEV_VERSION}" "${LIBEVDEV_URL}" "${LIBEVDEV_SHA256}" \
  "${OUTPUT_DIR}/libevdev-src" "libevdev-libevdev-${LIBEVDEV_VERSION}.tar.gz"
fetch_one "mtdev ${MTDEV_VERSION}" "${MTDEV_URL}" "${MTDEV_SHA256}" \
  "${OUTPUT_DIR}/mtdev-src" "mtdev-${MTDEV_VERSION}.tar.bz2"
fetch_one "libudev-zero ${LIBUDEV_ZERO_VERSION}" "${LIBUDEV_ZERO_URL}" "${LIBUDEV_ZERO_SHA256}" \
  "${OUTPUT_DIR}/libudev-zero-src" "libudev-zero-${LIBUDEV_ZERO_VERSION}.tar.gz"
fetch_one "libinput ${LIBINPUT_VERSION}" "${LIBINPUT_URL}" "${LIBINPUT_SHA256}" \
  "${OUTPUT_DIR}/libinput-src" "libinput-${LIBINPUT_VERSION}.tar.gz"
fetch_one "seatd ${SEATD_VERSION}" "${SEATD_URL}" "${SEATD_SHA256}" \
  "${OUTPUT_DIR}/seatd-src" "seatd-${SEATD_VERSION}.tar.gz"
fetch_one "hwdata ${HWDATA_VERSION}" "${HWDATA_URL}" "${HWDATA_SHA256}" \
  "${OUTPUT_DIR}/hwdata-src" "hwdata-${HWDATA_VERSION}.tar.gz"
fetch_one "libdisplay-info ${LIBDISPLAY_INFO_VERSION}" "${LIBDISPLAY_INFO_URL}" "${LIBDISPLAY_INFO_SHA256}" \
  "${OUTPUT_DIR}/libdisplay-info-src" "libdisplay-info-${LIBDISPLAY_INFO_VERSION}.tar.xz"
fetch_one "wlroots ${WLROOTS_VERSION}" "${WLROOTS_URL}" "${WLROOTS_SHA256}" \
  "${OUTPUT_DIR}/wlroots-src" "wlroots-${WLROOTS_VERSION}.tar.gz"
fetch_one "libxml2 ${LIBXML2_VERSION}" "${LIBXML2_URL}" "${LIBXML2_SHA256}" \
  "${OUTPUT_DIR}/libxml2-src" "libxml2-${LIBXML2_VERSION}.tar.xz"
fetch_one "labwc ${LABWC_VERSION}" "${LABWC_URL}" "${LABWC_SHA256}" \
  "${OUTPUT_DIR}/labwc-src" "labwc-${LABWC_VERSION}.tar.gz"
fetch_one "foot ${FOOT_VERSION}" "${FOOT_URL}" "${FOOT_SHA256}" \
  "${OUTPUT_DIR}/foot-src" "foot-${FOOT_VERSION}.tar.gz"
