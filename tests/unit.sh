#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
HOST="$(rustc -vV | grep '^host:' | awk '{print $2}')"

source "${ROOT_DIR}/tests/rust_build_compat_env.bash"
setup_rust_build_compat "${ROOT_DIR}"

cargo test -p arch_x86_64 --target "${HOST}"
cargo test -p drivers --target "${HOST}"
cargo test -p fs --target "${HOST}"
cargo test -p kernel --target "${HOST}"
