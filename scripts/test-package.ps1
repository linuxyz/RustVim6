# Validate the staged/extracted executable and help from an unrelated directory.
param([Parameter(Mandatory=$true)][string]$PackageRoot)
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path -LiteralPath $PackageRoot).Path
$exe = Join-Path $root 'rvim.exe'
if (-not (Test-Path -LiteralPath $exe)) { $exe = Join-Path $root 'rvim' }
if (-not (Test-Path -LiteralPath $exe)) { throw 'packaged executable missing' }
$runtime = Join-Path $root 'runtime'
foreach ($file in @('doc/help.txt', 'doc/tags', 'doc/uganda.txt')) {
    if (-not (Test-Path -LiteralPath (Join-Path $runtime $file))) { throw "packaged runtime missing $file" }
}
$workspace = Split-Path -Parent $PSScriptRoot
$scratch = Join-Path $workspace ('target/package-smoke-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $scratch | Out-Null
$result = Join-Path $scratch 'help-result.txt'
$script = Join-Path $scratch 'help-smoke.vim'
$escapedResult = $result.Replace('\', '/').Replace("'", "''")
@"
help
call assert_equal('help', &buftype)
call assert_equal('help.txt', expand('%:t'))
help 'number'
call assert_equal('help', &buftype)
call assert_true(stridx(getline('.'), "*'number'*") >= 0)
let g:help_errors = exists('v:errors') ? v:errors : []
call writefile([&runtimepath, expand('%:p'), string(g:help_errors)], '$escapedResult')
if len(g:help_errors) > 0
  cquit
endif
qall!
"@ | Set-Content -LiteralPath $script -Encoding utf8
$savedRvim = $env:RVIM_RUNTIME
$savedVim = $env:VIMRUNTIME
Push-Location $scratch
try {
    Remove-Item Env:RVIM_RUNTIME, Env:VIMRUNTIME -ErrorAction SilentlyContinue
    & $exe -es -u NONE -S $script
    if ($LASTEXITCODE -ne 0) { throw "packaged help smoke failed (exit $LASTEXITCODE)" }
    if (-not (Test-Path -LiteralPath $result)) { throw 'help smoke did not produce results' }
    $lines = Get-Content -LiteralPath $result
    $actual = [IO.Path]::GetFullPath($lines[0]).Replace('\\?\', '')
    $expected = [IO.Path]::GetFullPath($runtime)
    if ($actual -ne $expected) { throw "help used $actual instead of bundled $expected" }
    $doc = [IO.Path]::GetFullPath((Join-Path $runtime 'doc/options.txt'))
    if ([IO.Path]::GetFullPath($lines[1]).Replace('\\?\', '') -ne $doc -or $lines[2] -ne '[]') {
        throw "packaged help returned unexpected results: $($lines -join '; ')"
    }
    Write-Host "Packaged offline help passed: $root"
} finally {
    Pop-Location
    $env:RVIM_RUNTIME = $savedRvim
    $env:VIMRUNTIME = $savedVim
}
