# 覆盖率表（COVERAGE）

> **自动生成，请勿手改**：`cargo run -p rvim-tools -- coverage`。
> 分母固定为基线 vim/vim `v8.2.5172`：589 条 Ex 命令、429 个 option、350+ 内置函数。
> 数据源：`crates/rvim-ex/src/cmd_table.rs`（由 `ex_cmds.h` 生成）+ `crates/rvim-ex/src/implemented.rs`（已实现清单）+ `reference/vim/runtime` 运行时脚本实测调用频率。

## 1. Ex 命令（分母 589）

| 指标 | 数值 |
|---|---|
| 已实现（声明表口径） | 165 / 589（28.0%） |
| 已实现（**用户默认语料对数桶加权**，M2 出口口径） | 99 / 122（**81.1%**） |
| 广度（语料实际用到的编辑命令中有多少已实现） | 35 / 50（70.0%） |
| 已实现（用户默认语料原始调用次数加权） | 3165 / 3231（98.0%） |
| 已实现（全 runtime 语料原始加权，参考） | 111167 / 138894（80.0%） |
| 加权语料 | 25 个脚本、3231 条调用（全 runtime 语料：1512 个脚本、138894 条） |
| 目标（M2 出口） | 对数桶加权 ≥ 60% |

> 加权口径：对数桶 `1+floor(log2(n))`，语料为 `defaults.vim` + `plugin/*` 等用户默认脚本；
> 排除 M3 VimL 解释器类命令（`:call` `:if` `:let` 等，见 `rvim-tools/src/freq.rs` 的 `SCRIPTING_M3`）。
> 原始调用次数加权与全 runtime 语料数字一并给出，避免口径过于讨巧。

## 2. 已实现命令（165 条）

按命令表顺序：

```text
:abbreviate :abclear :args :argadd :argdelete :autocmd :augroup :buffer 
:badd :bdelete :bfirst :blast :bmodified :bnext :bprevious :break :buffers 
:bunload :bwipeout :cabbrev :cabclear :call :catch :cd :center :chdir :cmap 
:cmapclear :cnoremap :cnoreabbrev :copy :command :comclear :continue :cquit 
:cunmap :cunabbrev :delete :delcommand :delfunction :display :doautocmd 
:edit :echo :echoerr :echohl :echomsg :echon :else :elseif :endif :endfor 
:endtry :endwhile :enew :eval :execute :exit :file :files :filetype 
:finally :finish :for :function :global :help :iabbrev :iabclear :if :imap 
:imapclear :inoremap :inoreabbrev :iunmap :iunabbrev :join :k :list :left 
:let :lmap :lmapclear :lnoremap :lunmap :ls :move :mark :map :mapclear 
:marks :messages :nmap :nmapclear :nnoremap :noremap :nohlsearch 
:noreabbrev :number :nunmap :omap :omapclear :onoremap :ounmap :print :put 
:pwd :quit :quitall :qall :read :redo :redraw :registers :retab :return 
:right :runtime :substitute :saveas :set :setfiletype :setglobal :setlocal 
:silent :smap :smapclear :snoremap :source :sort :stop :sunmap :suspend 
:syntax :t :throw :try :undo :unabbreviate :unlet :unmap :update :vglobal 
:version :vmap :vmapclear :vnoremap :vunmap :write :wall :while :wq :wqall 
:xit :xmap :xmapclear :xnoremap :xunmap :yank :! :# :& :< :> :~
```

## 3. 未实现命令中权重最高的 30 条

| rank | 命令 | runtime 调用次数 | 地址类型 |
|---|---|---|---|
| 1 | `:wincmd` | 4 | other |
| 2 | `:highlight` | 3 | none |
| 3 | `:legacy` | 2 | none |
| 4 | `:mode` | 2 | none |
| 5 | `:packadd` | 2 | none |
| 6 | `:redir` | 2 | none |
| 7 | `:vimgrep` | 2 | other |
| 8 | `:aunmenu` | 1 | none |
| 9 | `:behave` | 1 | none |
| 10 | `:diffthis` | 1 | none |
| 11 | `:menutranslate` | 1 | none |
| 12 | `:noautocmd` | 1 | none |
| 13 | `:open` | 1 | lines |
| 14 | `:tlunmenu` | 1 | other |
| 15 | `:windo` | 1 | windows |

## 4. 其它兼容表

| 类别 | 已实现 | 分母 | 说明 |
|---|---|---|---|
| Options | 56 | 429 | 全局选项子集（window/buffer 作用域在 M5） |
| 内置函数 | 389 | 543 | 纯函数在 `rvim-viml`，宿主相关在 `rvim-ex`；M3 目标 ≥70% |


内置函数覆盖率：389/543（71.6%）。

