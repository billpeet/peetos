#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP_DIR="$(mktemp -d)"

cleanup() {
  rm -rf "${TMP_DIR}"
}
trap cleanup EXIT

FAKE_VERSION="0.0.0-test"
FAKE_SRC_DIR="${TMP_DIR}/dropbear-${FAKE_VERSION}"
FAKE_ARCHIVE="${TMP_DIR}/dropbear-${FAKE_VERSION}.tar.bz2"
WORK_DIR="${TMP_DIR}/work"

mkdir -p "${FAKE_SRC_DIR}"
cat > "${FAKE_SRC_DIR}/configure" <<'EOS'
#!/usr/bin/env sh
exit 0
EOS
chmod +x "${FAKE_SRC_DIR}/configure"
cat > "${FAKE_SRC_DIR}/CHANGES" <<'EOS'
fake dropbear tree
EOS

tar -cjf "${FAKE_ARCHIVE}" -C "${TMP_DIR}" "dropbear-${FAKE_VERSION}"

"${ROOT_DIR}/dropbear/fetch-dropbear.sh" \
  --version "${FAKE_VERSION}" \
  --archive "${FAKE_ARCHIVE}" \
  --output-dir "${WORK_DIR}"

if [[ ! -x "${WORK_DIR}/dropbear-src/configure" ]]; then
  echo "fetch-dropbear.sh did not materialize dropbear-src/configure" >&2
  exit 1
fi

if [[ ! -f "${ROOT_DIR}/dropbear/peetos/localoptions.h" ]]; then
  echo "dropbear peetos localoptions template missing" >&2
  exit 1
fi

if MUSL_CC="${WORK_DIR}/missing-musl-gcc" "${ROOT_DIR}/scripts/build_dropbear_multi.sh" --dropbear-src "${WORK_DIR}/dropbear-src" >/dev/null 2>&1; then
  echo "build_dropbear_multi.sh should fail when musl compiler wrapper is missing" >&2
  exit 1
fi
