# Fetch the Vim 9.2 Vim9 reference (default v9.2.1398) and extract the files
# needed for Vim9 compatibility tests. The editor baseline stays v8.2.5172;
# Vim9 script is anchored to this newer tag (see docs/VIM9.md).
param([string]$Tag = 'v9.2.1398')
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

if (!(Test-Path 'reference\vim\.git')) {
    Write-Host 'reference/vim missing; running scripts/fetch-reference.ps1 first'
    & pwsh -File scripts/fetch-reference.ps1
    if ($LASTEXITCODE -ne 0) { throw 'fetch-reference.ps1 failed' }
}

$ref = 'refs/tags/' + $Tag
git -C reference\vim fetch --depth 1 origin ($ref + ':' + $ref)
if ($LASTEXITCODE -ne 0) { throw ('cannot fetch ' + $Tag) }

$dest = Join-Path 'reference' ('vim9-' + $Tag)
$files = @(
    'runtime/autoload/ccomplete.vim',
    'runtime/autoload/dist/ft.vim',
    'runtime/autoload/dist/script.vim',
    'runtime/autoload/dist/vim9.vim',
    'runtime/autoload/dist/vim.vim',
    'runtime/autoload/dist/json.vim',
    'runtime/autoload/dist/man.vim',
    'runtime/autoload/dist/vimindent.vim',
    'src/testdir/util/vim9.vim',
    'src/testdir/test_vim9_expr.vim',
    'src/testdir/test_vim9_func.vim',
    'src/testdir/test_vim9_script.vim',
    'src/testdir/test_vim9_cmd.vim',
    'src/testdir/test_vim9_assign.vim',
    'src/testdir/test_vim9_class.vim',
    'src/testdir/test_vim9_enum.vim',
    'src/testdir/test_vim9_interface.vim',
    'src/testdir/test_vim9_typealias.vim',
    'src/testdir/test_vim9_generics.vim',
    'src/testdir/test_vim9_import.vim'
)

$encoding = New-Object System.Text.UTF8Encoding($false)
$newline = [Environment]::NewLine
foreach ($file in $files) {
    $out = Join-Path $dest $file
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $out) | Out-Null
    $lines = git -C reference\vim show (($Tag + ':') + $file)
    if ($LASTEXITCODE -ne 0) {
        Write-Host ('skipped (tag has no ' + $file + ')')
        continue
    }
    [IO.File]::WriteAllText($out, (($lines -join $newline) + $newline), $encoding)
}
Write-Host ('Vim9 reference extracted to ' + $dest)
