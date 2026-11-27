#!/bin/bash
# Redraws the icon: assets/icon.png (1024), assets/AppIcon.icns (Mac), assets/icon.ico (Windows).
# Every size is drawn natively by make-icon.swift. Needs macOS (swift, iconutil).
# Run: scripts/make-icon.sh [--sheet <out.png>]   (--sheet also draws a contact sheet of the packed files)
set -euo pipefail

sheet=""
if [ $# -eq 2 ] && [ "$1" = "--sheet" ]; then
  case "$2" in /*) sheet="$2" ;; *) sheet="$PWD/$2" ;; esac
elif [ $# -ne 0 ]; then
  echo "usage: $0 [--sheet <out.png>]" >&2
  exit 2
fi

cd "$(dirname "$0")/.."
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
swift scripts/make-icon.swift "$work"

set_dir="$work/AppIcon.iconset"
mkdir "$set_dir"
for s in 16 32 128 256 512; do
  cp "$work/mac-$s.png" "$set_dir/icon_${s}x${s}.png"
  cp "$work/mac-$((s * 2)).png" "$set_dir/icon_${s}x${s}@2x.png"
done
iconutil -c icns "$set_dir" -o assets/AppIcon.icns
cp "$work/mac-1024.png" assets/icon.png
cp "$work/icon.ico" assets/icon.ico
echo "Done: assets/icon.png, assets/AppIcon.icns, assets/icon.ico"

if [ -n "$sheet" ]; then
  swift scripts/make-icon.swift --sheet assets/icon.ico assets/AppIcon.icns "$sheet"
fi
