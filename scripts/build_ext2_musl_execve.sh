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
"${ROOT_DIR}/scripts/build_toybox_core.sh" \
  --out "${STAGE_ROOT}/bin/toybox"
chmod 0755 "${STAGE_ROOT}/bin/toybox"

# Applets are invoked as `toybox <applet>` to keep the smoke image compact.
cat > "${STAGE_ROOT}/bin/next_smoke.sh" <<'EOS'
fail=0
pwd || fail=1
toybox ls bin || fail=1
toybox mkdir -p /tmp/smoke || fail=1
echo alpha >/tmp/smoke/a || fail=1
toybox cat /tmp/smoke/a || fail=1
toybox cp /tmp/smoke/a /tmp/smoke/b || fail=1
toybox mv /tmp/smoke/b /tmp/smoke/c || fail=1
toybox ln /tmp/smoke/c /tmp/smoke/hard || fail=1
toybox touch /tmp/smoke/t || fail=1
toybox ls /tmp/smoke || fail=1
toybox rm -f /tmp/smoke/a /tmp/smoke/c /tmp/smoke/hard /tmp/smoke/t || fail=1
toybox rm -rf /tmp/smoke || fail=1

# -- P0 metadata syscalls (fchmodat, fchownat) --
echo meta >/tmp/meta || fail=1
toybox chmod 755 /tmp/meta || fail=1
toybox chown 0:0 /tmp/meta || fail=1
toybox rm -f /tmp/meta || fail=1

# -- shell primitives --
toybox true || fail=1
toybox false && fail=1
toybox test -f /bin/toybox || fail=1

# -- path/info utilities --
toybox basename /bin/toybox || fail=1
toybox dirname /bin/toybox || fail=1
toybox uname || fail=1

# -- text processing --
echo hello world >/tmp/wc_in || fail=1
toybox wc -l /tmp/wc_in || fail=1
toybox head -n 1 /tmp/wc_in || fail=1
toybox tail -n 1 /tmp/wc_in || fail=1
toybox rm -f /tmp/wc_in || fail=1

# -- pipe chains --
echo hello | toybox tr a-z A-Z || fail=1
echo one:two:three | toybox cut -d: -f2 || fail=1

# -- sort/uniq/tee --
echo -e "b\na\nb" >/tmp/sort_in || fail=1
toybox sort /tmp/sort_in | toybox uniq || fail=1
echo tee_test | toybox tee /tmp/tee_out || fail=1
toybox rm -f /tmp/sort_in /tmp/tee_out || fail=1

# -- misc utilities --
toybox seq 1 3 || fail=1
toybox sleep 0 || fail=1
toybox env || fail=1

echo [musl-ext2-cmd-smoke] done
if [ "$fail" -ne 0 ]; then
  echo [musl-ext2-cmd-smoke] failures
fi
EOS
chmod 0755 "${STAGE_ROOT}/bin/next_smoke.sh"

TMP_IMG="${TMP_DIR}/ext2.img"
# Keep image compact for QEMU smoke runs while leaving room for the
# static Toybox multicall payload.
mke2fs -q -t ext2 -F -b 1024 -I 256 -N 128 -L PEETOS_EXT2 -d "${STAGE_ROOT}" "${TMP_IMG}" 512

mv "${TMP_IMG}" "${OUT_IMG}"
chmod 0644 "${OUT_IMG}"

echo "Built ${OUT_IMG}"
