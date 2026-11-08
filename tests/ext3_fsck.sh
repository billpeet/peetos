#!/usr/bin/env bash
# Cross-check the kernel's ext2/ext3 writer against e2fsprogs: mutate the stock
# `mke2fs -t ext3` fixture through the host fs crate, sync it, and require that
# `e2fsck -fn` finds nothing to fix.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
HOST="$(rustc -vV | grep '^host:' | awk '{print $2}')"

if ! command -v e2fsck >/dev/null 2>&1; then
  echo "[ext3-fsck] e2fsck not found; install e2fsprogs" >&2
  exit 1
fi

source "${ROOT_DIR}/tests/rust_build_compat_env.bash"
setup_rust_build_compat "${ROOT_DIR}"

WORK_DIR="$(mktemp -d)"
cleanup() {
  rm -rf "${WORK_DIR}"
}
trap cleanup EXIT

IMG="${WORK_DIR}/mutated_ext3.img"
PEETOS_EXT3_FSCK_IMAGE="${IMG}" cargo test -p fs --target "${HOST}" --quiet \
  -- --exact tests::ext3_stock_image_mutations_sync_and_remount

if [[ ! -s "${IMG}" ]]; then
  echo "[ext3-fsck] mutation test did not export an image" >&2
  exit 1
fi

if ! e2fsck -fn "${IMG}"; then
  echo "[ext3-fsck] e2fsck reported problems in the kernel-written image" >&2
  exit 1
fi
echo "[ext3-fsck] e2fsck clean"

# Crash image: transactions committed to the journal but never checkpointed.
# e2fsck must see needs_recovery, replay the journal with its jbd2 recovery
# code (proving the on-disk journal format is Linux-compatible), and leave a
# filesystem that a forced read-only check finds clean.
CRASH_IMG="${WORK_DIR}/crashed_ext3.img"
PEETOS_EXT3_CRASH_IMAGE="${CRASH_IMG}" cargo test -p fs --target "${HOST}" --quiet \
  -- --exact tests::ext3_crash_image_needs_recovery_and_replays

if [[ ! -s "${CRASH_IMG}" ]]; then
  echo "[ext3-fsck] crash test did not export an image" >&2
  exit 1
fi
if ! dumpe2fs -h "${CRASH_IMG}" 2>/dev/null | grep -q needs_recovery; then
  echo "[ext3-fsck] crash image is missing the needs_recovery flag" >&2
  exit 1
fi

# The kernel enables JBD2 COMPAT_CHECKSUM, so e2fsck only replays these
# transactions if the kernel's commit-block CRC32s are correct.
if ! dumpe2fs -h "${CRASH_IMG}" 2>/dev/null | grep -i "journal features" | grep -q journal_checksum; then
  echo "[ext3-fsck] crash image journal lacks journal_checksum" >&2
  exit 1
fi

set +e
e2fsck -fy "${CRASH_IMG}"
replay_rc=$?
set -e
# 0 = clean, 1 = errors corrected (journal replay counts as a modification).
if [[ "${replay_rc}" -gt 1 ]]; then
  echo "[ext3-fsck] e2fsck journal replay failed (rc=${replay_rc})" >&2
  exit 1
fi
if dumpe2fs -h "${CRASH_IMG}" 2>/dev/null | grep -q needs_recovery; then
  echo "[ext3-fsck] needs_recovery still set after e2fsck replay" >&2
  exit 1
fi
if ! e2fsck -fn "${CRASH_IMG}"; then
  echo "[ext3-fsck] filesystem not clean after e2fsck journal replay" >&2
  exit 1
fi
if ! debugfs -R "cat /bigdir/crash_new" "${CRASH_IMG}" 2>/dev/null | grep -q "survives crash"; then
  echo "[ext3-fsck] replayed image lost the committed file" >&2
  exit 1
fi
echo "[ext3-fsck] e2fsck replayed the kernel journal cleanly"

# A journaled block damaged after commit: e2fsck must reject that
# transaction's checksum (and everything after it) but replay earlier ones.
CORRUPT_IMG="${WORK_DIR}/corrupt_ext3.img"
PEETOS_EXT3_CORRUPT_IMAGE="${CORRUPT_IMG}" cargo test -p fs --target "${HOST}" --quiet \
  -- --exact tests::ext3_corrupted_journal_block_stops_replay_at_that_transaction
set +e
e2fsck -fy "${CORRUPT_IMG}" >"${WORK_DIR}/corrupt.fsck" 2>&1
corrupt_rc=$?
set -e
if [[ "${corrupt_rc}" -gt 1 ]]; then
  echo "[ext3-fsck] e2fsck failed on the corrupted-journal image (rc=${corrupt_rc})" >&2
  cat "${WORK_DIR}/corrupt.fsck" >&2
  exit 1
