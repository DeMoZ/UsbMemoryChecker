#!/bin/bash
# Builds dist/UsbMemoryCheker.app (Apple Silicon), dist/UsbMemoryCheker-macOS.dmg and the update archive
# dist/UsbMemoryCheker-<version>-macos-arm64.zip. Run: scripts/build-macos.sh
set -euo pipefail
cd "$(dirname "$0")/.."

target=aarch64-apple-darwin
rustup target add "$target" >/dev/null
cargo test --quiet
cargo build --release --target "$target"

version=$(cargo metadata --no-deps --format-version 1 | python3 -c "import json,sys; print(json.load(sys.stdin)['packages'][0]['version'])")
app="dist/UsbMemoryCheker.app"
rm -rf "$app" dist/UsbMemoryCheker-macOS.dmg dist/UsbMemoryCheker-*-macos-arm64.zip
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "target/$target/release/usbmemorycheker" "$app/Contents/MacOS/usbmemorycheker"
cp assets/AppIcon.icns "$app/Contents/Resources/AppIcon.icns"
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>UsbMemoryCheker</string>
  <key>CFBundleDisplayName</key><string>UsbMemoryCheker</string>
  <key>CFBundleIdentifier</key><string>com.demoz.usbmemorycheker</string>
  <key>CFBundleExecutable</key><string>usbmemorycheker</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>LSApplicationCategoryType</key><string>public.app-category.utilities</string>
  <key>NSRemovableVolumesUsageDescription</key><string>UsbMemoryCheker writes test files to the drive you choose to measure its speed and real capacity.</string>
  <key>CFBundleDevelopmentRegion</key><string>en</string>
  <key>CFBundleAllowMixedLocalizations</key><true/>
  <key>CFBundleLocalizations</key>
  <array><string>en</string><string>ru</string><string>es</string><string>de</string><string>zh-Hans</string></array>
</dict>
</plist>
PLIST
# ad hoc signature: without it Apple Silicon won't start a downloaded binary even after removing quarantine
codesign --force --deep --sign - "$app"
# archive for the in-app update: ditto keeps the signature and attributes of the .app
ditto -c -k --keepParent "$app" "dist/UsbMemoryCheker-$version-macos-arm64.zip"

staging=$(mktemp -d)
cp -R "$app" "$staging/"
ln -s /Applications "$staging/Applications"
hdiutil create -quiet -volname "UsbMemoryCheker" -srcfolder "$staging" -ov -format UDZO dist/UsbMemoryCheker-macOS.dmg
rm -rf "$staging"
echo "Done: $app, dist/UsbMemoryCheker-macOS.dmg, dist/UsbMemoryCheker-$version-macos-arm64.zip"
