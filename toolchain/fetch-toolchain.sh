#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: fetch-toolchain.sh [options]

Download, verify, and extract the pinned sources of the PeetOS C/C++ cross
toolchain into <name>-src/ directories: GCC, plus the GMP, MPFR and MPC
libraries GCC needs, which are linked into the GCC source tree so they are
built in-tree.

Options:
  --output-dir <dir>   destination root (default: script dir)
  --force              remove existing source directories first
  --help               show this help
USAGE
}

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OUTPUT_DIR="${SCRIPT_DIR}"
FORCE=0

GCC_VERSION="15.2.0"
GCC_URL="https://ftp.gnu.org/gnu/gcc/gcc-${GCC_VERSION}/gcc-${GCC_VERSION}.tar.xz"
GCC_SHA256="438fd996826b0c82485a29da03a72d71d6e3541a83ec702df4271f6fe025d24e"

GMP_VERSION="6.3.0"
GMP_URL="https://ftp.gnu.org/gnu/gmp/gmp-${GMP_VERSION}.tar.xz"
GMP_SHA256="a3c2b80201b89e68616f4ad30bc66aee4927c3ce50e33929ca819d5c43538898"

MPFR_VERSION="4.2.2"
MPFR_URL="https://ftp.gnu.org/gnu/mpfr/mpfr-${MPFR_VERSION}.tar.xz"
MPFR_SHA256="b67ba0383ef7e8a8563734e2e889ef5ec3c3b898a01d00fa0a6869ad81c6ce01"

MPC_VERSION="1.3.1"
MPC_URL="https://ftp.gnu.org/gnu/mpc/mpc-${MPC_VERSION}.tar.gz"
MPC_SHA256="ab642492f5cf882b74aa0cb730cd410a81edcdbec895183ce930e706c1c759b8"

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

fetch_one "GCC ${GCC_VERSION}" "${GCC_URL}" "${GCC_SHA256}" \
  "${OUTPUT_DIR}/gcc-src" "gcc-${GCC_VERSION}.tar.xz"
fetch_one "GMP ${GMP_VERSION}" "${GMP_URL}" "${GMP_SHA256}" \
  "${OUTPUT_DIR}/gmp-src" "gmp-${GMP_VERSION}.tar.xz"
fetch_one "MPFR ${MPFR_VERSION}" "${MPFR_URL}" "${MPFR_SHA256}" \
  "${OUTPUT_DIR}/mpfr-src" "mpfr-${MPFR_VERSION}.tar.xz"
fetch_one "MPC ${MPC_VERSION}" "${MPC_URL}" "${MPC_SHA256}" \
  "${OUTPUT_DIR}/mpc-src" "mpc-${MPC_VERSION}.tar.gz"

# GCC's top-level configure builds gmp/mpfr/mpc in-tree when they appear as
# subdirectories of its source tree.
for lib in gmp mpfr mpc; do
  ln -sfn "../${lib}-src" "${OUTPUT_DIR}/gcc-src/${lib}"
done
