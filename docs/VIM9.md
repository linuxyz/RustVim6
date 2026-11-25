# Vim9 script（M9 / rvim 1.1）

> **参考版本**：Vim 9.2.1398（`https://github.com/vim/vim/tree/v9.2.1398`）。
> 编辑器主体仍以 v8.2.5172 为兼容基线；只有 **Vim9 script** 以 9.2.1398 为准。
> 9.2 基线新增了 class/interface/enum/type alias/generics（`src/vim9class.c`、`src/vim9generics.c`），
> 因此 M9 不再以“8.2 没有 class”为由跳过这些语法。

## 1. 出口状态

| 出口标准 | 结果 |
|---|---|
| Vim 9.2 的 Vim9 autoload 零错误 source | ✅ `runtime/autoload/dist/{ft,script,vim9,vim,json,man}.vim` 与 `runtime/autoload/ccomplete.vim`（9.2 版本）共 7 个可 source（`dist/vimindent.vim` 的 `$'…'` 反斜杠插值仍未通过）；`dist#ft#FTasm()` 等仍可调用 |
| Vim 9.2 的 Vim9 测试子集可跑并公开通过率 | ⚠️ class/enum/interface/typealias/generics 共 322 个 `Test_*`：**26 通过（8.1%）**，123 个执行到结尾；明细见 §6 |
| 发布 `rvim 1.1` | ✅ `Cargo.toml` 1.1.0 |

## 2. 引擎结构（`crates/rvim-viml/src/vim9/`）

| 文件 | 内容 | 对应 9.2 C 源 |
|---|---|---|
| `types.rs` | `Vim9Type`、可赋值性、默认值 | vim9type.c |
| `lexer.rs` | `#` 注释、`#{}`、`$"`/`$'` 插值、`0z`、`->`、`..`、`??`、`=>` | vim9compile.c（内联解析） |
| `parser.rs` | 逻辑行/heredoc/续行、表达式 Pratt 解析、`def/var/class/interface/enum/type/import` 块 | vim9expr.c / vim9compile.c / vim9class.c |
| `ast.rs` | 类型化 AST、`Vim9Func`、`VimClass`、`VimEnum`、`VimTypeAlias` | vim9.h |
| `eval.rs` | 求值器：作用域、调用、对象/方法/`this`、异常、import、`:vim9cmd` | vim9execute.c |
| `runtime.rs` | 函数/类/枚举/类型别名注册表、对象身份表、模块/import 绑定 | vim9script.c |

设计说明：计划中写的是“编译为字节码 + 栈式 VM”。当前交付是**类型检查 + 结构化 AST 求值器**
（没有生成 `vim9instr.c` 风格指令流），语义优先；该差异登记在 `docs/compat_deviations.md` §14。

## 3. 支持范围（相对 9.2.1398）

- `vim9script [noclear]`；脚本级 `var/const/final`、`def/enddef`、`export`、`import`。
- 类型：`any void bool number float string blob list<T> dict<T> func tuple<...>`；参数/返回值/变量声明做（运行期 + 部分静态）检查。
- 类（9.2 核心新增）：`class Name [extends Parent]`、`public var`、`static var`、`var`、`def new()`、
  `def Method()`、`static def`、`this.member` / `this.Method()`、`Class.new()`、`Class.static`，
  以及 `type(obj)`/`typename(obj)` → `t_object`/`object<Name>`、`type(Class)` → `t_class`/`class<Name>`。
- 枚举：`enum Name ... endenum`，`Name.Member` 取整数值，`type/typename` 返回 `t_enum`/`enum<Name>`。
- 接口：`interface ... endinterface` 可解析并注册（含无 body 的方法签名）；`implements` 目前只解析、不做约束检查。
- 类型别名：`type Alias = <type>` 可解析/注册；别名在类型检查中按 `Named` 宽松处理。
- 语句/表达式：`if/while/for/try/catch/finally`、`++/--`、复合赋值、`?`/`??`、`&&/||`、
  `..`、`->`、索引/切片、`{}`/`#{}`（含计算键）、lambda（表达式体）、`$"…{expr}…\"`/`$'…'` 插值。
