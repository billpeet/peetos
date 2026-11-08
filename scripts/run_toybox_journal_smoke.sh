#!/usr/bin/env bash
# QEMU crash/recovery smoke for the ext3 journal on real (emulated) AHCI I/O.
#
#  1. Convert a copy of the runtime musl image to ext3 (tune2fs -j), or with
#     PEETOS_JOURNAL_SMOKE_FS=ext4 rebuild it as writable ext4 (mke2fs -t
#     ext4 -O ^metadata_csum,uninit_bg: extents, 64bit, flex_bg, lazy-init
#     groups, crc16 descriptor checksums; docs/ext4_compat_plan.md M2), or
#     with PEETOS_JOURNAL_SMOKE_FS=ext4-csum as stock mke2fs -t ext4
#     (metadata_csum; the kernel switches the journal to JBD2 csum v3, M3).
#  2. Boot 1: bin/journal_smoke.sh syncs a baseline file, writes files until
#     dirty-pressure commits sit uncheckpointed in the journal, and prints a
#     crash point. QEMU is SIGKILLed there: everything still in the PeetOS
#     block write-back cache is lost, as on power failure.
#  3. A copy of the crashed image must say needs_recovery, and e2fsck (Linux
#     jbd2 recovery, including commit-block CRC32 checks) must replay it into
#     a clean filesystem.
#  4. Boot 2 on the crashed image: the kernel replays the journal itself, the
#     script verifies the baseline and that every surviving file is a byte
#     prefix of what was written, then syncs; e2fsck -fn must find the result
#     clean.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IMAGE="${ROOT_DIR}/build/peetos.img"
EXT2_IMG="${ROOT_DIR}/user/sample_ext2_musl.img"
WORK_DIR="$(mktemp -d)"
EXT3_IMG="${WORK_DIR}/ext3.img"
FS_KIND="${PEETOS_JOURNAL_SMOKE_FS:-ext3}"
case "${FS_KIND}" in
  ext3) MOUNT_NAME=ext3 ;;
  ext4|ext4-csum) MOUNT_NAME=ext4 ;;
  *) echo "[journal-smoke] unknown PEETOS_JOURNAL_SMOKE_FS=${FS_KIND}" >&2; exit 1 ;;
esac
LOG1="${WORK_DIR}/boot1.log"
LOG2="${WORK_DIR}/boot2.log"

source "${ROOT_DIR}/tests/rust_build_compat_env.bash"
setup_rust_build_compat "${ROOT_DIR}"

QEMU_PID=""
cleanup() {
  if [[ -n "${QEMU_PID}" ]]; then
    kill -9 "${QEMU_PID}" 2>/dev/null || true
  fi
  rm -rf "${WORK_DIR}"
}
trap cleanup EXIT

fail() {
  echo "[journal-smoke] $1" >&2
  for log in "${LOG1}" "${LOG2}"; do
    if [[ -f "${log}" ]]; then
      echo "----- $(basename "${log}") -----" >&2
      cat "${log}" >&2
    fi
  done
  exit 1
}

for tool in tune2fs resize2fs e2fsck dumpe2fs debugfs; do
  command -v "${tool}" >/dev/null 2>&1 || fail "${tool} not found; install e2fsprogs"
done

make -C "${ROOT_DIR}" image PROFILE=release KERNEL_FEATURES=musl-ext3-journal-smoke

if [[ "${MOUNT_NAME}" == "ext4" ]]; then
  # Rebuild the runtime image's contents as ext4: stock features for
  # ext4-csum, without metadata_csum (crc16 descriptors) for ext4.
  mkfs_opts=(-O ^metadata_csum,uninit_bg)
  if [[ "${FS_KIND}" == "ext4-csum" ]]; then
    mkfs_opts=()
  fi
  STAGE="${WORK_DIR}/stage"
  mkdir -p "${STAGE}"
  debugfs -R "rdump / ${STAGE}" "${EXT2_IMG}" >/dev/null 2>&1 || true
  [[ -x "${STAGE}/bin/toybox" ]] || fail "rdump did not recover bin/toybox"
  payload_kib="$(du -sk "${STAGE}" | awk '{print $1}')"
  mke2fs -q -t ext4 -F "${mkfs_opts[@]}" -L PEETOS_EXT4 -d "${STAGE}" \
    "${EXT3_IMG}" $((payload_kib * 2 + 16384))
  dumpe2fs -h "${EXT3_IMG}" 2>/dev/null | grep -i "filesystem features" | grep -q extent \
    || fail "mke2fs did not produce an ext4 image"
  if [[ "${FS_KIND}" == "ext4-csum" ]]; then
    dumpe2fs -h "${EXT3_IMG}" 2>/dev/null | grep -i "filesystem features" | grep -q metadata_csum \
      || fail "mke2fs did not produce a metadata_csum image"
  fi
else
  # Grow the runtime image so a 1 MiB journal fits, then add the journal.
  cp "${EXT2_IMG}" "${EXT3_IMG}"
  truncate -s +3M "${EXT3_IMG}"
  e2fsck -fy "${EXT3_IMG}" >/dev/null 2>&1 || true
  resize2fs -f "${EXT3_IMG}" >/dev/null 2>&1
  tune2fs -j -J size=1 "${EXT3_IMG}" >/dev/null
fi
dumpe2fs -h "${EXT3_IMG}" 2>/dev/null | grep -q has_journal || fail "image has no journal"

