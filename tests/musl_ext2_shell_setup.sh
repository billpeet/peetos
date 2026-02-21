#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LAUNCHER="${ROOT_DIR}/user/musl_execve_ext2.c"
EXT2_BUILDER="${ROOT_DIR}/scripts/build_ext2_musl_execve.sh"

if ! grep -q '"/mnt/ext2/bin/toybox"' "${LAUNCHER}"; then
  echo "musl ext2 launcher is not targeting /mnt/ext2/bin/toybox" >&2
  exit 1
fi

if ! grep -q '"sh"' "${LAUNCHER}"; then
  echo "musl ext2 launcher is missing toybox sh argv0" >&2
  exit 1
fi

if grep -q '"-c"' "${LAUNCHER}"; then
  echo "musl ext2 launcher regressed to non-interactive sh -c mode" >&2
  exit 1
fi

if ! grep -q 'PATH=/mnt/ext2/bin:/bin' "${LAUNCHER}"; then
  echo "musl ext2 launcher PATH is missing /mnt/ext2/bin" >&2
  exit 1
fi

if grep -q 'launching interactive toybox sh' "${LAUNCHER}"; then
  echo "musl ext2 launcher should not print a startup banner" >&2
  exit 1
fi

if ! grep -q 'PS1=' "${LAUNCHER}"; then
  echo "musl ext2 launcher is missing PS1 prompt override" >&2
  exit 1
fi

if ! grep -Fq 'PS1=\x1b[32m\\w $ \x1b[0m' "${LAUNCHER}"; then
  echo "musl ext2 launcher PS1 is missing cwd + dollar prompt format" >&2
  exit 1
fi

if ! grep -Fq '\x1b[32m' "${LAUNCHER}"; then
  echo "musl ext2 launcher PS1 is missing green ANSI color" >&2
  exit 1
fi

if ! grep -q 'build_toybox_core.sh' "${EXT2_BUILDER}"; then
  echo "ext2 builder is not using core toybox staging" >&2
  exit 1
fi

if ! grep -q '/bin/toybox' "${EXT2_BUILDER}"; then
  echo "ext2 builder is missing /bin/toybox staging" >&2
  exit 1
fi

# /bin/sh hardlink is now expected (musl popen/system require /bin/sh -c)
if ! grep -q 'ln.*bin/sh' "${EXT2_BUILDER}"; then
  echo "ext2 builder is missing /bin/sh hardlink for musl popen/system support" >&2
  exit 1
fi