- 互操作：legacy 脚本中的 `def`、Vim9 调用 legacy/内置函数、legacy 调用 Vim9 全局/autoload 函数、
  Vim9 脚本内嵌 legacy `func` 块、`:vim9cmd`、`:defcompile`、`:def`、`:var/:const/:final`。
- `has('vim9script')`/`has('float')` 为真；`:version` 的 Features 行包含 `+vim9script`。

## 4. 对象模型（实现说明）

对象在运行期是一个普通 `VimDict`（成员变量），其身份由 `Vim9State.objects` 以 `Rc` 地址登记到类；
因此 `VimDict` 的字符串/键/长度不暴露内部标记。方法不存进对象，而是从类定义查找并以 `this` 绑定调用。
这样 legacy 的字典函数仍可用于对象，同时保持成员访问控制（默认只读、`public` 可写、`_` 前缀受保护）。

## 5. Vim 9.2 参考的获取与测试

```powershell
pwsh -File scripts/fetch-vim9-reference.ps1            # fetch + extract v9.2.1398
pwsh -File scripts/vim9_conformance.ps1                # class/enum/interface/typealias/generics
```

参考文件落在 `reference/vim9-v9.2.1398/`（`/reference/` 已被 .gitignore 忽略）。
编辑器主体的 v8.2.5172 基线仍由 `scripts/fetch-reference.ps1` 提供。

## 6. 官方 Vim 9.2 测试通过率

口径：逐个调用 `Test_*`；`exit=0` 记为“执行到结尾”，`exit=0` 且 `v:errors` 为空记为“通过”。

| 文件 | Test_ 函数 | 通过 | 通过率 | 执行到结尾 |
|---|---|---|---|---|
| `test_vim9_class.vim` | 139 | 17 | 12.2% | 59 |
| `test_vim9_enum.vim` | 55 | 2 | 3.6% | 19 |
| `test_vim9_interface.vim` | 21 | 6 | 28.6% | 15 |
| `test_vim9_typealias.vim` | 42 | 1 | 2.4% | 18 |
| `test_vim9_generics.vim` | 65 | 0 | 0% | 12 |
| **合计** | **322** | **26** | **8.1%** | **123** |

失败分类：
1. 9.2 的断言大量要求**逐字错误文本/精确行号**（`assert_fails(..., {lnum})`），本实现未把
   `SOURCING_LNUM` 接进消息发射（`{lnum}` 参数被接受但忽略）。
2. 尚未实现的 9.2 语义：继承/`super`、接口实现约束、泛型函数/类型、`abstract` 方法体区分、
   `null_class`/`test_null_class()`、块 lambda、部分类型推断细节。
3. `test_vim9_expr/func/script/cmd/assign.vim`（9.2 版）仍有少数构造无法完整 source，
   因此当前公开口径只覆盖 9.2 新增的 class/enum/interface/typealias/generics 五个文件。

## 7. 已知限制（登记在 docs/compat_deviations.md §14）

- 没有字节码 VM；没有完整编译期类型检查/泛型实例化。
- 类继承 `extends` 只解析，父类成员/方法不参与查找；`super` 不支持。
- 接口只注册，不做“对象是否实现接口”的检查；`implements` 被解析但忽略。
- 类型别名只注册，不做别名展开后的严格类型检查。
- 闭包捕获是定义时快照（与 legacy lambda 相同），不是 9.2 的共享变量单元。
- 块 lambda `(x) => { ... }` 不支持（仅表达式体）。
- `:defcompile` 为 no-op；`assert_fails()` 的 `{lnum}`/`{context}` 不校验。
- `test_vim9_*` 的其余 5 个 9.2 文件尚不能完整 source（heredoc/块 lambda/嵌入式语言等），故未纳入公开通过率。

