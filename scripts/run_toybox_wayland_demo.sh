#!/usr/bin/env bash
set -euo pipefail

# Interactive peetwl session: the Wayland compositor owns the framebuffer and
# runs three wl-demo windows. Click to focus/raise, drag title bars to move,
# click a window to toggle its colour, press q in a window or use its close box.
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

source "${ROOT_DIR}/tests/rust_build_compat_env.bash"
setup_rust_build_compat "${ROOT_DIR}"

make -C "${ROOT_DIR}" image PROFILE=release KERNEL_FEATURES="musl-ext2-wayland-demo compositor-first usb-tablet"

exec qemu-system-x86_64 \
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
