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
ln "${STAGE_ROOT}/bin/toybox" "${STAGE_ROOT}/bin/sh"

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

# -- next-wave utilities --
toybox find /tmp -maxdepth 1 -type f 2>/dev/null || fail=1
echo findme >/tmp/greptest && toybox grep findme /tmp/greptest && toybox rm -f /tmp/greptest || fail=1
echo hello | toybox sed 's/hello/world/' || fail=1
echo one two three | toybox xargs echo || fail=1
toybox date || fail=1
toybox id || fail=1
toybox printf 'fmt %s\n' ok || fail=1
toybox readlink /bin/toybox 2>/dev/null; true
toybox stat /bin/toybox || fail=1
toybox du -s /bin || fail=1
toybox which toybox || fail=1
toybox dd if=/dev/zero of=/tmp/dd_out bs=64 count=1 2>/dev/null && toybox rm -f /tmp/dd_out || fail=1
toybox mktemp /tmp/tmp.XXXXXX >/dev/null && toybox rm -f /tmp/tmp.* || fail=1
toybox yes | toybox head -n 1 || fail=1
toybox rmdir /tmp/smoke 2>/dev/null; toybox mkdir /tmp/smoke && toybox rmdir /tmp/smoke || fail=1
echo abc >/tmp/cmp1 && echo abc >/tmp/cmp2 && toybox cmp /tmp/cmp1 /tmp/cmp2 && toybox rm -f /tmp/cmp1 /tmp/cmp2 || fail=1
echo -e "a\tb" | toybox paste -s || fail=1
echo hello | toybox nl || fail=1
echo hello | toybox rev || fail=1
echo hello | toybox fold -w 3 || fail=1
toybox seq 1 3 | toybox comm - - 2>/dev/null; true
toybox free || fail=1
toybox uptime || fail=1
toybox strings /bin/toybox | toybox head -n 1 || fail=1

# -- third-wave utilities --
echo tarme >/tmp/tar_in && toybox tar cf /tmp/test.tar /tmp/tar_in 2>/dev/null && toybox rm -f /tmp/tar_in && toybox tar tf /tmp/test.tar 2>/dev/null && toybox rm -f /tmp/test.tar || fail=1
echo aaa >/tmp/diff1 && echo bbb >/tmp/diff2 && toybox diff /tmp/diff1 /tmp/diff2 >/dev/null 2>&1; toybox rm -f /tmp/diff1 /tmp/diff2; true
echo -e "--- a\n+++ b\n@@ -1 +1 @@\n-old\n+new" >/tmp/test.patch; true
toybox md5sum /bin/toybox >/dev/null || fail=1
echo hello | toybox od -A x -t x1 >/dev/null || fail=1
echo -e "a\nb\nc" | toybox tac || fail=1
toybox realpath /bin/toybox || fail=1
toybox hostname 2>/dev/null; true
toybox expr 1 + 1 || fail=1
echo full >/tmp/trunc_test && toybox truncate -s 0 /tmp/trunc_test && toybox rm -f /tmp/trunc_test || fail=1
echo -e "a\tb" | toybox expand || fail=1
toybox df / 2>/dev/null; true
toybox printenv PATH 2>/dev/null; true
echo unlink_me >/tmp/unlink_test && toybox unlink /tmp/unlink_test || fail=1
toybox tty 2>/dev/null; true
toybox sync 2>/dev/null; true
toybox cal 2>/dev/null; true
toybox groups 2>/dev/null; true
toybox logname 2>/dev/null; true
toybox nproc || fail=1
toybox file /bin/toybox 2>/dev/null; true
echo hello | toybox gzip | toybox gunzip || fail=1

# -- fourth-wave: awk + install --
echo 'hello world' | toybox awk '{print $2}' || fail=1
echo -e "1 alpha\n2 beta\n3 gamma" | toybox awk '/beta/' || fail=1
echo -e "10\n20\n30" | toybox awk '{s+=$1} END {print s}' || fail=1
echo install_src >/tmp/inst_src && toybox install -m 755 /tmp/inst_src /tmp/inst_dst && toybox test -f /tmp/inst_dst && toybox rm -f /tmp/inst_src /tmp/inst_dst || fail=1
toybox install -d /tmp/inst_dir && toybox test -d /tmp/inst_dir && toybox rmdir /tmp/inst_dir || fail=1

