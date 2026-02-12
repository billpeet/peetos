#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BASE_ROOT="${ROOT_DIR}/user/ext2_root"
OUT_IMG="${ROOT_DIR}/user/sample_ext2_musl.img"

if ! command -v mke2fs >/dev/null 2>&1; then
  echo "mke2fs not found; install e2fsprogs" >&2
  exit 1
fi

if [[ ! -d "${BASE_ROOT}" ]]; then
  echo "base ext2 root not found: ${BASE_ROOT}" >&2
  exit 1
fi

TMP_DIR="$(mktemp -d)"
cleanup() {
  rm -rf "${TMP_DIR}"
}
trap cleanup EXIT

STAGE_ROOT="${TMP_DIR}/ext2_root"
mkdir -p "${STAGE_ROOT}"
cp -a "${BASE_ROOT}/." "${STAGE_ROOT}/"

mkdir -p "${STAGE_ROOT}/bin"
"${ROOT_DIR}/scripts/build_toybox_echo.sh" \
  --out "${STAGE_ROOT}/bin/echo"
chmod 0755 "${STAGE_ROOT}/bin/echo"

TMP_IMG="${TMP_DIR}/ext2.img"
# Keep image small enough for stage2 kernel staging when embedded.
mke2fs -q -t ext2 -F -b 1024 -I 256 -N 96 -L PEETOS_EXT2 -d "${STAGE_ROOT}" "${TMP_IMG}" 128

mv "${TMP_IMG}" "${OUT_IMG}"
chmod 0644 "${OUT_IMG}"

echo "Built ${OUT_IMG}"
