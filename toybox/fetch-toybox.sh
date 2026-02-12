#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: fetch-toybox.sh [options]

Download and extract a pinned Toybox source release into toybox-src.

Options:
  --version <ver>      Toybox version to fetch (default: 0.8.13)
  --url <url>          override download URL
  --archive <path>     use an existing local tarball instead of downloading
  --output-dir <dir>   destination root for toybox-src (default: script dir)
  --source-dir <name>  extracted source directory name (default: toybox-src)
  --force              remove existing source directory first
  --help               show this help
USAGE
}

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
VERSION="0.8.13"
OUTPUT_DIR="${SCRIPT_DIR}"
SOURCE_DIR_NAME="toybox-src"
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
  URL="https://landley.net/toybox/downloads/toybox-${VERSION}.tar.gz"
fi

if [[ -d "${SOURCE_DIR}" ]]; then
  if [[ "${FORCE}" -eq 1 ]]; then
    rm -rf "${SOURCE_DIR}"
  else
    echo "Toybox source already exists at ${SOURCE_DIR}"
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
  ARCHIVE_TO_EXTRACT="${WORK_DIR}/toybox-${VERSION}.tar.gz"
  echo "Downloading Toybox ${VERSION} from ${URL}"
  if command -v curl >/dev/null 2>&1; then
    curl --fail --location --output "${ARCHIVE_TO_EXTRACT}" "${URL}"
  elif command -v wget >/dev/null 2>&1; then
    wget -O "${ARCHIVE_TO_EXTRACT}" "${URL}"
  else
    echo "Neither curl nor wget is available" >&2
    exit 1
  fi
fi

tar -xzf "${ARCHIVE_TO_EXTRACT}" -C "${WORK_DIR}"

EXTRACTED_DIR=""
if [[ -d "${WORK_DIR}/toybox-${VERSION}" ]]; then
  EXTRACTED_DIR="${WORK_DIR}/toybox-${VERSION}"
else
  EXTRACTED_DIR="$(find "${WORK_DIR}" -mindepth 1 -maxdepth 1 -type d | head -n 1)"
fi

if [[ -z "${EXTRACTED_DIR}" || ! -d "${EXTRACTED_DIR}" ]]; then
  echo "Failed to locate extracted Toybox source directory" >&2
  exit 1
fi

mv "${EXTRACTED_DIR}" "${SOURCE_DIR}"

echo "Toybox source ready: ${SOURCE_DIR}"