# -- fifth-wave: remaining no-new-syscall applets --
toybox cksum /bin/toybox >/dev/null || fail=1
echo -e "a b\nb c" | toybox tsort || fail=1
echo hello | toybox uuencode test | toybox uudecode -o /tmp/uu_out && toybox test -f /tmp/uu_out && toybox rm -f /tmp/uu_out || fail=1
echo x >/tmp/cg && toybox chgrp 0 /tmp/cg && toybox rm -f /tmp/cg || fail=1
toybox link /bin/toybox /tmp/lk_test && toybox rm -f /tmp/lk_test || fail=1
echo hello | toybox iconv || fail=1
toybox getconf PAGE_SIZE >/dev/null || fail=1
toybox time toybox true 2>/dev/null || fail=1
toybox ulimit -a >/dev/null || fail=1
echo hello | toybox base64 | toybox base64 -d || fail=1
echo -e "hello\r\n" | toybox dos2unix | toybox cat || fail=1
echo hello | toybox xxd | toybox xxd -r || fail=1
toybox factor 12 || fail=1
toybox ascii >/dev/null || fail=1
toybox usleep 1 || fail=1
echo -e "a\nb\nc" | toybox shuf >/dev/null || fail=1
toybox mcookie >/dev/null || fail=1
toybox uuidgen >/dev/null || fail=1
toybox getopt -o ab -- -a -b >/dev/null || fail=1
echo hello | toybox hd >/dev/null || fail=1
echo hello | toybox hexdump >/dev/null || fail=1
echo hello | toybox fmt || fail=1
toybox help echo >/dev/null 2>&1; true
toybox nologin 2>/dev/null; true
echo hello | toybox ts >/dev/null 2>&1; true
toybox pwgen 8 1 >/dev/null || fail=1
toybox readelf -h /bin/toybox >/dev/null || fail=1
echo x >/tmp/shred_test && toybox shred -u /tmp/shred_test || fail=1
toybox clear >/dev/null 2>&1; true
toybox reset >/dev/null 2>&1; true

# -- sixth-wave: aliases and pure-computation applets --
toybox arch >/dev/null || fail=1
echo findme | toybox egrep findme >/dev/null || fail=1
echo findme | toybox fgrep findme >/dev/null || fail=1
toybox crc32 /bin/toybox >/dev/null || fail=1
toybox [ -f /bin/toybox ] || fail=1
echo hello | toybox gzip | toybox zcat || fail=1
toybox sha1sum /bin/toybox >/dev/null || fail=1
toybox sha256sum /bin/toybox >/dev/null || fail=1
toybox sha512sum /bin/toybox >/dev/null || fail=1
echo hello | toybox base32 | toybox base32 -d || fail=1
toybox sha3sum /bin/toybox >/dev/null || fail=1
toybox unicode 41 >/dev/null || fail=1
toybox mountpoint / 2>/dev/null; true
toybox whoami 2>/dev/null; true

# -- seventh-wave: networking applets (basic argv/help validation) --
toybox netcat --help 2>&1 | toybox head -n 1 || fail=1
toybox wget --help 2>&1 | toybox head -n 1 || fail=1
toybox ftpget --help 2>&1 | toybox head -n 1 || fail=1
toybox sntp --help 2>&1 | toybox head -n 1 || fail=1
toybox microcom --help 2>&1 | toybox head -n 1 || fail=1

echo [musl-ext2-cmd-smoke] done
if [ "$fail" -ne 0 ]; then
  echo [musl-ext2-cmd-smoke] failures
fi
EOS
chmod 0755 "${STAGE_ROOT}/bin/next_smoke.sh"

TMP_IMG="${TMP_DIR}/ext2.img"
# Keep image compact for QEMU smoke runs while leaving room for the
# static Toybox multicall payload (expanded for networking applets).
mke2fs -q -t ext2 -F -b 1024 -I 256 -N 384 -L PEETOS_EXT2 -d "${STAGE_ROOT}" "${TMP_IMG}" 3072

mv "${TMP_IMG}" "${OUT_IMG}"
chmod 0644 "${OUT_IMG}"

echo "Built ${OUT_IMG}"
