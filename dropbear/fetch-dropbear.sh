#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: fetch-dropbear.sh [options]

Download and extract a pinned Dropbear source release into dropbear-src.

Options:
  --version <ver>      Dropbear version to fetch (default: 2024.86)
  --url <url>          override download URL
  --archive <path>     use an existing local archive instead of downloading
  --output-dir <dir>   destination root for dropbear-src (default: script dir)
  --source-dir <name>  extracted source directory name (default: dropbear-src)
  --force              remove existing source directory first
  --help               show this help
USAGE
}

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
VERSION="2024.86"
OUTPUT_DIR="${SCRIPT_DIR}"
SOURCE_DIR_NAME="dropbear-src"
ARCHIVE_PATH=""
URL=""
FORCE=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --version)
      VERSION="$2"
      shift 2
      ;;
    --url)
      URL="$2"
      shift 2
      ;;
    --archive)
      ARCHIVE_PATH="$2"
      shift 2
      ;;
    --output-dir)
      OUTPUT_DIR="$2"
      shift 2
      ;;
    --source-dir)
      SOURCE_DIR_NAME="$2"
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
SOURCE_DIR="${OUTPUT_DIR}/${SOURCE_DIR_NAME}"

if [[ -z "${URL}" ]]; then
  URL="https://matt.ucc.asn.au/dropbear/releases/dropbear-${VERSION}.tar.bz2"
fi

if [[ -d "${SOURCE_DIR}" ]]; then
  if [[ "${FORCE}" -eq 1 ]]; then
    rm -rf "${SOURCE_DIR}"
  else
    echo "Dropbear source already exists at ${SOURCE_DIR}"
    echo "Use --force to replace it"
    exit 0
  fi
fi

WORK_DIR="$(mktemp -d)"
cleanup() {
  rm -rf "${WORK_DIR}"
}
trap cleanup EXIT

if [[ -n "${ARCHIVE_PATH}" ]]; then
  if [[ ! -f "${ARCHIVE_PATH}" ]]; then
    echo "Archive not found: ${ARCHIVE_PATH}" >&2
    exit 1
  fi
  ARCHIVE_TO_EXTRACT="${ARCHIVE_PATH}"
else
  ARCHIVE_TO_EXTRACT="${WORK_DIR}/dropbear-${VERSION}.tar.bz2"
  echo "Downloading Dropbear ${VERSION} from ${URL}"
  if command -v curl >/dev/null 2>&1; then
    curl --fail --location --output "${ARCHIVE_TO_EXTRACT}" "${URL}"
  elif command -v wget >/dev/null 2>&1; then
    wget -O "${ARCHIVE_TO_EXTRACT}" "${URL}"
  else
    echo "Neither curl nor wget is available" >&2
    exit 1
  fi
fi

tar -xf "${ARCHIVE_TO_EXTRACT}" -C "${WORK_DIR}"

EXTRACTED_DIR=""
if [[ -d "${WORK_DIR}/dropbear-${VERSION}" ]]; then
  EXTRACTED_DIR="${WORK_DIR}/dropbear-${VERSION}"
else
  EXTRACTED_DIR="$(find "${WORK_DIR}" -mindepth 1 -maxdepth 1 -type d | head -n 1)"
fi

if [[ -z "${EXTRACTED_DIR}" || ! -d "${EXTRACTED_DIR}" ]]; then
  echo "Failed to locate extracted Dropbear source directory" >&2
  exit 1
fi

mv "${EXTRACTED_DIR}" "${SOURCE_DIR}"

echo "Dropbear source ready: ${SOURCE_DIR}"
