# RustVim 实施计划（PLAN）

> 配套文档：架构见 [ARCH.md](ARCH.md)，Runtime 兼容清单见 [RUNTIME_COMPAT.md](RUNTIME_COMPAT.md)。
> 参考基线：vim/vim `v8.2.5172`。约束：**std-only、零第三方依赖、离线可构建**；主平台 Windows，兼顾 Unix。

## 0. 总体路线

```
M0 脚手架 ─► M1 Headless 编辑内核 ─► M2 Ex 命令体系 ─► M3 VimL 解释器
        ─► M4 TUI 终端前端 ─► M5 窗口/高级编辑 ─► M6 语法/拼写/加密
        ─► M7 Job/Channel/Terminal ─► M8 Vim9 script 与兼容收口
```
每个里程碑都要求：**可编译、可测试、有演示、有覆盖率数字**。兼容是连续过程——M1 起每个 PR 都更新三张覆盖率表（Ex 命令 / options / 内置函数）。

---

## M0 — Workspace 脚手架与工程基建（本次交付）

**目标**：建立可持续演进的骨架并跑通构建/测试闭环。

- [x] Cargo workspace（`bin/rvim` + `crates/rvim-*`），resolver=2，edition 2024
- [x] `rust-toolchain.toml`（1.85+）、`rustfmt.toml`（edition 2024）、workspace lint 约定：`unsafe_code` 全仓 deny（唯一平台例外见 ARCH §3）、`scripts/ci.ps1` 以 `clippy -D warnings` 作为警告门禁（测试代码同样零警告）
- [x] 零依赖验证：`cargo build --offline` 成功；`scripts/ci.ps1` 断言 `cargo tree` 只含 workspace 内 crate
- [x] 文档三件套：ARCH / PLAN / RUNTIME_COMPAT
- [x] 黄金测试 harness（`crates/rvim-app/tests/golden.rs`：`Session` 驱动脚本 → 断言缓冲/消息/寄存器/选项）
- [x] `rvim --version` / `rvim -es` 静默批处理入口骨架

**出口标准**：`cargo test --offline` 全绿；`rvim --version` 输出版本与基线 tag。

---

## M1 — Headless 编辑内核（本次交付核心）

**目标**：无 UI 也能完成「加载文件 → 编辑 → 保存」，所有行为可单测。

| 模块 | 内容 | 对齐 C 源 |
|---|---|---|
| rvim-core | UTF-8 字符工具、`Pos`、错误类型、字符类（宽字符/空白/单词） | charset.c/mbyte.c |
| rvim-buffer | `Buffer/Line`（Rc<str> CoW）、增删改、行定位、changedtick、fileformat | memline.c/buffer.c/change.c |
| rvim-undo | 线性撤销→撤销树、`u/C-R/g-/g+` 语义 | undo.c |
| rvim-editor | `Editor` 聚合、模式机、寄存器、标记、option 子集、消息日志 | buffer.c/register.c/mark.c/option.c |
| rvim-ex（子集） | `:q/w/e/r/set/noh/help(桩)` 等首批命令 + 范围解析 | ex_docmd.c/ex_cmds.c |
| rvim-normal（子集） | `h j k l w b e 0 $ ^ G gg x dd yy p u C-r i/a/A/o/O` 等 | normal.c/move.c/ops.c |
| bin/rvim | `rvim -es -c "..." file` 批处理：执行 Ex 序列、按 Vim 退出码退出 | main.c |

**出口标准**：
1. 黄金用例 ≥ 20 个（编辑、撤销、寄存器、`:set`、范围命令、保存往返）。✅ **已达标：34 个**（`crates/rvim-app/tests/golden.rs`，按 options/编辑撤销/寄存器/范围/替换/持久化/缓冲/normal/错误九组组织）。
2. `rvim -es -c '%d' -c 'put ="hello"' -c 'wq' out.txt` 类端到端脚本工作。✅ `scripts/smoke.ps1` 全绿（含磁盘往返与退出码；23 个场景，含 TUI 回放）。
3. 行/列边界不变量测试全绿；changedtick/撤销性质测试通过。✅ 撤销分支截断、空行不变量、cursor clamp 均有覆盖。

