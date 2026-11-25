# RustVim 架构设计（ARCH）

> 目标：用 Rust 重写 Vim，行为参照 [vim/vim `v8.2.5172`](https://github.com/vim/vim/tree/v8.2.5172)（Vim 8.2，patch 5172）。
> 设计约束：**编辑器核心 100% Rust 标准库（std-only）**；Windows GUI 平台层使用 `winapi` 0.3 绑定（Direct2D/DirectWrite），其余部分无第三方依赖。
> 平台优先 Windows（当前开发机），同时保持类 Unix 可移植性；通过分层架构保证 **Vim Runtime 兼容**可逐步达成。

---

## 1. 设计原则

| 原则 | 说明 |
|---|---|
| 语义优先，而非字面移植 | 复刻 Vim 的*可观察行为*（编辑语义、Ex 命令、VimL、options、autocmd、寄存器、标记、撤销），而非逐行翻译 C。 |
| 核心与 I/O 分离 | 编辑引擎是纯逻辑库（headless、可单测、确定性），终端/GUI/进程 I/O 都是其上层的薄适配层。 |
| 借用检查器友好 | 文档用 `Rc<RefCell<..>>`/句柄索引代替 C 的全局 `curbuf`/`curwin` 指针网；状态集中在 `Editor`，子系统通过句柄访问。 |
| 兼容性数据驱动 | Ex 命令表、options 表、内置函数表、事件表均为*声明式静态表*，对齐 Vim 的 `ex_cmds.h` / `optiondefs.h` / `evalfunc.c`，便于逐条核对覆盖率。 |
| 可演进 | 先 legacy VimL 解释器（runtime 全部基于它），Vim9 script 编译/字节码作为后期独立子系统；先终端 TUI，GUI/channel/job 后置。 |
| 安全边界 | 沙箱（sandbox）、restricted 模式（`-Z`）、textlock 等 Vim 安全机制建模为显式状态，禁止绕过。 |

---

## 2. 分层架构

依赖方向严格自上而下，**禁止反向依赖与同层环依赖**：

```
┌──────────────────────────────────────────────────────────────┐
│ L5 前端/宿主        bin/rvim (CLI 解析, -e/-es/-c/-u, 退出码)  │
│                     rvim-term (控制台 TUI) · rvim-gui(后置)    │
├──────────────────────────────────────────────────────────────┤
│ L4 应用编排         rvim-app: Editor/会话、启动序列、模式循环、  │
│                     事件循环、作业/通道调度(后置)               │
├──────────────────────────────────────────────────────────────┤
│ L3 语言/命令        rvim-viml (词法/解析/求值/函数/异常)        │
│                     rvim-ex (Ex 命令表+执行) · rvim-normal     │
│                     rvim-map · rvim-autocmd · rvim-cmdline     │
├──────────────────────────────────────────────────────────────┤
│ L2 编辑领域         rvim-buffer · rvim-window · rvim-editor    │
│                     rvim-undo · rvim-register · rvim-mark      │
│                     rvim-search · rvim-regex · rvim-syntax(后置)│
├──────────────────────────────────────────────────────────────┤
│ L1 基础             rvim-core (编码/文本/位置/错误/字符串表)    │
│                     rvim-termio (ansi/console 抽象, std-only)  │
└──────────────────────────────────────────────────────────────┘
```

### 2.1 C 源文件 → Rust crate 映射（参照 v8.2.5172）

| Vim C 模块（src/） | 职责 | RustVim crate |
|---|---|---|
| `charset.c` `mbyte.c` `strings.c` `ascii.h` | 字符/编码/UTF-8/字符串 | **rvim-core**（`encoding`, `charwidth`, `case`） |
| `memline.c` `memfile.c` `buffer.c` | 缓冲行、交换文件、buffer 管理 | **rvim-buffer** |
| `change.c` `ops.c` `edit.c` `indent.c` `textformat.c` | 文本变更/操作符/插入/缩进/排版 | **rvim-buffer** + **rvim-normal** |
| `undo.c` | 撤销树 | **rvim-undo** |
| `register.c` `mark.c` | 寄存器/标记 | **rvim-register** / **rvim-mark**（先并入 rvim-editor） |
| `window.c` `move.c` `textobject.c` | 窗口/移动/文本对象 | **rvim-editor::window**（M5 frame 树）/ **rvim-normal**（含 M5 `visual` + `textobjects`） |
| `normal.c` `nv_cmds.h` `nv_cmdidxs.h` | 普通模式命令分发 | **rvim-normal** |
| `ex_docmd.c` `ex_cmds.c` `ex_cmds.h` `ex_cmdidxs.h` `ex_cmds2.c` | Ex 解析/分发/589 条命令 | **rvim-ex** |
| `ex_getln.c` `cmdexpand.c` `cmdhist.c` | 命令行/补全/历史 | **rvim-cmdline** |
| `eval.c` `evalfunc.c` `evalvars.c` `typval.c` `list.c` `dict.c` `float.c` `blob.c` `json.c` | VimL 求值/类型/内置函数/JSON | **rvim-viml** |
| `ex_eval.c` `scriptfile.c` `userfunc.c` `debugger.c` | 脚本控制流/source/函数/调试 | **rvim-viml** |
| `vim9*.c`（9.2：含 vim9class.c/vim9generics.c） | Vim9 script 编译/执行 | **rvim-viml::vim9**（M9，参考 v9.2.1398；类型化 AST 求值器，含 class/enum/interface/type alias） |
| `option.c` `optionstr.c` `optiondefs.h`（429 项） | options | **rvim-editor::option** |
| `autocmd.c` | 自动命令 | **rvim-autocmd**（先并入 rvim-editor） |
| `map.c` | 映射/缩写 | **rvim-map**（先并入 rvim-editor） |
| `regexp.c` `regexp_bt.c` `regexp_nfa.c` | 回溯/NFA 正则引擎 | **rvim-regex** |
| `search.c` | 搜索/替换 | **rvim-search** |
| `getchar.c` | 输入队列/键码/映射展开 | **rvim-termio::input** + rvim-map |
| `screen.c` `drawline.c` `drawscreen.c` `ui.c` `highlight.c` | 屏幕/绘制/高亮 | **rvim-term**（TUI） |
| `term.c` `termlib.c` `termdefs.h` `libvterm/` | termcap/内置终端 | **rvim-termio** / **rvim-term** |
| `message.c` | 消息/回显/错误 | **rvim-termio::message** + rvim-app |
| `fileio.c` `bufwrite.c` `filepath.c` `findfile.c` | 文件读写/路径/通配 | **rvim-app::fs**（std::fs） |
| `fold.c` `diff.c`(`xdiff/`) `quickfix.c` | 折叠/diff/quickfix | **rvim-editor::fold** / **rvim-diff** + **rvim-editor::diff** / **rvim-ex::errorformat** + **rvim-editor::quickfix**（M5） |
| `sign.c` `textprop.c` `popupwin.c` | 标志/文本属性/弹窗 | **rvim-ex::host_meta** 数据面（M5，TUI 渲染 sign 列与 popup 覆盖层） |
| `syntax.c` `spell*.c` `tag.c` `cindent.c` | 语法/拼写/标签/C 缩进 | 后置（M5-M7） |
| `channel.c` `job.c` `terminal.c` `pty.c` | 通道/作业/终端 | **rvim-ex::m7**（M7：std::process + TCP + mpsc 事件；无 PTY/ConPTY） |
| `os_win32.c` `os_unix.c` | 平台层 | `cfg(windows)`/`cfg(unix)` 条件编译 |
| `main.c` | 入口/参数/启动序列 | **bin/rvim** + rvim-app::startup |
| `if_python*.c` `if_lua.c` `if_perl.xs` `if_ruby.c` `if_mzsch.c` `if_tcl.c` | 脚本语言接口 | **不移植**（ABI/嵌入风险高；以 JSON-RPC channel 替代） |
| `gui_*.c` `GvimExt/` `VisVim/` | 原生 GUI/OLE | 远期或不做（TUI 优先） |
| `crypt*.c` `blowfish.c` `sha256.c` | 加密/哈希 | **rvim-buffer::crypt**（M8，std-only 自研：zip/blowfish/blowfish2 + SHA-256） |
| `xxd/` | 十六进制工具 | 独立小工具 `rxxd`（可选） |

---

## 3. Cargo Workspace 划分

```text
RustVim6/
├─ Cargo.toml                # [workspace] resolver=2, members=crates/*, bin/*
├─ bin/
│  └─ rvim/                  # 可执行入口（CLI、启动序列、退出码）
├─ crates/
│  ├─ rvim-core/             # L1 无依赖基础库（编码、位置、错误、字符）
│  ├─ rvim-buffer/           # L2 文档模型：Buffer/Line/变更/文件格式
│  ├─ rvim-undo/             # L2 撤销树
│  ├─ rvim-diff/            # L2 行级 diff（对齐 src/xdiff，M5）
│  ├─ rvim-syntax/          # L2 语法高亮引擎：keyword/match/region、cluster、内置 20 种文件类型（M6）
│  ├─ rvim-editor/           # L2/L4 聚合：Editor/Window/Option/Register/Mark/Mode
│  ├─ rvim-regex/            # L2 正则（先字面量子集，后 BT/NFA）
│  ├─ rvim-search/           # L2 搜索替换
│  ├─ rvim-ex/               # L3 Ex 命令声明表（589 条，生成自 ex_cmds.h）+ 解析 + 执行
│  ├─ rvim-normal/           # L3 普通模式操作符/动作/文本对象
│  ├─ rvim-cmdline/          # L3 命令行模型：历史/补全/wildmenu（headless）
│  ├─ rvim-viml/             # L3 VimL：词法/表达式 AST+求值/作用域/纯内置函数/正则 MVP
│  ├─ rvim-termio/           # L1 终端后端：raw 模式/窗口尺寸/字节输入（唯一 unsafe 例外）
│  ├─ rvim-term/             # L5 屏幕层：Grid/Style/渲染布局/ANSI 绘制 + 键码解码
│  └─ rvim-app/              # L4 会话编排、启动 source 顺序、事件循环
├─ crates/rvim-tools/        # 仓库维护工具：gen-cmds / coverage / freq（std-only）
├─ runtime/                  # 兼容 Vim 官方 runtime（vendored，见 RUNTIME_COMPAT）
├─ tests/                    # 黄金/特性/兼容性测试
│  ├─ golden/                # 输入脚本 → 期望缓冲区/消息
│  └─ vim_compat/            # 对照真实 vim 的用例（CI 有 vim 时）
└─ docs/                     # ARCH.md / PLAN.md / RUNTIME_COMPAT.md
```

### 3.1 crate 依赖图（无环）

```
rvim-core ◄── rvim-buffer ◄── rvim-undo
     ▲              ▲
     └── rvim-diff ─┘（纯函数，无依赖，仅用 std）
        rvim-syntax ◄── rvim-viml（复用 MVP 正则）
     ▲              ▲
     ├──────────────┴── rvim-editor ◄── rvim-ex ◄── rvim-normal
     │                     ▲  ▲  ▲         ▲
     ├─ rvim-regex ◄────────┘  │  └─────────┤
     ├─ rvim-search ───────────┘            │
     └─ rvim-termio            rvim-viml ────┘（viml 依赖 core/buffer 做 typval 与 getbufline）
rvim-app 依赖以上全部领域 crate；rvim-term 依赖 termio+app；bin/rvim 依赖 app+term。
rvim-editor 依赖 rvim-syntax（每缓冲的语法状态与缓存），rvim-ex 通过它实现 `:syntax`/`:highlight`。
```

> 渐进落地：M1 阶段把 register/mark/map/autocmd/option 先放在 `rvim-editor` 内模块化，
> 待接口稳定后再独立成 crate，避免过早拆分导致的接口返工。

---

## 4. 核心数据模型

### 4.1 文本与位置（rvim-core / rvim-buffer）

```rust
/// 1. 行内字符偏移（UTF-8 字节偏移，对齐 Vim 的 col 语义；'' 等逐字处理）
pub type Col = usize;
/// 1. 缓冲区行号（Vim 语义为 1-based；内部存储 0-based，边界处显式转换）
pub type Lineno = usize;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Pos { pub line: Lineno, pub col: Col, pub off: u32 } // off 预留虚拟列

/// 一行文本：Rc<str> 写时复制，贴近 Vim memline 的块复用。行尾换行标志
/// 不在行内保存，由缓冲的 `final_newline` + 逐行 join 表示。
pub struct Line { text: Rc<str> }

/// 文档缓冲（对齐 struct bufdef）。字段私有，经 Editor/Buffer 方法访问。
pub struct Buffer {
    pub id: BufferId,             // 对应 b:bufnr()
    pub name: Option<PathBuf>,    // 对应 b:name / expand('%')
    lines: Vec<Line>,             // 逻辑行；最后一行可无换行
    changed: bool,                // '&modified'
    changedtick: u64,             // b:changedtick，每次变更 +1
    fileformat: FileFormat,       // unix/dos/mac（写盘时使用）
    final_newline: bool,          // 文件末尾是否有换行（'fixeol' 语义）
    readonly: bool,               // 只读标记
    pub last_cursor: Pos,
    // 局部选项/标记/撤销目前由 rvim-editor 侧（Options/MarkStore/UndoHistory）持有
}
```

设计要点：
- **Vim 行/列语义**：行号 1-based 对外、0-based 对内；`Pos.col` 以**字节偏移**存储（与 Vim 的 `col` 一致），插入/删除按 UTF-8 字符边界推进。宽字符/组合字符的显示宽度在终端层（`rvim-term`）处理；`rvim-core::charclass` 只做字符分类。
- **不可变行 + CoW**：`Rc<str>` 让整行复制/单步撤销 O(1) 行共享，模拟 Vim memline 块缓存，避免 `String` 全量拷贝。
- **changedtick**：每次变更/撤销/重做单调递增，经 `Editor` 暴露给 VimL `b:changedtick`（插件以它作缓存键）。

### 4.2 编辑器聚合状态（rvim-editor）

```rust
pub struct Editor {
    pub buffers: Slab<Buffer>,        // 句柄化缓冲池（buflisted/bufhidden 语义）
    pub windows: Windows,             // 窗口/标签页/frame 树（M5）
    pub current_buf: BufferId,        // 由 current_win.bufnr 派生
    pub current_win: WindowId,        // 见 window.rs（>= 1000）
    pub mode: Mode,                   // Normal/Insert/Visual/Cmdline/Select/OperatorPending/...
    pub options: GlobalOptions,       // 全局选项，对齐 429 项 optiondefs 表
    pub registers: RegisterStore,     // "0-"9, a-z, "", -, ., :, %, #, =, /, _
    pub global_marks: MarkStore,      // A-Z, 0-9
    pub maps: MapStore,               // 各 mode 的映射表
    pub abbreviations: AbbrevStore,
    pub autocmds: AutocmdStore,       // augroup + event -> Vec<AutoCmd>
    pub vars: Dict,                   // g:
    pub vvars: Dict,                  // v:（vim 内置只读/特殊变量）
    pub messages: MessageLog,         // :messages 历史
    pub cmdhist: CmdHistory,          // 命令行/搜索历史
    pub last_substitute: SubState,    // :s 状态，'~' 与 & 模式
    pub search_pattern: String,       // / 寄存器
    pub sandbox_depth: u32,           // sandbox 嵌套
    pub textlock: bool,
    pub runtimepath: Vec<PathBuf>,    // 'runtimepath'
    pub cwd: PathBuf,
}
```

### 4.3 模式状态机

```
        i/a/A/o/O/I/c/s/C ...            Esc/C-c
Normal ───────────────────────► Insert/Replace ──────────► Normal
  │  v/V/C-v                         Esc
  ├──────────────► Visual/Select ◄──────────────┐
  │  : / ?                         Esc/Enter
  └──────────────► Cmdline ─────────────────────┘
  │  d/c/y/... + motion
  └──────────────► OperatorPending（等待 motion/textobject）─► Normal
```
模式枚举带 pending 操作符状态（`PendingOp { op: Operator, count: u64, reg: RegisterName }`），
对齐 `normal.c` 的 `oap`（operator-action-pending）结构。

### 4.4 VimL 类型系统（rvim-viml，对齐 typval.h）

```rust
pub enum Value {
    Number(i64),
    Float(f64),
    String(Rc<str>),
    List(Rc<RefCell<Vec<Value>>>),     // 可变、有身份（same-cycle 检测）
    Dict(Rc<RefCell<IndexMap<String, Value>>>), // 保序哈希表（Vim dict 保序）
    Funcref(FuncRef),                  // 含闭包/partial 上下文与 sctx
    Blob(Vec<u8>),
    Bool(bool),                        // v:true/v:false/v:null 特殊值
    Null,
}
```
- 引用语义与自引用环检测（`test_refcount`/JSON 循环报错）按 Vim 规则实现。
- 数值提升规则（Number/Float 算术、字符串→数字弱转）集中在 `coerce` 模块，配全表单测。

### 4.5 声明式命令/选项表（兼容性骨架）

```rust
// 对齐 EXCMD() 宏：名字、标志位（EX_RANGE/EX_BANG/...）、地址类型、处理函数
pub struct ExDef {
    pub id: ExId,
    pub name: &'static str,
    pub aliases: &'static [&'static str],   // 最短合法缩写（w/wri/writ）
    pub flags: ExFlags,                     // bitflags，逐位对应 ex_cmds.h
    pub addr: AddrType,                     // ADDR_LINES/ADDR_BUFFERS/...
    pub handler: Handler,
}

// 对齐 struct vimoption：长名/短名/类型/作用域/默认值/校验
pub struct OptDef {
    pub name: &'static str, pub short: &'static str,
    pub kind: OptKind,          // Bool/Num/String/List/...
    pub scope: OptScope,        // global/local-window/local-buffer
    pub default: DefaultValue,
    pub flags: OptFlags,        // P_VI/P_VIM/P_VIMD（vi 兼容/vim 扩展）...
}
```
M2 已落地**完整 589 条声明表**：`crates/rvim-ex/src/cmd_table.rs` 由 `reference/vim/src/ex_cmds.h` 生成
（保留 vim 的*最短可用缩写*表序），`ExFlags`/`AddrKind` 逐位对齐 `EX_*` 与 `cmd_addr_T`；
已实现子集由 `crates/rvim-ex/src/exec.rs` 的 `IMPLEMENTED` 清单声明，覆盖率由
`cargo run -p rvim-tools -- coverage` 从代码生成，避免口径漂移。

---

## 5. 关键子系统设计

### 5.1 撤销（rvim-undo，对齐 undo.c）
- 以 **UndoTree**（分支树，非单链）存储 `UndoEntry { before: Vec<LineChange>, after: ..., time, pos, seq }`。
- 变更以行级 diff + 行内片段表达；`u`/`CTRL-R` 走树导航，`g-`/`g+` 按时间线遍历，预留 undofile 持久化格式。
- 插入期按可撤销序列（`undolevels`、键入停顿不切断——以命令边界切段，对齐 Vim 的 `u_save` 触发点）。

### 5.2 正则与搜索（rvim-regex / rvim-search）
- 分阶段：① 字面量 + `.``*``+``?``^$``[]``\<\>` ② 分组/反向引用/贪婪非贪婪 ③ NFA 引擎与 `\v\V\m\M\C\c` 魔法级别 ④ `\zs\ze\@!` 等零宽断言。
- API 保持与 Vim 正则语义一致（最左最长、换行处理、`\_.`），为 `:s`、`search()`、语法高亮共用。

### 5.3 Ex 执行管线（rvim-ex，对齐 ex_docmd.c）
```
原始行 → 去注释/拆 '|'（尊重字符串/bar 标志）→ 逐命令：
  范围解析(. , % , +N , /pat/ , 标记) → 命令名(缩写匹配) → [!] [count] [register] [args]
  → 沙箱/modifiable/restricted/textlock 守卫 → handler(ea: ExArgs) → 消息/错误
```
错误用 `Result<(), VimError>`，错误码对齐 `errors.h`（`E488` 等），消息文本进兼容测试。

### 5.4 VimL 解释器（rvim-viml）
- 流水线：`lexer → stmt parser（递归下降）→ AST → tree-walk evaluator`；表达式 Pratt parser。
- 作用域链 `g:/b:/w:/t:/v:/s:/a:/l:`、`let/unlet/if/while/for/try/catch/function/lambda/closure/echo/execute`。
- 内置函数表对齐 `evalfunc.c`（350+），按功能域分批实现，未实现项抛 `E883` 并登记覆盖率。
- 命令式 `:call` 与表达式 `func()` 共用同一求值核；`silent!/verbose` 修饰在调用栈帧上标记。
- **Vim9 script（M9 独立模块）**：`vim9script` 语境切换 → 词法/逻辑行 → 类型化 AST → 类型检查 → 求值器（`types/lexer/parser/ast/eval/runtime`）。与计划中的字节码 VM 不同，当前交付是结构化 AST 解释执行（偏差见 `compat_deviations.md` §13）；legacy/vim9 互操作经由 `Host` 桥：`call_user_function`、`source_script`、`define_legacy_function`、`echo_message`。

### 5.5 autocmd / map / 启动序列（rvim-app）
- 事件名表对齐 `autocmd.c` 的 `event_names[]`；嵌套深度、`nested`、`++once`、模式匹配（`*`/`?`/`[char]`，复用通配匹配器）。
- 启动顺序严格复刻 `starting.txt`：
  `-u/-U` 解析 → 环境(`SHELL/TERM/VIMINIT`) → 系统 vimrc → 用户 vimrc → plugin/(`load-plugins`) →
  `defaults.vim`（无 vimrc 时）→ packages(`pack/`) → filetype/indent/syntax on → `-c`/缓冲区参数 → BufRead/BufEnter 事件。
- `&compatible` 推断、`skip_defaults_vim`、`--clean`、`-N` 行为全部建模。

### 5.6 终端 I/O 与 TUI 驱动（rvim-termio / rvim-term / rvim-app::tui，std-only）
- 原始模式：Windows 用 `GetStdHandle`/`GetConsoleMode`/`SetConsoleMode` 打开 `ENABLE_VIRTUAL_TERMINAL_INPUT` 与 `ENABLE_VIRTUAL_TERMINAL_PROCESSING`、关掉行输入/回显/processed input；POSIX 用 `stty raw -echo`，退出时恢复（RAII）。拿不到控制台时返回 `Unsupported`，`rvim` 回落 `-es` 批量口径。
- 输入：`stdin` 读线程 + `mpsc` 通道投递字节块，主循环用 120ms `recv_timeout` 轮询控制台尺寸（std 无 select 的替代方案）。字节→键码（CSI/SS3/UTF-8/控制字符；跨读的不完整序列保留，未知 `CSI … ~` 消费为 `CSI-n~`）→映射展开（最长匹配、`timeoutlen` 超时回落字面量）→normal/insert/cmdline 分发。
- 输出：ANSI writer（备用屏 `?1049h/l`、光标定位、SGR、256/真彩）；`Grid` 行级差量刷新（`diff_ansi`），resize 时清屏并按新几何全量重绘。
- 屏幕单元：缓冲区文本先展开成显示单元——<Tab> 按 `'tabstop'` 补齐、控制字符显示为 `^X`（`transchar`）——再按 `left`/`top` 取视口，因此终端收到的永远是可打印字符；光标列同样按显示单元计算，行号栏宽度从可用列数里扣除。
- 消息行与搜索：`:echo` 的瞬时消息（`MessageLog::last_status`）与历史错误同屏显示，放不下时 `-- More --` 分页、任一键翻页；`/` `?` `n` `N` 由 `Tui` 自己实现（字面量子串 + `ignorecase` + 环绕，正则留 M6）。

### 5.8 窗口、标签页与折叠（rvim-editor::window / fold，M5）

窗口模型对齐 `window.c` 的 frame 树，全部是纯逻辑（不碰终端）：

\\\text
TabPage { id, layout: Layout, current, prev }
Layout  = Leaf(WindowId) | Row([Layout])   // :vsplit 并排，中间一列分隔符
                        | Col([Layout])   // :split 上下堆叠，每窗自带状态行
Window  { id (>=1000), tabpage, bufnr, cursor, top_line, left, curswant,
          local: HashMap<name, OptValue>, height, width, rect, quickfix }
\\\

- **布局**：`Windows::layout(rows, cols)` 按每个窗口的 `height/width` 期望值递归分配矩形；富余空间给最后一个子框（`win_equal()` 行为），不足时按比例压缩且不低于 `MIN_WIDTH/MIN_HEIGHT`。并排窗口之间留一列 `|`（VertSplit），堆叠窗口之间靠各自的状态行分隔，因此 `rect.rows` 含状态行。
- **窗口局部选项**：`Options::WINDOW_LOCAL` 声明 `number/relativenumber/list/wrap/scrolloff/foldmethod/foldlevel/foldcolumn/foldenable/diff/signcolumn/...`；`:set` 同时写全局与当前窗口，`:setlocal` 只写窗口，`&opt` 读窗口值（option.c 的 P_GLOBAL/P_LOCAL 语义）。
- **移动**：`<C-w>h/j/k/l` 按矩形几何找邻居（要求垂直/水平方向重叠、按距离取最近），没有邻居报 E11；`<C-w>w/W/t/b/p` 走窗口列表；`<C-w>x/r/H/J/K/L` 交换/轮转窗口内容。
- **折叠**：每个窗口一份折叠列表 `{start, end, level, closed}`，`'foldmethod'` 支持 manual（`zf`）、indent（缩进/ `'shiftwidth'`，`'shiftwidth'=0` 时用 `'tabstop'`）、marker（`{{{ }}}`，含 `{{{1` 层级）、diff（diff 模式自动）；`syntax/expr` 明确报告不支持。折叠行由 TUI 渲染为 `+-- N lines: text`，`'foldcolumn'` 显示层级。
- **diff**：`rvim-diff` 给出最短编辑脚本的变更块，编辑器据此计算每侧的 filler 行与变更行标记，`'foldmethod=diff'` 把连续变更行折起来；`]c`/`[c` 跳转变更，`:diffget`/`:diffput` 拷贝单个变更块。`vimdiff` 由 `rvim -d a b` 启动（每个文件一窗，全部 `:diffthis`）。
- **quickfix/location list**：全局栈最多 10 份列表，`:cexpr`/`:cgetexpr`/`:cfile` 用 `rvim-ex::errorformat` 把文本解析成条目；`:copen` 把列表画成一个 `buftype=quickfix` 的窗口。

### 5.9 语法高亮（rvim-syntax / rvim-editor::syntax，M6）

- **引擎是纯函数**：`Engine::line(&State, &str) -> (Vec<Span>, State)` 返回本行的高亮字节区间与组名，外加要带到下一行的状态（未闭合 region 的栈），对应 `syntax.c` 的 `syn_state`。
- **条目**：`keyword`（整词匹配，大小写由 `syntax case` 决定）、`match`（正则 + `contains`）、`region`（start/end 正则 + `contains`）；`@cluster` 在 `contains` 中展开为组名，`ALL`/`TOP` 表示全部。
- **优先级**：后定义的条目覆盖先定义的（倒序扫描）；span 按「起点升序、同起点长区间在前」排序，渲染时后画的覆盖先画的，所以嵌套项压过容器。
- **跨行**：行尾仍未闭合的 region 继续占用下一行（块注释、多行字符串），`State` 随行缓存；查询某行时从 `SYNC_LINES`（100）行前重启，等价于没有 `syn sync` 时的近似。
- **文件类型**：`rvim-syntax::lang` 内置 20 种定义；`Editor::ensure_syntax()` 先看 `'filetype'`，为空再按文件名后缀（`filetype.vim` 的子集）；结果按 (缓冲区, `b:changedtick`) 缓存，换缓冲/编辑即失效。
- **渲染**：`PaneView.highlights` 按内容行携带 `(start, end, Style)`；`draw_pane()` 先画文本再用 `Grid::set_style()` 覆盖，字节列经 `display_col()` 换算成显示列，因此 Tab 展开、横向滚动、行号/折叠/sign 栏都不受影响。
- **颜色**：`rvim-syntax::highlight` 维护组表与 `:hi link`（默认深色配色 + vim 默认 link 表）；TUI 只把 `HlAttr` 映射成终端 `Style`（含 italic）。

### 5.7 线程模型
- **单主线程 + 事件循环**：所有缓冲/模式状态只在主线程可变，避免重入锁——这与 Vim 单线程模型一致，保证 VimL 语义（`textlock`、定时器回调）简单正确。
- job/channel（M7）：子进程在 std::thread，I/O 字节经**通道**投递回主循环队列（mpsc），主循环在安全点排空，模拟 Vim 的 channel 事件驱动；不暴露跨线程可变句柄。
- 定时器/信号同样入队，不在其他线程触碰 Editor。

---

## 6. 错误处理与可观测性

- 领域错误统一 `VimError { code: &'static str (E***), msg: String, abort: AbortLevel }`；`:` 命令错误默认不杀进程，脚本中可被 `try/catch` 捕获，`throw`/异常值按 VimL 规则流动。
- 日志：自研轻量 `-V{N}{file}`  verbose 追踪（对齐 `-V`），分级记录 source/事件/命令，便于与真实 vim 的 `-V` 日志做差异比对。
- 所有 headless 行为纯函数化：给定「初始缓冲 + 按键/Ex 序列」确定性产出「终态缓冲 + 消息 + 寄存器」，是黄金测试与兼容对照的基础。

---

## 7. 不变量与测试挂钩

| 不变量 | 强制方式 |
|---|---|
| 行号/列偏移始终合法（0 ≤ col ≤ 行字节长，0 ≤ line < 行数） | `Buffer` 方法内断言 + debug_assert 单测 |
| 任意变更后 changedtick 单调递增、可撤销到任意历史序列 | 性质测试（模型对照） |
| Ex/VimL 守卫（sandbox/modifiable/restricted）不可绕过 | 命令表 flags 驱动，统一入口检查 |
| 核心无第三方依赖；Windows GUI 使用 winapi | CI 允许 `winapi` 绑定；核心依赖清单为空的断言测试 |
| 与 Vim 行为偏差可枚举 | `tests/vim_compat` 特性清单 + 覆盖率统计（命令/选项/函数三张表） |

---

## 8. 与参考实现的版本锚点

- 参考 tag：`v8.2.5172`（commit `c19f1a3488c1bb6fe5707dc6338b13cb9f598670`，tree `0faf2a34481cb45910a7a765c24370adc6c05735`）。
- **基线不入库**：`reference/vim`（src + runtime，约 57 MB）被 `.gitignore` 排除，用 `scripts/fetch-reference.ps1` 按上面的 tag/commit 浅克隆并校验 hash；`scripts/ci.ps1` 在缺失时自动拉取，runtime 相关用例与两条 smoke 场景在缺失时自我跳过（打印 `skipped`），`rvim-tools coverage` 直接报错提示先拉取。运行时也可用 `$RVIM_RUNTIME` 指向任意 Vim runtime 代替。
- 量化基线：**589** 条 Ex 命令（`ex_cmds.h` EXCMD）、**429** 个 option（`optiondefs.h` vimoption 表）、**350+** 内置函数（`evalfunc.c`）、runtime 含 **147** 个帮助文档、ftplugin 261、indent 143、compiler 100、keymap 82、autoload 39、colors 21。
- 所有兼容表以该 tag 为唯一对照源；升级基线需整体回归（见 PLAN 的版本策略）。
