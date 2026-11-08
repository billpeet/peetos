#!/usr/bin/env bash
# Build user/sample_ext4.img: a small image formatted with the *stock*
# `mke2fs -t ext4` feature set (e2fsprogs 1.47: has_journal ext_attr
# resize_inode dir_index filetype extent 64bit flex_bg sparse_super
# large_file huge_file dir_nlink extra_isize metadata_csum) plus structures
# the ext4 milestones need to exercise (docs/ext4_compat_plan.md):
#   - frag.bin: 20 extents separated by holes -> depth-1 extent tree
#     (index in the inode, one checksummed extent leaf block)
#   - bigdir/: two-block directory turned into an htree by `e2fsck -D`
#     (names and entry count stay inside the VFS name/child limits)
#   - xattr_block.txt: external xattr block; fastlink: inline symlink;
#     large.bin: sparse 5 GiB file (large_file / huge_file territory)
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_IMG="${ROOT_DIR}/user/sample_ext4.img"

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
printf 'hello ext4\n' > "${STAGE}/hello.txt"
printf 'nested\n' > "${STAGE}/sub/nested.txt"
printf 'xattr block file\n' > "${STAGE}/xattr_block.txt"

# 62 entries x 60-character names spill into a second 4 KiB block.
for i in $(seq 1 62); do
  name="$(printf 'entry_%03d_' "${i}")"
  while [[ ${#name} -lt 60 ]]; do name="${name}x"; done
  printf '%d\n' "${i}" > "${STAGE}/bigdir/${name}"
done

# Data in every other 4 KiB block: 20 separate extents.
: > "${STAGE}/frag.bin"
for i in $(seq 0 19); do
  printf 'fragment %02d\n' "${i}" | dd of="${STAGE}/frag.bin" bs=4096 seek=$((i * 2)) \
    conv=notrunc status=none
done

truncate -s 5G "${STAGE}/large.bin"
printf 'tail' | dd of="${STAGE}/large.bin" bs=1 seek=$((5 * 1024 * 1024 * 1024 - 4)) \
  conv=notrunc status=none
ln -s hello.txt "${STAGE}/fastlink"

head -c 600 /dev/zero | tr '\0' 'B' > "${WORK_DIR}/big.val"

# build_image <output> [extra mke2fs -O features]
build_image() {
  local out="$1"
  local features="${2:-}"
  local img="${WORK_DIR}/ext4.img"
  local extra=()
  if [[ -n "${features}" ]]; then
    extra=(-O "${features}")
  fi
  # Stock ext4 features unless `features` overrides some. `-g 1024` (blocks
  # per group, a layout parameter, not a feature) splits the 16 MiB image into
  # four groups so multi-group descriptors, flex_bg packing (all bitmaps and
  # inode tables in group 0) and INODE_UNINIT/BLOCK_UNINIT groups are present;
  # lazy_itable_init=1 keeps the lazy-init flags real mkfs.ext4 images carry.
  mke2fs -q -t ext4 -F -g 1024 -N 512 -L PEETOS_EXT4 -E lazy_itable_init=1 \
    "${extra[@]}" -d "${STAGE}" "${img}" 16M
  debugfs -w "${img}" -R "ea_set -f ${WORK_DIR}/big.val /xattr_block.txt user.peetos_block" \
    >/dev/null 2>&1
  e2fsck -fyD "${img}" >/dev/null 2>&1 || true
  e2fsck -fn "${img}" >/dev/null
  mv "${img}" "${out}"
  chmod 0644 "${out}"
  echo "Built ${out}"
}

# Stock image (metadata_csum: crc32c everywhere).
build_image "${OUT_IMG}"
# Pre-metadata_csum ext4 (the e2fsprogs < 1.43 default): uninit_bg gives crc16
# group descriptor checksums and lazy-init groups without crc32c, the M2
# target format. (On 1.47, ^metadata_csum alone does not add uninit_bg.)
build_image "${ROOT_DIR}/user/sample_ext4_nocsum.img" "^metadata_csum,uninit_bg"
