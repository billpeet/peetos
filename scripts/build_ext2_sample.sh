#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SRC_DIR="${ROOT_DIR}/user/ext2_root"
OUT_IMG="${ROOT_DIR}/user/sample_ext2.img"

if ! command -v mke2fs >/dev/null 2>&1; then
  echo "mke2fs not found; install e2fsprogs" >&2
  exit 1
fi

TMP_IMG="$(mktemp)"
cleanup() {
  rm -f "${TMP_IMG}"
}
trap cleanup EXIT

# Keep the embedded image small enough for the current stage bootloader path.
# 128 KiB ext2 image, 1 KiB blocks, and explicit inode count so create/mkdir tests have space.
mke2fs -q -t ext2 -F -b 1024 -I 256 -N 64 -L PEETOS_EXT2 -d "${SRC_DIR}" "${TMP_IMG}" 128
mv "${TMP_IMG}" "${OUT_IMG}"
chmod 0644 "${OUT_IMG}"

echo "Built ${OUT_IMG}"
