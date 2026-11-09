#!/usr/bin/env bash
set -euo pipefail

# Build the PeetOS C/C++ cross toolchain: GCC (C and C++) targeting
# x86_64-linux-musl against the repo's musl, with libstdc++, libsupc++,
# libgcc_s and libatomic as shared and static target libraries.
#
# Layout (all under toolchain/):
#   out/bin/x86_64-linux-musl-{gcc,g++,cpp,...}   the cross compilers
#   out/bin/x86_64-linux-musl-{as,ld,ar,...}      symlinks to the build
#                                                 machine's binutils
#   out/x86_64-linux-musl/lib64/                    libstdc++.so.6, libgcc_s.so.1,
#                                                 libatomic.so.1 and .a files
#   sysroot/usr/{include,lib}                     musl headers, crt objects,
#                                                 libc.a and libc.so
#
# No binutils are built: PeetOS user programs are plain x86_64 ELF, which the
# build machine's GNU as/ld already handle. Programs get the musl loader
# (/lib/ld-musl-x86_64.so.1) as PT_INTERP, the GCC default for this target.
#
# Needs musl built first (`make musl` and `make -C musl shared`), the sources
# from fetch-toolchain.sh, and a host C/C++ compiler plus m4 (for GMP).

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
TARGET="x86_64-linux-musl"
PREFIX="${SCRIPT_DIR}/out"
SYSROOT="${SCRIPT_DIR}/sysroot"
BUILD="${SCRIPT_DIR}/build"
GCC_SRC="${SCRIPT_DIR}/gcc-src"
MUSL_SYSROOT="${ROOT_DIR}/musl/sysroot"
MUSL_SHARED_LIBC="${ROOT_DIR}/musl/shared/lib/libc.so"
JOBS="${JOBS:-$(nproc 2>/dev/null || echo 2)}"

for tool in gcc g++ make m4 as ld ar; do
  if ! command -v "${tool}" >/dev/null 2>&1; then
    echo "${tool} not found on the build machine (see docs/cxx.md)" >&2
    exit 1
  fi
done
if [[ ! -d "${GCC_SRC}" || ! -e "${GCC_SRC}/gmp" ]]; then
  echo "GCC sources not found (run 'make -C toolchain fetch')" >&2
  exit 1
fi
if [[ ! -f "${MUSL_SYSROOT}/lib/libc.a" || ! -f "${MUSL_SHARED_LIBC}" ]]; then
  echo "musl not built (run 'make musl' and 'make -C musl shared')" >&2
  exit 1
fi

rm -rf "${PREFIX}" "${SYSROOT}" "${BUILD}"
mkdir -p "${PREFIX}/bin" "${PREFIX}/${TARGET}/bin" "${SYSROOT}/usr/lib" "${SYSROOT}/lib" "${BUILD}/gcc"

# --- sysroot: musl headers, startup objects and libraries ---------------------
cp -a "${MUSL_SYSROOT}/include" "${SYSROOT}/usr/include"
for f in "${MUSL_SYSROOT}"/lib/*; do
  case "$(basename "${f}")" in
    musl-gcc.specs) ;;
    *) cp -a "${f}" "${SYSROOT}/usr/lib/" ;;
  esac
done
# The shared libc (also the dynamic loader) replaces nothing in the static
# set; -lc prefers libc.so, -static still picks libc.a.
cp "${MUSL_SHARED_LIBC}" "${SYSROOT}/usr/lib/libc.so"
ln -sfn ../usr/lib/libc.so "${SYSROOT}/lib/ld-musl-x86_64.so.1"

# --- binutils: the build machine's, under target-prefixed names ----------------
for tool in as ld ar nm ranlib strip objcopy objdump readelf; do
  path="$(command -v "${tool}")"
  ln -sfn "${path}" "${PREFIX}/bin/${TARGET}-${tool}"
  ln -sfn "${path}" "${PREFIX}/${TARGET}/bin/${tool}"
done
export PATH="${PREFIX}/bin:${PATH}"

# --- GCC ---------------------------------------------------------------------------
echo "[toolchain] configuring GCC ($(basename "$(readlink -f "${GCC_SRC}")"))"
(
  cd "${BUILD}/gcc"
  "${GCC_SRC}/configure" \
    --target="${TARGET}" \
    --prefix="${PREFIX}" \
    --with-sysroot="${SYSROOT}" \
    --with-build-sysroot="${SYSROOT}" \
    --enable-languages=c,c++ \
    --disable-multilib \
    --disable-bootstrap \
    --disable-nls \
    --disable-werror \
    --disable-libsanitizer \
    --disable-libssp \
    --disable-libgomp \
    --disable-libquadmath \
    --disable-libitm \
    --disable-libvtv \
    --disable-gcov \
    --enable-shared \
    --enable-threads=posix \
    --enable-tls \
    --enable-__cxa_atexit \
    --enable-default-pie \
    --enable-libstdcxx-time=rt \
    --with-pic \
    > "${BUILD}/gcc-configure.log" 2>&1 || { tail -40 "${BUILD}/gcc-configure.log" >&2; exit 1; }
  echo "[toolchain] building GCC with ${JOBS} jobs (several minutes)"
  make -j"${JOBS}" all-gcc all-target-libgcc all-target-libstdc++-v3 all-target-libatomic \
    > "${BUILD}/gcc-build.log" 2>&1 || { grep -n -m5 -B2 -A8 'error' "${BUILD}/gcc-build.log" | tail -40 >&2; tail -30 "${BUILD}/gcc-build.log" >&2; exit 1; }
  make install-strip-gcc install-strip-target-libgcc install-strip-target-libstdc++-v3 install-strip-target-libatomic \
    > "${BUILD}/gcc-install.log" 2>&1 || { tail -30 "${BUILD}/gcc-install.log" >&2; exit 1; }
)

# libtool archives carry absolute build paths that confuse later links; the
# gdb pretty-printer loader is host-side only.
find "${PREFIX}" \( -name '*.la' -o -name '*-gdb.py' \) -delete

CXX="${PREFIX}/bin/${TARGET}-g++"
echo "[toolchain] $("${CXX}" --version | head -n 1)"
echo "[toolchain] target libraries:"
ls "${PREFIX}/${TARGET}/lib64" | grep -E '\.so\.[0-9]+$' | tr '\n' ' '
echo
