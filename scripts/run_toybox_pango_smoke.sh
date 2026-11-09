#!/usr/bin/env bash
set -euo pipefail

# 2D toolkit smoke: GLib, cairo (PNG/PDF/SVG via libpng + zlib) and pango,
# plus pango-list/pango-view, ending with a cairo + pango frame shown through
# DRM/KMS. Saves a screenshot of that frame to build/pango_smoke.png (QEMU
# monitor screendump).

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IMAGE="${ROOT_DIR}/build/peetos.img"
EXT2_IMG="${ROOT_DIR}/user/sample_ext2_musl.img"
LOG_FILE="$(mktemp)"
MON_SOCK="$(mktemp -u)"
SHOT_PPM="$(mktemp --suffix=.ppm)"
SHOT_PNG="${ROOT_DIR}/build/pango_smoke.png"
TIMEOUT_SECS="${PANGO_SMOKE_TIMEOUT_SECS:-120}"
QEMU_PID=""

source "${ROOT_DIR}/tests/rust_build_compat_env.bash"
setup_rust_build_compat "${ROOT_DIR}"

cleanup() {
  if [[ -n "${QEMU_PID}" ]]; then
    kill "${QEMU_PID}" 2>/dev/null || true
    wait "${QEMU_PID}" 2>/dev/null || true
  fi
  rm -f "${LOG_FILE}" "${MON_SOCK}" "${SHOT_PPM}"
}
trap cleanup EXIT

make -C "${ROOT_DIR}" image PROFILE=release KERNEL_FEATURES=musl-ext2-pango-smoke

qemu-system-x86_64 \
  -m "${PANGO_SMOKE_MEM:-256M}" \
  -drive id=disk0,file="${IMAGE}",if=none,format=raw \
  -device ich9-ahci,id=ahci \
  -device ide-hd,drive=disk0,bus=ahci.0 \
  -drive id=disk1,file="${EXT2_IMG}",if=none,format=raw \
  -device ide-hd,drive=disk1,bus=ahci.1 \
  -serial stdio \
  -display none \
  -monitor "unix:${MON_SOCK},server,nowait" \
  -no-reboot \
  -no-shutdown \
  > "${LOG_FILE}" 2>&1 &
QEMU_PID=$!

shot_taken=0
deadline=$((SECONDS + TIMEOUT_SECS))
while (( SECONDS < deadline )); do
  if [[ "${shot_taken}" -eq 0 ]] && grep -qF '[pango] frame on screen' "${LOG_FILE}"; then
    # The smoke holds the frame for a few seconds; grab it while it is up.
    python3 - "${MON_SOCK}" "${SHOT_PPM}" "${SHOT_PNG}" <<'PY' || true
import socket, struct, sys, time, zlib
sock_path, ppm, png = sys.argv[1:4]
s = socket.socket(socket.AF_UNIX)
s.connect(sock_path)
s.settimeout(5)
s.sendall(("screendump %s\n" % ppm).encode())
time.sleep(1.0)
s.close()
data = open(ppm, "rb").read()
# Parse binary PPM (P6) header: magic, width, height, maxval.
fields, pos = [], 0
while len(fields) < 4:
    while data[pos:pos + 1].isspace():
        pos += 1
    if data[pos:pos + 1] == b"#":
        pos = data.index(b"\n", pos) + 1
        continue
    end = pos
    while not data[end:end + 1].isspace():
        end += 1
    fields.append(data[pos:end])
    pos = end
pos += 1
w, h = int(fields[1]), int(fields[2])
rgb = data[pos:pos + w * h * 3]
raw = b"".join(b"\x00" + rgb[y * w * 3:(y + 1) * w * 3] for y in range(h))
def chunk(tag, body):
    return struct.pack(">I", len(body)) + tag + body + struct.pack(">I", zlib.crc32(tag + body) & 0xffffffff)
with open(png, "wb") as f:
    f.write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw, 6)) + chunk(b"IEND", b""))
print("[pango-smoke-runner] screenshot saved to %s (%dx%d)" % (png, w, h))
PY
    shot_taken=1
  fi
  if grep -q '\[musl-ext2-pango-smoke\] done' "${LOG_FILE}"; then
    break
  fi
  if ! kill -0 "${QEMU_PID}" 2>/dev/null; then
    break
  fi
  sleep 0.5
done
sleep 0.5

if ! grep -q '\[musl-ext2-pango-smoke\] done' "${LOG_FILE}"; then
  echo "2D toolkit smoke did not reach completion marker within ${TIMEOUT_SECS}s" >&2
  cat "${LOG_FILE}" >&2
  exit 1
fi

if grep -q '\[musl-ext2-pango-smoke\] failures' "${LOG_FILE}" || grep -qF '[pango] FAIL' "${LOG_FILE}"; then
  echo "2D toolkit smoke completed with failures" >&2
  cat "${LOG_FILE}" >&2
  exit 1
fi

# GLib, cairo and pango should not hit kernel syscall gaps.
if grep -q 'Unhandled syscall' "${LOG_FILE}"; then
  echo "2D toolkit smoke hit an unhandled syscall:" >&2
  grep 'Unhandled syscall' "${LOG_FILE}" | sort | uniq -c >&2
  exit 1
fi

for marker in \
  'DejaVu Sans' \
  '[musl-ext2-pango-smoke] pango-view wrote a PNG' \
  '[pango] GMainLoop runs timeout sources ok' \
  '[pango] GThread workers with GMutex ok' \
  '[pango] GRegex (pcre2) captures a group ok' \
  '[pango] GObject property notify ok' \
  '[pango] GIO GFile reads a file ok' \
  '[pango] statx matches stat ok' \
  '[pango] GIO g_file_query_info size and type ok' \
  '[pango] cairo linear gradient ok' \
  '[pango] cairo antialiases edges ok' \
  '[pango] PNG round trip preserves pixels ok' \
  '[pango] cairo PDF surface with embedded font ok' \
  '[pango] cairo SVG surface ok' \
  '[pango] pango resolves Sans through fontconfig ok' \
  '[pango] pango parses markup ok' \
  '[pango] pango finds real glyphs for Latin and Arabic ok' \
  '[pango] pango wraps a paragraph to a width ok' \
  '[pango] pango resolves an Arabic paragraph to right-to-left (fribidi) ok' \
  '[pango] cairo + pango frame is on screen ok' \
  '[pango] pass'; do
  if ! grep -qF "${marker}" "${LOG_FILE}"; then
    echo "2D toolkit smoke is missing marker: ${marker}" >&2
    cat "${LOG_FILE}" >&2
    exit 1
  fi
done

grep -E '\[pango\]|musl-ext2-pango-smoke' "${LOG_FILE}" | head -60 || true
echo "2D toolkit smoke completed successfully."
