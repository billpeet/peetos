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
mkdir -p "${STAGE_ROOT}/tmp"
mkdir -p "${STAGE_ROOT}/etc" "${STAGE_ROOT}/root/.ssh" "${STAGE_ROOT}/etc/dropbear"

if [[ ! -f "${STAGE_ROOT}/etc/passwd" ]]; then
  cat > "${STAGE_ROOT}/etc/passwd" <<'EOS'
root:x:0:0:root:/root:/bin/sh
EOS
  chmod 0644 "${STAGE_ROOT}/etc/passwd"
fi

if [[ ! -f "${STAGE_ROOT}/etc/group" ]]; then
  cat > "${STAGE_ROOT}/etc/group" <<'EOS'
root:x:0:
EOS
  chmod 0644 "${STAGE_ROOT}/etc/group"
fi

# Stage QuickJS interpreter + sample scripts when available.
if [[ -x "${ROOT_DIR}/user/qjs" ]]; then
  cp "${ROOT_DIR}/user/qjs" "${STAGE_ROOT}/bin/qjs"
  chmod 0755 "${STAGE_ROOT}/bin/qjs"
fi
if [[ -f "${ROOT_DIR}/quickjs/test_hello.js" ]]; then
  cp "${ROOT_DIR}/quickjs/test_hello.js" "${STAGE_ROOT}/tmp/test_hello.js"
  chmod 0644 "${STAGE_ROOT}/tmp/test_hello.js"
fi
if [[ -f "${ROOT_DIR}/quickjs/test_math.js" ]]; then
  cp "${ROOT_DIR}/quickjs/test_math.js" "${STAGE_ROOT}/tmp/test_math.js"
  chmod 0644 "${STAGE_ROOT}/tmp/test_math.js"
fi

# Stage MicroPython interpreter + sample scripts when available.
if [[ -x "${ROOT_DIR}/user/micropython" ]]; then
  cp "${ROOT_DIR}/user/micropython" "${STAGE_ROOT}/bin/micropython"
  chmod 0755 "${STAGE_ROOT}/bin/micropython"
fi
if [[ -f "${ROOT_DIR}/micropython/test_hello.py" ]]; then
  cp "${ROOT_DIR}/micropython/test_hello.py" "${STAGE_ROOT}/tmp/test_hello.py"
  chmod 0644 "${STAGE_ROOT}/tmp/test_hello.py"
fi
if [[ -f "${ROOT_DIR}/micropython/test_math.py" ]]; then
  cp "${ROOT_DIR}/micropython/test_math.py" "${STAGE_ROOT}/tmp/test_math.py"
  chmod 0644 "${STAGE_ROOT}/tmp/test_math.py"
fi

# Stage Dropbear binaries when available.
if [[ -x "${ROOT_DIR}/user/dropbear" ]]; then
  cp "${ROOT_DIR}/user/dropbear" "${STAGE_ROOT}/bin/dropbear"
  chmod 0755 "${STAGE_ROOT}/bin/dropbear"
fi
if [[ -x "${ROOT_DIR}/user/dropbearkey" ]]; then
  cp "${ROOT_DIR}/user/dropbearkey" "${STAGE_ROOT}/bin/dropbearkey"
  chmod 0755 "${STAGE_ROOT}/bin/dropbearkey"
fi
if [[ -f "${ROOT_DIR}/user/dropbear_ed25519_host_key" ]]; then
  cp "${ROOT_DIR}/user/dropbear_ed25519_host_key" "${STAGE_ROOT}/etc/dropbear/dropbear_ed25519_host_key"
  chmod 0600 "${STAGE_ROOT}/etc/dropbear/dropbear_ed25519_host_key"
fi

# Stage musl's dynamic loader (its shared libc) so dynamically linked programs
# can run, plus the dynamic-linking smoke binaries when built.
if [[ -f "${ROOT_DIR}/musl/shared/lib/libc.so" ]]; then
  mkdir -p "${STAGE_ROOT}/lib"
  cp "${ROOT_DIR}/musl/shared/lib/libc.so" "${STAGE_ROOT}/lib/ld-musl-x86_64.so.1"
  chmod 0755 "${STAGE_ROOT}/lib/ld-musl-x86_64.so.1"
fi
if [[ -x "${ROOT_DIR}/user/dynlink_smoke.elf" ]]; then
  mkdir -p "${STAGE_ROOT}/usr/lib"
  cp "${ROOT_DIR}/user/dynlink_smoke.elf" "${STAGE_ROOT}/bin/dynlink_smoke"
  chmod 0755 "${STAGE_ROOT}/bin/dynlink_smoke"
  cp "${ROOT_DIR}/user/libpeetdyn.so" "${ROOT_DIR}/user/libpeetplugin.so" "${STAGE_ROOT}/usr/lib/"
fi