> 复核轮 2（M0–M3 深度对拍复核）修正的 M1 语义：字节列（多字节行编辑）、新建文件（ENOENT）、`'readonly'` E45、标记随编辑调整、`b:changedtick`/`&modified`、`'fileformat'` 同步与 Windows 默认 `dos`、UTF-8 BOM、空缓冲 0 字节写盘、寄存器 unnamed/大写追加语义、位置参数进 arglist、`:e` 的 E37、`:cq` 退出码、选项校验、Windows 单绝对路径 chdir、Ex 模式起始光标。逐条见 `docs/compat_deviations.md` §7.2。
>
> 复核轮（review）修正的 M1 语义：错误计数收敛到 `Editor::emsg`（`:source` 内错误此前漏计导致退出码 0）、`:silent!` 不计错误、`:!` 失败走 `smsg` 而非 `emsg`、`--cmd` 先于 vimrc 执行、`+`/`+N`/`-S` 顺序、无命令地址（`:3` 跳行、Ex 模式下 `:1,3` 打印）、前导 `0` 动作与 Normal 模式光标钳制。详见 `docs/compat_deviations.md` §1.1。

**M1 当前兼容面（覆盖率表分母固定见 ARCH §8）**：Ex 命令 165/589（含 q/w/e/r/d/y/co/m/p/s/set/pu/buffer 族）、options 56/429、normal 引擎覆盖 h j k l 0 $ ^ G gg w b e x dd yy p u m 与 i/a/A/o/O/I 插入入口。退出码语义对齐 vim `getout()`：Ex 模式任何 emsg 置非零退出（参考 main.c:1571-1575、message.c:729）。

---

## M2 — Ex 命令体系与命令行（✅ 已完成）

