#!/usr/bin/env bash
set -euo pipefail

# End-to-end Thunar smoke: boots the compositor-first profile with peetwl
# running the Thunar file manager on /tmp/thunar-demo and a watcher shell,
# injects real PS/2 key presses through the QEMU monitor, and checks the
# window on screen, keyboard navigation and a folder created through
# Thunar's New Folder dialog (see scripts/thunar_smoke_driver.py).
# Set THUNAR_SHOTS=<dir> to keep PNG copies of the screendumps.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IMAGE="${ROOT_DIR}/build/peetos.img"
EXT2_IMG="${ROOT_DIR}/user/sample_ext2_musl.img"
LOG_FILE="$(mktemp)"
MONITOR_SOCK="$(mktemp -u /tmp/peetos-thunar-monitor.XXXXXX)"
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

make -C "${ROOT_DIR}" image PROFILE=release KERNEL_FEATURES="compositor-first musl-ext2-thunar-smoke ${THUNAR_SMOKE_EXTRA_FEATURES:-}"

qemu-system-x86_64 \
  -m "${THUNAR_SMOKE_MEM:-768M}" \
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
if [[ -n "${THUNAR_SHOTS:-}" ]]; then
  shots_args=(--shots "${THUNAR_SHOTS}")
fi

set +e
python3 "${ROOT_DIR}/scripts/thunar_smoke_driver.py" \
  --log "${LOG_FILE}" --monitor "${MONITOR_SOCK}" "${shots_args[@]}"
driver_status=$?
set -e

for _ in $(seq 1 100); do
  grep -q '\[musl-ext2-thunar-smoke\] done' "${LOG_FILE}" && break
  sleep 0.2
done

grep -E '\[(peetwl|thunar-watch)\] (output|ready|mapped|unmapped|focus|title|all clients|pass|FAIL|found|wrong|waiting|org)|drm flips|Gtk-|GLib-|Gdk-|thunar' "${LOG_FILE}" || true

if [[ "${driver_status}" -ne 0 ]]; then
  echo "Thunar smoke driver failed" >&2
  cat "${LOG_FILE}" >&2
  exit 1
fi
if ! grep -qE '\[peetwl\] drm flips=[1-9]' "${LOG_FILE}"; then
  echo "Thunar smoke did not present through DRM page flips" >&2
  cat "${LOG_FILE}" >&2
  exit 1
fi
if ! grep -q '\[musl-ext2-thunar-smoke\] done' "${LOG_FILE}" \
  || grep -q '\[musl-ext2-thunar-smoke\] failures' "${LOG_FILE}"; then
  echo "Thunar smoke payload did not finish cleanly" >&2
  cat "${LOG_FILE}" >&2
  exit 1
fi

# Thunar must not hit kernel syscall gaps or log GTK/GLib criticals.
for marker in   '[thunar-watch] org.xfce.FileManager owned: (true,)'   '[musl-ext2-thunar-smoke] Thunar settings saved through xfconf'; do
  if ! grep -qF "${marker}" "${LOG_FILE}"; then
    echo "Thunar smoke is missing marker: ${marker}" >&2
    exit 1
  fi
done
if grep -q "Failed to initialize Xfconf" "${LOG_FILE}"; then
  echo "Thunar could not reach xfconf on the session bus" >&2
  exit 1
fi

if grep -q "Unhandled syscall" "${LOG_FILE}"; then
  echo "Thunar smoke hit an unhandled syscall:" >&2
  grep "Unhandled syscall" "${LOG_FILE}" | sort | uniq -c >&2
  exit 1
fi
if grep -qE "(Gtk|Gdk|GLib|GLib-GObject)-(CRITICAL|ERROR)" "${LOG_FILE}"; then
  echo "Thunar smoke logged criticals:" >&2
  grep -E "(Gtk|Gdk|GLib|GLib-GObject)-(CRITICAL|ERROR)" "${LOG_FILE}" >&2
  exit 1
fi

echo "Thunar smoke completed successfully."