# Boots the kernel with the ext3 image as disk1; returns once `marker`
# appears in the serial log (QEMU is then killed) or the timeout expires.
boot_until() {
  local log="$1" marker="$2" timeout_s="$3"
  qemu-system-x86_64 \
    -drive id=disk0,file="${IMAGE}",if=none,format=raw \
    -device ich9-ahci,id=ahci \
    -device ide-hd,drive=disk0,bus=ahci.0 \
    -drive id=disk1,file="${EXT3_IMG}",if=none,format=raw \
    -device ide-hd,drive=disk1,bus=ahci.1 \
    -serial stdio \
    -display none \
    -no-reboot \
    -no-shutdown \
    > "${log}" 2>&1 &
  QEMU_PID=$!
  local waited=0
  while (( waited < timeout_s * 10 )); do
    if grep -aq -- "${marker}" "${log}"; then
      break
    fi
    if ! kill -0 "${QEMU_PID}" 2>/dev/null; then
      break
    fi
    sleep 0.1
    waited=$((waited + 1))
  done
  # SIGKILL: no orderly shutdown, nothing flushed beyond what the guest
  # already handed to the virtual disk.
  kill -9 "${QEMU_PID}" 2>/dev/null || true
  wait "${QEMU_PID}" 2>/dev/null || true
  QEMU_PID=""
  grep -aq -- "${marker}" "${log}"
}

boot_until "${LOG1}" "[ext3-journal-smoke] crash-point" 60 \
  || fail "boot 1 never reached the crash point"
grep -aq "\[VFS\] Mounted ${MOUNT_NAME} from block dev1" "${LOG1}" \
  || fail "boot 1 did not mount the image as ${MOUNT_NAME}"
if grep -aq "read-only" <(grep -a "\[VFS\] Mounted" "${LOG1}"); then
  fail "boot 1 mounted the root read-only"
fi
grep -aq "\[ext3-journal-smoke\] baseline synced" "${LOG1}" || fail "boot 1 did not sync the baseline"

# Debug aid: PEETOS_JOURNAL_SMOKE_KEEP=<dir> keeps the crashed image and the
# boot 1 serial log for offline inspection.
if [[ -n "${PEETOS_JOURNAL_SMOKE_KEEP:-}" ]]; then
  mkdir -p "${PEETOS_JOURNAL_SMOKE_KEEP}"
  cp "${EXT3_IMG}" "${PEETOS_JOURNAL_SMOKE_KEEP}/crashed.img"
  cp "${LOG1}" "${PEETOS_JOURNAL_SMOKE_KEEP}/boot1.log"
fi

# The crash must leave committed-but-uncheckpointed transactions behind,
# otherwise this test would not exercise recovery at all.
dumpe2fs -h "${EXT3_IMG}" 2>/dev/null | grep -i "filesystem features" | grep -q needs_recovery \
  || fail "crashed image has no needs_recovery flag (nothing left in the journal)"
dumpe2fs -h "${EXT3_IMG}" 2>/dev/null | grep -i "journal features" | grep -q journal_checksum \
  || fail "kernel did not enable journal_checksum"
if [[ "${FS_KIND}" == "ext4-csum" ]]; then
  dumpe2fs -h "${EXT3_IMG}" 2>/dev/null | grep -i "journal features" | grep -q journal_checksum_v3 \
    || fail "kernel did not switch the journal to csum v3"
fi

# Linux-side recovery of the AHCI-written journal, on a copy.
LINUX_IMG="${WORK_DIR}/linux_recovered.img"
cp "${EXT3_IMG}" "${LINUX_IMG}"
set +e
e2fsck -fy "${LINUX_IMG}" > "${WORK_DIR}/linux_replay.txt" 2>&1
linux_rc=$?
set -e
if [[ "${linux_rc}" -gt 1 ]]; then
  cat "${WORK_DIR}/linux_replay.txt" >&2
  fail "e2fsck could not recover the crashed image (rc=${linux_rc})"
fi
grep -q "recovering journal" "${WORK_DIR}/linux_replay.txt" || fail "e2fsck did not replay the journal"
e2fsck -fn "${LINUX_IMG}" > "${WORK_DIR}/linux_check.txt" 2>&1 \
  || { cat "${WORK_DIR}/linux_check.txt" >&2; fail "filesystem not clean after e2fsck recovery"; }
debugfs -R "cat /jsmoke/baseline" "${LINUX_IMG}" 2>/dev/null | grep -q "file baseline line 63" \
  || fail "synced baseline missing after e2fsck recovery"

# Kernel-side recovery: boot the crashed image again.
boot_until "${LOG2}" "[ext3-journal-smoke] done" 60 || fail "boot 2 did not finish verification"
grep -aq "\[ext3-journal-smoke\] recovery boot" "${LOG2}" || fail "boot 2 did not take the recovery path"
if grep -aq "\[ext3-journal-smoke\] failures" "${LOG2}"; then
  fail "boot 2 found damaged files after kernel journal recovery"
fi
grep -aE "\[ext3-journal-smoke\] verified [0-9]+ surviving files" "${LOG2}" >&2 || true
if grep -aq "\[EXT4\] .* mismatch" "${LOG1}" "${LOG2}"; then
  fail "kernel reported a checksum mismatch"
fi

e2fsck -fn "${EXT3_IMG}" > "${WORK_DIR}/kernel_check.txt" 2>&1 \
  || { cat "${WORK_DIR}/kernel_check.txt" >&2; fail "filesystem not clean after kernel recovery"; }
if dumpe2fs -h "${EXT3_IMG}" 2>/dev/null | grep -i "filesystem features" | grep -q needs_recovery; then
  fail "needs_recovery still set after kernel recovery and sync"
fi

echo "${FS_KIND} journal crash/recovery smoke completed successfully."
