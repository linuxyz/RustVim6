# 覆盖率表（COVERAGE）

> **自动生成，请勿手改**：`cargo run -p rvim-tools -- coverage`。
> 分母固定为基线 vim/vim `v8.2.5172`：589 条 Ex 命令、429 个 option、350+ 内置函数。
> 数据源：`crates/rvim-ex/src/cmd_table.rs`（由 `ex_cmds.h` 生成）+ `crates/rvim-ex/src/implemented.rs`（已实现清单）+ `reference/vim/runtime` 运行时脚本实测调用频率。

## 1. Ex 命令（分母 589）

| 指标 | 数值 |
|---|---|
| 已实现（声明表口径） | 311 / 589（52.8%） |
| 已实现（**用户默认语料对数桶加权**，M2 出口口径） | 122 / 122（**100.0%**） |
| 广度（语料实际用到的编辑命令中有多少已实现） | 50 / 50（100.0%） |
| 已实现（用户默认语料原始调用次数加权） | 3194 / 3231（98.9%） |
| 已实现（全 runtime 语料原始加权，参考） | 134864 / 138894（97.1%） |
| 加权语料 | 25 个脚本、3231 条调用（全 runtime 语料：1512 个脚本、138894 条） |
| 目标（M2 出口） | 对数桶加权 ≥ 60% |

> 加权口径：对数桶 `1+floor(log2(n))`，语料为 `defaults.vim` + `plugin/*` 等用户默认脚本；
> 排除 M3 VimL 解释器类命令（`:call` `:if` `:let` 等，见 `rvim-tools/src/freq.rs` 的 `SCRIPTING_M3`）。
> 原始调用次数加权与全 runtime 语料数字一并给出，避免口径过于讨巧。

## 2. 已实现命令（311 条）

按命令表顺序：

```text
:abbreviate :abclear :abstract :args :argadd :argdelete :argdo :autocmd
:augroup :aunmenu :buffer :badd :bdelete :behave :bfirst :blast :bmodified
:bnext :bprevious :break :buffers :bufdo :bunload :bwipeout :cNext :cabbrev
:cabclear :cabove :caddexpr :call :catch :cbelow :cc :cclose :cd :center
:cexpr :cfile :cfirst :cgetfile :cgetexpr :chdir :changes :clist :clast
:class :close :cmap :cmapclear :cnext :cnewer :cnoremap :cnoreabbrev :copy
:colder :command :comclear :continue :const :copen :cprevious :cquit
:crewind :cunmap :cunabbrev :cwindow :delete :def :defcompile :delcommand
:delfunction :display :diffupdate :diffget :diffoff :diffpatch :diffput
:diffsplit :diffthis :djump :dlist :doautocmd :edit :echo :echoerr :echohl
:echomsg :echon :else :elseif :endif :endinterface :endclass :endenum
:endfor :endtry :endwhile :enew :enum :eval :execute :exit :export :file
:files :filetype :final :finally :finish :fold :foldclose :folddoopen
:folddoclosed :foldopen :for :function :global :help :highlight :hide
:iabbrev :iabclear :if :imap :imapclear :import :inoremap :inoreabbrev
:interface :iunmap :iunabbrev :join :jumps :k :list :lNext :labove
:laddexpr :lbelow :lclose :left :let :lexpr :legacy :lfile :lfirst
:lgetfile :lgetexpr :ll :llast :llist :lmap :lmapclear :lnoremap :lnext
:lnewer :loadview :lolder :lopen :lprevious :lrewind :ltag :lunmap
:lvimgrep :lvimgrepadd :lwindow :ls :move :mark :map :mapclear :marks
:menutranslate :messages :mksession :mkspell :mkvimrc :mkview :mode :new
:nmap :nmapclear :nnoremap :noremap :noautocmd :nohlsearch :noreabbrev
:normal :number :nunmap :open :omap :omapclear :only :onoremap :ounmap
:print :packadd :packloadall :pop :ptag :put :pwd :quit :quitall :qall
:read :redo :redir :redraw :registers :resize :retab :return :right
:runtime :rviminfo :substitute :saveas :scriptversion :set :setfiletype
:setglobal :setlocal :silent :sleep :smap :smapclear :snoremap :source
:sort :split :spellgood :spelldump :spellinfo :spellrepall :spellrare
:spellundo :spellwrong :stop :stag :stselect :sunmap :suspend :sview
:syntax :t :tag :tags :tabclose :tabdo :tabedit :tabfirst :tabmove :tablast
:tabnext :tabnew :tabonly :tabprevious :tabs :terminal :tfirst :throw
:tjump :tlast :tlunmenu :tnext :tprevious :trewind :try :tselect :type
:undo :unabbreviate :unlet :unmap :update :vglobal :var :version :vertical
:vimgrep :vimgrepadd :vim9cmd :vmap :vmapclear :vnoremap :vnew :vsplit
:vunmap :write :wall :while :wincmd :windo :wq :wqall :wviminfo :xit :xmap
:xmapclear :xnoremap :xunmap :yank :! :# :& :< :> :~ :X
```

## 3. 未实现命令中权重最高的 30 条

| rank | 命令 | runtime 调用次数 | 地址类型 |
|---|---|---|---|

## 4. 其它兼容表

| 类别 | 已实现 | 分母 | 说明 |
|---|---|---|---|
| Options | 125 | 429 | 全局选项子集（window/buffer 作用域在 M5） |
| 内置函数 | 543 | 543 | 纯函数在 `rvim-viml`，宿主相关在 `rvim-ex`；M3 目标 ≥70% |


内置函数覆盖率：543/543（100.0%）。

