$ErrorActionPreference = 'Continue'
Set-Location C:\BUILD\RustVim6
$exe = 'target\debug\rvim.exe'

# The vendored Vim baseline is not versioned (scripts/fetch-reference.ps1); the
# scenarios that need its runtime scripts skip themselves when it is missing.
$hasReference = Test-Path (Join-Path (Get-Location) 'reference\vim\runtime\filetype.vim')
$work = Join-Path $env:TEMP 'rvim_smoke'
New-Item -ItemType Directory -Force -Path $work | Out-Null

function Write-NoBom([string]$path, [string]$text) {
    # Windows PowerShell's -Encoding utf8 writes a BOM; scripts should be plain
    # UTF-8. (rvim tolerates a BOM too, but the smoke file mirrors real vimrcs.)
    [System.IO.File]::WriteAllText($path, $text, (New-Object System.Text.UTF8Encoding($false)))
}

Write-Host '--- 1. --version ---'
& $exe --version

Write-Host '--- 2. -es set + print range ---'
$f1 = Join-Path $work 'sample.txt'
Set-Content -Path $f1 -Value ([char[]](97,108,112,104,97,10,98,101,116,97,10,103,97,109,109,97,10,100,101,108,116,97) -join '') -NoNewline -Encoding utf8
& $exe -es -c 'set et tabstop=4' -c '2,3p' $f1
Write-Host ('exit=' + $LASTEXITCODE)

Write-Host '--- 3. substitute then write, verify on disk ---'
$f2 = Join-Path $work 'sub.txt'
Set-Content -Path $f2 -Value 'cat bat cat' -NoNewline -Encoding utf8
& $exe -es -c '%s/cat/dog/g' -c 'wq' $f2
Write-Host ('exit=' + $LASTEXITCODE)
Write-Host ('disk: ' + (Get-Content $f2 -Raw))

Write-Host '--- 4. delete line + wq via stdin script ---'
$f3 = Join-Path $work 'del.txt'
Set-Content -Path $f3 -Value ([char[]](97,10,98,10,99) -join '') -NoNewline -Encoding utf8
'1d', 'wq' | & $exe -es $f3
Write-Host ('disk: ' + (Get-Content $f3 -Raw))

Write-Host '--- 5. unknown command yields non-zero exit but no crash ---'
& $exe -es -c 'boguscmd' -c 'q'
Write-Host ('exit=' + $LASTEXITCODE)

Write-Host '--- 6. dirty buffer blocks :q, :q! allowed ---'
$f4 = Join-Path $work 'dirty.txt'
Set-Content -Path $f4 -Value 'x' -NoNewline -Encoding utf8
& $exe -es -c 's/x/y/' -c 'q' $f4
Write-Host ('plain q exit=' + $LASTEXITCODE)
& $exe -es -c 's/x/y/' -c 'q!' $f4
Write-Host ('q! exit=' + $LASTEXITCODE)

Write-Host '--- 7. -u vimrc sources options, mappings and user commands ---'
$vimrc = Join-Path $work 'vimrc'
Write-NoBom $vimrc @'
" M2 smoke vimrc
set expandtab shiftwidth=2
nmap ,w :write<CR>
command -nargs=1 Greet echomsg '<args>'
autocmd BufRead *.log setlocal nowrap
'@
& $exe -es -u $vimrc -c 'Greet hello-from-vimrc' -c 'set sw?'
Write-Host ('vimrc exit=' + $LASTEXITCODE)

Write-Host '--- 8. :g deletes matching lines, :v keeps only matches ---'
$f5 = Join-Path $work 'glob.txt'
Set-Content -Path $f5 -Value ([char[]](97,108,112,104,97,10,98,101,116,97,10,97,108,112,104,97,50) -join '') -NoNewline -Encoding utf8
& $exe -es -c 'g/alpha/d' -c 'wq' $f5
Write-Host ('g/alpha/d -> ' + (Get-Content $f5 -Raw))

Write-Host '--- 9. :source honours :finish ---'
$script = Join-Path $work 'script.vim'
Write-NoBom $script @'
set tabstop=3
finish
set tabstop=9
'@
& $exe -es -c ('source ' + $script) -c 'set ts?'
Write-Host ('source exit=' + $LASTEXITCODE)

Write-Host '--- 10. :command user command with -nargs, :map listing ---'
& $exe -es -c "command -nargs=0 Hello echomsg 'hi-there'" -c 'Hello' -c 'nmap x dd' -c 'nmap'
Write-Host ('user-command exit=' + $LASTEXITCODE)

