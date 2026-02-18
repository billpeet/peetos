#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP_DIR="$(mktemp -d)"

cleanup() {
  rm -rf "${TMP_DIR}"
}
trap cleanup EXIT

FAKE_VERSION="0.0.0-test"
FAKE_SRC_DIR="${TMP_DIR}/micropython-${FAKE_VERSION}"
FAKE_ARCHIVE="${TMP_DIR}/micropython-${FAKE_VERSION}.tar.gz"
WORK_DIR="${TMP_DIR}/work"

mkdir -p "${FAKE_SRC_DIR}/ports/unix" "${FAKE_SRC_DIR}/mpy-cross"
cat > "${FAKE_SRC_DIR}/ports/unix/Makefile" <<'EOS'
all:
	@echo fake
EOS
cat > "${FAKE_SRC_DIR}/mpy-cross/Makefile" <<'EOS'
all:
	@echo fake
EOS

tar -czf "${FAKE_ARCHIVE}" -C "${TMP_DIR}" "micropython-${FAKE_VERSION}"

"${ROOT_DIR}/micropython/fetch-micropython.sh" \
  --version "${FAKE_VERSION}" \
  --archive "${FAKE_ARCHIVE}" \
  --output-dir "${WORK_DIR}"

if [[ ! -f "${WORK_DIR}/micropython-src/ports/unix/Makefile" ]]; then
  echo "fetch-micropython.sh did not materialize micropython-src/ports/unix/Makefile" >&2
  exit 1
fi

if MUSL_CC="${WORK_DIR}/missing-musl-gcc" "${ROOT_DIR}/scripts/build_micropython_unix.sh" --micropython-src "${WORK_DIR}/micropython-src" >/dev/null 2>&1; then
  echo "build_micropython_unix.sh should fail when musl compiler wrapper is missing" >&2
  exit 1
fi
