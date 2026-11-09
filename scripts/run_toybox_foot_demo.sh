#!/usr/bin/env bash
set -euo pipefail

# Interactive foot session: peetwl owns the framebuffer and runs one foot
# terminal with a toybox shell inside. Click the QEMU window to give it
# keyboard focus, then type into foot. Drag the title bar to move the window.
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

source "${ROOT_DIR}/tests/rust_build_compat_env.bash"
setup_rust_build_compat "${ROOT_DIR}"

make -C "${ROOT_DIR}" image PROFILE=release KERNEL_FEATURES="musl-ext2-foot-demo compositor-first usb-tablet"

# foot maps a 512 MiB sparse scrollback pool, so give the guest headroom.
exec qemu-system-x86_64 \
  -m "${FOOT_DEMO_MEM:-1G}" \
  -smp "${QEMU_SMP:-2}" \
  -drive id=disk0,file="${ROOT_DIR}/build/peetos.img",if=none,format=raw \
  -device ich9-ahci,id=ahci \
  -device ide-hd,drive=disk0,bus=ahci.0 \
  -drive id=disk1,file="${ROOT_DIR}/user/sample_ext2_musl.img",if=none,format=raw \
  -device ide-hd,drive=disk1,bus=ahci.1 \
  -usb \
  -device usb-tablet \
  -serial stdio \
  -no-reboot \
  -no-shutdown
