#!/bin/sh
# Genera dist/Forge-<versión>.dmg: la app y un acceso a Aplicaciones para arrastrarla.
#   ./scripts/make-dmg.sh
# Sin firma de Apple Developer: en otro Mac, la primera vez hay que abrirla con
# clic derecho → Abrir (o quitar la cuarentena: xattr -dr com.apple.quarantine Forge.app).
set -eu
cd "$(dirname "$0")/.."
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
STAGE=$(mktemp -d)
trap 'rm -rf "$STAGE"' EXIT

./scripts/install-app.sh "$STAGE" >/dev/null
ln -s /Applications "$STAGE/Applications"
mkdir -p dist
OUT="dist/Forge-$VERSION.dmg"
rm -f "$OUT"
hdiutil create -volname "Forge $VERSION" -srcfolder "$STAGE" -ov -format UDZO "$OUT" >/dev/null
echo "$OUT"