# Shared builds of the ported libraries, staged under their sonames for
# dynamic programs. With the rendering stack built (scripts/build_render_stack.sh)
# everything comes from its staged sysroot, plus fontconfig's /etc/fonts, the
# fonts, and the fc-* tools.
RENDER_STAGE="${ROOT_DIR}/wayland/out/stage"
if [[ -d "${RENDER_STAGE}/usr/lib" ]]; then
  mkdir -p "${STAGE_ROOT}/usr/lib"
  for lib in "${RENDER_STAGE}"/usr/lib/lib*.so*; do
    [[ -f "${lib}" && ! -L "${lib}" ]] || continue
    soname="$(readelf -d "${lib}" 2>/dev/null | sed -n 's/.*Library soname: \[\(.*\)\]/\1/p')"
    [[ -n "${soname}" ]] && cp "${lib}" "${STAGE_ROOT}/usr/lib/${soname}"
  done
  if [[ -d "${RENDER_STAGE}/etc/fonts" ]]; then
    mkdir -p "${STAGE_ROOT}/etc" "${STAGE_ROOT}/usr/share" "${STAGE_ROOT}/var/cache/fontconfig"
    # conf.d entries are symlinks into /usr/share/fontconfig/conf.avail;
    # stage them as plain files (dereferenced) along with conf.avail.
    cp -rL "${RENDER_STAGE}/etc/fonts" "${STAGE_ROOT}/etc/"
    cp -a "${RENDER_STAGE}/usr/share/fontconfig" "${STAGE_ROOT}/usr/share/"
    cp -a "${RENDER_STAGE}/usr/share/fonts" "${STAGE_ROOT}/usr/share/"
    for tool in fc-list fc-match fc-cache; do
      [[ -x "${RENDER_STAGE}/usr/bin/${tool}" ]] && cp "${RENDER_STAGE}/usr/bin/${tool}" "${STAGE_ROOT}/bin/"
    done
  fi
