# 兼容性偏差登记（compat_deviations）

> 基线：[`vim/vim@v8.2.5172`](https://github.com/vim/vim/tree/v8.2.5172)。
> 原则（见 [RUNTIME_COMPAT.md](RUNTIME_COMPAT.md) §1）：未实现必须**显式报错（带 E 号）**，
> 绝不静默给出错误结果；与 Vim 的每一处行为偏差在此登记，收口时逐条复核。

状态图例：✅ 已对齐 · ⚠️ 部分对齐（M 里程碑收口） · ❌ 未实现（显式报错）

## 1. 本轮（M1）已修复并对拍的兼容点

| 主题 | Vim 行为（参考源） | RustVim 现状 | 回归测试 |
|---|---|---|---|
| 插入入口后的模式 | `i/a/A/o/O/I` 进入 Insert，Esc 才回 Normal（`normal.c`） | ✅ `NormalEngine::feed` 对插入入口提前返回，不再被尾部无条件 `mode=Normal` 覆盖 | `mode_machine_via_normal_engine` |
| Ex 模式退出码 | `getout()` 中 `exitval += ex_exitval`；任何 `emsg()` 置 `ex_exitval=1`（`main.c:1571-1575`、`message.c:729`） | ✅ 会话级 `error_count>0` 时退出码 1；显式 `:cq N` 码优先；提取 `final_exit_code` 单测 | `final_exit_code::*`（4 个）、smoke #5/#6 |
| 命令词后数字参数 | `:1co3` `:2m3` 的数字是命令参数而非命令名一部分（`ex_docmd.c` 地址/参数切分） | ✅ 解析器在命令词已含字母后遇数字断词；前导数字仍按地址处理 | `numeric_argument_follows_command_word`、`copy_and_move_range_commands` |
| 黑洞寄存器 `_` | `"_` 丢弃文本，不写无名/编号/`0`/`-`（`register.c`） | ✅ `RegisterStore::{yank,delete}` 对 `_` 提前返回；Ex `:d _`/`:y _` 识别该名 | `named_register_isolation_and_black_hole` |
| `:w` 缩写 | `w` 是 `:write` 的合法缩写，虽与 wq/wall/wqall 共享词干 | ✅ 由 589 条命令表的*表序*保证：`write` 在 `wq`/`wall` 之前，首要前缀命中即 write（M2 起不再需要别名表） | `prefix_and_alias_lookup`（已并入 `abbreviation_resolution_matches_vim`）、`dos_fileformat_writes_crlf` |
| `'fileformat'` 与写盘 | buffer-local `&ff` 决定 `:write` 行尾（`bufwrite.c`） | ✅ `:set ff=dos/unix/mac` 经 `sync_buffer_fileformat` 回写当前缓冲，写盘按该格式输出 | `dos_fileformat_writes_crlf`、buffer fileio 单测 |

### 1.1 M1/M2 复核（review 轮修复）

| 主题 | Vim 行为（参考源） | RustVim 现状 | 回归测试 |
|---|---|---|---|
| 错误→退出码 | 每个 `emsg()` 都置 `ex_exitval=1`（`message.c:729`）；`getout()` 里 `exitval += ex_exitval`（`main.c:1575`） | ✅ 错误计数收敛到 `Editor::emsg`（`error_count`），`Session::sync_errors` 同步到报告；**source 内的错误不再漏计**（此前 `:source 坏文件` 退出码 0） | `m1_m2_review::script_errors_are_counted_even_when_no_command_fails`、`errors_are_counted_once_per_failure` |
| `:silent!` | `emsg_silent` 时 `emsg()` 提前返回，不置 `ex_exitval`（`message.c:700-729`） | ✅ `:silent!` 抑制消息且不计错误/不影响退出码；`:silent` 仍保留错误 | `m1_m2_review::silent_bang_swallows_the_error` |
| `:!` 失败 | 用 `smsg()` 输出 "shell returned N"，**不是** `emsg()`（`os_win32.c:5091`） | ✅ 走消息通道，不置错误计数 | —（探针验证：`!cmd /c exit 3` 退出码 0） |
| `--cmd` | 在读取 vimrc **之前**执行（`main.c:416,2463`） | ✅ 独立队列，先于 `-u vimrc` 执行（此前被静默丢弃） | `dash_cmd_is_separate_and_command_order_is_preserved`、smoke #13 |
| `+` / `+N` / `-c` / `-S` 顺序 | `+` 等价 `$`、`+N` 等价 `:N`；`-S` 以 `so file` 入同一命令队列按参数顺序执行（`main.c:1967-1975`、`2440-2458`） | ✅ 顺序保留；裸 `+` 跳末行（此前被错解为 `:0`） | `plus_arguments_map_to_last_line_or_ex_command`、smoke #14 |
| 无命令的地址 | `:3` 跳行、`:1,3` 仅在 Ex 模式打印、`:|`/`:3\\|cmd` 先打印再执行（`ex_docmd.c:1958-1971`、`2669-2711`）；越界行号夹到末行 | ✅ 同规则（`Editor::exmode` 标记 Ex 模式）；此前一律 `E492` | `m1_m2_review::{bare_line_number_jumps,line_range_prints_only_in_ex_mode,trailing_bar_prints_then_runs_next_command,line_number_past_end_is_clamped}` |
| 表达式后的 `\\|` | `:echo`/`:execute` 无 `EX_TRLBAR` 但带 `EX_EXPR_ARG`，表达式在 `\\|` 处结束（`ex_cmds.h:533`）；`:let`/`:eval`/`:call` 同理 | ✅ 解析层按 `EX_EXPR_ARG`/表达式命令切分（字符串内的 `\\|` 不切） | `m1_m2_review::{expression_commands_split_at_unquoted_bar,bar_inside_strings_is_not_split,let_allows_a_trailing_command}` |
| 命令别名与族语义 | 处理函数看到的是命令族的**规范名**，不是用户输入的缩写 | ✅ 分派前把 `p.command` 归一为表内规范名；此前 `:v`（vglobal）丢失取反、`:le`（left）走成居中、`:ab`/`:nm`/`:bf` 直接失败 | `m1_m2_review::abbreviated_commands_keep_their_family_semantics`、`vglobal_abbreviation_matches_full_name` |
| 用户命令 `<q-args>` / `<f-args>` | `<q-args>` 是整体加引号（内部引号翻倍）；`<f-args>` 按空白切分并逐个加引号（`usercmd` 展开） | ✅ 两者已实现；此前 `<q-args>` 等同 `<args>`（产生 E121） | `usercmd::tests::q_args_are_quoted_and_f_args_are_split` |
| `0` 与计数 | `0` 是"行首"动作；只有已开始输入计数时 `0` 才作为数字位（`normal.c`） | ✅ 前导 `0` 走动作，`10l` 仍按计数 10 | `golden::zero_key_is_column_motion_unless_a_count_is_running` |
| Normal 模式光标钳制 | 光标停在字符上，`l` 不越过行尾（无 `virtualedit` 时） | ✅ `l`/`h` 夹到最后一个字节（`$` 仍保留行尾位置供算子使用） | 同上 |


## 2. M2 已对齐（Ex 命令体系与命令行）

| 主题 | Vim 行为（参考源） | RustVim 现状 | 回归测试 |
|---|---|---|---|
| 589 条命令声明表 | `ex_cmds.h` `cmdnames[]`，按最短可用缩写排序 | ✅ 由 `ex_cmds.h` 生成 `cmd_table.rs`（589 条、表序保留），`cargo run -p rvim-tools -- gen-cmds` 可重建 | `table::tests::baseline_table_has_589_entries`、`table_is_in_curated_abbreviation_order` |
| 缩写解析 | `find_ex_command()`：小写整表扫描、大写从 `Next`、非字母从 `!`，首个前缀命中 | ✅ 同规则；`:w`→write、`:p`→print、`:r`→read、`:q`→quit、`:N`→Next 全部对齐（无需别名表） | `abbreviation_resolution_matches_vim` |
| `:dl`/`:dp`/`:del` | 删 `l`/`p` 尾字符并置 LIST/PRINT 标志 | ✅ 解析层同规则（`:del` → 命令词 `de`） | `delete_flag_quirk` |
| `:fina` | 旧脚本兼容映射到 `:finally` | ✅ | `finally_quirk` |
| `**` TRLBAR 语义 | 无 `EX_TRLBAR` 的命令把 `|` 当参数（`:!echo a|b`） | ✅ 解析按命令 flags 决定是否切分 | `bar_is_handled_by_trlbar_flag`、`autocmd_command_text_keeps_bars` |
| 搜索/标记地址 | `/pat/` `?pat?` `'a`、组合与偏移（`.,/end/+1`） | ✅ 延迟解析（`addr.rs`），含 `wrapscan`、`ignorecase`、`E486`/`E20`/`E16` | `addr::tests::*`、`search_address_delete`、`mark_address_delete` |
| `++opt` / `+cmd` | `:e ++ff=dos +5 file`、`:w ++bin` | ✅ 解析并作用于缓冲区/游标；非 UTF-8 `++enc` 显式 E474 | `fileopts_and_plus_cmd`、`write_with_ff_opt_uses_dos_line_endings`、`edit_with_plus_line_command_moves_cursor` |
| `:s` 标志与重复 | `g`/`i`/`n`/count 标志，`:&`/`:~` 复用上次替换 | ✅ 记录 `last_sub`，重复不匹配时报 E486 | `amp_repeats_last_substitute` |
| `:g`/`:v` | 按匹配行执行命令，修改类命令自底向上处理 | ✅ MODIFY 标志驱动方向（读命令保持正向输出顺序） | `global_*`、`vglobal_*` |
| `:sort` | `i n r u` 等标志、整缓冲默认范围 | ✅ 字面/数值排序、反向、去重 | `sort_*` |
| `:>`/`:<` | 每个额外 `<`/`>` 增加 amount；数字是**行数**（非列数） | ✅ 同语义（`EX_COUNT` 处理） | `shift_multiple_lines_with_count` |
| map/abbrev | 模式族、`noremap`、`<silent>/<buffer>/<expr>/<unique>`、最长 lhs 优先 | ✅ 存储 + 展开引擎（递归/非递归、E223、E31、E227）；`:map!` 为 insert+cmdline 变体 | `maps::tests::*`、`nmap_*`、`map_modes_*` |
| `:autocmd`/`:augroup` | 114 事件、`augroup` 作用域、`!` 删除、`++once/++nested` | ✅ 声明/列表/删除与事件名校验（E216）；`:doautocmd` 实际执行匹配命令 | `autocmd` 事件表测试、`m2_features.rs` autocmd 组 |
| `:command` 用户命令 | `-nargs/-bang/-bar/-range/-count/-complete/-expr`、`<args>`/`<q-args>`/`<f-args>` 等占位符 | ✅ 定义/重定义/删除/查找与占位符展开（q-args 加引号、f-args 切分加引号）；`-expr` 用户命令求值后执行（`<expr>` **映射**仍待 M4 键盘层） | `usercmd::tests::*`、`m2_features.rs` 用户命令组、`m3_dispatch.rs::expr_user_command_executes_the_evaluated_text` |
| `:source`/`:runtime`/vimrc | `\` 续行、`finish` 只结束当前脚本、`runtimepath` 搜索、`-u` 启动加载 | ✅ 全部实现；UTF-8 BOM 容错；嵌套深度上限 E169 | `source_*`、`runtime_sources_from_runtimepath`、smoke #7/#9 |
| 文件名通配 | `* ? [..] \` 与 `**` 递归 | ✅ 自研 matcher（`wildcard.rs`）+ `expand_glob`（`**` 递归） | `wildcard::tests::*`、`filename_completion_lists_directory` |
| 命令行模型 | 历史按 type 独立限额、前缀召回、补全分类、wildmenu | ✅ `rvim-cmdline`（history/complete/wildmenu，headless） | `rvim-cmdline` 单测 |

## 3. 已知偏差（后续里程碑收口）

| 主题 | Vim | RustVim | 收口 |
|---|---|---|---|
| 正则 | 完整 NFA/BT 正则引擎 | ⚠️ `:s`/`:g`/搜索地址仅字面量匹配 + `ignorecase`，无元字符 | M6（rvim-regex） |
| 撤销结构 | 撤销树（分支、`g-`/`g+`、undofile） | ⚠️ 线性 undo/redo 链；新编辑截断 redo 分支已对齐 | M5（rvim-undo 树化） |
| 窗口/标签 | 多窗口、tabpage | ❌ 单窗口；`split/vsplit/tab/wincmd` 调用走未实现报错 | M5 |
| VimL | 完整解释器、350+ 函数 | ✅ M3 已落地解释器并接通 autocmd 派发（见 §5）；剩余差距：`v:` 变量仅常用子集、Vim9/嵌入式 heredoc 不支持 | 随需求补 |
| 选项全集 | 429 项，含 window/buffer 作用域 | ⚠️ 56 项全局选项；`setlocal/setglobal` 暂与 `set` 同路径 | M5 |
| 交互前端 | 终端 TUI、键码映射、鼠标 | ⚠️ M4 已落地 TUI（正常/插入/命令行模式、映射展开、状态行、`~` 补白、横纵滚动、wildmenu、更多的分页）；**视觉模式、鼠标、软换行、`relativenumber`** 未实现 | M5 |
| quickfix/会话 | `make/cgrep/copen`、`mksession/mkview` | ❌ 声明表内，调用走未实现报错 | M5 |
| `:normal`/`:redir`/表达式命令 | 走键盘/重定向/VimL | ❌ 仍显式报错：`:normal` 需输入队列（M4）、`:redir` 需与命令行层对接（M4） | M4 |
| 命令全集 | 589 条 | ⚠️ 163 条落地（`docs/COVERAGE.md`），其余经 `ERV337` 显式报错 | M2 起持续 |

## 4. 不变量（已由测试强制，不视为偏差）

- 缓冲始终保留至少一行：删光全部行后保留一个空行（Vim 同样不产生零行文档）。
- `'compatible'` 永久关闭：尝试 `set compatible` 返回 `ERV010`，`set nocompatible` 为兼容 runtime 脚本的 no-op。
- changedtick 每次变更单调递增；所有变更经 `Editor` 单点落 undo，保证撤销/重做可重放。
- 覆盖率表数字只能由 `cargo run -p rvim-tools -- coverage` 生成（`--check` 在 CI 中校验是否过期）。
## 5. M3 已对齐（VimL 解释器）

| 主题 | Vim 行为（参考源） | RustVim 现状 | 回归测试 |
|---|---|---|---|
| 表达式语法 | eval.c `eval1()..eval9()`：优先级、三元、lambda、索引/切片/属性/`->` 方法 | ✅ 词法 + Pratt 解析 + 求值，优先级与 vim 一致 | `rvim-viml` 单测（expr/lexer/eval） |
| 类型与转换 | typval.h：Number/Float/String/List/Dict/Funcref/Blob/Bool/Null，弱类型转换 | ✅ 保序字典、引用语义容器、`str2nr`/float 格式化（13 位有效数字） | `value::tests::*` |
| 作用域 | `g: b: w: t: v: s: l: a:`，脚本作用域按 source 隔离，`<SID>` | ✅ 全部实现；`s:` 栈式隔离、函数帧、`a:1..a:N/a:0/a:000` | `env::tests::*`、`script_scope_is_isolated_per_script` |
| 语句控制流 | do_cmdline cstack：if/while/for/try、break/continue、finish、错误不中断脚本（除 abort） | ✅ 语句树解析 + 执行器；`:try` 内错误转为可捕获异常 | `m3_features.rs`（if/while/for/try/throw） |
| 函数 | userfunc.c：参数/变参、返回值、递归深度 E132、autoload、dict 函数 | ✅ 用户函数、lambda 闭包、Funcref 变量调用、autoload 按需 source | `functions_*`、`lambdas_*`、`autoload_function_loads_on_first_call` |
| `:execute`/`:eval`/`:echo` | 表达式求值后执行/显示；echo 支持多表达式 | ✅（echo 多表达式以「整体解析失败→空白拆分」回退） | `execute_builds_and_runs_commands` |
| 真实 runtime 脚本 | `defaults.vim`、`filetype.vim` 可 source | ✅ 两者零错误 source（811 autocmd / 2 函数） | `sources_defaults_vim_cleanly`、`sources_filetype_vim_cleanly` |
| **autocmd 事件派发** | `apply_autocmds()` 在打开/写入/切换缓冲等位置触发；`++once`、`++nested`、忙时抑制（`autocmd.c:1948,2166`）；`FileType` 以 force 触发（`optionstr.c:2551`） | ✅ `rvim-ex::autodispatch` 在 `:e`/`:w`/`:b*`/`:setf` 处派发；`:doautocmd` 复用同一派发器；此前只有声明、无派发 | `m3_dispatch.rs`（12 例）、`autodispatch::tests` |
| **filetype 检测** | `:filetype on` source `$VIMRUNTIME/filetype.vim`，`au BufRead *.py setf python` 类规则生效 | ⚠️ `setf` 直连规则生效（`.py`/`.vim`/`.sh` 等）；依赖 `dist#ft#*` 的规则（`.c` 等）因该 autoload 为 Vim9 而不可用 | `filetype_vim_detects_a_python_file` |
| `\\|\\|` 与命令分隔符 | 先解析表达式（消耗 `\\|\\|`）再找 `\\|`（`ex_docmd.c:check_nextcmd`） | ✅ `split_bars`/`find_bar` 跳过 `\\|\\|`；此前 `:echo 1 \\|\\| 0` 被切成两条命令 | `logical_or_is_not_a_command_separator` |
| `.` 的成员/连接判定 | 依被求值左侧类型：字典→成员访问，否则连接（`eval.c:6420`） | ✅ 求值期判定；`'a' . strlen('b')` 不再误判为变量（E121） | `dot_concatenation_with_a_function_call`、`dict_member_access_still_works` |
| Vim9 script | vim 8.2 可直接执行 | ❌ 显式 `ERV338`（旧版解释器仅支持 legacy VimL）；运行时中仅 3 个 autoload 文件属 Vim9 | `vim9_scripts_are_rejected_with_one_clear_error` |

### M3 已知偏差

| 主题 | Vim | RustVim | 收口 |
|---|---|---|---|
| 正则引擎 | 完整 Vim 正则（回溯/NFA、`\zs\ze\@!`、反向引用、多行、`\a\l\u` 等字符类简写） | ⚠️ 迷你引擎（`rvim_viml::regex_mvp`）由 `=~`/`!~`/`match*()`/`search()`/`:s`/`:g`/搜索地址共用；支持 `. * \+ \? \= \{n,m} ^ $`、字符类（含 `\d\w\s`）、`\(...\)` 捕获组（`matchlist()` 可取到）、`\|`、`\c/\C`；缺 `\zs \ze \@!`、反向引用 `\1`、多行、`\a\l\u` 简写 | M6（rvim-regex） |
| 内置函数覆盖 | 543 个（`evalfunc.c`） | ✅ 388 个已实现（71.5%），覆盖断言/测试助手、JSON、文件系统、窗口/标签/折叠、弹窗/属性/标记、补全与历史；通道/作业/终端/定时器/语法/拼写/客户端-服务器仍缺 | M7（channel/job/terminal）、M6（syntax/spell） |
| `v:` 变量 | 完整内置变量集合 | ⚠️ 常用子集（version/progname/errmsg/exception/count/key/val/lnum...） | 随需求补 |
| dict 函数 `self` | 完整 dict-function 绑定 | ⚠️ 仅绑定首个字典参数为 `self` | M4 |
| `:redir`/`:normal`/profiler | 重定向/键盘执行/性能剖析 | ❌ 未实现（`:redir` 依赖变量重定向、`:normal` 依赖输入队列） | M4 |
| testdir 对拍统计 | 官方 `test_eval*.vim` 通过率 | ❌ 未接入（需测试驱动与 `:normal`） | M4/M8 |
### M3 宿主层（编辑器相关内置函数）的显式近似

| 主题 | Vim | RustVim | 收口 |
|---|---|---|---|
| 窗口/标签几何 | 真实终端尺寸 | 单窗口模型：`winnr/tabpagenr/win_getid`=1；`winheight/winwidth/wincol/winline`=-1、`win_screenpos`=[1,1]（无 TUI 时不编造尺寸） | M4/M5 |
| 折叠 | 折叠引擎 | `foldlevel`=0、`foldclosed*`=-1、`foldtextresult`=""（当前无折叠引擎，数据模型为手动折叠预留） | M5 |
| 弹窗/文本属性/sign/匹配高亮 | 渲染 + 数据 | 数据模型完整（创建/查询/删除/优先级/options），无渲染；`popup_menu()` 返回 -1（需要 M4 输入循环） | M4/M5 |
| 命令历史 `hist*` | 单调递增 hisnum、`-1` 语义 | 位置编号 1..n，空时 0（脚本可观察到差异） | M4（命令行层接入后对齐） |
| `interrupt()` | 中断循环/脚本 | 返回 0，`while/for` 不响应（需 M4 输入层） | M4 |
| `test_*` | 分配器/鼠标/GC/输入缓冲钩子 | 记录到 `v:test_*` 或 no-op（Rust 无对应机制），语义正确的返回值为先 | 随子系统落地 |
| 通道/作业/终端/定时器/提示缓冲区/声音/客户端-服务器 | 全功能 | 未实现（E117），不伪造 | M7 |
## 6. M4 已对齐（终端 TUI 前端）

| 主题 | Vim 行为（参考源） | RustVim 现状 | 回归测试 |
|---|---|---|---|
| 屏幕模型 | 字符网格 + 差量重绘（`screen.c` `update_screen`/`screen_line`） | ✅ `rvim-term::Grid`（Cell/Style）、`to_ansi()` 全量重绘、`diff_ansi()` 只重绘变化行 | `grid::tests::*` |
| 布局 | 缓冲区视口 + 状态行 + 命令行/消息行（底部两行） | ✅ `render()` 同布局；EOF 后 `~` 补白；错误消息红色（`E123` 前缀判定） | `render::tests::*`、`m4_tui.rs` |
| 制表符与控制字符 | <Tab> 按 `'tabstop'` 展开成空格；不可打印字符显示为 `^X`（`screen.c`/`transchar()`） | ✅ `render::expand_cells()`：Tab 补到下一个 `'tabstop'` 边界、`^A`/`^@`/`^?` 成对显示；缓冲区、消息行同一套展开，光标列按显示单元计算 | `tabs_expand_to_the_next_tabstop`、`control_characters_are_shown_as_caret_pairs`、`tabbed_lines_render_as_spaces`、`tabs_in_the_buffer_are_expanded_to_spaces` |
| 状态行 | `文件名 [+柱]  行,列  百分比  模式`，右侧对齐；百分比走 `get_rel_pos()`（`All`/`Top`/`Bot`/n%） | ✅ 位置/百分比/模式右对齐（窄窗口自动裁剪文件名）；`All`/`Top`/`Bot` 与 `buffer.c:get_rel_pos()` 一致 | `initial_screen_shows_buffer_statusline_and_end_of_buffer`、`ruler_shows_all_top_and_bot_like_vim` |
| 键码解码 | `term.c`/`os_win32.c`：CSI/SS3/`modifyOtherKeys`、UTF-8、控制字符；未知码仍要消费 | ✅ `key::decode_keys`：方向键/Home/End/Ins/Del/PgUp/PgDn/F1-F12/`CSI 1;5A` 修饰键、跨读不完整序列；未知 `CSI … ~` 码（F13+/bracketed paste）消费掉并回退为 `CSI-n~`，不再永久等待 | `key::tests::*`、`unknown_key_codes_are_consumed`、`byte_source_decodes_across_reads` |
| 键位记法（intro.txt notation） | key_names_table[]（misc2.c）全量 <> 名字：CR/Enter/Return/NL/NewLine/LF、Tab/Esc/BS/BackSpace/Nul、Del/Delete/Ins、Up 族、xHome/kHome/zHome、k0-k9/kPlus、F1-F37、xF1-xF4、PasteStart/PasteEnd、Char-N（十进制/0x/0 八进制）、t_xx、修饰键 C-/S-/M-(A-)/D- 任意顺序、鼠标键名 | ✅ rvim_editor::keynames 镜像该表（别名、修饰键规范化到 C-S-M-D、<S-a>→A、<Nul>→<C-@>、未知 <foo> 保持字面量、to_notation 按 vim 大小写输出 <Esc>/<Up>/<k0>/<xHome>/<LeftMouse>）；映射解析、:map 列表、终端编解码共用同一张表 | keynames::tests::*（11 例）、m4_tui.rs::special_keys_work_in_mappings、mapping_listing_uses_canonical_notation |
| 映射歧义超时 | 前缀有歧义时等 timeoutlen（默认 1000ms），超时后把已输入键当字面量（timeout 关闭则不超时） | ✅ 输入循环 120ms 轮询 + flush_pending_timeout()；新增 timeout/timeoutlen 选项 | `ambiguous_mapping_prefix_flushes_after_timeoutlen` |
| 窗口 resize | 收到 SIGWINCH/窗口事件立即按新尺寸重绘 | ✅ 输入循环带 120ms 超时（读线程 + channel），每次超时轮询控制台尺寸；变化时清屏并按新几何全量重绘 | `resizing_the_terminal_changes_the_screen_geometry` |
| 计数 + 插入（`3ihello<Esc>`） | `edit.c: ins_esc()` → `stuff_inserted()`：插入命令前给的计数会把输入文本再重复 count-1 次 | ✅ `NormalEngine::repeat_insert_text`：`i/a/I/A` 在光标处再插入、`o/O` 再开 count-1 行；`<Esc>` 后光标停在最后插入字符上 | `counted_insert_repeats_the_typed_text`、`counted_insert_repeats_text_in_the_tui`、`counted_open_inserts_multiple_lines` |
| 插入会话的撤销块 | 整个插入会话（含计数重复）记录为**一个** undo 步（`undo.c` 的 u_save 分组） | ✅ `Editor::begin_undo_block`/`end_undo_block` 在插入入口与 `<Esc>` 处包裹，用「快照 + 最小差分」合成单条 `RangeEdit`；新增 `<C-r>` 重做绑定 | `one_undo_step_per_insert_session`、`undo_after_insert_session_and_redo` |
| 计数与前缀命令 | `3dd`/`2yy`/`3gg`/`2x` 按计数重复；`gg` 回首行 | ✅ 计数经 `Mode::MotionPending` 传入 `feed_pair`；`g` 前缀在 TUI 侧正确识别为两键命令（review 修复） | `counts_repeat_operators_and_gg`、`gg_jumps_back_to_the_first_line`、`operator_counts_apply_to_two_key_commands` |
| 操作符 + 动作 | `dw`/`dj`/`d$`/`yw`/`cw`/`cc`/`dgg`/`dG` 等（`normal.c do_pending_operator()`），计数可写在操作符之后（`d2w`），`cw` 在词上等价 `ce`，`D`/`C`/`Y` 为 `d$`/`c$`/`yy` | ✅ `operators::motion_range()` 把动作解析成字符区间或整行区间（`j/k/G/gg` 行式），`delete_range`/`yank_range`/`change_range` 落到寄存器 + 光标；`NormalEngine::feed_operator_key()` 接受操作符后的计数与 `gg` 第二键；`<Esc>` 取消不移动光标 | `operator_motion_commands_work`、`operator_counts_and_backward_motions`、`yank_and_change_motions`、`operator_gg_and_upper_g`、`escape_cancels_a_pending_operator_without_moving_the_cursor`、`escape_drops_a_pending_count` |
| 搜索 | `/` 从光标后一个字符开始、`?` 反向、`n`/`N` 重复（`n` 保持方向）、环绕、空模式复用上次模式（无则 `E35`） | ✅ `Tui::search()`/`repeat_search()`；环绕时最后一轮从起始行行首重扫（与 vim 一致，唯一匹配会停在同一处）；字面量子串匹配（正则 M6） | `search_starts_after_the_cursor_on_the_current_line`、`n_and_shift_n_repeat_the_search_in_both_directions`、`backward_search_and_empty_pattern`、`forward_search_moves_the_cursor` |
| 消息行 | `:echo` 等瞬时消息显示在消息行；输出放不下时 `-- More --` 分页，末页后“Press ENTER or type command to continue” | ✅ `collect_messages()` 同时取历史与 `MessageLog::last_status`（`E492` 只显示一次）；任一键翻到下一页并吞掉该键，末页后的键清屏后照常执行 | `echo_output_reaches_the_message_line`、`queued_messages_page_with_more`、`an_error_is_reported_once`、`more_prompt_is_appended_to_the_message` |
| 终端后端 | Win32 `SetConsoleMode`（VT in/out）+ `GetConsoleScreenBufferInfo` | ✅ Windows FFI（**全仓唯一 `unsafe` 例外**，限定在 `rvim-termio`）；POSIX 用 `stty`；无终端时明确报错 | `smoke #21-23`、`rvim file` 无 tty 路径 |
| 模式循环 | normal/insert/cmdline 分发 + 映射展开 | ✅ `rvim_app::tui::Tui`：`feed_key` → 映射展开（最长匹配、`noremap` 不递归）→ 模式分发；两键命令 `dd`/`yy`/`gg`/`dw`/… | `m4_tui.rs`（46 例） |
| 命令行 | 行编辑、`wildmenu`、`/`/`?` 搜索、`<Tab>` 补全 | ✅ 行编辑（左/右/Home/End/BS/Del）、`<Tab>`/`<C-N>`/`<C-P>` 补全 + wildmenu 行、`/`/`?` 字面量搜索（`ignorecase`+环绕） | `tab_completion_opens_the_wildmenu`、`forward_search_moves_the_cursor`、`command_line_runs_ex_commands` |
| 滚动 | 光标始终可见（横/纵）；`<C-f>`/`<C-b>`/`<PageDown>`/`<PageUp>` 翻一屏 | ✅ `Tui::grid` 调整 `top`/`left` 跟随光标（横滚扣除 `'number'` 行号栏宽度）；翻页保持光标屏幕行并移动 `top` | `vertical_scroll_follows_the_cursor`、`long_lines_scroll_*`、`page_down_and_up_scroll_one_screen`、`horizontal_scroll_leaves_room_for_the_number_gutter` |
| 无终端可验证性 | — | ✅ `--script <file>` / `--keys <notation>` 回放按键并打印最终屏幕（冒烟与测试驱动用） | `smoke #21-23` |

### M4 已知偏差（收口 M5/M6）

| 主题 | Vim | RustVim | 收口 |
|---|---|---|---|
| 视觉模式 / 鼠标 | `v/V/<C-v>`、`set mouse` | ❌ 未实现（`feed` 仅 normal/insert） | M5 |
| 软换行 / `relativenumber` / `scrolloff` | `'wrap'` 默认开 | ❌ 仅横向滚动；无 `relativenumber`/`scrolloff` | M5 |
| 命令行历史键 | `<Up>/<Down>` 召回历史 | ⚠️ `rvim-cmdline` 的历史模型已就绪，但未接入 TUI 键盘 | M5 |
| `<expr>` 映射 | 求值 RHS 作为按键 | ⚠️ 显式 `E15`（需把 M3 求值器传入展开循环） | M5 |
| `<t_xx>` 与鼠标键 | terminfo 字节序列、鼠标事件分发 | ⚠️ `<t_xx>` 可解析/列出但无 terminfo 无法编码成字节；鼠标键名可解析可映射，解码与 `set mouse` 待 M5 | M5 |
| `:!` 外部命令显示 | 命令输出 + `Press ENTER` 提示 | ⚠️ 输出直接写终端后重绘，无 hit-enter | M5 |
| 插入模式的 `<C-w>`/`<C-u>`/`<C-r>`/`<C-a>` | 删前一词、删到行首、插入寄存器、重新插入上次文本 | ❌ 未绑定（`<Esc>`/`<CR>`/`<BS>`/`<Del>`/方向键已接） | M5 |
| `'autoindent'`/`'smartindent'` 换行缩进 | 插入模式 `<CR>` 继承当前缩进 | ❌ 换行后从第 1 列开始 | M5 |
| `<C-a>`/`<C-x>` 数字增减 | 光标下（或下一个）数字 +1/-1，支持计数与 `nrformats` | ✅ M5 已实现（`rvim-normal::addsub`，见 §9）；仅未做视觉模式 `g<C-a>` 与 64 位回绕 | — |
| 长消息软折行 | 超过窗口宽度的消息折行显示并分页 | ⚠️ 单行内截断，`-- More --` 只按消息条数分页 | M5 |
| `'termguicolors'` / 语法高亮颜色 | 真彩 + 高亮组 | ⚠️ 颜色模型支持 256/真彩，但无高亮组（等 syntax） | M6 |
| 三端人工验证 | kitty / Windows Terminal / conhost | ⚠️ 本环境无交互终端，仅以 `--script` 逐帧验证；三端肉眼验证待人工执行 | 收口时 |
## 7. M0–M3 深度复核轮（review round 2）已修复

> 复核方式：差分对拍（`rvim -es` vs `vim -N -u NONE -i NONE -es`，缓冲用 `:w!` 逐字节比较，值用
> `writefile()` 捕获、退出码单独比较），再以 `reference/vim/src` 逐条核对语义。下表只列**行为变化**；
> 未提及的行为维持原状。回归用例分布在 `crates/rvim-app/tests/*` 与各 crate 的 `#[cfg(test)]` 中。

### 7.1 M0 工程基建

| 主题 | 修正 |
|---|---|
| CI 门禁 | 新增 `scripts/ci.ps1`：`cargo fmt --check` → `clippy -D warnings` → `test --offline` → `cargo tree` 零第三方依赖 → `coverage --check` → `smoke.ps1`，逐关失败即退出 |
| 工具链一致性 | `rustfmt.toml` edition 改为 2024（与 `Cargo.toml` 一致）；清理仓库根遗留临时文件；`.gitignore` 覆盖开发期 scratch 目录 |
| 文档真实性 | PLAN 的黄金用例数（34）/冒烟场景数（23）/Ex 选项数（56）与测试布局；ARCH 的 `Line`/`Buffer`/`Editor`/`Pos` 字段、列语义（字节偏移）、changedtick 说明均改为与代码一致 |

### 7.2 M1 编辑内核

| 主题 | Vim 行为（参考源） | 修正 |
|---|---|---|
| 多字节列 | `col` 是字节偏移，插入/删除按 `mb_ptr2len` 前进（`mbyte.c:1105`、`ops.c:604-731`） | `Buffer::insert_text`/`delete_chars` 直接使用字节列（对齐到字符边界），`Editor` 的插入后列与撤销光标按 UTF-8 字节长度计算；`lx`/`lix`/`$x` 在多字节行上正确 |
| 新建文件 | 打开不存在的文件是**新缓冲**，`:w` 创建之（`fileio.c:500-512` `BF_NEW`） | `read_file` 对 `NotFound` 返回空缓冲；`rvim -es -c wq new.txt` 退出 0 并创建文件 |
| `'readonly'` | 非 `!` 写盘报 `E45`，` :w!` 覆盖（`bufwrite.c` `check_readonly`） | `save_current` 把选项同步到 buffer，`Buffer::save` 报 E45；` :w!` 写成功 |
| 标记随编辑调整 | `mark_adjust()` 在删除/插入/撤销时移动/失效标记（`mark.c:998`、`undo.c:2886-2897`） | 新增 `MarkStore::adjust` 并由所有 `RangeEdit`（含 undo/redo）集中调用，`':1,2d'+u`、`:s`、`:g/d`+u 后 `line("'a")` 与 vim 一致 |
| `b:changedtick` / `&modified` | 每次变更/撤销/重做单调递增；`&modified` 反映缓冲状态（`:set nomodified` 清除） | `b:changedtick` 接 `Editor::changedtick()`；`&modified` 接 `Buffer::is_changed()`，`:set [no]modified` 走 `Editor::set_modified` |
| `'fileformat'` | 读文件时设为 buffer-local 选项；Windows 默认 `dos`（`optiondefs.h:928`、`option.h` DFLT_FF） | `Buffer::open`/切换/`:e` 同步选项；CRLF 文件 `&ff=dos`，新文件在 Windows 写 CRLF，`:set ff=` 继续同步缓冲 |
| UTF-8 BOM | 读取时剥离并记 `'bomb'`，写盘回写（`fileio.c:43,868-905`） | `split_bom` + `Buffer.bomb`，`getline(1)` 不含 BOM，重写后 BOM 保留 |
| 空缓冲写盘 | 一个空行的缓冲写出 0 字节（`bufwrite.c` `ML_EMPTY`） | `Buffer` 增加 empty 标志，`:enew`+`:w` 产出 0 字节；有文本时仍带一个行尾 |
| 寄存器 | 命名 yank 同时进 unnamed 但不进 `"0`；大写目标追加到小写寄存器并成为 unnamed；`_` 丢弃一切（`ops.c:685-731`、`register.c:216-263`） | `RegisterStore::yank/delete` 重写；命名 yank 不再写 `0`，`1y a`+`put`、`1y a`+`2y A`、`"Ap`、`getreg('A')` 全部对齐 |
| 位置参数 | 多个文件：编辑第一个，其余进参数列表（`main.c` `create_windows`） | `rvim a.txt b.txt` 当前缓冲为 a.txt，`argv()`/`:args` 含两者 |
| `:cq` 与退出码 | 脚本内 `:cq N` 终止进程；`getout()` 为 `exitval += ex_exitval`（`main.c:1563-1575`） | `Session::source` 记录 quit/exit_code；有错误时显式码 +1（`:cq 3` + 错误 → 4） |
| 选项校验 | `tabstop<1`/`shiftwidth<0` 报 E487，非法 `fileformat` 报 E474 且保留旧值（`option.c`、`optiondefs.h`） | `Options::set` 存储前校验 |
| `:e` 脏缓冲 | 未保存修改时 `:e` 报 `E37`，`:e!` 覆盖（`ex_cmds.c:2593-2604`） | `cmd_edit` 检查 `is_changed()` |
| Ex 模式起始光标 | `-e/-es` 打开文件后光标在最后一行（`main.c:719-721`） | 启动时 `Editor::start_ex_mode_cursor()` |
| Windows 单绝对路径 | 只有一个全限定文件名时切换到其目录（`main.c:275-288`） | `main` 在打开文件后 `chdir` 并同步 `editor.cwd` |
| 启动空缓冲复用 | 首个文件复用 1 号 `[No Name]` 缓冲（`bufnr('%')==1`） | `Editor::open_file` 复用未修改的空启动缓冲 |

### 7.3 M3 VimL 解释器

| 主题 | Vim 行为（参考源） | 修正 |
|---|---|---|
| 整数算术与除零 | 64 位回绕；legacy 脚本整除零返回 VARNUM_MAX/MIN、模零返回 0，**不报错**（`eval.c:60-110`） | 加减乘/一元负号改 `wrapping_*`；`/`、`%` 零按 vim legacy 语义（此前 `+` 溢出会 panic） |
| 浮点格式化 | `vim_snprintf("%g")`：6 位有效数字、保留小数点、指数去 `+`/前导零（`strings.c:2678-2790`） | `format_float`/`printf %g/%e/%f/%x/%o/%b` 按该规则重写（此前输出 13 位/直接拼接宽度） |
| 真值与一元运算 | `:if`/`!` 经 `tv_get_number` 转换，字符串按数字真假（`eval.c:182`）；`!1.0` 仍是 Float | `Value::truthy` 字符串走 `str_to_number`；`!` 对 Float 保留浮点类型 |
| `is`/`isnot` | 同类型且容器比身份（`typval.c` EXPR_IS） | 解析器接受关键字形式与 `is#`/`is?`；类型不同判不等 |
| 字符串索引/切片 | 越界或负索引得空串，不报 E684；列表与字符串切片规则不同（`eval.c:4691-4708`、`list.c:1150-1184`） | 索引越界返 ""；切片分字符串/列表算法，反向与越界为空 |
| 容器比较 | 元素级比较区分类型（`tv_equal`，`typval.c:1748`）；列表排序比较非法（E692 等） | `tv_equal` 用于 list/dict/`index()`/`count()`/`uniq()`；`>`/`<` 对容器报 E692/E736/E978/E694 |
| `str2nr` 转换 | 不跳前导空白、`-` 仅在首位、溢出钳到 VARNUM_MAX/MIN（`charset.c:1993-2009`） | `str_to_number`/数字字面量按此实现（`' 12'+0`→0、`+7`→0、巨数字面量钳位） |
| `map()`/`filter()` | 原地修改并返回同一容器（`list.c:2305-2424`、`dict.c:1302`） | 原地写回 list/dict（此前返回新容器） |
| 参数作用域 | `a:1..a:N` 仅含 `...` 变参（`userfunc.c:2729-2813`） | 命名参数只在名字下可见；`a:firstline/a:lastline` 补齐 |
| 作用域字典 | 裸 `g:`/`b:`/`w:`/`t:`/`s:`/`l:`/`a:`/`v:` 是作用域字典（`evalvars.c:3203-3236`） | `get(g:,'x',d)`、`type(v:)`、`exists('g:')`、`filter(g:,…)` 可用；b:/w:/t: 为快照（见 7.4） |
| 异常跨函数 | 抛出值原样穿过调用栈；`:try` 内的错误可被捕获；`abort` 函数首个错误即返回 -1（`eval.txt:2726-2728`、`ex_eval.c:144/1537`） | `pending_throw` 保留原值；`abort` 属性解析并生效；错误在 try 中转为 `Vim(cmd):E…` |
| `:finally` 中 `:return` | 丢弃挂起异常并立即返回（`eval.txt:4000-4008`） | finally 的 flow 生效并清除挂起异常 |
| 双引号转义 | 未知转义丢弃反斜杠、`\X41`/`\x`/`<Char-65>` 等（`typval.c:2116-2210`） | lexer 转义表重写 |
| `:execute` | 逐个求值表达式并以空格连接（`eval.c:6699-6757`） | `:execute 'let g:x =' 1 + 2` 得到 3 |
| 其他语法 | `0o17` 八进制、`..` 连接、`len()`（字符串按字节、数字按位数、Float/Bool 为 0）、`exists('v:true')`、`v:null` 比较（`charset.c:1859`、`eval.c:3143`、`evalfunc.c:7079`、`typval.c:1170-1239`） | 全部对齐 |
| 深递归 | 超过 `'maxfuncdepth'`（默认 100）报可捕获 `E132`（`userfunc.c:2536-2544`） | 编辑器主循环运行在 64 MiB 栈线程上（`rvim_ex::script::run_with_deep_stack`），`s:Rec(30)`=30，150 深 → `Vim(return):E132` |
| 内置函数族 | `expand()`（`%`/`#`/`~`/`$VAR`/`<cword>` 及修饰符）、`fnamemodify`、`getline({l},{e})`、`getreg` 未设寄存器为空、`printf`、`split`、`get` 默认 0、`count` 起始下标、`sort`/`map`/`filter` 字符串表达式、`call()`、`range()`、`index`/`uniq` 类型敏感比较 | 逐项修正（详见 `crates/rvim-viml/src/builtins.rs` 与 `crates/rvim-ex/src/script.rs` 注释） |

### 7.4 M2 Ex 命令体系（本轮修复）

| 主题 | 修正 |
|---|---|
| `:delete`/`:yank` 计数 | `:2d 2` 删 2-3 行、`:2y 2` 取 2 行（`ex_docmd.c:2354-2378` EX_COUNT） |
| `:retab` | 无 `!` 时不把纯空格缩进改成 Tab；`:retab 0` 用当前 `'tabstop'`（`indent.c:1693-1747`） |
| 反向范围 | 脚本/Ex 模式下报 `E493`，不再静默交换（`ex_docmd.c:2146-2162`） |
| `:g`/`:v` | 修改类命令按**升序**处理且行号随编辑移动（`ex_cmds.c:4907-4910,5085`）：`:g/^/m0` 反转、`:g/pat/t0` 等与 vim 一致；无匹配时仅 `smsg` 不计错误 |
| `:s` | `e` 标志抑制 E486、尾随计数表示行数、替换串 `&` 展开、空模式复用上次搜索（`ex_cmds.c:3895/3928/3715/3756`） |
| `:autocmd` | `:au!` 只作用于当前 augroup；无 `/` 的模式匹配文件名尾部（`autocmd.c:1072-1088`、`fileio.c:5386-5397`） |
| `:runtime` | 无 `!` 只 source runtimepath 中第一个命中，`!` 按 rtp 顺序全部 source（`scriptfile.c:248-276,440`） |
| 用户命令 | `-range=%` 缺省 `1,$`、`-count` 暴露 `<count>`、`-bar` 允许尾随 `\|`（`usercmd.c:816-912,1669`） |
| `:put` | `:put =expr`（列表逐行、字符串按行）与 `:0put`/`:put!` 方向（`ex_docmd.c:2335-2344,7989`） |
| `:copy`/`:move` | 目标地址支持 `$`/`.`/`+N`/`'a`，光标停在最后移动行（`ex_docmd.c:8013-8040`） |
| `:read` | 插在地址行**之后**（范围取末地址），`:0read` 插到首行前（`ex_docmd.c:7369/7401`） |
| `:join` | `:2j` 合并 2-3 行，`:2,2j` 不动（`ex_docmd.c:8074-8091`） |
| `:sort` | `:sort!` 反向；`r`/`x`/`o`/`b`/`f` 标志按 vim 语义（`ex_cmds.c:406-480,604-611`） |
| `:left`/`:center` | 缩进参数与既有缩进处理（`ex_cmds.c:156-185`） |
| 地址解析 | 行首相对地址（`-1d` ≡ `.-1d`）、空第二地址默认当前行、`:N` 地址后接命令等（`ex_docmd.c:3295,4501-4596`） |
| 命令补全 | 新增 `:k`/`:mark`（`ex_mark`），Ex 命令实现数 163→165 |
| `:source` | 不再回退 runtimepath 搜索（`scriptfile.c:1150-1175`） |

### 7.5 本轮登记但未修复的偏差（显式记录，不静默）

| 主题 | Vim | RustVim | 收口 |
|---|---|---|---|
| 字符串字节语义 | 字符串是字节串：`len('é')`=2（已对齐）、`s[0]` 返回单字节、`strpart` 按字节 | `Value::String` 是 Rust UTF-8 `String`：索引/切片/`strpart` 按字符，无法表示非法 UTF-8 单字节 | 若需要，M6 前评估 `Vec<u8>` 化收益 |
| 闭包捕获 | lambda 按引用捕获外层变量与 `a:` 参数，函数返回后仍共享 | 创建时按值快照，`a:` 不可见 | M5/M6（需帧改 `Rc<RefCell>`） |
| `<SID>`/`<SNR>` | 脚本局部函数绑定脚本 id，`function('s:F')` 可在别处调用，`expand('<SID>')` 返回 `<SNR>N_` | `s:` funcref 在别的脚本中不可调用；`<SID>` 字面返回 | M3 后续 |
| 单行复合块 | `if 1 \| cmd \| endif` 合法（do_cmdline 跨 `\|` 维护 cstack） | 块解析按物理行，报 E171/E170 等 | M3 后续（需语句流化） |
| dict 迭代顺序 | 哈希表顺序（`keys()/values()/items()/string()/for`） | 插入顺序（保序字典；语义正确但可观察顺序不同） | 收口时评估是否复刻哈希顺序 |
| 正则范围 | 完整 vim 正则 | MVP（`=~`/`!~`/`match*()`/`:s`/`:g`/搜索地址共用）：支持基础元字符、字符类、捕获组与 `\|`；无 `\zs\ze\@!`、反向引用、多行、`\a\l\u` 简写 | M6（rvim-regex） |
| `:s` 替换特殊项 | `\1`-`\9`、`\u\l\e\E`、`\=` 表达式 | 仅 `&`/`\&`、`\n`、`\t`、`\\`；反向引用与大小写修饰未实现 | M6 |
| `b:`/`w:`/`t:` 作用域字典 | 写穿透到真实变量 | 读可用、写不传播（快照） | M3 后续 |
| `:move` 内标记 | 移动范围中的标记随文本移动 | delete+insert 组合导致标记不跟随 | M3 后续（ex 侧 move-aware adjust） |
| `:s///c` 确认 | 交互确认 | 忽略 `c`（批处理无输入层） | M5（接输入层） |
| 撤销粒度/光标 | 脚本执行期间不 sync undo（整脚本一步）；undo 后光标在首个被撤销行 | `:s`/`:g`/`:>`/`:retab` 等多行命令已是一个 undo 步；整个脚本仍按命令分步；undo 光标取记录位置（文本正确） | M5（undo 树/分组） |
| `'packpath'` | 429 项之一，`:runtime START/OPT` 按 packpath 搜索 | 选项表暂无 `packpath`：`:runtime START/OPT` 退化为 `'runtimepath'`（与 vim 默认 packpath 相同），`:set packpath=` 报 E518 | M5（随选项表扩充） |
| 用户命令 `-range=%`/`-range=N` | `UserCommand` 保存缺省范围/count 属性 | 功能正确，但内部用 count 槽编码缺省范围，`:command` 列表会显示哨兵值 | M3 收口 |
| `:g` 内的其他行数变更命令 | pass2 用标记跟踪行移动，任意子命令都正确 | 行移动跟踪覆盖 delete/move/copy 族子命令；`:g/x/j` 等其它改变行数的子命令可能让后续匹配行号偏移 | M3 后续 |
| `'maxfuncdepth'` | 可设置选项（默认 100） | 深度上限固定 100（`funcdepth_limit()` 读选项但选项表暂无该项） | M5（随选项表扩充） |
| `dict` 函数 `self` | 完整 dict-function 绑定 | 仅绑定首个字典参数 | M4/M5 |
| `v:` 变量 | 完整集合 | 常用子集（含 v:true/false/null、v:exception、v:errmsg 等） | 随需求补 |
| 选项作用域 | 429 项、global/window/buffer | 56 项、全局模型；`setglobal` 不保留 buffer 局部值 | M5 |

## 8. M4 复核轮（review round 3）已修复

> 复核方式：直接驱动 `Tui`（与 `rvim --keys` 同一代码路径）逐帧检查终端行为，并与
> `reference/vim/src` 的 `normal.c` `do_pending_operator()`、`search.c` `searchit()`、
> `message.c`（hit-enter / `-- More --`）、`screen.c`（`transchar` / Tab 展开）、
> `buffer.c` `get_rel_pos()` 逐条对照。下表只列**行为变化**；回归用例集中在
> `crates/rvim-app/tests/m4_tui.rs` 与各 crate 的 `#[cfg(test)]`。

| 主题 | 问题 | 修正 |
|---|---|---|
| 未知 CSI 码卡死输入 | `decode_csi()` 对表里没有的 `CSI … ~`（F13+、bracketed paste 等）返回“序列未完成”，字节永远留在缓冲区，键盘从此全部失效 | 消费整个序列并回退为 `Key::Special("CSI-n~")`；`ByteKeySource` 在 EOF 处逐字符吐出残余字节，不再一次丢掉尾部 |
| Tab 与控制字符写进网格 | 缓冲区里的 `\t` 直接作为单元格字符输出，终端把它当制表位跳到下一站，整屏错位；其他控制字节同理 | 新增 `render::expand_cells()`：Tab 按 `'tabstop'` 补到下一个边界、控制字符显示为 `^X`（`^A`、`^?`、NUL 为 `^@`）；光标列改用显示单元（`display_col()`）；消息行同一套展开（`:echo "a\nb"` 显示 `a^Jb`） |
| `:echo` 消息不显示 | `collect_messages()` 只读 `MessageLog::history()`，而 `:echo` 只写 `last_status`，TUI 里 `:echo` 被静默丢弃 | 同时取 `take_status()`；错误/`:p` 已进历史，按最后一条历史去重，`E492` 不再入队两次 |
| 消息分页失效 | `next_message()` 是死代码：队列里第 2 条及以后的消息永远不显示，末页也没人推进 | 消息行显示 `-- More --`；任一键翻页并吞掉该键，末页后的键清屏后照常执行（`message.c` 的 hit-enter 行为） |
| `/` 搜索跳过当前行 | 从 `cursor.line + 1` 起步，光标右侧同一行的匹配被跳过 | 从光标后一个字符开始扫当前行，再环绕；环绕时多扫一轮起始行行首（唯一匹配会停回原处） |
| `?`/`n`/`N` 未绑定 | 反向搜索与重复搜索在 TUI 里不存在（`?` 会被当普通键继续插入文本） | 新增 `search()`/`repeat_search()`；空模式复用上次模式，无历史报 `E35` |
| 操作符后的动作被吞掉 | `d`/`y` 之后只要不是同一个字母就取消：`dw`/`dj`/`d$`/`yw`/`cw`/`D` 全部静默无操作 | 新增 `operators::motion_range()` 与 `delete_range`/`yank_range`/`change_range`，`NormalEngine::feed_operator_key()` 支持操作符后的计数（`d2w`/`d3l`）与 `gg`；补 `D`/`C`/`Y` 单键形式；`cw` 在词上等价 `ce`；`<Esc>` 取消不移动光标并丢弃计数 |
| 状态行百分比 | 只按光标行算百分比（`total<=1` 才显示 `All`），2 行文件在 6 行窗口里显示 `50%` | 按 `buffer.c:get_rel_pos()`：整屏装下 → `All`，顶部 → `Top`，底部 → `Bot`，否则按窗口上方行数占比 |
| 行号栏与横向滚动 | 横滚宽度按整屏列数算，`'number'` 打开后光标会被行号栏盖住 | `gutter_width()` 统一计算行号栏宽度，`grid()` 用 `cols - gutter` 作为文本列数 |
| 翻页键 | `<C-f>`/`<C-b>` 未绑定；`<PageDown>` 直接跳末行、`<PageUp>` 只挪 `top` 不动光标 | 翻一屏并保持光标屏幕行；`<C-f>`/`<C-b>`/`<PageDown>`/`<PageUp>` 同一实现 |
| 插入模式 `<Esc>` 误移光标 | `<Esc>` 取消挂起操作符时也走 `finish_insert_mode()`，光标左移一格并触发 `end_undo_block()` | 只有插入类模式才收尾插入会话；操作符挂起时只清状态 |

## 9. M5 复核轮（review round 4）已修复

> 复核方式：对照 `reference/vim/src` 的 `window.c`（frame 树、`win_split`、`win_equal`、`win_vert_neighbor`）、
> `fold.c`（`foldUpdate`/`foldOpen`/`foldClose`/`foldtext`）、`diff.c`/`xdiff`、
> `quickfix.c`（`efm_to_regpat`/`qf_parse_line`）、`textobject.c`、`normal.c`（`nv_dot`/`nv_visual`）
> 逐条重写并用 `--keys` 逐帧验证。回归用例：`crates/rvim-app/tests/m5_windows.rs`（21）、
> `m5_folds.rs`（9）、`m5_diff.rs`（6）、`m5_quickfix.rs`（7）、`m5_signs.rs`（6），
> 加上 crate 内单测（rvim-diff 18、fold 18、foldcmd 10、handlers_fold 9、errorformat 31、
> quickfix 17、handlers_qf 28、diff 16、handlers_diff 12、textobjects 21、viminfo 17、
> window 12、handlers_win 6、host_win 30）。

| 主题 | 问题 | 修正 |
|---|---|---|
| `<C-a>`/`<C-x>` 数字增减 | 未实现（normal 引擎没有这两个键，插入模式下 ``C-a`` 也未绑定） | `rvim-normal::addsub` 按 `ops.c do_addsub()`/`normal.c nv_addsub()` 实现：十进制/`0x` 十六进制/`0b` 二进制/`0` 八进制（受 `'nrformats'` 控制，默认 `bin,hex`）、`-` 号属于数字、前导零保持宽度、十六进制保持大小写、光标不在数字上时取行内下一个数字、`alpha`/`unsigned` 选项、计数与 `.` 重复、一次 undo 一步；TUI 把 `<C-a>`/`<C-x>` 交给引擎。回归用例 13 条（`addsub` 9 + `m4_tui` 4）。**未做**：视觉模式 `g<C-a>` 顺序编号、超过 64 位时的回绕（改为截断） |
| 计数插入的重复方式 | `3oabc<Esc>` 把“打开行 + 文本”整体重放（`edit.c ins_esc()` 的 `start_redo_ins()`），因此 `3o` 的每一份都在上一份**下方**、`3O` 的在**上方**，光标落在最后插入的那一处 | 此前是用字符串拼接复制：`3O` 的副本被插到光标下方（光标落错行），文本里含 `<CR>` 时还会把带换行的字符串当成一行塞进缓冲区 | 改为重放 `enter_insert(entry)` + 逐字符 `type_char`，`3o`/`3O`/`3i`/`3A` 的方向与光标、多行文本、单步 undo 全部与 vim 一致；回归用例 4 条（`m4_tui.rs`） |
| `'showcmd'` | 没有实现：`set showcmd` 后光标行右侧不显示未完成的命令，而且 `'showcmd'` 的默认值是 off（vim 在 nocompatible 下默认 on，`optiondefs.h` 无 `P_VI_DEF` → `VIM_DEFAULT`） | 默认改为 on；TUI 在命令行行右端按 `normal.c display_showcmd()` 画出 10 列字段（`Columns-(SHOWCMD_COLS+1)` 起，超长取尾部）：计数（`2d`/`d2`）、操作符与文本对象前缀（`di`）、`g`/`z`/`]`/`[`/`<C-w>`（显示为 `^W`）、寄存器前缀（`"a`/`"ad`）、视觉模式选区大小（字符数/行数/`行x列`）；命令行编辑时让位给 `:` 提示符。回归用例 5 条（`m4_tui.rs`） |
| 列选模式 `I`/`A` 无效 | TUI 的视觉模式分发里没有 `I`/`A` 分支（落到“动作”分支后什么都不做，随后 `x` 又把选区删掉）；即使进入插入模式也没有记录块，`<Esc>` 后不会把文本复制到块内其它行 | 视觉模式分发 `I`/`A`（仅 `VIsualBlock`）；`Editor` 新增 `BlockInsert`，`finish_insert` 在插入会话结束时按 `ops.c block_insert()` 把文本复制到块内其余行（`I` 跳过短行、`A` 先补空格到块右边界），整段仍是**一个** undo 步；视觉模式新增计数（`3I`、`v3j`）。回归用例 8 条（`m5_windows.rs`） |
| 插入模式 `<CR>` 丢换行 | ① `Buffer::insert_text` 复用 `split_lines` 的**文件**约定（尾部换行不产生新行），所以行尾 `<CR>` 不分行、光标被钳回行首；② TUI 自己用 `replace_lines` 处理 `<CR>`，没有记入插入会话，导致 `3i%<CR><Esc>` 与 `.` 重复时换行被吞掉（只剩 `%`） | `insert_text` 在合并文本以换行结尾时补回空行（`edit.c ins_eol` 语义），`<CR>` 统一走 `insert::type_char('\n')` 记入插入会话；新增 `1000i%<CR><Esc>`、`3i%<CR>`、`. ` 重复换行与 `insert_text` 行尾换行的回归用例 |
| 单窗口假设 | M1–M4 的 `Editor` 只有一个 `Window`，`winnr()`/`winheight()`/`getwininfo()` 等内置函数硬编码 1 和 -1 | 新增 `rvim-editor::window`（`TabPage`/`Layout` frame 树/`Window`/`Windows`），`Editor.windows` 取代单窗口字段；`host_win.rs` 全部窗口/标签内置函数改查真实模型 |
| 窗口局部选项 | 选项表只有全局模型，`:setlocal` 等同 `:set` | 新增 `WINDOW_LOCAL` 表与 `Window.local`：`:set` 写全局+当前窗口、`:setlocal` 只写窗口、`:setglobal` 只写全局，`&opt` 读生效值；`w:` 变量按窗口 id 分隔 |
| `<C-w>` 邻居判定 | 初版把“垂直重叠”用在左右方向，相切的两窗互不可达 | 按 `window.c` 重写：左右移动要求行重叠、上下移动要求列重叠，允许相切 |
| `:vertical` 修饰符 | 只置标志，不执行后续命令（`:vertical split` 什么都不做） | 修饰符解析参数并把后续命令交给同一执行器，之后清除标志 |
| 文本对象范围 | 对象返回“最后一个字节”的包含式终点，操作符要半开区间，导致少删/多删一个字符 | 把含端点转成开区间，行级对象走 `line_range` |
| 视觉模式文本对象 | `viw` 选中后被清空选区，`d` 变成普通 `d` | 视觉模式的对象只扩展选区，不结束选择 |
| `.` 重复 | 只注册了句点寄存器，`.` 无操作 | 新增 `RepeatChange`（插入会话/操作符+动作/`x D C`/`p P`），`.` 按 vim 语义重放 |
| 可视区标记 | `MarkStore::resolve` 只认 a-z/A-Z，`:'<,'>d` 报 E20 | `resolve` 支持可视区起点/终点/上次修改等缓冲区局部特殊标记 |
| diff 模式 | 缺失 | `rvim-diff`（Myers + 变更块滑动归并）、`Editor::diff_*`（filler/变更行/`]c`/`[c`/`:diffget`/`:diffput`）、`rvim -d` 即 vimdiff |
| quickfix | 缺失（`setqflist()` 返回空、`:copen` 未实现） | `rvim-ex::errorformat` + `rvim-editor::quickfix`（10 层栈、qflist/loclist、qf 窗口）+ `c*`/`l*` 全族命令与 4 个内置函数 |
| 折叠 | M2 占位（`MoreState.folds` 从未填充，`foldclosed()` 永远 -1） | 新增 `rvim-editor::fold`（每窗口一份折叠表、manual/indent/marker/diff）、`rvim-normal::foldcmd`（z 系列）、`:fold*` 命令；TUI 渲染折叠摘要与 `'foldcolumn'` |
| 跳转/change list | `jumplist` 只有空结构，`:jumps` 未实现 | `Editor` 新增真实 jump/change list，`<C-o>`/`<C-i>`、`:jumps`、`:changes` 可用；viminfo 读写（`:wviminfo`/`:rviminfo`/`-i`/`'viminfo'`） |
| sign/textprop/popup | 数据面完整但不可见 | TUI 新增 sign 列（`'signcolumn'=auto` 时按需占位）与 popup 覆盖层；`prop_*` 仍只做数据面 |

### 9.1 本轮登记但未实现的偏差（显式记录，不静默）

| 主题 | Vim | RustVim | 收口 |
|---|---|---|---|
| `'foldtext'` 自定义表达式 | 支持表达式/函数 | 只实现默认 `foldtext()` 文本 | M6 随表达式求值接线 |
| `foldmethod=expr`/`syntax` | 求值/语法驱动 | 保留手动折叠并显式报告不支持 | M6 |
| `:diffpatch` | 从 patch 生成对照缓冲 | 未实现 | 收口时评估 |
| `'diffopt'` | icase/iwhite/context 等 | 固定行为（filler + 6 行上下文），不接受选项 | 收口时补 |
| quickfix 多窗口 | 每标签页可各开一个 qf 窗口 | 只建模一个 qf 窗口，`:lopen` 复用 | 收口时补 |
| `'buftype'` | qf 窗口 `buftype=quickfix` | 选项表无该项，用 `Window.quickfix` 标记 | 随选项表扩充 |
| textprop 高亮 | `prop_add` 影响显示 | 只存数据，渲染忽略 | M6 随 syntax 一起接 |
| popup filter/回调 | `filter`/`callback` 可选 | 仅数据面 + 静态覆盖层 | M7 |
| viminfo 字段 | 全字段（含 jumplist/搜索模式） | marks/registers/history/cwd/global 子集，UTF-8 文本 | 收口时补 |
| `w:`/`t:` 变量 | 写穿透真实窗口/标签 | 按窗口/标签字典存放，外部切换在下一次内置函数调用时同步 | M6 |
| `<C-w>r`/`x`/`H J K L` | 调整 frame 树位置 | 交换/轮转窗口“内容”（几何不变），两窗口情形等价 | 收口时按 frame 重写 |
| 每缓冲窗口位置 | 每个窗口记住每个缓冲的光标/视口 | 换缓冲时视口跟随该缓冲最后光标 | 收口时补 `b_wininfo` |
| 列选模式的列 | 用虚拟列（`curswant`），短行被夹住后块仍是原宽度，`A` 会补齐到该宽度 | 用字节列：`<C-v>` 的块右边界取 anchor 与光标的**当前**字节列，光标在短行被夹住时块会变窄（`<C-v>lllj` 在 `abcd`/`x` 上得到 0..1 而不是 0..3） | 收口时引入 `getvcol()/curswant` 虚拟列模型 |