- ✅ **完整 589 条命令声明表落库**：`crates/rvim-ex/src/cmd_table.rs` 由 `reference/vim/src/ex_cmds.h` 生成（`cargo run -p rvim-tools -- gen-cmds`），保留 vim 的*最短可用缩写*排序；查找实现对齐 `find_ex_command()`（小写整表扫描、大写从 `Next` 起、非字母从 `!` 起、首个前缀命中）。覆盖率表 `docs/COVERAGE.md` 由 `cargo run -p rvim-tools -- coverage` 自动生成（`--check` 校验是否过期）。
- ✅ **解析器扩展**：范围（`. $ % N +N -N`、`,`/`;`、`/pat/` `?pat?` `'a` 及其组合的延迟解析）、TRLBAR 感知的 `|` 切分（`:!echo a|b` 的 bar 属于参数）、注释（`EX_NOTRLCOM` 遵守）、`++opt=val` 与 `+cmd`（`EX_ARGOPT`/`EX_CMDARG`）、`:dl`/`:dp`/`:d` 与 `:fina`→`:finally` 两个 vim 怪癖、自研 glob（`* ? [a-z] [^..] \`，`crates/rvim-ex/src/wildcard.rs`）。
- ✅ **命令族**（165 条落地，复核轮新增 `:k`/`:mark`；其余见 `docs/COVERAGE.md`）：缓冲区/参数列表（`args argadd argdelete badd bfirst blast bmodified bnext bprevious bunload bwipeout buffer buffers ls files`）、编辑（`s copy move retab sort global vglobal join left right center > < # & ~`）、写盘/退出（`write wq xit wall update saveas wqall quitall exit`）、脚本生态（`autocmd augroup doautocmd command delcommand comclear source runtime finish`）。
- ✅ **`rvim-cmdline`**：`: /` 历史（按 kind 独立限额、前缀召回）、命令名补全（含用户命令）、参数补全分类、文件名补全、`set` 选项补全、wildmenu 循环、路径展开。
- ✅ **map/abbrev**：`:map/nmap/vmap/xmap/smap/omap/imap/lmap/cmap` 全族 + `noremap` 变体 + `unmap/mapclear` + `map!` 记为 insert/cmdline 变体；`:abbreviate` 全族；`<buffer>/<nowait>/<silent>/<special>/<script>/<expr>/<unique>` 存储；递归/非递归展开引擎（深度上限 E223，`<expr>` 明确报 E15，评估器接线待后续里程碑）。
- ✅ **`:source` 跑通简单 vimrc**：`-u {vimrc}` / `-S` / `:source` / `:runtime`，`\` 续行、`finish` 只结束当前脚本、UTF-8 BOM 容错、嵌套深度上限；`scripts/smoke.ps1` 覆盖 vimrc/`:g`/`:source`/`:command` 场景。

**M2 出口标准（已达成）**：
1. **频率加权实现率 ≥ 60%**：`docs/COVERAGE.md` 口径为*对数桶加权*（`1+floor(log2 n)`，语料 `defaults.vim` + `plugin/*`，排除 M3 VimL 解释器类命令）→ **99/122 = 81.1%**；同时给出原始调用次数加权（98.0%）、全 runtime 语料加权（80.0%）、广度 35/50（70.0%）与表口径 163/589（27.7%），口径与排除清单在 `rvim-tools/src/freq.rs` 可复核。
2. **`:source` 能跑通无表达式 vimrc**：✅ 见上，smoke #7/#9 与 `m2_features.rs` 的 source/BOM/续行/`:finish` 用例。

> 复核轮（review）修正的 M2 语义：处理函数改看**表内规范名**（`:v` 取反、`:le` 左对齐、`:ab`/`:nm`/`:bf` 此前直接失败）、表达式命令在 `|` 处结束（`EX_EXPR_ARG`，`:echo 'a' | echo 'b'`）、无命令地址（`:3` 跳行/`:1,3` Ex 模式打印/`:3|cmd`）、用户命令 `<q-args>` 加引号与 `<f-args>` 切分。回归用例集中在 `crates/rvim-app/tests/m1_m2_review.rs`。

> 复核轮 2 修正的 M2 语义：`:d`/`:y` 计数、`:retab` 语义、反向范围 E493、`:g`/`:v` 升序与行移动跟踪、空模式复用上次搜索、`:s` 的 `e`/计数/`&`、`:autocmd!` 分组作用域与尾部模式匹配、`:runtime` 顺序、用户命令 `-bar/-range=%/-count`、`:put!`/`:0put`、`:2j`、`:sort!` 与标志、`:g` 无匹配仅 smsg、`:left N`、行首相对地址、空第二地址、`:m+1`/`:t+1`、`:source` 不回退 rtp、新增 `:k`/`:mark`。逐条见 `docs/compat_deviations.md` §7.4。
>
**M2 明确留给后续里程碑**：窗口族（`sp/vs/wincmd` → M5）、quickfix（`make/cgrep/copen` → M5）、会话/视图（`mksession/mkview` → M5）、`:redir`/表达式相关命令（→ M3/M4）、`:normal`（✅ 补齐复核已实现）、正则（`s`/`g` 仍为字面量匹配 → M6 `rvim-regex`）、autocmd 事件实际触发（声明已可用，事件派发随 M3/M4/M5 接入）。

---

## M3 — VimL（legacy script）解释器（✅ 已完成）

**已完成（`rvim-viml` + `rvim-ex::script`）**：
1. ✅ **词法+表达式**：字符串（单/双引号与转义、`\<Esc>` 键名）、数字（10/16/2/8 进制）、浮点、列表、字典（保序）、Blob、lambda（`{x -> ...}`）、`|| && == != > >= < <= =~ !~ is isnot #/?` 大小写后缀、算术/拼接/三元/索引/切片/属性/方法调用 `->`；作用域 `g: b: w: t: v: s: l: a:` 完整解析。
2. ✅ **语句+函数**：`:let`（含 `+= -= *= /= %= .=`、列表/字典/切片赋值、解构）、`:unlet`、`:if/elseif/else/endif`、`:while/endwhile`、`:for/endfor`（range 迭代）、`:try/catch/finally/endtry`、`:throw`、`:return`、`:break`/`:continue`、`:function[!]`/`:endfunction`（参数、`...` 变参、`dict` 属性、脚本作用域）、`:delfunction`、`:execute`、`:eval`、`:call`、`:echo/echomsg/echon/echoerr`（表达式求值）。
3. ✅ **脚本执行**：`do_cmdline` 式语句树解析（控制流为节点，普通命令仍是 Ex 行）；`\` 续行；`:finish` 只结束当前脚本；错误默认不中断脚本（try 内为严格模式）；脚本作用域 `s:` 按脚本隔离、`<SID>`；`:source`/`:runtime`/`:runtime!`/`-u vimrc` 全部走同一执行器。
4. ✅ **函数生态**：用户函数调用栈/`a:1..a:N/a:0/a:000`/`l:`、Funcref 与 lambda（闭包捕获）、`function()`/`call()`/`map()`/`filter()`/`sort()` 回调、autoload（`foo#bar#baz()` → `autoload/foo/bar.vim` 按需 source）、`:delfunction`。
5. ✅ **内置函数 389/543（71.6%）**：纯函数分 7 个模块（core + a-e/f-l/m-r/s1/s2 + assert/misc），宿主相关分 4 组（editor 核心、单窗口/标签/折叠、弹窗/属性/标记/历史、arglist/字符位置/补全/高亮组）；`assert_*`/`test_*` 全族按 vim 测试助手语义实现。
6. ✅ **迷你正则引擎**（`regex_mvp`）：字面量 `.` `*` `\+` `\?` `\=` `\{n,m}` `^ $` 字符类（含 `\d\w\s`）`\(...\)` `\|`、`\c/\C`；用于 `=~`/`!~`、`:g`、搜索地址与 `match()` 系列（完整 NFA 仍留 M6）。
7. ✅ **真实脚本验证**：`:source reference/vim/runtime/defaults.vim` 与 `filetype.vim`（1871 行、811 条 autocmd、2 个函数）**零错误执行**，成为 M3 回归测试。

**出口标准进度**：
1. `:source runtime/defaults.vim` 与 `filetype.vim`：✅ 全部零错误 source（`m3_features.rs` 断言无 E 错误、选项/映射/autocmd/函数/`did_load_filetypes` 生效）。
2. 内置函数覆盖率 ≥70%：✅ **389/543（71.6%）**（`crates/rvim-tools/tests/builtin_coverage.rs` 门禁测试确保不再回退），见 `docs/COVERAGE.md`。
3. **脚本兼容通过率统计**：官方 `testdir` 驱动依赖 M4 的输入层（`:normal`/按键循环）暂未接入，改用**当前可运行的口径**：`reference/vim/runtime/autoload/**/*.vim` 共 52 个脚本，**33 个零错误 source（63.5%）**（`cargo test -p rvim-app --test m3_dispatch` 只覆盖其中样本，完整统计用 `scripts/smoke.ps1` 的手工口径复核）。未通过的 19 个分成三类，均已登记：
   - **Vim9 script（3 个）**：`autoload/dist/ft.vim`、`autoload/dist/script.vim`、`autoload/ccomplete.vim` → 显式 `ERV338`；
   - **嵌入式语言 heredoc**（`python << EOF`、`perl` 等，如 `pythoncomplete.vim`、`netrw.vim`）→ 解释器不支持宿主语言，正文被当作 Ex 行解析；
   - **Vim5 时代遗留**（`ada.vim` 等使用裸 `version` 变量）。

### M3 收尾（autocmd 派发与脚本生态接通）

| 能力 | 说明 | 回归测试 |
|---|---|---|
| **autocmd 事件派发** | `rvim-ex::autodispatch` 在打开/写入文件、切换缓冲、`:setf` 等位置触发事件；`++once` 用过即删、`++nested`/忙时抑制按 `autocmd.c:1948` 规则实现，`:doautocmd` 走同一派发器 | `m3_dispatch.rs`（12 例）、`autodispatch::tests` |
| **FileType 以 force 触发** | `'filetype'` 变化时 `FileType` 绕过"忙时抑制"（`optionstr.c:2551`），因此 `au BufRead *.py setf python` → `FileType python` 链路成立 | `setfiletype_fires_the_filetype_event` |
| **filetype 检测真正生效** | `:filetype on` 首次开启时 source 运行时 `filetype.vim`（`filetype_script_loaded` 去重）；`.py`/`.vim`/`.sh` 等 `setf` 规则可直接检测 | `filetype_vim_detects_a_python_file` |
| **`\\|\\|` 不当作命令分隔符** | `:echo a \|\| b`、`:if (a \|\| b)`、`:let x = a \|\| b` 保持完整（vim 先解析表达式再找 `\|`） | `logical_or_is_not_a_command_separator` |
| **`.` 的连接/成员二义性** | 依被求值左侧类型决定：字典才走成员访问，否则连接；`'a' . strlen('b')` 此前误判成变量而 E121（`eval.c:6420`） | `dot_concatenation_with_a_function_call`、`dict_member_access_still_works` |
| **Vim9 脚本显式拒绝** | 旧版解释器遇到 `vim9script` 直接 `ERV338`，避免逐行 `var`/`def` 报错刷屏 | `vim9_scripts_are_rejected_with_one_clear_error` |

> **复核轮 2（M3）**：整数回绕与 legacy 除零、`%g` 浮点格式化、字符串真值、`is/isnot`、字符串索引/切片越界、类型敏感容器比较、`vim_str2nr` 语义、`map()`/`filter()` 原地修改、`a:1..a:N` 仅变参、作用域字典 `g:`/`v:` 等、异常跨函数与 `abort`、`:finally` 中的 `:return`、双引号转义、`:execute` 多表达式、`0o17`/`..`、`len()`、深递归 E132、`printf`/`split`/`get`/`count`/`sort`/`call`/`expand`/`getline`/`fnamemodify` 等内置函数修正。逐条见 `docs/compat_deviations.md` §7.3。
>
**已知偏差（登记 docs/compat_deviations.md）**：正则仅 MVP 子集；`:echo` 多表达式按空白回退拆分；`v:` 只读变量集合不完整；dict-function 的 `self` 仅绑定首参；`:redir`/`:normal`/profiler 未实现；错误信息文本未逐字对齐；Vim9 script 与嵌入式语言 heredoc 不支持；`.c` 等依赖 `dist#ft#*` 的检测规则因该 autoload 是 Vim9 而失效。