elif compgen -G "${ROOT_DIR}/wayland/out/lib/*.so.[0-9]*" >/dev/null; then
  mkdir -p "${STAGE_ROOT}/usr/lib"
  cp -L "${ROOT_DIR}"/wayland/out/lib/*.so.[0-9]* "${STAGE_ROOT}/usr/lib/"
fi
if [[ -x "${RENDER_STAGE}/usr/bin/foot" ]]; then
  cp "${RENDER_STAGE}/usr/bin/foot" "${STAGE_ROOT}/bin/foot"
  mkdir -p "${STAGE_ROOT}/etc/xdg/foot"
  cp "${RENDER_STAGE}/etc/xdg/foot/foot.ini" "${STAGE_ROOT}/etc/xdg/foot/" 2>/dev/null || true
fi
if [[ -x "${ROOT_DIR}/user/text_smoke.elf" ]]; then
  cp "${ROOT_DIR}/user/text_smoke.elf" "${STAGE_ROOT}/bin/text_smoke"
  chmod 0755 "${STAGE_ROOT}/bin/text_smoke"
fi

# C++ runtime from the PeetOS cross toolchain (toolchain/build-toolchain.sh):
# libstdc++, libgcc_s (the unwinder) and libatomic, under their sonames.
CXX_RUNTIME_DIR="${ROOT_DIR}/toolchain/out/x86_64-linux-musl/lib64"
if [[ -f "${CXX_RUNTIME_DIR}/libstdc++.so.6" ]]; then
  mkdir -p "${STAGE_ROOT}/usr/lib"
  for lib in libstdc++.so.6 libgcc_s.so.1 libatomic.so.1; do
    cp -L "${CXX_RUNTIME_DIR}/${lib}" "${STAGE_ROOT}/usr/lib/${lib}"
  done
fi
if [[ -x "${ROOT_DIR}/user/cxx_smoke.elf" ]]; then
  mkdir -p "${STAGE_ROOT}/usr/lib"
  cp "${ROOT_DIR}/user/cxx_smoke.elf" "${STAGE_ROOT}/bin/cxx_smoke"
  chmod 0755 "${STAGE_ROOT}/bin/cxx_smoke"
  cp "${ROOT_DIR}/user/libpeetcxx.so" "${ROOT_DIR}/user/libpeetcxxplugin.so" "${STAGE_ROOT}/usr/lib/"
fi

if [[ -x "${ROOT_DIR}/user/drm_smoke.elf" ]]; then
  cp "${ROOT_DIR}/user/drm_smoke.elf" "${STAGE_ROOT}/bin/drm_smoke"
  chmod 0755 "${STAGE_ROOT}/bin/drm_smoke"
fi

# Stage the verified xkeyboard-config subset (scripts/build_xkbcommon.sh) at
# libxkbcommon's default root when it has been built.
if [[ -f "${ROOT_DIR}/wayland/out/share/X11/xkb/rules/evdev" ]]; then
  mkdir -p "${STAGE_ROOT}/usr/share/X11"
  cp -a "${ROOT_DIR}/wayland/out/share/X11/xkb" "${STAGE_ROOT}/usr/share/X11/xkb"
fi

# Stage the peetwl compositor and wl-demo client when available.
if [[ -x "${ROOT_DIR}/user/peetwl.elf" ]]; then
  cp "${ROOT_DIR}/user/peetwl.elf" "${STAGE_ROOT}/bin/peetwl"
  chmod 0755 "${STAGE_ROOT}/bin/peetwl"
fi
if [[ -x "${ROOT_DIR}/user/wl_demo.elf" ]]; then
  cp "${ROOT_DIR}/user/wl_demo.elf" "${STAGE_ROOT}/bin/wl-demo"
  chmod 0755 "${STAGE_ROOT}/bin/wl-demo"
fi

# Stage libwayland server/client smoke helper when available.
if [[ -x "${ROOT_DIR}/user/wayland_smoke.elf" ]]; then
  cp "${ROOT_DIR}/user/wayland_smoke.elf" "${STAGE_ROOT}/bin/wayland_smoke"
  chmod 0755 "${STAGE_ROOT}/bin/wayland_smoke"
fi

# Stage upstream libwayland test-suite binaries when built
# (scripts/build_wayland_tests.sh); run by bin/wayland_tests.sh.
WAYLAND_TESTS_DIR="${ROOT_DIR}/wayland/out/tests"
if [[ -f "${WAYLAND_TESTS_DIR}/TESTS" ]]; then
  mkdir -p "${STAGE_ROOT}/wltests"
  cp "${WAYLAND_TESTS_DIR}/TESTS" "${STAGE_ROOT}/wltests/TESTS"
  chmod 0644 "${STAGE_ROOT}/wltests/TESTS"
  cp "${WAYLAND_TESTS_DIR}/exec-fd-leak-checker" "${STAGE_ROOT}/wltests/"
  while read -r wl_test; do
    cp "${WAYLAND_TESTS_DIR}/${wl_test}" "${STAGE_ROOT}/wltests/${wl_test}"
  done < "${WAYLAND_TESTS_DIR}/TESTS"
  chmod 0755 "${STAGE_ROOT}"/wltests/*-test "${STAGE_ROOT}/wltests/exec-fd-leak-checker"
fi

# Stage FPU/SSE context-switch smoke helper when available.
if [[ -x "${ROOT_DIR}/user/fpu_context_smoke.elf" ]]; then
  cp "${ROOT_DIR}/user/fpu_context_smoke.elf" "${STAGE_ROOT}/bin/fpu_context_smoke"
  chmod 0755 "${STAGE_ROOT}/bin/fpu_context_smoke"
fi

# Stage ext3 journal crash/recovery workload helper when available.
if [[ -x "${ROOT_DIR}/user/journal_smoke_helper.elf" ]]; then
  cp "${ROOT_DIR}/user/journal_smoke_helper.elf" "${STAGE_ROOT}/bin/journal_workload"
  chmod 0755 "${STAGE_ROOT}/bin/journal_workload"
fi

# Stage process-model (COW fork / pthreads) smoke helper when available.
if [[ -x "${ROOT_DIR}/user/proc_model_smoke.elf" ]]; then
  cp "${ROOT_DIR}/user/proc_model_smoke.elf" "${STAGE_ROOT}/bin/proc_model_smoke"
  chmod 0755 "${STAGE_ROOT}/bin/proc_model_smoke"
fi

# Stage AF_UNIX SCM_RIGHTS smoke helper when available.
if [[ -x "${ROOT_DIR}/user/unix_rights_smoke.elf" ]]; then
  cp "${ROOT_DIR}/user/unix_rights_smoke.elf" "${STAGE_ROOT}/bin/unix_rights_smoke"
  chmod 0755 "${STAGE_ROOT}/bin/unix_rights_smoke"
fi

# Stage Phase 2 GUI/Wayland smoke helpers when available.
if [[ -x "${ROOT_DIR}/user/epoll_compositor_smoke.elf" ]]; then
  cp "${ROOT_DIR}/user/epoll_compositor_smoke.elf" "${STAGE_ROOT}/bin/epoll_compositor_smoke"
  chmod 0755 "${STAGE_ROOT}/bin/epoll_compositor_smoke"
fi
if [[ -x "${ROOT_DIR}/user/gui_compositor_smoke.elf" ]]; then
  cp "${ROOT_DIR}/user/gui_compositor_smoke.elf" "${STAGE_ROOT}/bin/gui_compositor_smoke"
  chmod 0755 "${STAGE_ROOT}/bin/gui_compositor_smoke"
fi
if [[ -x "${ROOT_DIR}/user/shared_buffer_rights_smoke.elf" ]]; then
  cp "${ROOT_DIR}/user/shared_buffer_rights_smoke.elf" "${STAGE_ROOT}/bin/shared_buffer_rights_smoke"
  chmod 0755 "${STAGE_ROOT}/bin/shared_buffer_rights_smoke"
fi
if [[ -x "${ROOT_DIR}/user/gui_compositor_smoke.elf" ]]; then
  cp "${ROOT_DIR}/user/gui_compositor_smoke.elf" "${STAGE_ROOT}/bin/gui_compositor_smoke"
  chmod 0755 "${STAGE_ROOT}/bin/gui_compositor_smoke"
fi
if [[ -x "${ROOT_DIR}/user/peet_gui_compositor_smoke.elf" ]]; then
  cp "${ROOT_DIR}/user/peet_gui_compositor_smoke.elf" "${STAGE_ROOT}/bin/peet_gui_compositor_smoke"
  chmod 0755 "${STAGE_ROOT}/bin/peet_gui_compositor_smoke"
fi
if [[ -x "${ROOT_DIR}/user/peet_shell_demo.elf" ]]; then
  cp "${ROOT_DIR}/user/peet_shell_demo.elf" "${STAGE_ROOT}/bin/peet_shell_demo"
  chmod 0755 "${STAGE_ROOT}/bin/peet_shell_demo"
fi

# Applets are invoked as `toybox <applet>` to keep the smoke image compact.
cat > "${STAGE_ROOT}/bin/next_smoke.sh" <<'EOS'
fail=0
set -x
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
# ping in this toybox profile doesn't support --help, so just assert applet presence.
toybox | toybox grep -w ping >/dev/null || fail=1

echo [musl-ext2-cmd-smoke] done
if [ "$fail" -ne 0 ]; then
  echo [musl-ext2-cmd-smoke] failures
fi
EOS
chmod 0755 "${STAGE_ROOT}/bin/next_smoke.sh"

cat > "${STAGE_ROOT}/bin/ping_smoke.sh" <<'EOS'
fail=0
target=10.0.2.2

echo [musl-ext2-ping-smoke] pinging ${target}
toybox ping -c 1 -W 2 "${target}" || fail=1

echo [musl-ext2-ping-smoke] done
if [ "$fail" -ne 0 ]; then
  echo [musl-ext2-ping-smoke] failures
fi
EOS
chmod 0755 "${STAGE_ROOT}/bin/ping_smoke.sh"

cat > "${STAGE_ROOT}/bin/quickjs_smoke.sh" <<'EOS'
fail=0

if [ ! -x bin/qjs ]; then
  echo [musl-ext2-quickjs-smoke] missing bin/qjs
  fail=1
else
  if [ -f tmp/test_hello.js ]; then
    bin/qjs tmp/test_hello.js || fail=1
  else
    echo [musl-ext2-quickjs-smoke] missing tmp/test_hello.js
    fail=1
  fi

  if [ -f tmp/test_math.js ]; then
    bin/qjs tmp/test_math.js || fail=1
  else
    echo [musl-ext2-quickjs-smoke] missing tmp/test_math.js
    fail=1
  fi
fi

echo [musl-ext2-quickjs-smoke] done
if [ "$fail" -ne 0 ]; then
  echo [musl-ext2-quickjs-smoke] failures
fi
EOS
chmod 0755 "${STAGE_ROOT}/bin/quickjs_smoke.sh"

cat > "${STAGE_ROOT}/bin/micropython_smoke.sh" <<'EOS'
fail=0

if [ ! -x bin/micropython ]; then
  echo [musl-ext2-micropython-smoke] missing bin/micropython
  fail=1
else
  if [ -f tmp/test_hello.py ]; then
    bin/micropython tmp/test_hello.py || fail=1
  else
    echo [musl-ext2-micropython-smoke] missing tmp/test_hello.py
    fail=1
  fi

  if [ -f tmp/test_math.py ]; then
    bin/micropython tmp/test_math.py || fail=1
  else
    echo [musl-ext2-micropython-smoke] missing tmp/test_math.py
    fail=1
  fi
fi

echo [musl-ext2-micropython-smoke] done
if [ "$fail" -ne 0 ]; then
  echo [musl-ext2-micropython-smoke] failures
fi
EOS
chmod 0755 "${STAGE_ROOT}/bin/micropython_smoke.sh"

cat > "${STAGE_ROOT}/bin/gui_smoke.sh" <<'EOS'
fail=0

if [ ! -x bin/gui_compositor_smoke ]; then
  echo [musl-ext2-gui-smoke] missing bin/gui_compositor_smoke
  fail=1
else
  bin/gui_compositor_smoke smoke || fail=1
fi

echo [musl-ext2-gui-smoke] done
if [ "$fail" -ne 0 ]; then
  echo [musl-ext2-gui-smoke] failures
fi
EOS
chmod 0755 "${STAGE_ROOT}/bin/gui_smoke.sh"

cat > "${STAGE_ROOT}/bin/gui_demo.sh" <<'EOS'
fail=0

if [ ! -x bin/gui_compositor_smoke ]; then
  echo [musl-ext2-gui-demo] missing bin/gui_compositor_smoke
  fail=1
else
  bin/gui_compositor_smoke demo || fail=1
fi

echo [musl-ext2-gui-demo] done
if [ "$fail" -ne 0 ]; then
  echo [musl-ext2-gui-demo] failures
fi
EOS
chmod 0755 "${STAGE_ROOT}/bin/gui_demo.sh"

cat > "${STAGE_ROOT}/bin/dropbear_smoke.sh" <<'EOS'
fail=0
daemon_pid=
pidfile=/tmp/dropbear.pid
key_path=etc/dropbear/dropbear_ed25519_host_key

if [ ! -x bin/dropbear ]; then
  echo [musl-ext2-dropbear-smoke] missing bin/dropbear
  fail=1
else
  bin/dropbear -V || fail=1
fi

if [ ! -x bin/dropbearkey ]; then
  echo [musl-ext2-dropbear-smoke] missing bin/dropbearkey
  fail=1
else
  bin/dropbearkey -h >/tmp/dropbearkey_help.txt 2>&1 || fail=1
fi

if [ "$fail" -eq 0 ]; then
  if [ ! -f "$key_path" ]; then
    echo [musl-ext2-dropbear-smoke] missing "$key_path"
    fail=1
  fi
fi

if [ "$fail" -eq 0 ]; then
  rm -f "$pidfile"
  bin/dropbear -r "$key_path" -p 2222 -P "$pidfile" >/tmp/dropbeard.log 2>&1 || fail=1
  sleep 2
  if [ -f "$pidfile" ]; then
    daemon_pid="$(cat "$pidfile" 2>/dev/null)"
  fi
  if [ -n "$daemon_pid" ]; then
    echo [musl-ext2-dropbear-smoke] daemon running pid="$daemon_pid"
    toybox kill "$daemon_pid" >/dev/null 2>&1 || true
    daemon_pid=
  else
    bin/dropbear -F -r "$key_path" -p 2222 >/tmp/dropbear_probe.log 2>&1 &
    probe_pid="$!"
    sleep 1
    if toybox kill -0 "$probe_pid" >/dev/null 2>&1; then
      echo [musl-ext2-dropbear-smoke] daemon bind probe did not fail
      toybox kill "$probe_pid" >/dev/null 2>&1 || true
      wait "$probe_pid" >/dev/null 2>&1 || true
      fail=1
    else
      wait "$probe_pid" >/dev/null 2>&1 || true
      echo [musl-ext2-dropbear-smoke] daemon running
    fi
  fi
fi

if [ -n "$daemon_pid" ]; then
  toybox kill "$daemon_pid" >/dev/null 2>&1 || true
  wait "$daemon_pid" >/dev/null 2>&1 || true
fi

echo [musl-ext2-dropbear-smoke] done
if [ "$fail" -ne 0 ]; then
  echo [musl-ext2-dropbear-smoke] failures
fi
EOS
chmod 0755 "${STAGE_ROOT}/bin/dropbear_smoke.sh"

cat > "${STAGE_ROOT}/bin/unix_rights_smoke.sh" <<'EOS'
fail=0

if [ ! -x bin/unix_rights_smoke ]; then
  echo [musl-ext2-unix-rights-smoke] missing bin/unix_rights_smoke
  fail=1
else
  bin/unix_rights_smoke || fail=1
fi

echo [musl-ext2-unix-rights-smoke] done
if [ "$fail" -ne 0 ]; then
  echo [musl-ext2-unix-rights-smoke] failures
fi
EOS
chmod 0755 "${STAGE_ROOT}/bin/unix_rights_smoke.sh"

# Runs inside foot: proves the terminal shows output and passes keyboard
# input through its pty to the shell.
cat > "${STAGE_ROOT}/bin/foot_child.sh" <<'EOS'
echo "[foot-child] started TERM=$TERM" > /dev/console
echo "Hello from foot on PeetOS!"
echo "fcft + fontconfig + freetype + pixman, under peetwl via DRM/KMS"
echo
ls /
echo
printf 'type a line: '
echo "[foot-child] prompt shown" > /dev/console
line=$(head -n 1)
echo "you typed: $line"
echo "[foot-child] got $line" > /dev/console
sleep 1
EOS

cat > "${STAGE_ROOT}/bin/foot_smoke.sh" <<'EOS'
fail=0
export LANG=C.UTF-8 HOME=/tmp

if [ ! -x bin/peetwl ] || [ ! -x bin/foot ]; then
  echo [musl-ext2-foot-smoke] missing bin/peetwl or bin/foot
  fail=1
else
  bin/peetwl --smoke \
    --client "/mnt/ext2/bin/foot -e /mnt/ext2/bin/toybox sh /mnt/ext2/bin/foot_child.sh" || fail=1
fi

echo [musl-ext2-foot-smoke] done
if [ "$fail" -ne 0 ]; then
  echo [musl-ext2-foot-smoke] failures
fi
EOS
chmod 0755 "${STAGE_ROOT}/bin/foot_smoke.sh"

# Interactive foot session (make run-foot-demo): peetwl runs one foot
# window with a toybox shell. `exit` or ^D closes foot; peetwl (not in
# --smoke mode) keeps running with an empty desktop until QEMU is closed.
cat > "${STAGE_ROOT}/bin/foot_demo_shell.sh" <<'EOS'
cd /mnt/ext2
echo "Welcome to foot on PeetOS."
echo "This is toybox sh in a pty; there is no line editing or history yet."
echo
exec toybox sh -i
EOS

cat > "${STAGE_ROOT}/bin/foot_demo.sh" <<'EOS'
export LANG=C.UTF-8 HOME=/tmp
if [ ! -x bin/peetwl ] || [ ! -x bin/foot ]; then
  echo [musl-ext2-foot-demo] missing bin/peetwl or bin/foot
  exit 1
fi
echo [musl-ext2-foot-demo] starting peetwl with an interactive foot terminal
exec bin/peetwl \
  --client "/mnt/ext2/bin/foot -e /mnt/ext2/bin/toybox sh /mnt/ext2/bin/foot_demo_shell.sh"
EOS
chmod 0755 "${STAGE_ROOT}/bin/foot_demo.sh"

cat > "${STAGE_ROOT}/bin/text_smoke.sh" <<'EOS'
fail=0

if [ ! -x bin/text_smoke ] || [ ! -f /etc/fonts/fonts.conf ]; then
  echo [musl-ext2-text-smoke] missing bin/text_smoke or /etc/fonts/fonts.conf
  fail=1
else
  echo [musl-ext2-text-smoke] fc-list:
  bin/fc-list : family file || fail=1
  bin/fc-match monospace || fail=1
  bin/text_smoke || fail=1
fi

echo [musl-ext2-text-smoke] done
if [ "$fail" -ne 0 ]; then
  echo [musl-ext2-text-smoke] failures
fi
EOS
chmod 0755 "${STAGE_ROOT}/bin/text_smoke.sh"

cat > "${STAGE_ROOT}/bin/cxx_smoke.sh" <<'EOS'
fail=0

if [ ! -x bin/cxx_smoke ] || [ ! -f /usr/lib/libstdc++.so.6 ]; then
  echo [musl-ext2-cxx-smoke] missing bin/cxx_smoke or /usr/lib/libstdc++.so.6
  fail=1
else
  bin/cxx_smoke || fail=1
fi

echo [musl-ext2-cxx-smoke] done
if [ "$fail" -ne 0 ]; then
  echo [musl-ext2-cxx-smoke] failures
fi
EOS
chmod 0755 "${STAGE_ROOT}/bin/cxx_smoke.sh"

cat > "${STAGE_ROOT}/bin/drm_smoke.sh" <<'EOS'
fail=0

if [ ! -x bin/drm_smoke ] || [ ! -f /usr/lib/libdrm.so.2 ]; then
  echo [musl-ext2-drm-smoke] missing bin/drm_smoke or /usr/lib/libdrm.so.2
  fail=1
else
  bin/drm_smoke || fail=1
fi

echo [musl-ext2-drm-smoke] done
if [ "$fail" -ne 0 ]; then
  echo [musl-ext2-drm-smoke] failures
fi
EOS
chmod 0755 "${STAGE_ROOT}/bin/drm_smoke.sh"

cat > "${STAGE_ROOT}/bin/dynlink_smoke.sh" <<'EOS'
fail=0

if [ ! -x bin/dynlink_smoke ] || [ ! -f /lib/ld-musl-x86_64.so.1 ]; then
  echo [musl-ext2-dynlink-smoke] missing bin/dynlink_smoke or /lib/ld-musl-x86_64.so.1
  fail=1
else
  bin/dynlink_smoke || fail=1
fi

echo [musl-ext2-dynlink-smoke] done
if [ "$fail" -ne 0 ]; then
  echo [musl-ext2-dynlink-smoke] failures
fi
EOS
chmod 0755 "${STAGE_ROOT}/bin/dynlink_smoke.sh"

cat > "${STAGE_ROOT}/bin/wlcomp_smoke.sh" <<'EOS'
fail=0

if [ ! -x bin/peetwl ] || [ ! -x bin/wl-demo ]; then
  echo [musl-ext2-wlcomp-smoke] missing bin/peetwl or bin/wl-demo
  fail=1
else
  bin/peetwl --smoke --xkb-layout de \
    --client "/mnt/ext2/bin/wl-demo Alpha 0xd05050" \
    --client "/mnt/ext2/bin/wl-demo Beta 0x4080d0" || fail=1
fi

echo [musl-ext2-wlcomp-smoke] done
if [ "$fail" -ne 0 ]; then
  echo [musl-ext2-wlcomp-smoke] failures
fi
EOS
chmod 0755 "${STAGE_ROOT}/bin/wlcomp_smoke.sh"

cat > "${STAGE_ROOT}/bin/wayland_demo.sh" <<'EOS'
if [ ! -x bin/peetwl ] || [ ! -x bin/wl-demo ]; then
  echo [musl-ext2-wayland-demo] missing bin/peetwl or bin/wl-demo
  exit 1
fi
echo [musl-ext2-wayland-demo] starting peetwl with three wl-demo windows
exec bin/peetwl \
  --client "/mnt/ext2/bin/wl-demo One 0x3070c0 --animate" \
  --client "/mnt/ext2/bin/wl-demo Two 0xc07030" \
  --client "/mnt/ext2/bin/wl-demo Three 0x40a060 --size 240x160"
EOS
chmod 0755 "${STAGE_ROOT}/bin/wayland_demo.sh"

cat > "${STAGE_ROOT}/bin/wayland_smoke.sh" <<'EOS'
fail=0

if [ ! -x bin/wayland_smoke ]; then
  echo [musl-ext2-wayland-smoke] missing bin/wayland_smoke
  fail=1
else
  bin/wayland_smoke || fail=1
fi

echo [musl-ext2-wayland-smoke] done
if [ "$fail" -ne 0 ]; then
  echo [musl-ext2-wayland-smoke] failures
fi
EOS
chmod 0755 "${STAGE_ROOT}/bin/wayland_smoke.sh"

cat > "${STAGE_ROOT}/bin/wayland_tests.sh" <<'EOS'
fail=0
# Upstream test-runner: exec-fd-leak-checker lives next to the tests, and
# each runner creates its own wayland-tests-XXXXXX dir under XDG_RUNTIME_DIR.
export TEST_BUILD_DIR=/mnt/ext2/wltests
export XDG_RUNTIME_DIR=/tmp

if [ ! -f wltests/TESTS ]; then
  echo [musl-ext2-wayland-tests] missing wltests/TESTS
  fail=1
else
  for t in $(toybox cat wltests/TESTS); do
    echo "[wl-tests] run $t"
    wltests/$t
    rc=$?
    if [ "$rc" -eq 0 ]; then
      echo "[wl-tests] PASS $t"
    else
      echo "[wl-tests] FAIL $t rc=$rc"
      fail=1
    fi
  done
fi

echo [musl-ext2-wayland-tests] done
if [ "$fail" -ne 0 ]; then
  echo [musl-ext2-wayland-tests] failures
fi
EOS
chmod 0755 "${STAGE_ROOT}/bin/wayland_tests.sh"

cat > "${STAGE_ROOT}/bin/fpu_smoke.sh" <<'EOS'
fail=0

if [ ! -x bin/fpu_context_smoke ]; then
  echo [musl-ext2-fpu-smoke] missing bin/fpu_context_smoke
  fail=1
else
  bin/fpu_context_smoke || fail=1
fi

echo [musl-ext2-fpu-smoke] done
if [ "$fail" -ne 0 ]; then
  echo [musl-ext2-fpu-smoke] failures
fi
EOS
chmod 0755 "${STAGE_ROOT}/bin/fpu_smoke.sh"
cat > "${STAGE_ROOT}/bin/proc_smoke.sh" <<'EOS'
fail=0

if [ ! -x bin/proc_model_smoke ]; then
  echo [musl-ext2-proc-smoke] missing bin/proc_model_smoke
  fail=1
else
  bin/proc_model_smoke || fail=1
fi

echo [musl-ext2-proc-smoke] done
if [ "$fail" -ne 0 ]; then
  echo [musl-ext2-proc-smoke] failures
fi
EOS
chmod 0755 "${STAGE_ROOT}/bin/proc_smoke.sh"

# ext3 journal workload (musl-ext3-journal-smoke); the logic lives in the
# bin/journal_workload helper (user/journal_smoke_helper.c). Boot 2 sees the
# baseline written by boot 1 and switches to verification.
cat > "${STAGE_ROOT}/bin/journal_smoke.sh" <<'EOS'
if [ ! -x bin/journal_workload ]; then
  echo [ext3-journal-smoke] missing bin/journal_workload
  echo [ext3-journal-smoke] failures
elif [ -f jsmoke/baseline ]; then
  bin/journal_workload verify
else
  bin/journal_workload write
fi
EOS
chmod 0755 "${STAGE_ROOT}/bin/journal_smoke.sh"

cat > "${STAGE_ROOT}/bin/epoll_smoke.sh" <<'EOS'
fail=0

if [ ! -x bin/epoll_compositor_smoke ]; then
  echo [musl-ext2-epoll-smoke] missing bin/epoll_compositor_smoke
  fail=1
else
  bin/epoll_compositor_smoke || fail=1
fi

echo [musl-ext2-epoll-smoke] done
if [ "$fail" -ne 0 ]; then
  echo [musl-ext2-epoll-smoke] failures
fi
EOS
chmod 0755 "${STAGE_ROOT}/bin/epoll_smoke.sh"

cat > "${STAGE_ROOT}/bin/shared_buffer_smoke.sh" <<'EOS'
fail=0

if [ ! -x bin/shared_buffer_rights_smoke ]; then
  echo [musl-ext2-shared-buffer-smoke] missing bin/shared_buffer_rights_smoke
  fail=1
else
  bin/shared_buffer_rights_smoke || fail=1
fi

echo [musl-ext2-shared-buffer-smoke] done
if [ "$fail" -ne 0 ]; then
  echo [musl-ext2-shared-buffer-smoke] failures
fi
EOS
chmod 0755 "${STAGE_ROOT}/bin/shared_buffer_smoke.sh"

cat > "${STAGE_ROOT}/bin/console_first_smoke.sh" <<'EOS'
fail=0
if [ "${PEETOS_DISPLAY_MODE}" != "console-first" ]; then
  echo [musl-ext2-console-first-smoke] unexpected-display-mode "${PEETOS_DISPLAY_MODE}"
  fail=1
fi
pwd || fail=1
toybox ls /bin >/dev/null || fail=1
echo console-first >/tmp/console_first || fail=1
toybox cat /tmp/console_first || fail=1
toybox uname || fail=1
toybox rm -f /tmp/console_first || fail=1

echo [musl-ext2-console-first-smoke] done
if [ "$fail" -ne 0 ]; then
  echo [musl-ext2-console-first-smoke] failures
fi
EOS
chmod 0755 "${STAGE_ROOT}/bin/console_first_smoke.sh"

cat > "${STAGE_ROOT}/bin/gui_smoke.sh" <<'EOS'
fail=0
if [ "${PEETOS_DISPLAY_MODE}" != "compositor-first" ]; then
  echo [musl-ext2-gui-smoke] unexpected-display-mode "${PEETOS_DISPLAY_MODE}"
  fail=1
fi
if [ "${PEETOS_FB}" != "/dev/fb0" ]; then
  echo [musl-ext2-gui-smoke] unexpected-fb "${PEETOS_FB}"
  fail=1
fi
if [ "${PEETOS_INPUT_KEYBOARD}" != "/dev/input/event0" ]; then
  echo [musl-ext2-gui-smoke] unexpected-kbd "${PEETOS_INPUT_KEYBOARD}"
  fail=1
fi
if [ "${PEETOS_INPUT_POINTER}" != "/dev/input/event1" ]; then
  echo [musl-ext2-gui-smoke] unexpected-ptr "${PEETOS_INPUT_POINTER}"
  fail=1
fi
/bin/gui_compositor_smoke || fail=1

echo [musl-ext2-gui-smoke] done
if [ "$fail" -ne 0 ]; then
  echo [musl-ext2-gui-smoke] failures
fi
EOS
chmod 0755 "${STAGE_ROOT}/bin/gui_smoke.sh"

cat > "${STAGE_ROOT}/bin/peet_gui_smoke.sh" <<'EOS'
fail=0
if [ "${PEETOS_DISPLAY_MODE}" != "compositor-first" ]; then
  echo [musl-ext2-peet-gui-smoke] unexpected-display-mode "${PEETOS_DISPLAY_MODE}"
  fail=1
fi
if [ "${PEETOS_FB}" != "/dev/fb0" ]; then
  echo [musl-ext2-peet-gui-smoke] unexpected-fb "${PEETOS_FB}"
  fail=1
fi
if [ "${PEETOS_INPUT_KEYBOARD}" != "/dev/input/event0" ]; then
  echo [musl-ext2-peet-gui-smoke] unexpected-kbd "${PEETOS_INPUT_KEYBOARD}"
  fail=1
fi
if [ "${PEETOS_INPUT_POINTER}" != "/dev/input/event1" ]; then
  echo [musl-ext2-peet-gui-smoke] unexpected-ptr "${PEETOS_INPUT_POINTER}"
  fail=1
fi
if [ ! -x bin/peet_gui_compositor_smoke ]; then
  echo [musl-ext2-peet-gui-smoke] missing bin/peet_gui_compositor_smoke
  fail=1
else
  /bin/peet_gui_compositor_smoke || fail=1
fi

echo [musl-ext2-peet-gui-smoke] done
if [ "$fail" -ne 0 ]; then
  echo [musl-ext2-peet-gui-smoke] failures
fi
EOS
chmod 0755 "${STAGE_ROOT}/bin/peet_gui_smoke.sh"

cat > "${STAGE_ROOT}/bin/peet_shell_demo.sh" <<'EOS'
fail=0
if [ "${PEETOS_DISPLAY_MODE}" != "compositor-first" ]; then
  echo [musl-ext2-peet-shell-demo] unexpected-display-mode "${PEETOS_DISPLAY_MODE}"
  fail=1
fi
if [ "${PEETOS_FB}" != "/dev/fb0" ]; then
  echo [musl-ext2-peet-shell-demo] unexpected-fb "${PEETOS_FB}"
  fail=1
fi
if [ "${PEETOS_INPUT_KEYBOARD}" != "/dev/input/event0" ]; then
  echo [musl-ext2-peet-shell-demo] unexpected-kbd "${PEETOS_INPUT_KEYBOARD}"
  fail=1
fi
if [ "${PEETOS_INPUT_POINTER}" != "/dev/input/event1" ]; then
  echo [musl-ext2-peet-shell-demo] unexpected-ptr "${PEETOS_INPUT_POINTER}"
  fail=1
fi
if [ ! -x bin/peet_shell_demo ]; then
  echo [musl-ext2-peet-shell-demo] missing bin/peet_shell_demo
  fail=1
else
  /bin/peet_shell_demo || fail=1
fi

echo [musl-ext2-peet-shell-demo] done
if [ "$fail" -ne 0 ]; then
  echo [musl-ext2-peet-shell-demo] failures
fi
EOS
chmod 0755 "${STAGE_ROOT}/bin/peet_shell_demo.sh"

TMP_IMG="${TMP_DIR}/ext2.img"
# Keep image compact but size it from staged payload so larger binaries
# (for example QuickJS qjs) do not overflow ext2 population.
payload_kib="$(du -sk "${STAGE_ROOT}" | awk '{print $1}')"
# Keep a modest metadata/slack margin above staged payload size.
img_kib=$((payload_kib + 1024))
if [[ -d "${STAGE_ROOT}/wltests" ]]; then
  # Upstream libwayland tests create sockets, locks and temp dirs in /tmp.
  img_kib=$((img_kib + 1024))
fi
if [[ "${img_kib}" -lt 3072 ]]; then
  img_kib=3072
fi
# Stock mke2fs ext2 features (ext_attr, resize_inode, dir_index, filetype,
# sparse_super, large_file): the kernel implements all of them, so the runtime
# image needs no feature overrides.
# Inodes: one per staged file/dir plus headroom for files created at runtime
# (sockets, locks, memfd backing files, test temp files).
staged_entries="$(find "${STAGE_ROOT}" | wc -l)"
inode_count=$((staged_entries + 256))
if [[ "${inode_count}" -lt 384 ]]; then
  inode_count=384
fi
mke2fs -q -t ext2 -F -b 1024 -I 256 -N "${inode_count}" \
  -L PEETOS_EXT2 -d "${STAGE_ROOT}" "${TMP_IMG}" "${img_kib}"

mv "${TMP_IMG}" "${OUT_IMG}"
chmod 0644 "${OUT_IMG}"

echo "Built ${OUT_IMG}"
