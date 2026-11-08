#!/usr/bin/env bash
# QEMU smoke: boot with a stock mkfs.ext4 root, mounted read-only
# (docs/ext4_compat_plan.md M1).
#
# The runtime musl image is dumped (debugfs rdump) and rebuilt as a stock
# ext4 image (extents, 64bit, flex_bg, metadata_csum, 4 KiB blocks). Its
# bin/journal_smoke.sh, which the musl-ext3-journal-smoke init runs, is
# replaced by a read-only checker: toybox (executed from extent-mapped
# blocks) lists directories, reads a marker file, shows /proc/mounts, and
# confirms that a write is refused. The disk image must be byte-identical
# after the boot.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IMAGE="${ROOT_DIR}/build/peetos.img"
EXT2_IMG="${ROOT_DIR}/user/sample_ext2_musl.img"
WORK_DIR="$(mktemp -d)"
EXT4_IMG="${WORK_DIR}/ext4.img"
LOG_FILE="${WORK_DIR}/boot.log"
MARKER_TEXT="ext4 read-only root marker"

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
  echo "[ext4-ro-smoke] $1" >&2
  if [[ -f "${LOG_FILE}" ]]; then
    cat "${LOG_FILE}" >&2
  fi
  exit 1
}

for tool in debugfs mke2fs e2fsck dumpe2fs; do
  command -v "${tool}" >/dev/null 2>&1 || fail "${tool} not found; install e2fsprogs"
done

make -C "${ROOT_DIR}" image PROFILE=release KERNEL_FEATURES=musl-ext3-journal-smoke

STAGE="${WORK_DIR}/root"
mkdir -p "${STAGE}"
# Ownership warnings are expected when not running as root.
debugfs -R "rdump / ${STAGE}" "${EXT2_IMG}" >/dev/null 2>&1 || true
[[ -x "${STAGE}/bin/toybox" ]] || fail "rdump did not recover bin/toybox"
printf '%s\n' "${MARKER_TEXT}" > "${STAGE}/ro_marker.txt"
cat > "${STAGE}/bin/journal_smoke.sh" <<'EOS'
echo [ext4-ro-smoke] start
toybox ls bin > /dev/null && echo [ext4-ro-smoke] listed bin
toybox cat ro_marker.txt
toybox cat /proc/mounts
if toybox touch ro_probe 2>/dev/null; then
  echo [ext4-ro-smoke] FAIL write allowed
else
  echo [ext4-ro-smoke] write refused
fi
echo [ext4-ro-smoke] done
EOS

payload_kib="$(du -sk "${STAGE}" | awk '{print $1}')"
img_kib=$((payload_kib * 2 + 16384))
mke2fs -q -t ext4 -F -L PEETOS_EXT4 -d "${STAGE}" "${EXT4_IMG}" "${img_kib}"
e2fsck -fn "${EXT4_IMG}" >/dev/null 2>&1 || fail "fresh ext4 image is not clean"
dumpe2fs -h "${EXT4_IMG}" 2>/dev/null | grep -i "filesystem features" | grep -q metadata_csum \
  || fail "image lacks stock ext4 features"
cp "${EXT4_IMG}" "${WORK_DIR}/before.img"

qemu-system-x86_64 \
  -drive id=disk0,file="${IMAGE}",if=none,format=raw \
  -device ich9-ahci,id=ahci \
  -device ide-hd,drive=disk0,bus=ahci.0 \
  -drive id=disk1,file="${EXT4_IMG}",if=none,format=raw \
  -device ide-hd,drive=disk1,bus=ahci.1 \
  -serial stdio \
  -display none \
  -no-reboot \
  -no-shutdown \
  > "${LOG_FILE}" 2>&1 &
QEMU_PID=$!
waited=0
while (( waited < 600 )); do
  grep -aq "\[ext4-ro-smoke\] done" "${LOG_FILE}" && break
  kill -0 "${QEMU_PID}" 2>/dev/null || break
  sleep 0.1
  waited=$((waited + 1))
done
kill -9 "${QEMU_PID}" 2>/dev/null || true
wait "${QEMU_PID}" 2>/dev/null || true
QEMU_PID=""

grep -aq "\[ext4-ro-smoke\] done" "${LOG_FILE}" || fail "checker did not finish"
grep -aq "\[VFS\] Mounted ext4 from block dev1 at / (.*read-only)" "${LOG_FILE}" \
  || fail "root was not mounted as read-only ext4"
grep -aq "\[ext4-ro-smoke\] listed bin" "${LOG_FILE}" || fail "directory listing failed"
grep -aq "${MARKER_TEXT}" "${LOG_FILE}" || fail "marker file was not read"
grep -aq "ext4 / ext4 ro" "${LOG_FILE}" || fail "/proc/mounts does not show ext4 ro"
grep -aq "\[ext4-ro-smoke\] write refused" "${LOG_FILE}" || fail "write was not refused"
if grep -aq "\[EXT4\] .* mismatch" "${LOG_FILE}"; then
  fail "checksum mismatch reported on a clean image"
fi
cmp -s "${EXT4_IMG}" "${WORK_DIR}/before.img" || fail "read-only mount modified the image"

echo "ext4 read-only root smoke completed successfully."
