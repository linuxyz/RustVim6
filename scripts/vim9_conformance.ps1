# Vim9 conformance probe (M9) against the Vim 9.2 Vim9 reference.
#
# The editor baseline is v8.2.5172, but Vim9 script is anchored to
# v9.2.1398 (docs/VIM9.md). This script fetches/extracts the 9.2 test files
# (scripts/fetch-vim9-reference.ps1) and runs every Test_ function, reporting
# both "run to completion" and "all assertions passed".
param([string[]]$Files = @(
    'test_vim9_class.vim',
    'test_vim9_enum.vim',
    'test_vim9_interface.vim',
    'test_vim9_typealias.vim',
    'test_vim9_generics.vim'
))
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$ref = Join-Path 'reference' 'vim9-v9.2.1398'
$testdir = (Resolve-Path (Join-Path $ref 'src\testdir')).Path
if (!(Test-Path (Join-Path $testdir 'util\vim9.vim'))) {
    Write-Host 'Vim9 9.2 reference missing; running scripts/fetch-vim9-reference.ps1'
    & pwsh -File scripts/fetch-vim9-reference.ps1
    if ($LASTEXITCODE -ne 0) { throw 'fetch-vim9-reference.ps1 failed' }
}

$exe = 'target\debug\rvim.exe'
if (!(Test-Path $exe)) { $exe = 'target\release\rvim.exe' }
if (!(Test-Path $exe)) {
    Write-Host 'rvim.exe not found; run cargo build --offline first'
    exit 1
}
$exe = (Resolve-Path $exe).Path
$errFile = 'XrvimConformance.err'
$totalPass = 0
$totalRun = 0
$totalTests = 0

Push-Location $testdir
try {
    foreach ($file in $Files) {
        $path = Join-Path $testdir $file
        if (!(Test-Path $path)) {
            Write-Host ('skipped: ' + $file + ' missing')
            continue
        }
        $names = Select-String -Path $path -Pattern '^def (Test_\w+)' |
            ForEach-Object { $_.Matches[0].Groups[1].Value }
        $pass = 0
        $run = 0
        $failures = @()
        foreach ($testName in $names) {
            Remove-Item $errFile -ErrorAction SilentlyContinue
            $call = 'call ' + $testName + '()'
            $dump = "call writefile([string(len(get(v:, 'errors', [])))], '" + $errFile + "')"
            $output = & $exe -es -u NONE -S $file -c $call -c $dump -c 'qall!' 2>&1 | Out-String
            $code = $LASTEXITCODE
            $errors = if (Test-Path $errFile) { [int](Get-Content $errFile -Raw) } else { -1 }
            if ($code -eq 0) { $run++ }
            if ($code -eq 0 -and $errors -eq 0) {
                $pass++
            } else {
                $first = ($output -split [Environment]::NewLine | Where-Object { $_ -match '\S' } | Select-Object -First 1)
                $failures += ($testName + ' (exit=' + $code + ' errors=' + $errors + ') ' + $first)
            }
            Remove-Item $errFile -ErrorAction SilentlyContinue
        }
        $count = $names.Count
        $rate = if ($count -gt 0) { [math]::Round(100.0 * $pass / $count, 1) } else { 0.0 }
        Write-Host ($file + ': ' + $pass + '/' + $count + ' pass (' + $rate + '%), ' + $run + ' run to completion')
        $failures | Select-Object -First 3 | ForEach-Object { Write-Host ('  FAIL ' + $_) }
        $totalPass += $pass
        $totalRun += $run
        $totalTests += $count
    }
} finally {
    Pop-Location
}

$rate = if ($totalTests -gt 0) { [math]::Round(100.0 * $totalPass / $totalTests, 1) } else { 0.0 }
Write-Host ('Vim9 9.2 conformance: ' + $totalPass + '/' + $totalTests + ' Test_ functions pass (' + $rate + '%), ' + $totalRun + ' run to completion')
exit 0