---
## M4 — 终端 TUI 前端（✅ 已完成）

- ✅ **`rvim-termio`**：终端后端。Windows 用 `SetConsoleMode`（VT 输入/输出）+ `GetConsoleScreenBufferInfo`（尺寸），POSIX 走 `stty`；无终端时返回明确错误让 `rvim` 回落批量口径。**全仓唯一 `unsafe` 例外**（`#[allow(unsafe_code)]`，集中在平台模块，见 ARCH §3）。
- ✅ **`rvim-term`**：屏幕层。`Grid`（Cell/Style/ANSI 全量与差量绘制）、`render()` 布局（缓冲视口 + 状态行 + 命令行/消息行 + `~` 补白 + `'number'` 行号栏）、`key::decode_keys`（CSI/SS3/modifyOtherKeys/UTF-8/控制字符，跨读半序列不吞字节）。
- ✅ **模式循环**：`rvim-app::tui::Tui` 消费 `Key`、产出 `Grid`：映射展开（最长匹配、`noremap` 不递归、`<Nop>`）→ normal/insert/cmdline 分发；两键命令 `dd`/`yy`/`gg`；插入模式回车分行、退格并上次行、`<Esc>` 回 normal 并左移一格。
- ✅ **命令行**：行编辑、`<Tab>`/`<C-N>`/`<C-P>` 补全与 wildmenu、`/` 搜索（`ignorecase` + 环绕 + `E486`）、`:q`/`:q!`/`:cq` 退出码与批量口径一致。
- ✅ **可验证的 TUI**：`rvim --script <keys>` / `--keys <notation> --cols --rows` 回放按键并打印最终屏幕，冒烟与测试都走这条通道（本环境无交互终端，故不依赖人工肉眼）。
- ✅ **插入命令的计数与撤销分组**：`3ihello<Esc>` 按 vim 语义重复输入文本（`i/a/I/A` 就地重复、`o/O` 多开行）；整个插入会话合并为一个 undo 步（快照 + 最小差分），`<C-r>` 重做已接线。
- ✅ **键位记法（:help notation）**：`rvim_editor::keynames` 镜像 vim `key_names_table[]` 全量名字（别名、`<Char-N>`、`<t_xx>`、鼠标键名、`<F37>`、`<k0>`-`<k9>`、`<xHome>` 等），修饰键任意顺序规范化，`:map` 按 vim 大小写输出；映射歧义在 timeoutlen 后回落为字面键。
- ✅ **窗口 resize 跟随**：输入循环以 120ms 超时轮询控制台尺寸（std 无 select，用读线程 + channel），变化时清屏并按新几何重绘；`3dd`/`2yy`/`3gg` 等计数与前缀命令、`gg` 回首行均已修复并有回归用例。
- ✅ **alternate screen**（`?1049h/l`）、256 色/真彩的 SGR 编码已就绪（高亮组在 M6 与 syntax 一起接）。
- ✅ **`'showcmd'`**：命令行行右端 10 列显示未完成命令（计数 / 操作符 / `"a` 寄存器 / `<C-w>` 显示为 `^W` / 视觉选区大小），默认 on（nocompatible 语义，见 `m4_tui.rs` 的 5 条用例）。
- ⚠️ 未做：鼠标/软换行/`<expr>` 映射/`:!` 的 `Press ENTER`（登记在 compat_deviations §6）；视觉模式、`relativenumber`、命令行历史键、插入模式控制键与 `'autoindent'` 已在 M5 补齐复核中实现。

