# Build once and package a relocatable RustVim with its Vim runtime/help.
# CI supplies -SkipBuild after its release build; use -WithoutRuntime only for
# an explicitly minimal package. Default release packages require offline help.
param(
    [ValidateSet('windows', 'unix')][string]$Target = 'windows',
    [string]$Runtime = '',
    [ValidateSet('', 'x64', 'x86_64', 'aarch64')][string]$Architecture = '',
    [switch]$WithoutRuntime,
    [switch]$SkipBuild
)
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path -Parent $PSScriptRoot)

if (-not $SkipBuild) {
    cargo build --release --locked
    if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
}
$version = [regex]::Match((Get-Content Cargo.toml -Raw), '(?m)^version\s*=\s*"([^"]+)"').Groups[1].Value
if (-not $version) { throw 'workspace version missing from Cargo.toml' }
if (-not $Architecture) {
    $Architecture = if ($env:PROCESSOR_ARCHITECTURE -match 'ARM64') { 'aarch64' } else { 'x86_64' }
}
$platform = if ($Target -eq 'windows') { "windows-$Architecture" } else { "linux-$Architecture" }
$name = "rvim-$version-$platform"
$dist = [IO.Path]::GetFullPath((Join-Path (Get-Location) 'target/dist'))
$stage = [IO.Path]::GetFullPath((Join-Path $dist $name))
$exe = if ($Target -eq 'windows') { 'target/release/rvim.exe' } else { 'target/release/rvim' }
if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) { throw "missing $exe; build release first" }

if (-not $WithoutRuntime) {
    if (-not $Runtime) { $Runtime = $env:RVIM_RUNTIME }
    if (-not $Runtime) { $Runtime = 'reference/vim/runtime' }
    $Runtime = [IO.Path]::GetFullPath($Runtime)
    foreach ($required in @('filetype.vim', 'doc/help.txt', 'doc/tags', 'doc/uganda.txt')) {
        if (-not (Test-Path -LiteralPath (Join-Path $Runtime $required) -PathType Leaf)) {
            throw "runtime missing $required; run scripts/fetch-reference.ps1 or supply -Runtime"
        }
    }
}
# Check the final absolute target before replacing a prior staging directory.
$prefix = $dist.TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
if (-not $stage.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase)) { throw 'stage must remain under target/dist' }
if (Test-Path -LiteralPath $stage) { Remove-Item -LiteralPath $stage -Recurse -Force }
New-Item -ItemType Directory -Force -Path $stage | Out-Null
Copy-Item -LiteralPath $exe -Destination $stage
Copy-Item -LiteralPath README.md, LICENSE -Destination $stage
Copy-Item -LiteralPath docs -Destination (Join-Path $stage 'docs') -Recurse
if (-not $WithoutRuntime) { Copy-Item -LiteralPath $Runtime -Destination (Join-Path $stage 'runtime') -Recurse }
@"
RustVim $version ($platform)
Run rvim with a file, or use --gui on Windows.
Offline help: :help, :help 'number', :help :split
Follow |links| with Ctrl-]; return with Ctrl-T; close with :helpclose.
The bundled runtime is located beside the executable and works from any directory.
Optional override: RVIM_RUNTIME or VIMRUNTIME.
"@ | Set-Content -LiteralPath (Join-Path $stage 'RUNNING.txt') -Encoding utf8

$archive = Join-Path $dist $name
if ($Target -eq 'windows') {
    Compress-Archive -Path (Join-Path $stage '*') -DestinationPath "$archive.zip" -Force
    Write-Host "created $archive.zip"
} else {
    tar -czf "$archive.tar.gz" -C $stage .
    if ($LASTEXITCODE -ne 0) { throw 'tar failed' }
    Write-Host "created $archive.tar.gz"
}
Write-Host "staged $stage"
