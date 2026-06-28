param(
    [string]$OutDir = "target\release-assets"
)

$ErrorActionPreference = 'Stop'
$Root = Resolve-Path (Join-Path $PSScriptRoot '..')
$Out = Join-Path $Root $OutDir
New-Item -ItemType Directory -Force -Path $Out | Out-Null

cargo build --release

$Exe = Join-Path $Root 'target\release\warmup-companion.exe'
if (-not (Test-Path $Exe)) {
    throw "Missing release binary: $Exe"
}

$Setup = Join-Path $Root 'target\warmup-companion-setup.exe'
if (Get-Command makensis -ErrorAction SilentlyContinue) {
    makensis (Join-Path $Root 'install\warmup-companion.nsi')
} elseif (-not (Test-Path $Setup)) {
    throw "makensis not found and installer does not exist: $Setup"
}

$Assets = @($Exe)
if (Test-Path $Setup) {
    $Assets += $Setup
}

foreach ($Asset in $Assets) {
    $Name = Split-Path $Asset -Leaf
    $Dest = Join-Path $Out $Name
    Copy-Item $Asset $Dest -Force

    $Hash = (Get-FileHash $Dest -Algorithm SHA256).Hash.ToLowerInvariant()
    "$Hash  $Name" | Set-Content -Path "$Dest.sha256" -NoNewline -Encoding ascii
    Write-Host "Wrote $Dest"
    Write-Host "Wrote $Dest.sha256"
}
