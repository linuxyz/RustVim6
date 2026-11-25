# CI gate for RustVim. Runs every check the milestone plan requires and fails
# on the first regression:
#   1. formatting         cargo fmt --all -- --check
#   2. lints              cargo clippy --offline --all-targets (zero warnings)
#   3. tests              cargo test --offline
#   4. zero dependencies  cargo tree  (only workspace crates)
#   5. coverage tables    cargo run -p rvim-tools -- coverage --check
#   6. batch smoke        scripts/smoke.ps1
#
# Usage:  pwsh -File scripts/ci.ps1 [-SkipSmoke]
param(
    [switch]$SkipSmoke
)
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path -Parent $PSScriptRoot)

function Invoke-Gate([string]$name, [scriptblock]$body) {
    Write-Host "=== $name ===" -ForegroundColor Cyan
    & $body
    if ($LASTEXITCODE -ne 0) {
        Write-Host "GATE FAILED: $name (exit $LASTEXITCODE)" -ForegroundColor Red
        exit $LASTEXITCODE
    }
}

# The Vim baseline (reference/vim, ~57 MB) is not versioned: the runtime-script
# tests, the coverage corpus and two smoke scenarios need it. Fetch it once.
if (-not (Test-Path 'reference/vim/runtime/filetype.vim')) {
    Write-Host '=== fetch reference/vim (not versioned) ===' -ForegroundColor Cyan
    pwsh -NoProfile -File scripts/fetch-reference.ps1
    if ($LASTEXITCODE -ne 0) {
        Write-Host 'cannot fetch reference/vim; see docs/ARCH.md section 8' -ForegroundColor Red
        exit 1
    }
}

Invoke-Gate 'cargo fmt --check' { cargo fmt --all -- --check }
Invoke-Gate 'cargo clippy' { cargo clippy --offline --all-targets -- -D warnings }
Invoke-Gate 'cargo test' { cargo test --offline }

Invoke-Gate 'zero third-party dependencies' {
    # Every dependency line in `cargo tree` must be a workspace crate.
    # The Windows GUI platform layer is allowed to use winapi.
    $tree = cargo tree --offline --prefix none 2>&1
    if ($LASTEXITCODE -ne 0) {
        $tree | Write-Host
        exit 1
    }
    $external = $tree | Where-Object { $_ -match '^[a-zA-Z0-9_]' } |
        Where-Object { $_ -notmatch '^rvim' } |
        Where-Object { $_ -notmatch '^\(' } |
        Where-Object { $_ -notmatch '^#' } |
        ForEach-Object { ($_ -split ' ')[0] } |
        Sort-Object -Unique
    $allowed = @('winapi', 'winapi-i686-pc-windows-gnu', 'winapi-x86_64-pc-windows-gnu')
    $external = $external | Where-Object { $allowed -notcontains $_ }
    if ($external) {
        Write-Host "third-party crates found: $($external -join ', ')" -ForegroundColor Red
        exit 1
    }
}

Invoke-Gate 'coverage tables up to date' { cargo run --offline -q -p rvim-tools -- coverage --check }

if (-not $SkipSmoke) {
    Invoke-Gate 'batch smoke scenarios' { pwsh -NoProfile -File scripts/smoke.ps1 }
}

Write-Host 'ALL GATES PASSED' -ForegroundColor Green
