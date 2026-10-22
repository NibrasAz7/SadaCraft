# Regenerates the SadaCraft icon set from icons/icon-source.svg.
# Requires `pnpm install` to have been run in app/frontend (uses the
# locally installed @tauri-apps/cli; no extra rasterizer needed).
# Usage: powershell -File tools/gen-icons.ps1

$ErrorActionPreference = 'Stop'

$repoRoot = Split-Path -Parent $PSScriptRoot
$frontend = Join-Path $repoRoot 'app\frontend'
$source   = 'src-tauri\icons\icon-source.svg'
$outDir   = 'src-tauri\icons'

$tauri = Join-Path $frontend 'node_modules\.bin\tauri.cmd'
if (-not (Test-Path $tauri)) {
    throw "tauri CLI not found at $tauri - run 'pnpm install' in app/frontend first."
}

Push-Location $frontend
try {
    & $tauri icon $source -o $outDir
    if ($LASTEXITCODE -ne 0) { throw "tauri icon failed with exit code $LASTEXITCODE" }
} finally {
    Pop-Location
}

Write-Host "Icons regenerated into app\frontend\src-tauri\icons"
Write-Host "Note: app\frontend\public\{icon_128x128,icon_32x32@2x,logo-collapsed,logo}.png are separate copies - refresh them too if the mark changes."
