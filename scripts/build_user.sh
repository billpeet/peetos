#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

nasm -f elf64 "${ROOT_DIR}/user/hello.asm" -o "${ROOT_DIR}/build/hello.o"
ld -nostdlib -static -z max-page-size=0x1000 -Ttext 0x400000 \
  -o "${ROOT_DIR}/user/hello.elf" "${ROOT_DIR}/build/hello.o"

nasm -f elf64 "${ROOT_DIR}/user/hello_dyn.asm" -o "${ROOT_DIR}/build/hello_dyn.o"
ld -nostdlib -pie -z max-page-size=0x1000 --dynamic-linker /lib/ld-peetos.so \
  -o "${ROOT_DIR}/user/hello_dyn.elf" "${ROOT_DIR}/build/hello_dyn.o"

nasm -f elf64 "${ROOT_DIR}/user/ld_peetos.asm" -o "${ROOT_DIR}/build/ld_peetos.o"
ld -nostdlib -shared -z max-page-size=0x1000 -e _start \
  -o "${ROOT_DIR}/user/ld-peetos.so" "${ROOT_DIR}/build/ld_peetos.o"

if [[ ! -f "${ROOT_DIR}/user/sample_ext2.img" ]]; then
  "${ROOT_DIR}/scripts/build_ext2_sample.sh"
fi

echo "Built user/hello.elf, user/hello_dyn.elf, user/ld-peetos.so, and user/sample_ext2.img"
