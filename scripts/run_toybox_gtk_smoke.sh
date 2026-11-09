#!/usr/bin/env bash
set -euo pipefail

# End-to-end GTK 3 smoke: boots the compositor-first profile with peetwl
# running user/gtk_smoke.c, injects real PS/2 key presses and pointer clicks
# through the QEMU monitor, and checks the window on screen, typed text in a
# GtkEntry and a GtkButton click (see scripts/gtk_smoke_driver.py).
# Set GTK_SHOTS=<dir> to keep PNG copies of the screendumps.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IMAGE="${ROOT_DIR}/build/peetos.img"
EXT2_IMG="${ROOT_DIR}/user/sample_ext2_musl.img"
LOG_FILE="$(mktemp)"
MONITOR_SOCK="$(mktemp -u /tmp/peetos-gtk-monitor.XXXXXX)"
QEMU_PID=""

source "${ROOT_DIR}/tests/rust_build_compat_env.bash"
setup_rust_build_compat "${ROOT_DIR}"

cleanup() {
  if [[ -n "${QEMU_PID}" ]]; then
    kill "${QEMU_PID}" 2>/dev/null || true
    wait "${QEMU_PID}" 2>/dev/null || true
  fi
  rm -f "${LOG_FILE}" "${MONITOR_SOCK}"
}
trap cleanup EXIT

make -C "${ROOT_DIR}" image PROFILE=release KERNEL_FEATURES="compositor-first musl-ext2-gtk-smoke ${GTK_SMOKE_EXTRA_FEATURES:-}"

qemu-system-x86_64 \
  -m "${GTK_SMOKE_MEM:-512M}" \
  -drive id=disk0,file="${IMAGE}",if=none,format=raw \
  -device ich9-ahci,id=ahci \
  -device ide-hd,drive=disk0,bus=ahci.0 \
  -drive id=disk1,file="${EXT2_IMG}",if=none,format=raw \
  -device ide-hd,drive=disk1,bus=ahci.1 \
  -serial file:"${LOG_FILE}" \
  -display none \
  -monitor unix:"${MONITOR_SOCK}",server,nowait \
  -no-reboot \
  -no-shutdown &
QEMU_PID=$!

for _ in $(seq 1 100); do
  [[ -S "${MONITOR_SOCK}" ]] && break
  sleep 0.1
done
if [[ ! -S "${MONITOR_SOCK}" ]]; then
  echo "QEMU monitor socket did not appear" >&2
  exit 1
fi

shots_args=()
if [[ -n "${GTK_SHOTS:-}" ]]; then
  shots_args=(--shots "${GTK_SHOTS}")
fi

set +e
python3 "${ROOT_DIR}/scripts/gtk_smoke_driver.py" \
  --log "${LOG_FILE}" --monitor "${MONITOR_SOCK}" "${shots_args[@]}"
driver_status=$?
set -e

for _ in $(seq 1 100); do
  grep -q '\[musl-ext2-gtk-smoke\] done' "${LOG_FILE}" && break
  sleep 0.2
done

grep -E '\[(peetwl|gtk-smoke)\] (output|ready|mapped|unmapped|focus|all clients|pass|FAIL|start|first|widget|entry|button|exiting)|drm flips|Gtk-|GLib-|Gdk-' "${LOG_FILE}" || true

if [[ "${driver_status}" -ne 0 ]]; then
  echo "GTK smoke driver failed" >&2
  cat "${LOG_FILE}" >&2
  exit 1
fi
if ! grep -qE '\[peetwl\] drm flips=[1-9]' "${LOG_FILE}"; then
  echo "GTK smoke did not present through DRM page flips" >&2
  cat "${LOG_FILE}" >&2
  exit 1
fi
if ! grep -q '\[musl-ext2-gtk-smoke\] done' "${LOG_FILE}" \
  || grep -q '\[musl-ext2-gtk-smoke\] failures' "${LOG_FILE}"; then
  echo "GTK smoke payload did not finish cleanly" >&2
  cat "${LOG_FILE}" >&2
  exit 1
fi

# GTK must not hit kernel syscall gaps or log GTK/GLib criticals.
if grep -q "Unhandled syscall" "${LOG_FILE}"; then
  echo "GTK smoke hit an unhandled syscall:" >&2
  grep "Unhandled syscall" "${LOG_FILE}" | sort | uniq -c >&2
  exit 1
fi
if grep -qE "(Gtk|Gdk|GLib|GLib-GObject)-(CRITICAL|ERROR)" "${LOG_FILE}"; then
  echo "GTK smoke logged criticals:" >&2
  grep -E "(Gtk|Gdk|GLib|GLib-GObject)-(CRITICAL|ERROR)" "${LOG_FILE}" >&2
  exit 1
fi

echo "GTK smoke completed successfully."
