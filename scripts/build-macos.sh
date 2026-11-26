#!/bin/bash
# Builds dist/UsbMemoryChecker.app (Apple Silicon), dist/UsbMemoryChecker-macOS.dmg and the update archive
# dist/UsbMemoryChecker-<version>-macos-arm64.zip. Run: scripts/build-macos.sh
set -euo pipefail
cd "$(dirname "$0")/.."

target=aarch64-apple-darwin
rustup target add "$target" >/dev/null
cargo test --quiet
cargo build --release --target "$target"

version=$(cargo metadata --no-deps --format-version 1 | python3 -c "import json,sys; print(json.load(sys.stdin)['packages'][0]['version'])")
app="dist/UsbMemoryChecker.app"
rm -rf "$app" dist/UsbMemoryChecker-macOS.dmg dist/UsbMemoryChecker-*-macos-arm64.zip
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "target/$target/release/usbmemorychecker" "$app/Contents/MacOS/usbmemorychecker"
cp assets/AppIcon.icns "$app/Contents/Resources/AppIcon.icns"
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>UsbMemoryChecker</string>
  <key>CFBundleDisplayName</key><string>UsbMemoryChecker</string>
  <key>CFBundleIdentifier</key><string>com.demoz.usbmemorychecker</string>
  <key>CFBundleExecutable</key><string>usbmemorychecker</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>LSApplicationCategoryType</key><string>public.app-category.utilities</string>
  <key>NSRemovableVolumesUsageDescription</key><string>UsbMemoryChecker writes test files to the drive you choose to measure its speed and real capacity.</string>
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
ditto -c -k --keepParent "$app" "dist/UsbMemoryChecker-$version-macos-arm64.zip"

staging=$(mktemp -d)
cp -R "$app" "$staging/"
ln -s /Applications "$staging/Applications"
hdiutil create -quiet -volname "UsbMemoryChecker" -srcfolder "$staging" -ov -format UDZO dist/UsbMemoryChecker-macOS.dmg
rm -rf "$staging"
echo "Done: $app, dist/UsbMemoryChecker-macOS.dmg, dist/UsbMemoryChecker-$version-macos-arm64.zip"
