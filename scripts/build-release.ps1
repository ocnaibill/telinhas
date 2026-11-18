<#
.SYNOPSIS
  Builds the Telinhas installer and the latest.json the app updates itself from.

.EXAMPLE
  .\scripts\build-release.ps1 -BaseUrl https://example.org/dl -Server wss://example.org/ws

  -BaseUrl  Where the files will be served from, without the file name. The app
            looks for latest.json there on every start. Required.
  -Server   A signaling server to build into the app, so nobody types it (the
            version for friends). Leave out for a build that asks for it.
  -Notes    A line about what changed, shown in latest.json.

Needs: Rust, the Visual Studio Build Tools, LLVM (LIBCLANG_PATH), the FFmpeg
shared build (FFMPEG_DIR), cmake and Inno Setup 6.
Writes to dist\: Telinhas-Setup-<version>.exe, Telinhas-Setup.exe (the same file
under a name that never changes, for the first download) and latest.json.
Copy those three to the folder the server serves at /dl.
#>
param(
    [Parameter(Mandatory = $true)] [string] $BaseUrl,
    [string] $Server = "",
    [string] $Notes = "",
    [string] $FfmpegDir = $env:FFMPEG_DIR
)

$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
Set-Location $root

if (-not $FfmpegDir) { throw "Set FFMPEG_DIR (or pass -FfmpegDir) to the FFmpeg folder that has bin\*.dll." }
$BaseUrl = $BaseUrl.TrimEnd("/")

$iscc = @(
    "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
    "${env:ProgramFiles}\Inno Setup 6\ISCC.exe",
    "${env:LOCALAPPDATA}\Programs\Inno Setup 6\ISCC.exe"
) | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $iscc) { throw "Inno Setup 6 not found. Install it with: winget install JRSoftware.InnoSetup" }

$meta = cargo metadata --format-version 1 --no-deps | ConvertFrom-Json
$version = ($meta.packages | Where-Object { $_.name -eq "telinhas" }).version
$targetDir = $meta.target_directory
Write-Host "Building Telinhas $version"

# What the build bakes in: where to look for updates, and optionally the server.
$env:TELINHAS_UPDATE_URL = "$BaseUrl/latest.json"
if ($Server) { $env:TELINHAS_DEFAULT_SERVER = $Server } else { Remove-Item Env:TELINHAS_DEFAULT_SERVER -ErrorAction SilentlyContinue }

cargo build --release -p telinhas
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

# The exe and the FFmpeg DLLs, side by side, and nothing else.
$dist = Join-Path $root "dist"
$stage = Join-Path $dist "stage"
Remove-Item $stage -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $stage | Out-Null
Copy-Item (Join-Path $targetDir "release\telinhas.exe") $stage
Copy-Item (Join-Path $FfmpegDir "bin\*.dll") $stage
Write-Host ("Staged: " + ((Get-ChildItem $stage).Name -join ", "))

& $iscc "/DAppVersion=$version" "/DSourceDir=$stage" "/DOutputDir=$dist" (Join-Path $root "installer\telinhas.iss")
if ($LASTEXITCODE -ne 0) { throw "Inno Setup failed" }

$installerName = "Telinhas-Setup-$version.exe"
$installer = Join-Path $dist $installerName
Copy-Item $installer (Join-Path $dist "Telinhas-Setup.exe") -Force

$hash = (Get-FileHash $installer -Algorithm SHA256).Hash.ToLower()
$manifest = [ordered]@{
    version   = $version
    installer = $installerName
    sha256    = $hash
    notes     = $Notes
} | ConvertTo-Json
# UTF-8 without a byte order mark.
[System.IO.File]::WriteAllText((Join-Path $dist "latest.json"), $manifest, (New-Object System.Text.UTF8Encoding $false))

Write-Host ""
Write-Host "Done. Copy these to the folder served at $BaseUrl :"
Write-Host "  $installerName"
Write-Host "  Telinhas-Setup.exe"
Write-Host "  latest.json"
Write-Host "Friends install from: $BaseUrl/Telinhas-Setup.exe"