fi
if ! e2fsck -fn "${CORRUPT_IMG}" >/dev/null 2>&1; then
  echo "[ext3-fsck] filesystem not clean after rejecting the corrupted transaction" >&2
  exit 1
fi
if ! debugfs -R "cat /sub/csum_a" "${CORRUPT_IMG}" 2>/dev/null | grep -q intact; then
  echo "[ext3-fsck] e2fsck dropped a transaction committed before the corruption" >&2
  exit 1
fi
if debugfs -R "stat /sub/csum_b" "${CORRUPT_IMG}" 2>/dev/null | grep -q "Inode:"; then
  echo "[ext3-fsck] e2fsck replayed a transaction whose checksum does not match" >&2
  exit 1
fi
echo "[ext3-fsck] e2fsck rejected the corrupted transaction by checksum"

# One transaction with two descriptor blocks: Linux's recovery must replay
# all of it as a unit.
MULTI_IMG="${WORK_DIR}/multidesc_ext3.img"
PEETOS_EXT3_MULTIDESC_IMAGE="${MULTI_IMG}" cargo test -p fs --target "${HOST}" --quiet \
  -- --exact tests::ext3_large_transaction_spans_descriptor_blocks_and_replays
set +e
e2fsck -fy "${MULTI_IMG}" >/dev/null 2>&1
multi_rc=$?
set -e
if [[ "${multi_rc}" -gt 1 ]]; then
  echo "[ext3-fsck] e2fsck failed to replay the multi-descriptor transaction (rc=${multi_rc})" >&2
  exit 1
fi
mismatched=0
block_size=1024
while read -r block value; do
  expected="$(printf '%02x' "${value}")"
  if [[ -n "$(dd if="${MULTI_IMG}" bs="${block_size}" skip="${block}" count=1 status=none \
      | od -An -v -tx1 | tr -s ' \n' '\n' | grep -v -e '^$' -e "^${expected}\$" | head -1)" ]]; then
    mismatched=$((mismatched + 1))
  fi
done < "${MULTI_IMG}.blocks"
if [[ "${mismatched}" -ne 0 ]]; then
  echo "[ext3-fsck] ${mismatched} journaled blocks were not replayed by e2fsck" >&2
  exit 1
fi
echo "[ext3-fsck] e2fsck replayed the multi-descriptor transaction"

# Crash at every write with a reordering write cache: every image the kernel
# recovered must be a consistent filesystem.
CRASH_DIR="${WORK_DIR}/crash_points"
mkdir -p "${CRASH_DIR}"
PEETOS_EXT3_CRASH_DIR="${CRASH_DIR}" cargo test -p fs --target "${HOST}" --quiet \
  -- --exact tests::ext3_crash_at_every_write_with_reordering_cache_stays_consistent
