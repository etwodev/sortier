<#
.SYNOPSIS
Builds FFmpeg's static libraries for Sortier's Windows build with vcpkg, once per machine.

.DESCRIPTION
Installs what vcpkg.json lists into .vcpkg\installed (git-ignored), where .cargo\config.toml points
ffmpeg-sys-next. vcpkg fetches its own MSYS2, NASM and FFmpeg source, so nothing else needs setting up.
The first run builds FFmpeg (usually 10 to 30 minutes); later runs reuse vcpkg's binary cache.

Uses vcpkg from the PATH, or else the one Visual Studio installs with its C++ tools.
#>
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot

$vcpkg = (Get-Command vcpkg -ErrorAction SilentlyContinue).Source
if (-not $vcpkg) {
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (Test-Path $vswhere) {
        $vs = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
        if ($vs -and (Test-Path (Join-Path $vs 'VC\vcpkg\vcpkg.exe'))) {
            $vcpkg = Join-Path $vs 'VC\vcpkg\vcpkg.exe'
        }
    }
}
if (-not $vcpkg) {
    throw 'vcpkg not found. Install Visual Studio''s "C++ build tools" with its vcpkg component, or clone https://github.com/microsoft/vcpkg, run bootstrap-vcpkg.bat and put it on the PATH.'
}

$triplet = if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') { 'arm64-windows-static-md' } else { 'x64-windows-static-md' }
$root = Join-Path $repo '.vcpkg'
New-Item -ItemType Directory -Force $root | Out-Null
# The vcpkg crate only accepts a VCPKG_ROOT holding this marker.
$marker = Join-Path $root '.vcpkg-root'
if (-not (Test-Path $marker)) { New-Item -ItemType File $marker | Out-Null }

Write-Host "Installing FFmpeg ($triplet) with $vcpkg"
& $vcpkg install --triplet $triplet "--x-manifest-root=$repo" "--x-install-root=$(Join-Path $root 'installed')"
if ($LASTEXITCODE -ne 0) { throw "vcpkg failed (exit code $LASTEXITCODE)" }
Write-Host 'FFmpeg is ready; cargo build will link it.'
