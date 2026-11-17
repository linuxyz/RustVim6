# RustVim 架构设计（ARCH）

> 目标：用 Rust 重写 Vim，行为参照 [vim/vim `v8.2.5172`](https://github.com/vim/vim/tree/v8.2.5172)（Vim 8.2，patch 5172）。
> 设计约束：**100% Rust 标准库（std-only），零第三方 crate 依赖**（本机沙箱无法访问 crates.io），保证离线可构建；
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
| `window.c` `move.c` `textobject.c` | 窗口/移动/文本对象 | **rvim-window** / **rvim-normal** |
| `normal.c` `nv_cmds.h` `nv_cmdidxs.h` | 普通模式命令分发 | **rvim-normal** |
| `ex_docmd.c` `ex_cmds.c` `ex_cmds.h` `ex_cmdidxs.h` `ex_cmds2.c` | Ex 解析/分发/589 条命令 | **rvim-ex** |
| `ex_getln.c` `cmdexpand.c` `cmdhist.c` | 命令行/补全/历史 | **rvim-cmdline** |
| `eval.c` `evalfunc.c` `evalvars.c` `typval.c` `list.c` `dict.c` `float.c` `blob.c` `json.c` | VimL 求值/类型/内置函数/JSON | **rvim-viml** |
| `ex_eval.c` `scriptfile.c` `userfunc.c` `debugger.c` | 脚本控制流/source/函数/调试 | **rvim-viml** |
| `vim9*.c`（8 个文件） | Vim9 script 编译/字节码/执行 | **rvim-viml::vim9**（后置） |
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
| `fold.c` `diff.c`(`xdiff/`) `sign.c` `textprop.c` `popupwin.c` `quickfix.c` | 折叠/diff/标志/文本属性/弹窗/qf | 后置 crate（M5+） |
| `syntax.c` `spell*.c` `tag.c` `cindent.c` | 语法/拼写/标签/C 缩进 | 后置（M5-M7） |
| `channel.c` `job.c` `terminal.c` `pty.c` | 通道/作业/终端 | 后置（M7，std::process + 自研协议） |
| `os_win32.c` `os_unix.c` | 平台层 | `cfg(windows)`/`cfg(unix)` 条件编译 |
| `main.c` | 入口/参数/启动序列 | **bin/rvim** + rvim-app::startup |
| `if_python*.c` `if_lua.c` `if_perl.xs` `if_ruby.c` `if_mzsch.c` `if_tcl.c` | 脚本语言接口 | **不移植**（ABI/嵌入风险高；以 JSON-RPC channel 替代） |
| `gui_*.c` `GvimExt/` `VisVim/` | 原生 GUI/OLE | 远期或不做（TUI 优先） |
| `crypt*.c` `blowfish.c` `sha256.c` | 加密/哈希 | std-only 自研（M6） |
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
     ├──────────────┴── rvim-editor ◄── rvim-ex ◄── rvim-normal
     │                     ▲  ▲  ▲         ▲
     ├─ rvim-regex ◄────────┘  │  └─────────┤
     ├─ rvim-search ───────────┘            │
     └─ rvim-termio            rvim-viml ────┘（viml 依赖 core/buffer 做 typval 与 getbufline）
rvim-app 依赖以上全部领域 crate；rvim-term 依赖 termio+app；bin/rvim 依赖 app+term。
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
    pub windows: Vec<Window>,         // 窗口（split/tab 后置；M1 单窗口）
    pub current_buf: BufferId,
    pub current_win: WindowId,
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
- **Vim9 script（后置独立模块）**：`vim9script` 词法语境切换 → 类型检查 → 字节码 → 栈式 VM，文件划分镜像 `vim9compile/vim9execute/vim9instr/vim9type`。legacy 与 vim9 互操作边界严格按官方规则。

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
| 无第三方依赖、可离线 `cargo build` | CI 用 `--offline` 构建；依赖清单为空的断言测试 |
| 与 Vim 行为偏差可枚举 | `tests/vim_compat` 特性清单 + 覆盖率统计（命令/选项/函数三张表） |

---

## 8. 与参考实现的版本锚点

- 参考 tag：`v8.2.5172`（commit `c19f1a3488c1bb6fe5707dc6338b13cb9f598670`，tree `0faf2a34481cb45910a7a765c24370adc6c05735`）。
- **基线不入库**：`reference/vim`（src + runtime，约 57 MB）被 `.gitignore` 排除，用 `scripts/fetch-reference.ps1` 按上面的 tag/commit 浅克隆并校验 hash；`scripts/ci.ps1` 在缺失时自动拉取，runtime 相关用例与两条 smoke 场景在缺失时自我跳过（打印 `skipped`），`rvim-tools coverage` 直接报错提示先拉取。运行时也可用 `$RVIM_RUNTIME` 指向任意 Vim runtime 代替。
- 量化基线：**589** 条 Ex 命令（`ex_cmds.h` EXCMD）、**429** 个 option（`optiondefs.h` vimoption 表）、**350+** 内置函数（`evalfunc.c`）、runtime 含 **147** 个帮助文档、ftplugin 261、indent 143、compiler 100、keymap 82、autoload 39、colors 21。
- 所有兼容表以该 tag 为唯一对照源；升级基线需整体回归（见 PLAN 的版本策略）。