checked=0
for img in "${CRASH_DIR}"/*.img; do
  if ! e2fsck -fn "${img}" >"${img}.fsck" 2>&1; then
    echo "[ext3-fsck] inconsistent filesystem after recovery: $(basename "${img}")" >&2
    cat "${img}.fsck" >&2
    exit 1
  fi
  checked=$((checked + 1))
done
if [[ "${checked}" -eq 0 ]]; then
  echo "[ext3-fsck] crash-point test produced no images" >&2
  exit 1
fi
echo "[ext3-fsck] ${checked} recovered crash-point images are e2fsck clean"

# ext4: the kernel recovers a Linux-format journal (64bit + csum v3, written
# by debugfs) on a read-only mount; the result, including the superblock and
# journal superblock checksums it rewrote, must be clean for e2fsck.
EXT4_IMG="${WORK_DIR}/ext4_replayed.img"
PEETOS_EXT4_REPLAYED_IMAGE="${EXT4_IMG}" cargo test -p fs --target "${HOST}" --quiet \
  -- --exact tests::ext4_linux_format_journal_replays_on_read_only_mount
if [[ ! -s "${EXT4_IMG}" ]]; then
  echo "[ext3-fsck] ext4 replay test did not export an image" >&2
  exit 1
fi
if ! e2fsck -fn "${EXT4_IMG}"; then
  echo "[ext3-fsck] ext4 image not clean after kernel journal recovery" >&2
  exit 1
fi
echo "[ext3-fsck] e2fsck accepts the kernel's recovery of a csum-v3 ext4 journal"

# ext4 writes (M2): extent trees, lazy group init, gdt_csum descriptors,
# extra_isize inodes, fast/slow symlinks on user/sample_ext4_nocsum.img.
EXT4_MUT_IMG="${WORK_DIR}/ext4_mutated.img"
PEETOS_EXT4_FSCK_IMAGE="${EXT4_MUT_IMG}" cargo test -p fs --target "${HOST}" --quiet \
  -- --exact tests::ext4_mutations_sync_and_remount
if [[ ! -s "${EXT4_MUT_IMG}" ]]; then
  echo "[ext3-fsck] ext4 mutation test did not export an image" >&2
  exit 1
fi
if ! e2fsck -fn "${EXT4_MUT_IMG}"; then
  echo "[ext3-fsck] e2fsck reported problems in the kernel-written ext4 image" >&2
  exit 1
fi
echo "[ext3-fsck] e2fsck clean on kernel-written ext4 (extents, uninit_bg)"

EXT4_CRASH_DIR="${WORK_DIR}/ext4_crash_points"
mkdir -p "${EXT4_CRASH_DIR}"
PEETOS_EXT4_CRASH_DIR="${EXT4_CRASH_DIR}" cargo test -p fs --target "${HOST}" --quiet \
  -- --exact tests::ext4_crash_at_every_write_with_reordering_cache_stays_consistent
checked=0
for img in "${EXT4_CRASH_DIR}"/*.img; do
  if ! e2fsck -fn "${img}" >"${img}.fsck" 2>&1; then
    echo "[ext3-fsck] inconsistent ext4 filesystem after recovery: $(basename "${img}")" >&2
    cat "${img}.fsck" >&2
    exit 1
  fi
  checked=$((checked + 1))
done
if [[ "${checked}" -eq 0 ]]; then
  echo "[ext3-fsck] ext4 crash-point test produced no images" >&2
  exit 1
fi
echo "[ext3-fsck] ${checked} recovered ext4 crash-point images are e2fsck clean"

# ext4 metadata_csum writes (M3) on the stock user/sample_ext4.img: inode,
# descriptor, bitmap, extent, directory-tail and superblock checksums.
EXT4_CSUM_IMG="${WORK_DIR}/ext4_csum_mutated.img"
PEETOS_EXT4_CSUM_FSCK_IMAGE="${EXT4_CSUM_IMG}" cargo test -p fs --target "${HOST}" --quiet \
  -- --exact tests::ext4_csum_mutations_keep_every_checksum_valid
if [[ ! -s "${EXT4_CSUM_IMG}" ]]; then
  echo "[ext3-fsck] ext4 metadata_csum mutation test did not export an image" >&2
  exit 1
fi
if ! e2fsck -fn "${EXT4_CSUM_IMG}"; then
  echo "[ext3-fsck] e2fsck reported problems in the kernel-written metadata_csum image" >&2
  exit 1
fi
echo "[ext3-fsck] e2fsck clean on kernel-written stock ext4 (metadata_csum)"

# Kernel-written JBD2 csum-v3 + 64bit transactions, left unrecovered: e2fsck's
# jbd2 recovery must accept every tag, descriptor and commit checksum.
EXT4_CSUM_CRASH_IMG="${WORK_DIR}/ext4_csum_crashed.img"
PEETOS_EXT4_CSUM_CRASH_IMAGE="${EXT4_CSUM_CRASH_IMG}" cargo test -p fs --target "${HOST}" --quiet \
  -- --exact tests::ext4_csum_crash_image_replays_kernel_written_v3_journal
if ! dumpe2fs -h "${EXT4_CSUM_CRASH_IMG}" 2>/dev/null | grep -q needs_recovery; then
  echo "[ext3-fsck] ext4 csum crash image is missing needs_recovery" >&2
  exit 1
fi
if ! dumpe2fs -h "${EXT4_CSUM_CRASH_IMG}" 2>/dev/null | grep -i "journal features" \
  | grep -q journal_checksum_v3; then
  echo "[ext3-fsck] ext4 csum crash image journal is not csum v3" >&2
  exit 1
fi
set +e
e2fsck -fy "${EXT4_CSUM_CRASH_IMG}" >"${WORK_DIR}/ext4_csum_replay.fsck" 2>&1
replay_rc=$?
set -e
if [[ "${replay_rc}" -gt 1 ]]; then
  echo "[ext3-fsck] e2fsck could not replay the kernel's csum-v3 journal (rc=${replay_rc})" >&2
  cat "${WORK_DIR}/ext4_csum_replay.fsck" >&2
  exit 1
fi
if ! e2fsck -fn "${EXT4_CSUM_CRASH_IMG}"; then
  echo "[ext3-fsck] ext4 image not clean after e2fsck replayed the csum-v3 journal" >&2
  exit 1
fi
if [[ "$(debugfs -R "cat /sub/crash_csum" "${EXT4_CSUM_CRASH_IMG}" 2>/dev/null)" != "survives crash" ]]; then
  echo "[ext3-fsck] e2fsck replay lost the committed csum-v3 transaction" >&2
  exit 1
fi
echo "[ext3-fsck] e2fsck replays kernel-written csum-v3 transactions cleanly"

EXT4_CSUM_CRASH_DIR="${WORK_DIR}/ext4_csum_crash_points"
mkdir -p "${EXT4_CSUM_CRASH_DIR}"
PEETOS_EXT4_CSUM_CRASH_DIR="${EXT4_CSUM_CRASH_DIR}" cargo test -p fs --target "${HOST}" --quiet \
  -- --exact tests::ext4_csum_crash_at_every_write_with_reordering_cache_stays_consistent
checked=0
for img in "${EXT4_CSUM_CRASH_DIR}"/*.img; do
  if ! e2fsck -fn "${img}" >"${img}.fsck" 2>&1; then
    echo "[ext3-fsck] inconsistent metadata_csum filesystem after recovery: $(basename "${img}")" >&2
    cat "${img}.fsck" >&2
    exit 1
  fi
  checked=$((checked + 1))
done
if [[ "${checked}" -eq 0 ]]; then
  echo "[ext3-fsck] ext4 metadata_csum crash-point test produced no images" >&2
  exit 1
fi
echo "[ext3-fsck] ${checked} recovered metadata_csum crash-point images are e2fsck clean"

# htree maintenance (M4): kernel-built indexes must pass e2fsck's htree
# checks (hash order per leaf, counts/limits, dx_tail checksums).
for spec in "PEETOS_EXT4_HTREE_FSCK_IMAGE:ext4_large_directory_keeps_a_valid_htree:ext4_htree.img" \
            "PEETOS_EXT3_HTREE_FSCK_IMAGE:ext3_htree_grows_a_second_index_level:ext3_htree.img"; do
  IFS=: read -r env_name test_name img_name <<< "${spec}"
  img="${WORK_DIR}/${img_name}"
  env "${env_name}=${img}" cargo test -p fs --target "${HOST}" --quiet -- --exact "tests::${test_name}"
  if [[ ! -s "${img}" ]]; then
    echo "[ext3-fsck] ${test_name} did not export an image" >&2
    exit 1
  fi
  if ! e2fsck -fn "${img}" >"${img}.fsck" 2>&1; then
    echo "[ext3-fsck] e2fsck rejected the kernel-built htree in ${img_name}" >&2
    cat "${img}.fsck" >&2
    exit 1
  fi
done
echo "[ext3-fsck] e2fsck clean on kernel-built htree directories (ext4 csum, ext3 two-level)"

HASH_DIR="${WORK_DIR}/htree_hashes"
mkdir -p "${HASH_DIR}"
PEETOS_HTREE_HASH_FSCK_DIR="${HASH_DIR}" cargo test -p fs --target "${HOST}" --quiet \
  -- --exact tests::htree_hash_variants_index_high_byte_names
checked=0
for img in "${HASH_DIR}"/*.img; do
  if ! e2fsck -fn "${img}" >"${img}.fsck" 2>&1; then
    echo "[ext3-fsck] e2fsck disagrees with the kernel's hash for $(basename "${img}")" >&2
    cat "${img}.fsck" >&2
    exit 1
  fi
  checked=$((checked + 1))
done
if [[ "${checked}" -ne 6 ]]; then
  echo "[ext3-fsck] expected 6 hash-variant images, got ${checked}" >&2
  exit 1
fi
echo "[ext3-fsck] e2fsck accepts all 6 htree hash variants"

HTREE_CRASH_DIR="${WORK_DIR}/htree_crash_points"
mkdir -p "${HTREE_CRASH_DIR}"
PEETOS_EXT4_HTREE_CRASH_DIR="${HTREE_CRASH_DIR}" cargo test -p fs --target "${HOST}" --quiet \
  -- --exact tests::ext4_htree_split_crash_points_stay_consistent
checked=0
for img in "${HTREE_CRASH_DIR}"/*.img; do
  if ! e2fsck -fn "${img}" >"${img}.fsck" 2>&1; then
    echo "[ext3-fsck] inconsistent htree after recovery: $(basename "${img}")" >&2
    cat "${img}.fsck" >&2
    exit 1
  fi
  checked=$((checked + 1))
done
if [[ "${checked}" -eq 0 ]]; then
  echo "[ext3-fsck] htree crash-point test produced no images" >&2
  exit 1
fi
echo "[ext3-fsck] ${checked} recovered htree-split crash-point images are e2fsck clean"
