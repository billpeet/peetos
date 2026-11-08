#!/usr/bin/env bash
set -euo pipefail

# Software rendering stack smoke: fontconfig + freetype + pixman text shown
# through DRM/KMS. Also saves a screenshot of the rendered frame to
# build/text_smoke.png (QEMU monitor screendump).

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IMAGE="${ROOT_DIR}/build/peetos.img"
EXT2_IMG="${ROOT_DIR}/user/sample_ext2_musl.img"
LOG_FILE="$(mktemp)"
MON_SOCK="$(mktemp -u)"
SHOT_PPM="$(mktemp --suffix=.ppm)"
SHOT_PNG="${ROOT_DIR}/build/text_smoke.png"
TIMEOUT_SECS="${TEXT_SMOKE_TIMEOUT_SECS:-120}"
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

make -C "${ROOT_DIR}" image PROFILE=release KERNEL_FEATURES=musl-ext2-text-smoke

qemu-system-x86_64 \
  -m "${TEXT_SMOKE_MEM:-256M}" \
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
  if [[ "${shot_taken}" -eq 0 ]] && grep -q '\[text\] frame on screen' "${LOG_FILE}"; then
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
print("[text-smoke-runner] screenshot saved to %s (%dx%d)" % (png, w, h))
PY
    shot_taken=1
  fi
  if grep -q '\[musl-ext2-text-smoke\] done' "${LOG_FILE}"; then
    break
  fi
  if ! kill -0 "${QEMU_PID}" 2>/dev/null; then
    break
  fi
  sleep 0.5
done
sleep 0.5

if ! grep -q '\[musl-ext2-text-smoke\] done' "${LOG_FILE}"; then
  echo "rendering-stack smoke did not reach completion marker within ${TIMEOUT_SECS}s" >&2
  cat "${LOG_FILE}" >&2
  exit 1
fi

if grep -q '\[musl-ext2-text-smoke\] failures' "${LOG_FILE}" || grep -q '\[text\] FAIL' "${LOG_FILE}"; then
  echo "rendering-stack smoke completed with failures" >&2
  cat "${LOG_FILE}" >&2
  exit 1
fi

for marker in \
  'DejaVu Sans Mono' \
  '[text] fontconfig matches monospace to DejaVu Sans Mono ok' \
  '[text] fontconfig matches sans-serif to DejaVu Sans ok' \
  '[text] freetype rasterizes an antialiased glyph ok' \
  '[text] pixman composites glyph masks ok' \
  '[text] pixman scaled bilinear composite ok' \
  '[text] rendered text is on screen ok' \
  '[text] pass'; do
  if ! grep -qF "${marker}" "${LOG_FILE}"; then
    echo "rendering-stack smoke is missing marker: ${marker}" >&2
    cat "${LOG_FILE}" >&2
    exit 1
  fi
done

grep -E '\[text\]|fc-match|DejaVu' "${LOG_FILE}" | head -40 || true
echo "Rendering-stack smoke completed successfully."
