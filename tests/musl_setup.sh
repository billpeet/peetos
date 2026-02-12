#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP_DIR="$(mktemp -d)"

cleanup() {
  rm -rf "${TMP_DIR}"
}
trap cleanup EXIT

FAKE_VERSION="9.9.9"
FAKE_SRC_DIR="${TMP_DIR}/musl-${FAKE_VERSION}"
FAKE_ARCHIVE="${TMP_DIR}/musl-${FAKE_VERSION}.tar.gz"
WORK_DIR="${TMP_DIR}/work"

mkdir -p "${FAKE_SRC_DIR}"
cat > "${FAKE_SRC_DIR}/configure" <<'EOS'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$@" > configure.args
: > config.mak
EOS
chmod +x "${FAKE_SRC_DIR}/configure"

tar -czf "${FAKE_ARCHIVE}" -C "${TMP_DIR}" "musl-${FAKE_VERSION}"

"${ROOT_DIR}/musl/fetch-musl.sh" \
  --version "${FAKE_VERSION}" \
  --archive "${FAKE_ARCHIVE}" \
  --output-dir "${WORK_DIR}"

if [[ ! -x "${WORK_DIR}/musl-src/configure" ]]; then
  echo "fetch-musl.sh did not materialize musl-src/configure" >&2
  exit 1
fi

DRY_RUN_OUT="$("${ROOT_DIR}/musl/configure-musl.sh" \
  --musl-dir "${WORK_DIR}/musl-src" \
  --sysroot "${WORK_DIR}/sysroot" \
  --target "x86_64-linux-musl" \
  --cross-prefix "x86_64-peetos-" \
  --dry-run)"

echo "${DRY_RUN_OUT}" | rg -q -- "--target=x86_64-linux-musl"
echo "${DRY_RUN_OUT}" | rg -q -- "CC=x86_64-peetos-gcc"

"${ROOT_DIR}/musl/configure-musl.sh" \
  --musl-dir "${WORK_DIR}/musl-src" \
  --sysroot "${WORK_DIR}/sysroot" \
  --target "x86_64-linux-musl" \
  --cc "fake-cc" \
  --ar "fake-ar" \
  --ranlib "fake-ranlib" \
  --cflags "-O0"

if [[ ! -f "${WORK_DIR}/musl-src/config.mak" ]]; then
  echo "configure-musl.sh did not run configure script" >&2
  exit 1
fi

rg -q -- "--target=x86_64-linux-musl" "${WORK_DIR}/musl-src/configure.args"
rg -q -- "CC=fake-cc" "${WORK_DIR}/musl-src/configure.args"
rg -q -- "AR=fake-ar" "${WORK_DIR}/musl-src/configure.args"
rg -q -- "RANLIB=fake-ranlib" "${WORK_DIR}/musl-src/configure.args"

if MUSL_CC="${WORK_DIR}/missing-musl-gcc" "${ROOT_DIR}/scripts/build_musl_echo.sh" >/dev/null 2>&1; then
  echo "build_musl_echo.sh should fail when musl compiler wrapper is missing" >&2
  exit 1
fi
