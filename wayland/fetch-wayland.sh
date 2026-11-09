#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: fetch-wayland.sh [options]

Download, verify, and extract the pinned source releases of the PeetOS
graphics userland into <name>-src/ directories: libwayland, libffi,
wayland-protocols, libxkbcommon and libdrm, plus the software rendering stack
(pixman, expat, freetype, fontconfig, harfbuzz, utf8proc), the DejaVu
fonts, and tllist, fcft and the foot terminal.

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
fetch_one "foot ${FOOT_VERSION}" "${FOOT_URL}" "${FOOT_SHA256}" \
  "${OUTPUT_DIR}/foot-src" "foot-${FOOT_VERSION}.tar.gz"
