#!/bin/zsh
# Builds "Belge Araçları.app" (universal: Apple Silicon + Intel) and a DMG into ./dist
# Usage: ./Scripts/build.sh [--install]   (--install also copies the app into /Applications)
set -euo pipefail

ROOT="${0:A:h:h}"
cd "$ROOT"

APP_NAME="Belge Araçları"
EXECUTABLE="BelgeAraclari"
BUNDLE_ID="com.azargozalli.belgearaclari"
VERSION="1.2.1"
BUILD_NUMBER="1"
DIST="$ROOT/dist"
APP="$DIST/$APP_NAME.app"

echo "▸ Derleniyor (arm64 + x86_64)…"
swift build -c release --arch arm64 --arch x86_64
BIN="$(swift build -c release --arch arm64 --arch x86_64 --show-bin-path)/$EXECUTABLE"

echo "▸ Uygulama paketi oluşturuluyor…"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/$EXECUTABLE"

ICON_SRC="$ROOT/Resources/AppIcon.png"
if [[ ! -f "$ICON_SRC" ]]; then
  swift "$ROOT/Scripts/make_icon.swift" "$ICON_SRC"
fi
ICONSET="$(mktemp -d)/AppIcon.iconset"
mkdir -p "$ICONSET"
for s in 16 32 128 256 512; do
  sips -z $s $s "$ICON_SRC" --out "$ICONSET/icon_${s}x${s}.png" >/dev/null
  sips -z $((s*2)) $((s*2)) "$ICON_SRC" --out "$ICONSET/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/AppIcon.icns"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>$APP_NAME</string>
  <key>CFBundleDisplayName</key><string>$APP_NAME</string>
  <key>CFBundleExecutable</key><string>$EXECUTABLE</string>
  <key>CFBundleIdentifier</key><string>$BUNDLE_ID</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$BUILD_NUMBER</string>
  <key>CFBundleDevelopmentRegion</key><string>tr</string>
  <key>LSMinimumSystemVersion</key><string>14.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.productivity</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
  <key>NSHumanReadableCopyright</key><string>© 2026 Azar Gozalli · MIT</string>
  <key>CFBundleDocumentTypes</key>
  <array>
    <dict>
      <key>CFBundleTypeName</key><string>PDF</string>
      <key>CFBundleTypeRole</key><string>Editor</string>
      <key>LSHandlerRank</key><string>Alternate</string>
      <key>LSItemContentTypes</key><array><string>com.adobe.pdf</string></array>
    </dict>
    <dict>
      <key>CFBundleTypeName</key><string>UYAP UDF</string>
      <key>CFBundleTypeRole</key><string>Viewer</string>
      <key>LSHandlerRank</key><string>Alternate</string>
      <key>CFBundleTypeExtensions</key><array><string>udf</string></array>
    </dict>
    <dict>
      <key>CFBundleTypeName</key><string>Resim</string>
      <key>CFBundleTypeRole</key><string>Viewer</string>
      <key>LSHandlerRank</key><string>Alternate</string>
      <key>LSItemContentTypes</key>
      <array>
        <string>public.jpeg</string><string>public.png</string><string>org.webmproject.webp</string>
        <string>public.heic</string><string>public.heif</string><string>public.tiff</string>
        <string>com.compuserve.gif</string><string>com.microsoft.bmp</string>
      </array>
    </dict>
  </array>
</dict>
</plist>
PLIST
printf "APPL????" > "$APP/Contents/PkgInfo"

echo "▸ İmzalanıyor (ad-hoc)…"
codesign --force --deep --sign - --options runtime --timestamp=none "$APP"
codesign --verify --verbose=1 "$APP"

echo "▸ DMG hazırlanıyor…"
DMG="$DIST/Belge-Araclari-$VERSION.dmg"
STAGE="$(mktemp -d)"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
rm -f "$DMG"
hdiutil create -volname "$APP_NAME" -srcfolder "$STAGE" -ov -format UDZO "$DMG" >/dev/null
rm -rf "$STAGE"

if [[ "${1:-}" == "--install" ]]; then
  echo "▸ /Applications klasörüne kuruluyor…"
  rm -rf "/Applications/$APP_NAME.app"
  cp -R "$APP" "/Applications/"
fi

echo "✓ Hazır:"
echo "  $APP"
echo "  $DMG"
