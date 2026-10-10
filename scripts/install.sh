#!/bin/sh
# Instala (o actualiza) Forge desde la última versión de GitHub Releases:
#   curl -fsSL https://raw.githubusercontent.com/DiegoAndres717/forge/main/scripts/install.sh | sh
# Descarga el DMG, comprueba su SHA-256, copia Forge.app a /Applications (o a
# ~/Applications si no hay permiso) y enlaza el comando `forge` en el PATH.
# Opcional: FORGE_VERSION=0.4.1 (una versión concreta), FORGE_DIR=~/Apps (otra carpeta).
set -eu

REPO="DiegoAndres717/forge"

say() { printf '%s\n' "$*"; }
fail() { printf 'forge: %s\n' "$*" >&2; exit 1; }

[ "$(uname -s)" = "Darwin" ] || fail "Forge es una app de macOS (13 Ventura o superior)."
for tool in curl shasum hdiutil ditto; do
  command -v "$tool" >/dev/null 2>&1 || fail "falta $tool"
done

# Versión: la pedida o la última publicada (la redirección de /releases/latest dice cuál es).
version="${FORGE_VERSION:-}"
if [ -z "$version" ]; then
  latest=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/$REPO/releases/latest") \
    || fail "no se pudo consultar la última versión"
  version="${latest##*/v}"
fi
version="${version#v}"
case "$version" in
  [0-9]*.[0-9]*.[0-9]*) ;;
  *) fail "versión no válida: $version" ;;
esac

# /Applications si se puede escribir; si no, ~/Applications (sin pedir sudo).
apps="${FORGE_DIR:-/Applications}"
if [ -z "${FORGE_DIR:-}" ] && [ ! -w "$apps" ]; then
  apps="$HOME/Applications"
fi
mkdir -p "$apps" || fail "no se pudo crear $apps"
target="$apps/Forge.app"

# Esa copia abierta: hay que cerrarla antes de reemplazarla.
if pgrep -f "$target/Contents/MacOS/forge" >/dev/null 2>&1; then
  fail "cierra Forge (⌘Q) y vuelve a ejecutar el instalador"
fi

tmp=$(mktemp -d)
mount="$tmp/mnt"
cleanup() {
  hdiutil detach "$mount" -quiet >/dev/null 2>&1 || true
  rm -rf "$tmp"
}
trap cleanup EXIT INT TERM

dmg="Forge-$version.dmg"
base="https://github.com/$REPO/releases/download/v$version"
say "Descargando Forge ${version}…"
curl -fL --progress-bar -o "$tmp/$dmg" "$base/$dmg" || fail "no se pudo descargar $dmg"
curl -fsSL -o "$tmp/$dmg.sha256" "$base/$dmg.sha256" || fail "no se pudo descargar $dmg.sha256"
(cd "$tmp" && shasum -a 256 -c "$dmg.sha256" >/dev/null 2>&1) \
  || fail "la suma SHA-256 no coincide: la descarga está dañada o no es la publicada"

mkdir -p "$mount"
hdiutil attach "$tmp/$dmg" -nobrowse -readonly -quiet -mountpoint "$mount" \
  || fail "no se pudo abrir $dmg"
[ -d "$mount/Forge.app" ] || fail "$dmg no contiene Forge.app"

rm -rf "$target"
ditto "$mount/Forge.app" "$target"
# Descargado con curl no lleva cuarentena; por si acaso (y para copias anteriores).
xattr -dr com.apple.quarantine "$target" 2>/dev/null || true
say "Forge $version instalado en $target"

# El comando `forge` en el PATH, si hay una carpeta de binarios en la que se pueda escribir.
bin=""
for dir in /opt/homebrew/bin /usr/local/bin "$HOME/.local/bin"; do
  case ":$PATH:" in *":$dir:"*) ;; *) continue ;; esac
  if [ -d "$dir" ] && [ -w "$dir" ]; then
    bin="$dir"
    break
  fi
done
if [ -n "$bin" ]; then
  ln -sf "$target/Contents/MacOS/forge" "$bin/forge"
  say "Comando forge: $bin/forge"
else
  say "Para usar forge en la terminal:"
  say "  sudo ln -sf \"$target/Contents/MacOS/forge\" /usr/local/bin/forge"
fi
say "Ábrelo desde Aplicaciones o con: open \"$target\""
