#!/usr/bin/env bash
# The journal crash/recovery smoke on a stock mke2fs -t ext4 root
# (metadata_csum, JBD2 csum v3; docs/ext4_compat_plan.md M3).
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PEETOS_JOURNAL_SMOKE_FS=ext4-csum "${ROOT_DIR}/scripts/run_toybox_journal_smoke.sh"
