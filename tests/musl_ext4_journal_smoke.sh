#!/usr/bin/env bash
# The journal crash/recovery smoke on a writable ext4 root
# (docs/ext4_compat_plan.md M2).
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PEETOS_JOURNAL_SMOKE_FS=ext4 "${ROOT_DIR}/scripts/run_toybox_journal_smoke.sh"
