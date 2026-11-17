# Fetch the pinned Vim baseline (reference implementation + $VIMRUNTIME).
#
# The checkout is ~57 MB and deliberately NOT versioned (see .gitignore), so a
# fresh clone has to run this once before the full test suite, the coverage
# table check and the smoke scenarios can exercise the real runtime scripts.
#
# Pinned anchor (docs/ARCH.md §8):
#   tag    v8.2.5172
#   commit c19f1a3488c1bb6fe5707dc6338b13cb9f598670
#   tree   0faf2a34481cb45910a7a765c24370adc6c05735
#
# Usage: pwsh -NoProfile -File scripts/fetch-reference.ps1 [-Force]
param(
    [switch]$Force
)
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path -Parent $PSScriptRoot)

$tag = 'v8.2.5172'
$commit = 'c19f1a3488c1bb6fe5707dc6338b13cb9f598670'
$tree = '0faf2a34481cb45910a7a765c24370adc6c05735'
$dest = Join-Path (Get-Location) 'reference/vim'

if ((Test-Path (Join-Path $dest 'runtime/filetype.vim')) -and -not $Force) {
    Write-Host "reference/vim already present (use -Force to re-fetch)" -ForegroundColor Yellow
    exit 0
}
if ($Force -and (Test-Path $dest)) {
    Remove-Item -Recurse -Force $dest
}

New-Item -ItemType Directory -Force -Path (Split-Path -Parent $dest) | Out-Null
Write-Host "cloning vim $tag (shallow) into reference/vim ..."
git clone --quiet --depth 1 --branch $tag https://github.com/vim/vim.git $dest
if ($LASTEXITCODE -ne 0) {
    Write-Host "git clone failed: check network/proxy access to github.com" -ForegroundColor Red
    exit 1
}

$head = (git -C $dest rev-parse HEAD).Trim()
$headTree = (git -C $dest rev-parse 'HEAD^{tree}').Trim()
if ($head -ne $commit -or $headTree -ne $tree) {
    Write-Host "baseline mismatch: HEAD=$head tree=$headTree" -ForegroundColor Red
    Write-Host "expected:            HEAD=$commit tree=$tree (docs/ARCH.md §8)" -ForegroundColor Red
    exit 1
}
Write-Host "reference/vim ready: $tag ($headTree)" -ForegroundColor Green
