#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
HOST="$(rustc -vV | rg '^host:' | awk '{print $2}')"

cargo test -p kernel --target "${HOST}"