> **复核轮 3（M4）**：以 `--keys` / `Tui` 逐帧对照 `reference/vim/src` 修复一批 TUI 行为——未知 `CSI … ~` 码不再卡死输入；Tab 与控制字符按 `'tabstop'` 展开为显示单元（消息行同规则）；`:echo` 瞬时消息与 `-- More --` 分页接回消息行；`/` 从光标后开始并支持 `?`/`n`/`N`（空模式复用上次模式，无则 `E35`）；操作符 + 动作（`dw`/`dj`/`d$`/`yw`/`cw`/`cc`/`dgg`/`dG`、`D`/`C`/`Y`、`d2w`/`d3l`）与 `<Esc>` 取消语义；状态行 `All`/`Top`/`Bot` 百分比；扣除行号栏的横向滚动与 `<C-f>`/`<C-b>`/`<PageDown>`/`<PageUp>` 翻页。逐条见 `docs/compat_deviations.md` §8，回归用例在 `m4_tui.rs`（46 例）与 `rvim-term` 单测。
>
> **M4 遗留（登记在 compat_deviations §6 已知偏差）**：鼠标/软换行、`<expr>` 映射、`<t_xx>` 编码、`:!` 的 Press ENTER、长消息折行、语法高亮（M6）。
> **M5 补齐复核已收口**：视觉模式（M5）、`'relativenumber'`/`'scrolloff'`/`'sidescrolloff'`、命令行历史键（`<Up>`/`<Down>`/`<C-p>`/`<C-n>`、`<C-u>`/`<C-w>`）、插入模式 `<C-w>`/`<C-u>`/`<C-r>`/`<C-a>`/`<C-t>`/`<C-d>`（按 redo 键序重放）、`'autoindent'`、`<C-a>`/`<C-x>`、`:normal`。