## 8. 与旧版（v8.2.5172）M9 的关系

v8.2.5172 的 3 个 Vim9 autoload（`autoload/dist/ft.vim`、`dist/script.vim`、`ccomplete.vim`）
仍然零错误 source，并作为 M9 的回归用例保留；9.2 的 7/8 个 autoload 是新增的参考目标（`dist/vimindent.vim` 待收口）。
…'` 反斜杠插值仍未通过）；`dist#ft#FTasm()` 等仍可调用 |
| Vim 9.2 的 Vim9 测试子集可跑并公开通过率 | ⚠️ class/enum/interface/typealias/generics 共 322 个 `Test_*`：**26 通过（8.1%）**，123 个执行到结尾；明细见 §6 |
| 发布 `rvim 1.1` | ✅ `Cargo.toml` 1.1.0 |

## 2. 引擎结构（`crates/rvim-viml/src/vim9/`）

| 文件 | 内容 | 对应 9.2 C 源 |
|---|---|---|
| `types.rs` | `Vim9Type`、可赋值性、默认值 | vim9type.c |
| `lexer.rs` | `#` 注释、`#{}`、`$"`/`$'` 插值、`0z`、`->`、`..`、`??`、`=>` | vim9compile.c（内联解析） |
| `parser.rs` | 逻辑行/heredoc/续行、表达式 Pratt 解析、`def/var/class/interface/enum/type/import` 块 | vim9expr.c / vim9compile.c / vim9class.c |
| `ast.rs` | 类型化 AST、`Vim9Func`、`VimClass`、`VimEnum`、`VimTypeAlias` | vim9.h |
| `eval.rs` | 求值器：作用域、调用、对象/方法/`this`、异常、import、`:vim9cmd` | vim9execute.c |
| `runtime.rs` | 函数/类/枚举/类型别名注册表、对象身份表、模块/import 绑定 | vim9script.c |

设计说明：计划中写的是“编译为字节码 + 栈式 VM”。当前交付是**类型检查 + 结构化 AST 求值器**
（没有生成 `vim9instr.c` 风格指令流），语义优先；该差异登记在 `docs/compat_deviations.md` §14。

## 3. 支持范围（相对 9.2.1398）

- `vim9script [noclear]`；脚本级 `var/const/final`、`def/enddef`、`export`、`import`。
- 类型：`any void bool number float string blob list<T> dict<T> func tuple<...>`；参数/返回值/变量声明做（运行期 + 部分静态）检查。
- 类（9.2 核心新增）：`class Name [extends Parent]`、`public var`、`static var`、`var`、`def new()`、
  `def Method()`、`static def`、`this.member` / `this.Method()`、`Class.new()`、`Class.static`，
  以及 `type(obj)`/`typename(obj)` → `t_object`/`object<Name>`、`type(Class)` → `t_class`/`class<Name>`。
- 枚举：`enum Name ... endenum`，`Name.Member` 取整数值，`type/typename` 返回 `t_enum`/`enum<Name>`。
- 接口：`interface ... endinterface` 可解析并注册（含无 body 的方法签名）；`implements` 目前只解析、不做约束检查。
- 类型别名：`type Alias = <type>` 可解析/注册；别名在类型检查中按 `Named` 宽松处理。
- 语句/表达式：`if/while/for/try/catch/finally`、`++/--`、复合赋值、`?`/`??`、`&&/||`、
  `..`、`->`、索引/切片、`{}`/`#{}`（含计算键）、lambda（表达式体）、`$"…{expr}…\"`/`$'…'` 插值。
- 互操作：legacy 脚本中的 `def`、Vim9 调用 legacy/内置函数、legacy 调用 Vim9 全局/autoload 函数、
  Vim9 脚本内嵌 legacy `func` 块、`:vim9cmd`、`:defcompile`、`:def`、`:var/:const/:final`。
