#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

nasm -f elf64 "${ROOT_DIR}/user/hello.asm" -o "${ROOT_DIR}/build/hello.o"
ld -nostdlib -static -z max-page-size=0x1000 -Ttext 0x400000 \
  -o "${ROOT_DIR}/user/hello.elf" "${ROOT_DIR}/build/hello.o"

echo "Built user/hello.elf"
