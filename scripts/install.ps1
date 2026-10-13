# Instala (o actualiza) Forge en Windows desde la última versión de GitHub Releases:
#   irm https://raw.githubusercontent.com/DiegoAndres717/forge/main/scripts/install.ps1 | iex
# Descarga el .zip de su arquitectura, comprueba su SHA-256, deja forge.exe en
# %LOCALAPPDATA%\Programs\Forge, lo añade al PATH del usuario y crea el acceso en el menú
# Inicio. Para actualizar, se vuelve a ejecutar (también con Forge abierto).
# Opcional: $env:FORGE_VERSION = '0.7.0' (una versión concreta), $env:FORGE_DIR = 'D:\Forge'.
# Pruebas: FORGE_RELEASE_DIR (carpeta local con los .zip en vez de GitHub) y
# FORGE_SKIP_USER_SETUP=1 (sin tocar el PATH del usuario ni el menú Inicio).
& {
  $ErrorActionPreference = 'Stop'
  $ProgressPreference = 'SilentlyContinue'
  $repo = 'DiegoAndres717/forge'

  $arch = switch ([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture) {
    'X64' { 'x86_64' }
    'Arm64' { 'aarch64' }
    default { throw "forge: arquitectura no soportada: $_" }
  }

  $version = $env:FORGE_VERSION
  if (-not $version) {
    $version = (Invoke-RestMethod "https://api.github.com/repos/$repo/releases/latest").tag_name
  }
  $version = $version.TrimStart('v')
  if ($version -notmatch '^\d+\.\d+\.\d+$') { throw "forge: versión no válida: $version" }

  $zip = "forge-$version-windows-$arch.zip"
  $tmp = Join-Path ([System.IO.Path]::GetTempPath()) "forge-install-$PID"
  New-Item -ItemType Directory -Force $tmp | Out-Null
  try {
    Write-Host "Descargando Forge $version..."
    if ($env:FORGE_RELEASE_DIR) {
      Copy-Item (Join-Path $env:FORGE_RELEASE_DIR $zip), (Join-Path $env:FORGE_RELEASE_DIR "$zip.sha256") $tmp
    } else {
      $base = "https://github.com/$repo/releases/download/v$version"
      Invoke-WebRequest "$base/$zip" -OutFile (Join-Path $tmp $zip)
      Invoke-WebRequest "$base/$zip.sha256" -OutFile (Join-Path $tmp "$zip.sha256")
    }
    $expected = ((Get-Content (Join-Path $tmp "$zip.sha256") -Raw).Trim() -split '\s+')[0].ToLower()
    # Con .NET y no Get-FileHash: lanzado desde PowerShell 7, un Windows PowerShell hereda su
    # PSModulePath y no encuentra el módulo de Get-FileHash.
    $stream = [System.IO.File]::OpenRead((Join-Path $tmp $zip))
    try {
      $actual = [System.BitConverter]::ToString([System.Security.Cryptography.SHA256]::Create().ComputeHash($stream)).Replace('-', '').ToLower()
    } finally { $stream.Dispose() }
    if ($expected -ne $actual) {
      throw 'forge: la suma SHA-256 no coincide: la descarga está dañada o no es la publicada'
    }
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    [System.IO.Compression.ZipFile]::ExtractToDirectory((Join-Path $tmp $zip), (Join-Path $tmp 'out'))
    $new = Join-Path $tmp 'out\forge.exe'
    if (-not (Test-Path $new)) { throw "forge: $zip no contiene forge.exe" }

    $dir = if ($env:FORGE_DIR) { $env:FORGE_DIR } else { Join-Path $env:LOCALAPPDATA 'Programs\Forge' }
    New-Item -ItemType Directory -Force $dir | Out-Null
    Get-ChildItem $dir -Filter 'forge.exe.old-*' -ErrorAction SilentlyContinue |
      Remove-Item -Force -ErrorAction SilentlyContinue
    $exe = Join-Path $dir 'forge.exe'
    # Un forge.exe abierto no se puede borrar pero sí renombrar: el nuevo ocupa su lugar.
    if (Test-Path $exe) { Rename-Item $exe "forge.exe.old-$([DateTime]::Now.Ticks)" }
    Copy-Item $new $exe
    Write-Host "Forge $version instalado en $exe"

    if ($env:FORGE_SKIP_USER_SETUP -eq '1') { return }
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    if (-not (($userPath -split ';') -contains $dir)) {
      [Environment]::SetEnvironmentVariable('Path', ((@($dir) + ($userPath -split ';' | Where-Object { $_ })) -join ';'), 'User')
      Write-Host 'Comando forge añadido al PATH (abre una terminal nueva para usarlo).'
    }

    $menu = Join-Path ([Environment]::GetFolderPath('Programs')) 'Forge.lnk'
    $shortcut = (New-Object -ComObject WScript.Shell).CreateShortcut($menu)
    $shortcut.TargetPath = $exe
    $shortcut.WorkingDirectory = $env:USERPROFILE
    $shortcut.Save()
    Write-Host 'Ábrelo desde el menú Inicio (Forge) o con: forge'
  } finally {
    Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue
  }
}