**出口标准进度**：交互打开文件、编辑、`/` 搜索、`:!` 外部命令均可用（单窗口，分屏留 M5）；**三端（kitty / Windows Terminal / conhost）肉眼验证需要真实终端，本环境无法执行**，以 `--script` 逐帧断言替代并登记待人工收口。

---

## M5 — 窗口、标签页与高级编辑（✅ 已完成）

- ✅ **多窗口/tabpage**（`rvim-editor::window`）：`TabPage` + `Layout` frame 树（`Row` 并排 = `:vsplit`、`Col` 堆叠 = `:split`）、`Window` 携带光标/视口/窗口局部选项/矩形。命令：`:split :vsplit :sview :new :vnew :close :only :hide :resize :vertical :wincmd :tabnew :tabedit :tabclose :tabonly :tabnext :tabprevious :tabfirst :tablast :tabmove :tabs :windo :tabdo :bufdo :argdo`；键位 `<C-w>` 后接 `h j k l w W t b p c q o n s v x r H J K L = + - _ |`。
- ✅ **窗口局部选项与 `win_execute()`**：`Options::WINDOW_LOCAL` 表 + `Window.local`；`:set` 写全局与当前窗口、`:setlocal` 只写窗口、`&opt` 读窗口值；`w:` 变量按窗口 id 分开存放。
- ✅ **折叠**（`rvim-editor::fold`）：manual/indent/marker/diff 四种 `'foldmethod'`（`syntax/expr` 显式报告不支持），`zf zF zo zc za zR zM zd zE zj zk zO zC zv`、`:fold :foldopen :foldclose :folddoopen :folddoclosed`、`foldclosed() foldclosedend() foldlevel() foldtext() foldtextresult()`；TUI 把闭合折叠画成 `+-- N lines: text`，`'foldcolumn'` 画层级。
- ✅ **diff 模式**（`rvim-diff` + `rvim-editor::diff`）：Myers 最短编辑脚本（对齐 `src/xdiff` 的变更块、滑动归并与 unified 输出），`:diffthis :diffoff :diffsplit :diffupdate :diffget :diffput`、filler 行对齐、变更行标记、`'foldmethod=diff'`、`]c`/`[c` 导航；`rvim -d a b` 即 `vimdiff` 工作流。
- ✅ **quickfix/location list**（`rvim-ex::errorformat` + `rvim-editor::quickfix`）：`'errorformat'` 编译器（`%f %l %c %v %m %t %n %r %p %o %s`、`%*` skip、`%#` 量词、`%-G` 忽略、E372/E373/E374），`:copen :cclose :cnext :cprevious :cfirst :clast :cc :cwindow :cexpr :caddexpr :cgetexpr :cfile :cgetfile :clist :colder :cnewer` 与 `l*` 全族，`setqflist()/getqflist()/setloclist()/getloclist()`，quickfix 窗口。
- ✅ **视觉模式与高级编辑**（`rvim-normal::visual`）：`v/V/<C-v>`、`o/O` 换端、`gv`、`d y c x s p P > < ~ u U J`、块选择的行列操作与块 `I/A`（`I` 跳过短行、`A` 补齐到块右边界，整段一个 undo 步）、视觉模式计数（`3I`/`v3j`）、`:'<,'>` 命令行前缀；**文本对象全集**（`rvim-normal::textobjects`，覆盖 `iw aw iW aW is as ip ap it at`、各类括号与引号对象）同时用于 `d/y/c` 与视觉模式；**`.` 重复**（`RepeatChange`：插入会话、操作符+动作、`x D C`、`p P`，带计数）。
- ✅ **数字增减**（`rvim-normal::addsub`）：`<C-a>`/`<C-x>` 按 `ops.c do_addsub()` 实现，支持十进制/十六进制/二进制/八进制（`'nrformats'`）、负号、前导零宽度、大小写、计数、`.` 重复与单步 undo。
- ✅ **标记/跳转/change list 与 viminfo**：`:jumps :changes`、`<C-o>`/`<C-i>`、`g;`/`g,`、`:wviminfo/:rviminfo`、`'viminfo'` 与 `-i`。
- ✅ **sign / textprop / popup API**：`sign_define/sign_place/sign_unplace/sign_jump/sign_getplaced/sign_placelist`、`prop_type_add/prop_add/prop_list/prop_remove/prop_clear`、`popup_create/popup_close/popup_list/popup_getpos/popup_move`（数据面完整，TUI 渲染 sign 列与 popup 覆盖层）。