Write-Host '--- 11. VimL: functions, control flow, try/catch ---'
$viml = Join-Path $work 'viml.vim'
$vimlText = @('function! s:Twice(x)', '  return a:x * 2', 'endfunction', 'let g:acc = []', 'for i in range(1, 3)', '  call add(g:acc, s:Twice(i))', 'endfor', 'try', '  throw ''boom''', 'catch /boom/', '  call add(g:acc, ''caught'')', 'finally', '  call add(g:acc, ''done'')', 'endtry', 'echomsg string(g:acc)', 'if s:Twice(3) == 6', '  echomsg ''math-ok''', 'endif') -join "`n"
Write-NoBom $viml $vimlText
& $exe -es -S $viml
Write-Host ('viml exit=' + $LASTEXITCODE)

Write-Host '--- 12. source the real defaults.vim (M3 exit criterion) ---'
if ($hasReference) {
    & $exe -es -u (Join-Path (Get-Location) 'reference\vim\runtime\defaults.vim') -c 'set ruler?'
    Write-Host ('defaults exit=' + $LASTEXITCODE)
} else {
    Write-Host 'skipped: defaults.vim (reference/vim missing; run scripts/fetch-reference.ps1)'
}

# --- M1/M2 review regressions ---------------------------------------------

Write-Host '--- 13. errors inside a sourced script make the run fail ---'
$bad = Join-Path $work 'bad.vim'
Write-NoBom $bad "let g:ok = 1`nbogus_command_here`nlet g:after = 2"
& $exe -es -u NONE -c ('source ' + $bad) -c 'q'
Write-Host ('source-error exit=' + $LASTEXITCODE + ' (expect 1)')

Write-Host '--- 14. --cmd runs before the vimrc ---'
$vrc = Join-Path $work 'pre.vimrc'
Write-NoBom $vrc "echomsg 'vc:' . g:pre"
& $exe -es --cmd "let g:pre = 'PRE'" -u $vrc -c 'q'
Write-Host ('--cmd exit=' + $LASTEXITCODE + ' (expect vc:PRE)')

Write-Host '--- 15. +N jumps to a line, bare + jumps to the last line ---'
$f6 = Join-Path $work 'jump.txt'
Set-Content -Path $f6 -Value ([char[]](111,110,101,10,116,119,111,10,116,104,114,101,101) -join '') -NoNewline -Encoding utf8
& $exe -es -u NONE '+2' -c 'p' $f6 -c 'q'
& $exe -es -u NONE '+' -c 'p' $f6 -c 'q'

Write-Host '--- 16. :silent! suppresses the error, plain errors still fail ---'
& $exe -es -u NONE -c 'silent! boguscmd' -c 'q'
Write-Host ('silent-bang exit=' + $LASTEXITCODE + ' (expect 0)')

Write-Host '--- 17. address-only lines: :N jumps, :1,2 prints in Ex mode, :v inverts ---'
$f7 = Join-Path $work 'addr.txt'
Set-Content -Path $f7 -Value ([char[]](97,108,112,104,97,10,98,101,116,97,10,97,108,112,104,97,50) -join '') -NoNewline -Encoding utf8
& $exe -es -u NONE -c '1,2' $f7 -c 'q'
& $exe -es -u NONE -c 'v/alpha/d' -c '%p' -c 'q!' $f7
Write-Host ('addr exit=' + $LASTEXITCODE)

# --- M3: autocmd dispatch and filetype detection ---------------------------

Write-Host '--- 18. autocmd events fire on open/write ---'
$f8 = Join-Path $work 'dispatch.py'
Write-NoBom $f8 "print(1)"
# NOTE: single-quoted VimL strings — PowerShell 5.1 mangles embedded double
# quotes when rebuilding a native command line.
& $exe -es -u NONE -c "au BufRead * echo 'read-event'" -c "au BufWritePre * echo 'pre-event'" -c ('e ' + $f8) -c '1s/print/pass/' -c 'w'
Write-Host ('dispatch exit=' + $LASTEXITCODE)

Write-Host '--- 19. :filetype on + real filetype.vim detection ---'
if ($hasReference) {
    & $exe -es -c 'filetype on' -c ('e ' + $f8) -c 'set ft?'
    Write-Host ('filetype exit=' + $LASTEXITCODE)
} else {
    Write-Host 'skipped: filetype.vim (reference/vim missing; run scripts/fetch-reference.ps1)'
}

Write-Host '--- 20. FileType chain and VimL expression rules ---'
& $exe -es -u NONE -c 'au BufRead *.py setf python' -c "au FileType python echo 'ft:' . &ft" -c ('e ' + $f8) -c 'q!'
& $exe -es -u NONE -c 'echo 1 || 0' -c "echo 'a' . strlen('ab')" -c 'q'
Write-Host ('m3-chain exit=' + $LASTEXITCODE)

# --- M4: interactive TUI (driven headlessly through --keys) ----------------

Write-Host '--- 21. TUI: scripted keys edit and write ---'
$f9 = Join-Path $work 'tui.txt'
Write-NoBom $f9 "one`ntwo"
& $exe --keys 'A!<Esc>:wq<CR>' --cols 30 --rows 6 $f9
Write-Host ('tui edit exit=' + $LASTEXITCODE)
Write-Host ('disk: ' + ((Get-Content $f9 -Raw) -replace "`r", '|'))

