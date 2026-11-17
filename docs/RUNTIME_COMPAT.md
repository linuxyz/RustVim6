# Vim Runtime 兼容性设计（RUNTIME_COMPAT）

> 本文定义 RustVim 对 **Vim Runtime**（随 Vim 分发的 vimscript 生态、启动序列、内置命令/选项/函数/事件）的兼容范围、方法与验收口径。
> 基线唯一来源：[`vim/vim@v8.2.5172`](https://github.com/vim/vim/tree/v8.2.5172)，即 Vim 8.2 patch 1–5172。

---

## 1. 兼容总目标

让真实 Vim 的 runtime 文件与用户 vimrc **不加修改或仅最小修改**即可被 RustVim 加载执行，具体分四个兼容面：

| 兼容面 | 含义 | 验收物 |
|---|---|---|
| A. Runtime 文件 | 官方 `runtime/` 目录全部脚本能被 source、autoload 按需加载 | 各目录加载冒烟测试 + 快照 |
| B. VimL 语言 | legacy vimscript 语法/语义、内置函数、类型转换、异常 | testdir 对拍通过率 |
| C. Ex/Option/事件 | 589 命令、429 选项、全部 autocmd 事件的行为与缩写/flags | 三张覆盖率表 + 特性测试 |
| D. 数据格式 | swap/undo/viminfo/spell/tags/session/加密文件互读互写 | 与真实 vim 交叉读写用例 |

兼容原则：**未实现要显式报错（带 E 号），绝不静默给出错误结果**；与 Vim 的任何偏差登记到 `docs/compat_deviations.md`。

---

## 2. 官方 runtime 目录兼容清单（v8.2.5172 实测布局）

| 目录/文件 | 规模（基线实测） | 作用 | RustVim 接入点 | 里程碑 |
|---|---|---|---|---|
| `plugin/` | 13 项 | 启动时全量 source 的全局插件 | 启动序列 load-plugins | M3 |
| `autoload/` | 39 项（顶层） | `name#fn()` 首次调用按需加载 | VimL 解析器 + rtp 搜索 | M3 |
| `pack/` | 1 项（packages） | `packadd` 可选包（start/opt） | 包加载器 | M5 |
| `ftplugin/` | 261 项 | 文件类型插件（BufRead 后按 &ft 加载） | ft 检测 + 事件驱动 | M3/M6 |
| `indent/` | 143 项 | 缩进脚本（`=`/自动缩进回调） | indentexpr 求值 | M6 |
| `syntax/` | 大目录（>500） | 语法高亮脚本 | syntax 引擎 | M6 |
| `colors/` | 21 项 | `:colorscheme` | highlight 组系统 | M4/M6 |
| `compiler/` | 100 项 | `:compiler`（errorformat/makeprg） | quickfix 子系统 | M5 |
| `keymap/` | 82 项 | `:set keymap=` 输入法映射 | 映射/编码层 | M6 |
| `spell/` | 69 项 | 拼写词典与后缀文件 | spell 引擎（.spl 解析） | M6 |
| `lang/` | 147 项 | 菜单/消息翻译 | gettext 子集（`locale`） | M7 |
| `doc/` | 228 项（147 个 .txt） | `:help` 标签系统 | tags 文件解析 + help 窗口 | M5 |
| `tutor/` | 75 项 | vimtutor 多语言教程 | 原样分发（无逻辑） | M0 打包 |
| `macros/` | 17 项 | less/matchit 等宏脚本 | runtime 分发 + 脚本支持 | M3 |
| `print/` | 33 项 | postscript 打印支持 | hardcopy（低优先级） | M8/不支持登记 |
| `tools/` | 23 项 | 外部辅助工具 | 原样分发 | 打包 |
| `import/` | 1 项 | Vim9 import 模块 | vim9 子系统 | M8 |
| `bitmaps icons pixmaps` | 少量 | GUI 资源 | TUI 不消费；GUI 期处理 | 后置 |
| `defaults.vim` | 155 行 | 无 vimrc 时的默认配置（nocompatible/ruler/augroup...） | **启动序列原样 source** | M3 |
| `filetype.vim` | 68 KB | 文件类型检测总分发 | VimL 驱动（要求函数/augroup 齐备） | M3 末/M6 |
| `menu.vim synmenu.vim makemenu.vim` | — | 菜单体系 | TUI 不加载；GUI 期支持 | 后置 |
| `optwin.vim` | 68 KB | `:options` 窗口 | 选项窗口 | M5 |
| `mswin.vim evim.vim` | — | Windows 键位/易模式 | 原样分发，映射层支持 | M4 |
| `scripts.vim ftoff.vim ftplugin.vim indent.vim ...` | — | ft/indent 开关与分发 | 启动序列 | M3/M6 |
| `termcap` | 4 KB | 终端能力描述 | termio 键码/能力表的数据参考 | M4 |

### 2.1 runtime 获取策略
1. 首选：构建期通过 `git fetch --depth=1 --branch v8.2.5172` + sparse-checkout 把官方 `runtime/` 原样 vendor 到 `runtime/vendor/vim-8.2.5172/`（保持文件逐字节一致，便于升级 diff）。
2. 当前沙箱无 git 远程凭据/crates 通道，M1 仓库先内置 **最小 runtime**（`runtime/minimal/`：自建 defaults 等价脚本与示例 plugin），并在 `tools/fetch_runtime` 中固化拉取脚本，环境具备网络时一条命令补齐官方 runtime。
3. `$VIMRUNTIME` 解析顺序：环境变量 → 可执行文件同级 `../share/rvim/runtime` → 编译期 `RVIM_RUNTIME` 路径 → 内置 minimal 兜底。
4. `'runtimepath'` 默认值对齐 Vim（用户目录 `~`.vim 风格 → Windows `~/AppData/Local/rvim`，平台分支按 `os_win32.c` 规则）。

---

## 3. 启动序列兼容（对齐 runtime/doc/starting.txt）

RustVim 严格按以下顺序（M1 先实现标注 ✅ 的环节，其余为占位但顺序固定）：

```
1. 解析 CLI（-v/-e/-es/-c/-u/-U/-N/-C/-Z/--clean/--cmd/+cmd, -i, -S, -m/-R, --version）      ✅ M1 部分
2. 初始化内置选项默认值（optiondefs 429 项默认值表）                                            ✅ 子集
3. SHELL/TERM/COMSPEC/VIMINIT 等环境变量                                                        ✅ 子集
4. -u 指定/系统 vimrc/用户 vimrc（含 .exrc 与 'exrc' 安全确认）                                 M3
5. 加载 plugin/*（load-plugins，受 &loadplugins/--clean 控制）                                  M3
6. 无 vimrc 时 source defaults.vim（skip_defaults_vim 可跳过）                                   M3
7. packages: pack/*/start/* 自动加载                                                            M5
8. filetype on / indent on / syntax on 的 autocmd 注册                                          ✅ on/detect 会 source filetype.vim（indent/syntax 待 M6）
9. -c/--cmd/+command 与 -S 脚本按序执行                                                         ✅ 已对齐（单一有序队列；--cmd 先于 vimrc；+ = $、+N = :N）
10. 打开参数文件，触发 BufReadPre/BufRead/BufEnter/FileType 等事件链                             ✅ 已对齐（`rvim-ex::autodispatch`，含 ++once/++nested 语义）
```
- `&compatible` 推断规则（有无 vimrc、`-C/-N`、defaults.vim 内 `set nocompatible`）逐条复刻。
- `-es` 静默批处理模式：无屏幕、stdout 只收 `:print/:echo/:write` 类输出、退出码规则与 Vim 一致（显式 `:cq N`/`:q N` 码优先，否则**任意 `emsg()`**——含 `:source` 脚本内的错误——置 1；`:silent!` 抑制的错误不计）——这是自动化对拍的主通道。

---

## 4. VimL（legacy vimscript）兼容范围

### 4.1 语法特性分期
| 特性 | 里程碑 | 备注 |
|---|---|---|
| 变量/作用域 g:b:w:t:v:l:a:s: | M3-1 | s: 脚本作用域含 <SID> 编号 |
| 表达式/运算符/弱类型转换 | M3-1 | 对齐 `eval.txt` 全部转换表（含 String→Number 前导数字规则） |
| List/Dict/Float/Blob/JSON | M3-1/3 | 引用语义、保序 dict、自引用环检测 |
| if/while/for/try/catch/throw | M3-2 | 异常值类型与 v:exception/v:throwpoint |
| function/lambda/closure/partial | M3-2 | 可变参数 ...、范围函数（two-line 调用约定） |
| autoload 函数 `foo#bar()` | M3-3 | 与 autoload/ 文件布局联动 |
| :execute/:eval/silent/verbose/lockvar | M3-3 | |
| Funcref/字典函数/方法调用 `list->map()` | M3-3 | 8.2 已有 lambda 风格调用 |
| 内置函数 350+ | M3-3 起持续 | 见 §6 覆盖率表 |
| 调试器 :debug/breakpoint/profile | M3-4 | |
| **vim9script/def/import/class/字节码** | **M8** | 独立子系统，见 ARCH §5.4 |

### 4.2 特殊变量 v: 全集
M3 建立 `v:` 变量表（`v:true false null version errmsg warningmsg shell_error count argc regtype ...`），
其中 `v:version` 报告兼容基线（802），另设 `v:rvimversion` 暴露自身版本；`has('patch-8.2.5172')` 按基线返回 1。

### 4.3 has()/feature 兼容
- 维护 `+feature` 表对齐 `:version` 输出；未实现特性返回 0 并登记。
- 平台名 `win32/win64/unix/mac/linux`、`gui_running/terminal/clipboard/python3_` 等按实际能力如实报告，**不谎称存在**。

---

## 5. Ex 命令 / 选项 / 事件兼容

### 5.1 Ex 命令（基线 589 条）
- 静态表 `ExDef` 全量落库（M2）：含全名、最短缩写、`EX_*` flags（RANGE/BANG/NEEDARG/SBOXOK/MODIFY/RESTRICT...）、ADDR 类型。
- 缩写匹配规则复刻 ex_docmd：按表顺序唯一前缀匹配，歧义报 `E464`；用户自定义命令（`:command`）参与同一命名空间并覆盖内置。
- 守卫语义：sandbox 内仅 `EX_SBOXOK` 可执行；non-modifiable 拒绝 `EX_MODIFY`；restricted(-Z) 拒绝 `EX_RESTRICT`；textlock 检查 `EX_LOCK_OK`。
- 覆盖率分母固定 589，工具自动生成 `docs/coverage/ex_commands.md`。

### 5.2 选项（基线 429 项）
- 全量 `OptDef` 表：长名/短名、类型（bool/num/string/list）、作用域（global/win/buffer）、默认值、vi/vim 来源标记、`'opt'=` 校验与回调。
- 窗口/缓冲局部选项的影子值机制（`setlocal/setglobal` 区别、新建窗口继承规则）按 option.c 复刻。
- `:set` 系列全部形式：`set opt? opt& opt+= opt-= opt^=`、多值列表、逗号列表、`:setglobal/:setlocal`、`:options` 窗口（M5）。
- M1 先实现内核所需子集（`compatible/expandtab/tabstop/shiftwidth/number/ignorecase/fileformat/...`），未入表项 `:set` 时报 `E518` 而非忽略。

### 5.3 autocmd 事件（对齐 autocmd.c event_names）
M3 落全量事件名表（约 80+：BufNewFile/BufReadPre/BufRead/BufEnter/FileType/InsertLeave/TextChanged/TextChangedI/CompleteDone/CmdlineLeave/...），
支持 `augroup/nested/++once/++nested/buffer-local/pattern 通配`；事件触发点在编辑内核的固定埋位（文件打开、写入前后、模式切换、变更后），
保证插件依赖的事件**时序与 Vim 相同**（先 Pre 后 Post、BufEnter 在窗口切换之后等）。

---

## 6. 内置函数与 `:call` 兼容

- 基线：evalfunc.c 约 350+ 函数；M3 起按域分批：字符串 → 集合 → 数学/转换 → 缓冲/窗口/标签 → 搜索/正则 → 路径/glob → 系统/job → JSON/Blob/float → 调试/测试。
- 签名（必选/可选参数个数、类型）按函数表强校验，参数数目错误报 `E119/E118` 同号。
- 测试函数 `test_*()` 仅实现对测试 harness 有价值的子集（如 `test_override` 部分），其余明确报未实现。
- `system()/readfile()/writefile()` 行尾/编码/二进制模式（`'b'` flag）逐一对拍。

---

## 7. 兼容等级定义与验收

| 等级 | 定义 | 目标 |
|---|---|---|
| L0 内核 | headless 编辑/撤销/保存/基础 Ex 正确 | **M1（本次）** |
| L1 脚本 | defaults.vim/filetype.vim 可 source，常用插件（commentary/surround 类）可跑 | M3 |
| L2 交互 | TUI 全模式编辑体验与 vim 一致，常见 runtime 高亮/ftplugin 生效 | M4/M6 |
| L3 生态 | job/channel/terminal/spell/encrypt/tags 齐备，testdir 通过率 ≥ 90% | M7 |
| L4 全兼容 | vim9、剩余边角清零，偏差清单仅含显式不支持项（GUI/语言绑定） | M8 / 1.0 |

验收度量（每次里程碑自动产出）：
1. 三张覆盖率表：Ex x/589、Option x/429、Function x/350+。
2. `src/testdir` 对拍通过率与失败分类。
3. 黄金快照数量与漂移率（漂移必须人工 review 并更新偏差文档）。
4. 数据交叉测试：真实 vim 写 → rvim 读、rvim 写 → vim 读（swap/undo/viminfo/session/spell）。

## 8. 明确不兼容项（前置声明）
- Python/Lua/Perl/Ruby/Tcl/Mzscheme 嵌入接口（`:python` 等）：不实现 ABI 嵌入；引导用户走 JSON-RPC channel（M7），`has('python3')` 如实返回 0。
- 原生 GUI（gtk/motif/haiku/OLE/VisVim/GvimExt）：不移植；TUI 为唯一官方前端，远期再评估。
- 16 位/Amiga/VMS/QNX/Photon 等历史平台：不支持；平台矩阵仅 Windows/Linux/macOS。
- 任何静默行为偏离：禁止。无法实现时抛出对应 E 号错误并在 `docs/compat_deviations.md` 登记。
