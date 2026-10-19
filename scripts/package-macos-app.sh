#!/usr/bin/env bash
# Builds Sphere.app: the desktop app with sphered, sphere-host and the sphere CLI next to it, an icon rendered
# from the core, ad-hoc signed. Notifications then show as Sphere, and Option+Space activates the app cleanly.
# usage: scripts/package-macos-app.sh            -> target/Sphere.app, target/Sphere-<version>.dmg and .zip
# The app is ad-hoc signed, not notarized: on another Mac, open it the first time with right-click > Open.
set -euo pipefail
cd "$(dirname "$0")/.."
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
cargo build --profile dist -p sphere-app -p sphered -p sphere-host -p sphere-cli
app=target/Sphere.app
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp target/dist/sphere-app target/dist/sphered target/dist/sphere-host target/dist/sphere "$app/Contents/MacOS/"
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Sphere</string>
  <key>CFBundleDisplayName</key><string>Sphere</string>
  <key>CFBundleIdentifier</key><string>os.sphere.app</string>
  <key>CFBundleExecutable</key><string>sphere-app</string>
  <key>CFBundleIconFile</key><string>Sphere</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${version}</string>
  <key>CFBundleVersion</key><string>${version}</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
  <key>NSCalendarsUsageDescription</key><string>Sphere shows your next 24 hours of events on its home screen.</string>
  <key>NSCalendarsFullAccessUsageDescription</key><string>Sphere shows your next 24 hours of events on its home screen.</string>
</dict>
</plist>
PLIST
# the icon: the core, rendered offscreen and cropped square
tmp=$(mktemp -d)
target/dist/sphere-app --shot "$tmp/core.png" --icon --frames 120 --size 1600x1000 >/dev/null 2>&1 || true
if [ -s "$tmp/core.png" ]; then
  sips -c 860 860 --cropOffset 8 370 "$tmp/core.png" --out "$tmp/sq.png" >/dev/null
  mkdir -p "$tmp/Sphere.iconset"
  for s in 16 32 128 256 512; do
    sips -z $s $s "$tmp/sq.png" --out "$tmp/Sphere.iconset/icon_${s}x${s}.png" >/dev/null
    d=$((s * 2)); sips -z $d $d "$tmp/sq.png" --out "$tmp/Sphere.iconset/icon_${s}x${s}@2x.png" >/dev/null
  done
  iconutil -c icns "$tmp/Sphere.iconset" -o "$app/Contents/Resources/Sphere.icns"
fi
rm -rf "$tmp"
codesign --force --deep --sign - "$app" >/dev/null
# a disk image to share: the app and a link to Applications, to drag it across
dmg=target/Sphere-${version}.dmg
stage=$(mktemp -d)
cp -R "$app" "$stage/"
ln -s /Applications "$stage/Applications"
rm -f "$dmg"
hdiutil create -quiet -volname "Sphere" -srcfolder "$stage" -ov -format UDZO "$dmg"
rm -rf "$stage"
# and a zip next to it
zip=target/Sphere-${version}-macos.zip
rm -f "$zip"
(cd target && ditto -c -k --keepParent Sphere.app "$(basename "$zip")")
echo "$app"
echo "$dmg"
echo "$zip"