Write-Host '--- 22. TUI: screen shows buffer, end-of-buffer and statusline ---'
& $exe --keys '<Nop>' --cols 24 --rows 5 $f9

Write-Host '--- 23. TUI: mapping, dd and message line ---'
& $exe --keys ':nmap Q dd<CR>Q' --cols 30 --rows 6 $f9
Write-Host ('tui keys exit=' + $LASTEXITCODE)
& $exe --keys ':boguscmd<CR>' --cols 40 --rows 5 $f9
Write-Host ('tui error exit=' + $LASTEXITCODE + ' (expect 1)')

# --- M5: windows, tabs, folds, diff and quickfix --------------------------

Write-Host '--- 24. M5: :split / :vsplit / <C-w> move ---'
$f10 = Join-Path $work 'm5.txt'
Write-NoBom $f10 "alpha`nbeta`ngamma`ndelta"
& $exe --keys ':split<CR>:vsplit<CR><C-w>w<C-w>w<C-w>c<C-w>c' --cols 40 --rows 10 $f10
Write-Host ('m5 windows exit=' + $LASTEXITCODE)

Write-Host '--- 25. M5: tabpages and tabline ---'
& $exe --keys ':tabnew<CR>:tabnext 1<CR>:tabprevious<CR>' --cols 60 --rows 8 $f10
Write-Host ('m5 tabs exit=' + $LASTEXITCODE)

Write-Host '--- 26. M5: folds (zf, zo, zR, zM) ---'
& $exe --keys 'Vjzfzo:set foldcolumn=1<CR>zM' --cols 40 --rows 10 $f10
Write-Host ('m5 folds exit=' + $LASTEXITCODE)

Write-Host '--- 27. M5: visual mode and text objects ---'
& $exe --keys 'viwd' --cols 30 --rows 6 $f10
Write-Host ('m5 visual exit=' + $LASTEXITCODE)

Write-Host '--- 28. M5: dot repeat ---'
& $exe --keys 'ihello<Esc>.x' --cols 30 --rows 6 $f10
Write-Host ('m5 repeat exit=' + $LASTEXITCODE)

Write-Host '--- 29. M5: vimdiff (-d) with a changed line ---'
$f11 = Join-Path $work 'm5b.txt'
Write-NoBom $f11 "alpha`nBETA`ngamma`ndelta"
& $exe -d --keys ']c[c' --cols 70 --rows 10 $f10 $f11
Write-Host ('m5 vimdiff exit=' + $LASTEXITCODE)

Write-Host '--- 30. M5: quickfix (:cexpr, :copen, :cnext) ---'
& $exe --keys (":cexpr ['" + ($f10 -replace '\\','/') + ":2:1: m5 error']<CR>:copen<CR>:cnext<CR>:cclose<CR>") --cols 70 --rows 12 $f10
Write-Host ('m5 quickfix exit=' + $LASTEXITCODE)

Write-Host '--- 31. M5: jumps/changes and viminfo round trip ---'
$vifile = Join-Path $work 'viminfo.txt'
& $exe -es -c 'call setline(1, "x")' -c ('wviminfo ' + $vifile) -c 'q!' $f10
& $exe -es -c 'jumps' -c 'changes' -c 'q' $f10
Write-Host ('m5 session exit=' + $LASTEXITCODE)
# --- 32. Cross-milestone gaps closed by the M0-M5 review ------------------

Write-Host '--- 32. :normal runs normal/insert/visual keys ---'
$f12 = Join-Path $work 'normal.txt'
Write-NoBom $f12 ("one" + [Environment]::NewLine + "two" + [Environment]::NewLine + "three")
& $exe -es -c '%normal! A;' -c 'wq' $f12
Write-Host ('normal exit=' + $LASTEXITCODE)
Write-Host ('disk: ' + ((Get-Content $f12 -Raw) -replace "\r", '|'))
& $exe -es -c 'normal! VU' -c 'wq' $f12
Write-Host ('disk: ' + ((Get-Content $f12 -Raw) -replace "\r", '|'))

Write-Host '--- 33. TUI: command-line history and insert-mode <C-w> ---'
& $exe --keys ':echo 42<CR>:<Up><CR>' --cols 40 --rows 6 $f12
Write-Host ('history exit=' + $LASTEXITCODE)
& $exe --keys 'A foo bar<C-w>baz<Esc>' --cols 40 --rows 6 $f12
Write-Host ('insert ctrl-w exit=' + $LASTEXITCODE)

Write-Host '--- 34. TUI: relativenumber and scrolloff ---'
& $exe --keys ':set rnu<CR>2G' --cols 20 --rows 8 $f12
Write-Host ('rnu exit=' + $LASTEXITCODE)
& $exe --keys ':set so=2<CR>3G' --cols 20 --rows 8 $f12
Write-Host ('scrolloff exit=' + $LASTEXITCODE)
