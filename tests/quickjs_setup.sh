#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP_DIR="$(mktemp -d)"

cleanup() {
  rm -rf "${TMP_DIR}"
}
trap cleanup EXIT

FAKE_VERSION="0.0.0-test"
FAKE_SRC_DIR="${TMP_DIR}/quickjs-${FAKE_VERSION}"
FAKE_ARCHIVE="${TMP_DIR}/quickjs-${FAKE_VERSION}.tar.xz"
WORK_DIR="${TMP_DIR}/work"

mkdir -p "${FAKE_SRC_DIR}"
cat > "${FAKE_SRC_DIR}/qjs.c" <<'EOS'
int main(void) { return 0; }
EOS
cat > "${FAKE_SRC_DIR}/quickjs-libc.c" <<'EOS'
/* enable the os.Worker API. IT relies on POSIX threads */
#define USE_WORKER
EOS

# xz archive to match upstream quickjs release format.
tar -cJf "${FAKE_ARCHIVE}" -C "${TMP_DIR}" "quickjs-${FAKE_VERSION}"

"${ROOT_DIR}/quickjs/fetch-quickjs.sh" \
  --version "${FAKE_VERSION}" \
  --archive "${FAKE_ARCHIVE}" \
  --output-dir "${WORK_DIR}"

if [[ ! -f "${WORK_DIR}/quickjs-src/qjs.c" ]]; then
  echo "fetch-quickjs.sh did not materialize quickjs-src/qjs.c" >&2
  exit 1
fi

if [[ ! -f "${ROOT_DIR}/quickjs/peetos/Makefile.peetos" ]]; then
  echo "quickjs peetos Makefile template missing" >&2
  exit 1
fi

if [[ ! -f "${ROOT_DIR}/quickjs/peetos/repl_stub.c" ]]; then
  echo "quickjs repl stub template missing" >&2
  exit 1
fi

if MUSL_CC="${WORK_DIR}/missing-musl-gcc" "${ROOT_DIR}/scripts/build_quickjs_qjs.sh" --quickjs-src "${WORK_DIR}/quickjs-src" >/dev/null 2>&1; then
  echo "build_quickjs_qjs.sh should fail when musl compiler wrapper is missing" >&2
  exit 1
fi
