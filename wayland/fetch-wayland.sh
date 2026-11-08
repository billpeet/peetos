#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: fetch-wayland.sh [options]

Download, verify, and extract pinned libwayland, libffi, wayland-protocols,
libxkbcommon and libdrm source releases into wayland-src/, libffi-src/,
wayland-protocols-src/, libxkbcommon-src/ and libdrm-src/.

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
