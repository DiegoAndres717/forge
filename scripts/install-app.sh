#!/bin/sh
# Compila Forge y lo instala como Forge.app (por defecto en ~/Applications).
# Volver a ejecutarlo actualiza la app con el código actual.
#   ./scripts/install-app.sh            → ~/Applications/Forge.app
#   ./scripts/install-app.sh /Applications
set -eu
cd "$(dirname "$0")/.."
command -v cargo >/dev/null 2>&1 || export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"

DEST="${1:-$HOME/Applications}"
APP="$DEST/Forge.app"
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)

cargo build --release

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

# Icono .icns con todas las resoluciones de macOS a partir de assets/icon.png.
ICONSET="$TMP/Forge.iconset"
mkdir "$ICONSET"
for size in 16 32 128 256 512; do
    sips -z "$size" "$size" assets/icon.png --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
    double=$((size * 2))
    sips -z "$double" "$double" assets/icon.png --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$TMP/Forge.icns"

mkdir -p "$DEST"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/forge "$APP/Contents/MacOS/forge"
cp "$TMP/Forge.icns" "$APP/Contents/Resources/Forge.icns"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>Forge</string>
    <key>CFBundleDisplayName</key><string>Forge</string>
    <key>CFBundleIdentifier</key><string>dev.forge.app</string>
    <key>CFBundleExecutable</key><string>forge</string>
    <key>CFBundleIconFile</key><string>Forge</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$VERSION</string>
    <key>CFBundleVersion</key><string>$VERSION</string>
    <key>LSMinimumSystemVersion</key><string>13.0</string>
    <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
    <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST

# Firma local (ad hoc): evita avisos de binario sin firmar al abrirla desde el Finder.
codesign --force --sign - "$APP" >/dev/null 2>&1 || echo "Aviso: no se pudo firmar (la app funciona igual)."
# Para que el Finder y el Dock relean el icono.
touch "$APP"

echo "Instalado: $APP"
