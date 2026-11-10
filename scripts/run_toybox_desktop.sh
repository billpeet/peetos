#!/usr/bin/env bash
set -euo pipefail

# Interactive desktop session: init runs peet-session, which starts a D-Bus
# session bus and labwc together; labwc's autostart brings up the sfwbar
# panel (application menu, launchers, taskbar, clock). See docs/desktop.md.
#
# Click the QEMU window to give it the mouse and keyboard (Ctrl+Alt+G
# releases them). Alt+F2 opens the fuzzel launcher; the panel's buttons
# start the foot terminal and the Thunar file manager; a click on the
# desktop opens labwc's menu, whose "Log Out" ends the session.
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

source "${ROOT_DIR}/tests/rust_build_compat_env.bash"
setup_rust_build_compat "${ROOT_DIR}"

make -C "${ROOT_DIR}" image PROFILE=release KERNEL_FEATURES="musl-ext2-desktop compositor-first"

# GTK applications and foot's sparse scrollback pool need headroom.
exec qemu-system-x86_64 \
  -m "${DESKTOP_MEM:-1G}" \
  -smp "${QEMU_SMP:-2}" \
  -drive id=disk0,file="${ROOT_DIR}/build/peetos.img",if=none,format=raw \
  -device ich9-ahci,id=ahci \
  -device ide-hd,drive=disk0,bus=ahci.0 \
  -drive id=disk1,file="${ROOT_DIR}/user/sample_ext2_musl.img",if=none,format=raw \
  -device ide-hd,drive=disk1,bus=ahci.1 \
  -serial stdio \
  -no-reboot \
  -no-shutdown
