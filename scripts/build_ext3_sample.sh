#!/usr/bin/env bash
# Build user/sample_ext3.img: a small image formatted with the *stock*
# `mke2fs -t ext3` feature set (has_journal, ext_attr, resize_inode,
# dir_index, filetype, sparse_super, large_file) plus on-disk structures that
# exercise each of those features. Host unit tests in crates/fs mount it to
# prove real ext3 images work, not just hand-patched ext2 images.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_IMG="${ROOT_DIR}/user/sample_ext3.img"

for tool in mke2fs debugfs e2fsck; do
  if ! command -v "${tool}" >/dev/null 2>&1; then
    echo "${tool} not found; install e2fsprogs" >&2
    exit 1
  fi
done

WORK_DIR="$(mktemp -d)"
cleanup() {
  rm -rf "${WORK_DIR}"
}
trap cleanup EXIT

STAGE="${WORK_DIR}/root"
mkdir -p "${STAGE}/bigdir" "${STAGE}/sub"
printf 'hello ext3\n' > "${STAGE}/hello.txt"
printf 'xattr block file\n' > "${STAGE}/xattr_block.txt"
printf 'xattr inline file\n' > "${STAGE}/xattr_inline.txt"
printf 'nested\n' > "${STAGE}/sub/nested.txt"
# Multi-block directory so `e2fsck -D` turns it into an htree (INDEX_FL).
# Stay below the VFS per-directory child cap (VFS_MAX_CHILDREN).
for i in $(seq 1 50); do
  printf '%d\n' "${i}" > "${STAGE}/bigdir/entry_with_a_fairly_long_name_$(printf %03d "${i}")"
done
# Sparse 5 GiB file: needs i_size_high (large_file).
truncate -s 5G "${STAGE}/large.bin"
printf 'tail' | dd of="${STAGE}/large.bin" bs=1 seek=$((5 * 1024 * 1024 * 1024 - 4)) \
  conv=notrunc status=none
# Short target => fast symlink (target stored inline in i_block).
ln -s hello.txt "${STAGE}/fastlink"

IMG="${WORK_DIR}/ext3.img"
# Stock ext3 features: no -O overrides. 1 KiB blocks keep the image small;
# 2048 blocks is the smallest size that fits the 1024-block journal.
mke2fs -q -t ext3 -F -b 1024 -N 192 -L PEETOS_EXT3 -d "${STAGE}" "${IMG}" 2048

# A value too large for the in-inode xattr area forces an external xattr block
# (i_file_acl); a short value stays in the 256-byte inode's extra space.
head -c 600 /dev/zero | tr '\0' 'B' > "${WORK_DIR}/big.val"
debugfs -w "${IMG}" -R "ea_set -f ${WORK_DIR}/big.val /xattr_block.txt user.peetos_block" \
  >/dev/null 2>&1
debugfs -w "${IMG}" -R "ea_set /xattr_inline.txt user.peetos small" >/dev/null 2>&1

# Build htree indexes for multi-block directories, then verify cleanliness.
e2fsck -fyD "${IMG}" >/dev/null 2>&1 || true
e2fsck -fn "${IMG}" >/dev/null

mv "${IMG}" "${OUT_IMG}"
chmod 0644 "${OUT_IMG}"
echo "Built ${OUT_IMG}"
