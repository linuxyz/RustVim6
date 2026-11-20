# Package RustVim for release (M8).
#
# Creates a relocatable tree under target/dist/ with the executable, docs and
# (unless -WithoutRuntime) a copy of the Vim runtime, then archives it.  The
# runtime copy makes :syntax/:filetype/:help work without the source tree.
#
# Usage:
#   pwsh -File scripts/package.ps1
#   pwsh -File scripts/package.ps1 -WithoutRuntime -Target unix
#   pwsh -File scripts/package.ps1 -Runtime C:\path\to\runtime
param(
    [string]$Target = "windows",
    [string]$Runtime = "",
    [switch]$WithoutRuntime,
    [switch]$SkipBuild
)
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path -Parent $PSScriptRoot)

if (-not $SkipBuild) {
    cargo build --release --offline
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
}

$toml = Get-Content Cargo.toml -Raw
$version = [regex]::Match($toml, '(?m)^version\s*=\s*"([^"]+)"').Groups[1].Value
if (-not $version) { $version = "0.0.0" }
$arch = if ($env:PROCESSOR_ARCHITECTURE -match 'ARM64') { "aarch64" } else { "x86_64" }
$platform = if ($Target -eq "windows") { "windows-$arch" } else { "linux-$arch" }
$name = "rvim-$version-$platform"
$dist = Join-Path (Get-Location) "target\dist"
$stage = Join-Path $dist $name
if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
New-Item -ItemType Directory -Force $stage | Out-Null

$exe = if ($Target -eq "windows") { "target\release\rvim.exe" } else { "target\release\rvim" }
if (-not (Test-Path $exe)) { throw "missing $exe; build release first" }
Copy-Item $exe (Join-Path $stage (Split-Path $exe -Leaf))

foreach ($file in @("README.md", "LICENSE")) {
    if (Test-Path $file) { Copy-Item $file $stage }
}
if (Test-Path "docs") {
    Copy-Item "docs" (Join-Path $stage "docs") -Recurse
}

if (-not $WithoutRuntime) {
    if (-not $Runtime) {
        if (Test-Path "reference\vim\runtime") { $Runtime = "reference\vim\runtime" }
        elseif ($env:RVIM_RUNTIME) { $Runtime = $env:RVIM_RUNTIME }
    }
    if ($Runtime -and (Test-Path $Runtime)) {
        Copy-Item $Runtime (Join-Path $stage "runtime") -Recurse
    } else {
        Write-Warning "no Vim runtime found; the package will rely on RVIM_RUNTIME"
    }
}

# A tiny launcher/manifest so users can run the package without the source tree.
@"
RustVim $version ($platform)
Run: .\rvim.exe [file]
Runtime: .\runtime (or set RVIM_RUNTIME)
"@ | Set-Content (Join-Path $stage "RUNNING.txt") -Encoding UTF8

$archive = Join-Path $dist $name
if ($Target -eq "windows") {
    $zip = "$archive.zip"
    if (Test-Path $zip) { Remove-Item $zip -Force }
    Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip
    Write-Host "created $zip"
} else {
    $tar = "$archive.tar.gz"
    if (Test-Path $tar) { Remove-Item $tar -Force }
    tar -czf $tar -C $stage .
    if ($LASTEXITCODE -ne 0) { throw "tar failed" }
    Write-Host "created $tar"
}
Write-Host "staged $stage"
