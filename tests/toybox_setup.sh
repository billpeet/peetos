#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP_DIR="$(mktemp -d)"

cleanup() {
  rm -rf "${TMP_DIR}"
}
trap cleanup EXIT

FAKE_VERSION="0.0.0-test"
FAKE_SRC_DIR="${TMP_DIR}/toybox-${FAKE_VERSION}"
FAKE_ARCHIVE="${TMP_DIR}/toybox-${FAKE_VERSION}.tar.gz"
WORK_DIR="${TMP_DIR}/work"

mkdir -p "${FAKE_SRC_DIR}"
cat > "${FAKE_SRC_DIR}/Makefile" <<'EOS'
all:
	@echo fake
EOS

tar -czf "${FAKE_ARCHIVE}" -C "${TMP_DIR}" "toybox-${FAKE_VERSION}"

"${ROOT_DIR}/toybox/fetch-toybox.sh" \
  --version "${FAKE_VERSION}" \
  --archive "${FAKE_ARCHIVE}" \
  --output-dir "${WORK_DIR}"

if [[ ! -f "${WORK_DIR}/toybox-src/Makefile" ]]; then
  echo "fetch-toybox.sh did not materialize toybox-src/Makefile" >&2
  exit 1
fi

if [[ ! -f "${ROOT_DIR}/toybox/configs/shell-core.config" ]]; then
  echo "shell-core toybox config fragment missing" >&2
  exit 1
fi

for symbol in CONFIG_SH CONFIG_ECHO CONFIG_CAT CONFIG_LS CONFIG_MKDIR CONFIG_RM CONFIG_PWD CONFIG_CP CONFIG_MV CONFIG_LN CONFIG_TOUCH; do
  if ! grep -q "^${symbol}=y$" "${ROOT_DIR}/toybox/configs/shell-core.config"; then
    echo "shell-core config missing ${symbol}=y" >&2
    exit 1
  fi
done

if MUSL_CC="${WORK_DIR}/missing-musl-gcc" "${ROOT_DIR}/scripts/build_toybox_echo.sh" --toybox-src "${WORK_DIR}/toybox-src" >/dev/null 2>&1; then
  echo "build_toybox_echo.sh should fail when musl compiler wrapper is missing" >&2
  exit 1
fi

if MUSL_CC="${WORK_DIR}/missing-musl-gcc" "${ROOT_DIR}/scripts/build_toybox_sh.sh" --toybox-src "${WORK_DIR}/toybox-src" >/dev/null 2>&1; then
  echo "build_toybox_sh.sh should fail when musl compiler wrapper is missing" >&2
  exit 1
fi

if MUSL_CC="${WORK_DIR}/missing-musl-gcc" "${ROOT_DIR}/scripts/build_toybox_core.sh" --toybox-src "${WORK_DIR}/toybox-src" >/dev/null 2>&1; then
  echo "build_toybox_core.sh should fail when musl compiler wrapper is missing" >&2
  exit 1
fi