- `has('vim9script')`/`has('float')` 为真；`:version` 的 Features 行包含 `+vim9script`。

## 4. 对象模型（实现说明）

对象在运行期是一个普通 `VimDict`（成员变量），其身份由 `Vim9State.objects` 以 `Rc` 地址登记到类；
因此 `VimDict` 的字符串/键/长度不暴露内部标记。方法不存进对象，而是从类定义查找并以 `this` 绑定调用。
这样 legacy 的字典函数仍可用于对象，同时保持成员访问控制（默认只读、`public` 可写、`_` 前缀受保护）。

## 5. Vim 9.2 参考的获取与测试

```powershell
pwsh -File scripts/fetch-vim9-reference.ps1            # fetch + extract v9.2.1398
pwsh -File scripts/vim9_conformance.ps1                # class/enum/interface/typealias/generics
```

参考文件落在 `reference/vim9-v9.2.1398/`（`/reference/` 已被 .gitignore 忽略）。
编辑器主体的 v8.2.5172 基线仍由 `scripts/fetch-reference.ps1` 提供。

## 6. 官方 Vim 9.2 测试通过率

口径：逐个调用 `Test_*`；`exit=0` 记为“执行到结尾”，`exit=0` 且 `v:errors` 为空记为“通过”。

| 文件 | Test_ 函数 | 通过 | 通过率 | 执行到结尾 |
|---|---|---|---|---|
| `test_vim9_class.vim` | 139 | 17 | 12.2% | 59 |
| `test_vim9_enum.vim` | 55 | 2 | 3.6% | 19 |
| `test_vim9_interface.vim` | 21 | 6 | 28.6% | 15 |
| `test_vim9_typealias.vim` | 42 | 1 | 2.4% | 18 |
| `test_vim9_generics.vim` | 65 | 0 | 0% | 12 |
| **合计** | **322** | **26** | **8.1%** | **123** |

失败分类：
1. 9.2 的断言大量要求**逐字错误文本/精确行号**（`assert_fails(..., {lnum})`），本实现未把
   `SOURCING_LNUM` 接进消息发射（`{lnum}` 参数被接受但忽略）。
2. 尚未实现的 9.2 语义：继承/`super`、接口实现约束、泛型函数/类型、`abstract` 方法体区分、
   `null_class`/`test_null_class()`、块 lambda、部分类型推断细节。
3. `test_vim9_expr/func/script/cmd/assign.vim`（9.2 版）仍有少数构造无法完整 source，
   因此当前公开口径只覆盖 9.2 新增的 class/enum/interface/typealias/generics 五个文件。

## 7. 已知限制（登记在 docs/compat_deviations.md §14）

- 没有字节码 VM；没有完整编译期类型检查/泛型实例化。
- 类继承 `extends` 只解析，父类成员/方法不参与查找；`super` 不支持。
- 接口只注册，不做“对象是否实现接口”的检查；`implements` 被解析但忽略。
- 类型别名只注册，不做别名展开后的严格类型检查。
- 闭包捕获是定义时快照（与 legacy lambda 相同），不是 9.2 的共享变量单元。
- 块 lambda `(x) => { ... }` 不支持（仅表达式体）。
- `:defcompile` 为 no-op；`assert_fails()` 的 `{lnum}`/`{context}` 不校验。
- `test_vim9_*` 的其余 5 个 9.2 文件尚不能完整 source（heredoc/块 lambda/嵌入式语言等），故未纳入公开通过率。

## 8. 与旧版（v8.2.5172）M9 的关系

v8.2.5172 的 3 个 Vim9 autoload（`autoload/dist/ft.vim`、`dist/script.vim`、`ccomplete.vim`）
仍然零错误 source，并作为 M9 的回归用例保留；9.2 的 8 个 autoload 是新增的参考目标。