> **复核轮 4（M5）**：窗口几何与 `<C-w>` 邻居判定按 `window.c` 重写（垂直/水平重叠带允许相切）、`:vertical` 真正包裹后续命令、`windo/tabdo/bufdo/argdo` 在迭代中容错、tabline 仅在多标签时出现、每个窗口独立状态行（非活动窗口用 StatusLineNC）、折叠/视觉选区/块操作逐条对拍 `normal.c`/`fold.c`。回归用例见 `crates/rvim-app/tests/m5_windows.rs`、`m5_editing.rs`、`m5_folds.rs`、`m5_diff.rs`、`m5_quickfix.rs`。

**出口标准进度**：
1. **`vimdiff` 等价工作流**：✅ `rvim -d a b` 打开两个窗口并全部 `:diffthis`，filler 对齐、`]c`/`[c`、`:diffget`/`:diffput`、`zo`/`zR` 展开 diff 折叠可用（`m5_diff.rs`）。
2. **窗口/折叠类用例通过率**：⚠️ 官方 `src/testdir` 依赖 `term_start`/GUI 与完整 `:normal` 键循环，本环境无法运行；改用等价自研用例统计并公开口径（各 `m5_*.rs` 的 `#[test]` 数量与 `scripts/smoke.ps1` 场景）。
3. **量化口径（`cargo run -p rvim-tools -- coverage`）**：Ex 命令 **246/589（41.8%）**、options **80/429（18.6%）**、内置函数 **393/543（72.4%）**；M2 出口的用户语料对数桶加权仍为 **104/122（85.2%）**。TUI 特性用例 49 例（windows 21 / folds 9 / diff 6 / quickfix 7 / signs 6），crate 单测新增约 200 例（rvim-diff 18、fold 18、foldcmd 10、handlers_fold 9、errorformat 31、quickfix 17、handlers_qf 28、diff 16、handlers_diff 12、textobjects 21、viminfo 17、window 12、handlers_win 6、handlers_session 4），全仓 **1021** 个测试（补齐复核后）。
4. **已知未做**（登记 `docs/compat_deviations.md` \9）：自定义 `'foldtext'` 表达式、`foldmethod=expr/syntax`、`:diffpatch`、popup 的 filter/回调、textprop 的高亮渲染、viminfo 只读写 RustVim 建模的字段子集。

---

## M6 — 语法高亮、拼写、文件类型与加密

- 自研正则 NFA 引擎收口（`regexp_nfa.c` 语义），`:syntax` 引擎（keyword/match/region、contains、cluster、`syn-stack`）。
- `filetype.vim`/ftplugin/indent/syntax 全量 runtime 消费；`filetype detection` 对照表测试。
- spell：`.spl` 读取/拼写检查/建议（`spell.c/spellfile.c/spellsuggest.c`）。
- 标签栈（`tagstack`、`tags` 文件格式）、`cindent/smartindent`。
- 加密：bfish/blowfish2/zip 与 swapfile 格式（std-only 自研 SHA256/BF/XChaCha 等价实现，密文互读校验）。

**出口标准**：常见 20 种文件类型高亮与 vim 截图级对比；加密 swap/文件可与真实 vim 互读。

---

## M7 — Job / Channel / Terminal / 网络

- `job_start/term_start`：std::process + pty（Unix posix/openpty FFI；Windows ConPTY FFI）。
- channel：TCP（std::net）、JSON-RPC、socket/pipe（Windows named pipe FFI）、`ch_sendraw/ch_evalexpr`。
- `:terminal` 内置终端（移植 libvterm 屏幕解析为 Rust）、`term_wait/term_getline`。
- 定时器 `timer_start`、`:!cmd`、`system()`、clientserver（命名管道/socket）。
- 线程模型严格遵守 ARCH §5.7：子线程只投递字节事件，主线程单写 Editor。

**出口标准**：JSON-RPC 插件（如 LSP 客户端骨架）可跑；终端里可交互运行 `python`/`pwsh`。

---

## M8 — Vim9 script 与兼容收口

