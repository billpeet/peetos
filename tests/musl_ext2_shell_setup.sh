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

if ! grep -q 'build_toybox_core.sh' "${EXT2_BUILDER}"; then
  echo "ext2 builder is not using core toybox staging" >&2
  exit 1
fi

if ! grep -q '/bin/toybox' "${EXT2_BUILDER}"; then
  echo "ext2 builder is missing /bin/toybox staging" >&2
  exit 1
fi

if grep -q '/bin/sh' "${EXT2_BUILDER}"; then
  echo "ext2 builder unexpectedly stages /bin/sh (image should stay multicall-only)" >&2
  exit 1
fi
