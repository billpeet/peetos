#!/usr/bin/env bash
set -euo pipefail

setup_rust_build_compat() {
  local root_dir="$1"

  export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-${root_dir}/target}"
  export TMPDIR="${TMPDIR:-${CARGO_TARGET_DIR}/tmp}"
  mkdir -p "${TMPDIR}"

  if [[ "$(uname -s)" == "Linux" ]]; then
    local tool_dir="${CARGO_TARGET_DIR}/host-test-tools"
    local link_compat_src="${root_dir}/tests/host_link_compat.c"
    local link_compat_so="${tool_dir}/host_link_compat.so"
    mkdir -p "${tool_dir}"

    if [[ ! -f "${link_compat_so}" || "${link_compat_so}" -ot "${link_compat_src}" ]]; then
      "${CC:-cc}" -shared -fPIC -O2 -o "${link_compat_so}" "${link_compat_src}" -ldl
    fi

    export LD_PRELOAD="${link_compat_so}${LD_PRELOAD:+:${LD_PRELOAD}}"
  fi
}
