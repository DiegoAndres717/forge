#!/bin/sh
# Receta de Homebrew (cask) de una versión publicada:
#   ./scripts/cask.sh 0.4.1 <sha256 del DMG> > Casks/forge.rb
# La usa el workflow de release para actualizar DiegoAndres717/homebrew-tap.
set -eu
version="$1"
sha="$2"
cat <<CASK
cask "forge" do
  version "$version"
  sha256 "$sha"

  url "https://github.com/DiegoAndres717/forge/releases/download/v#{version}/Forge-#{version}.dmg"
  name "Forge"
  desc "Native workspace for building software with AI agents"
  homepage "https://github.com/DiegoAndres717/forge"

  depends_on macos: :ventura

  app "Forge.app"
  binary "#{appdir}/Forge.app/Contents/MacOS/forge"

  # Forge aún no está notarizado por Apple: sin esto, macOS diría que no se puede abrir.
  postflight_steps do
    run "/usr/bin/xattr", args: ["-dr", "com.apple.quarantine", "{{appdir}}/Forge.app"]
  end

  zap trash: [
    "~/.config/forge",
    "~/Library/Application Support/Forge",
  ]
end
CASK
