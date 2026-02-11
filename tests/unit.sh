#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
HOST="$(rustc -vV | rg '^host:' | awk '{print $2}')"

cargo test -p arch_x86_64 --target "${HOST}"
cargo test -p drivers --target "${HOST}"
cargo test -p fs --target "${HOST}"
cargo test -p kernel --target "${HOST}"