- `vim9script` 语境、类型系统（`vim9type.c`）、编译器（`vim9compile.c`）、字节码 VM（`vim9execute.c/vim9instr.c`）、`def/enddef`、`import`（`runtime/import/`）、class（若基线含）。
- legacy/vim9 互操作、`defcompile`、性能基准（对拍相同脚本的 vim 耗时）。
- 兼容收口：剩余 Ex/options/functions 清零或显式标记「不支持+理由」；`src/testdir` 通过率报告；`+feature` 列表（`has()`）对齐。
- 打包：Windows 单 exe、Unix tarball、安装器（NSIS 等价流程）、runtime 合并安装与 `$VIMRUNTIME` 解析。

**出口标准**：对外发布 `rvim 1.0`，声明兼容等级（见 RUNTIME_COMPAT §7）。

---

## 测试策略（贯穿所有里程碑）

### 四层金字塔
1. **单元测试**（crate 内 `#[cfg(test)]`）：纯函数、边界、不变量；目标核心 crate 行覆盖 ≥ 85%。
2. **黄金测试**（`crates/rvim-app/tests/golden.rs`）：脚本 → 期望的缓冲内容/消息/寄存器/选项快照，diff 即回归。
3. **特性测试**（`crates/rvim-app/tests/{m1_m2_review,m2_features,m3_features,m3_dispatch,m4_tui}.rs`）：按里程碑/`help` 主题组织，每条断言一个可观察行为。
4. **兼容对照**：同一 Ex/VimL 脚本分别喂给 `rvim -es` 与真实 `vim -es`，逐字节比对缓冲/状态文件与退出码（对拍脚手架在开发期临时目录中搭建，不随仓库分发；结论沉淀为上面的黄金/特性用例与 `docs/compat_deviations.md` 的登记行）。

### 开发环境
- `reference/vim`（Vim v8.2.5172 基线，src + runtime ≈ 57 MB）**不入库**：首次工作前跑 `pwsh -File scripts/fetch-reference.ps1`（浅克隆 + 校验 commit/tree hash，见 ARCH §8）；`scripts/ci.ps1` 会在缺失时自动拉取。
- 缺少基线时仓库仍可 `cargo build`/`cargo test`：依赖 runtime 脚本的 3 条用例与 2 条 smoke 场景打印 `skipped` 后通过，`rvim-tools coverage` 明确报错而不是输出空语料数字。
- 只想用别的 runtime：`$env:RVIM_RUNTIME='C:\path\to\vim\runtime'`（`default_runtimepath()` 优先读它）。

### 对拍工具
- 差分对拍：脚本分别喂 `rvim -es` 与 `vim -N -u NONE -i NONE -es`，用 `call writefile([...])` 捕获值（`vim -es` 抑制 `:echo`）、用 `:w!` 比较缓冲字节；真实 vim 不可用时以黄金/特性用例为基线。
- 覆盖率表自动生成：`cargo run -p rvim-tools -- coverage` 扫描静态表输出 Markdown（命令 589、选项 429、函数 350+ 分母固定，防止口径漂移）。
- 性质测试：文档模型随机操作序列（插入/删除/撤销）后，重做结果必须等于直接序列结果（undo/redo 一致性）。

### CI 矩阵
`cargo fmt --check` → `cargo clippy --all-targets` → `cargo test --offline` → 零依赖审计 → 黄金快照 →（可选）vim 对拍。Windows + Linux 双跑。

---

## 风险管理

| 风险 | 影响 | 缓解 |
|---|---|---|
| 无 crates.io，终端/正则/JSON 全自研 | 工期 | 严格分阶段；正则/JSON 先子集后全集；FFI 只碰 OS 原生 API |
| VimL 语义暗坑多（弱类型、作用域、异常） | M3 延期 | 以 testdir 用例驱动；先求语义正确再优化；建语义决策记录（`docs/adr/`） |
| 正则 NFA 复杂度 | M6 瓶颈 | BT 引擎先行覆盖 90% 场景，NFA 仅在性能/特性需要时补 |
| Windows ConPTY/控制台差异 | M4/M7 | VT 优先、API 降级；conhost 与 Windows Terminal 双验 |
| 兼容范围无止境 | 永远无法收口 | 固定 v8.2.5172 基线；覆盖率表驱动；不支持项显式登记而非静默偏离 |
| 人力有限 | 整体节奏 | 每里程碑独立可交付可用；headless 内核本身即长期有用（可作库/编辑器组件） |

## 版本与兼容策略
- 语义化版本：0.x 期里程碑即 minor；1.0 对应 M8 出口。
- `--version` 同时报告 RustVim 版本与锚定的 Vim tag/patch（`Included patches: 1-5172` 兼容展示）。
- 升级 Vim 基线需走 RFC：更新三张表分母、全量对拍、文档标注差异。
