#!/bin/bash
# Redraws the icon: assets/icon.png (1024), assets/AppIcon.icns (Mac), assets/icon.ico (Windows).
# Needs macOS (swift, sips, iconutil).
set -euo pipefail
cd "$(dirname "$0")/.."
work=$(mktemp -d)
swift scripts/make-icon.swift assets/icon.png

mkdir "$work/AppIcon.iconset"
for s in 16 32 128 256 512; do
  sips -z $s $s assets/icon.png --out "$work/AppIcon.iconset/icon_${s}x${s}.png" >/dev/null
  sips -z $((s * 2)) $((s * 2)) assets/icon.png --out "$work/AppIcon.iconset/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$work/AppIcon.iconset" -o assets/AppIcon.icns

# .ico from PNG images 16–256 (ICO may hold PNGs)
for s in 16 24 32 48 64 128 256; do sips -z $s $s assets/icon.png --out "$work/ico_$s.png" >/dev/null; done
python3 - "$work" <<'PY'
import struct, sys
work = sys.argv[1]
sizes = [16, 24, 32, 48, 64, 128, 256]
data = [open(f"{work}/ico_{s}.png", "rb").read() for s in sizes]
out = struct.pack("<HHH", 0, 1, len(sizes))
offset = 6 + 16 * len(sizes)
for s, d in zip(sizes, data):
    out += struct.pack("<BBBBHHII", s % 256, s % 256, 0, 0, 1, 32, len(d), offset)
    offset += len(d)
open("assets/icon.ico", "wb").write(out + b"".join(data))
PY
rm -rf "$work"
echo "Done: assets/icon.png, assets/AppIcon.icns, assets/icon.ico"
