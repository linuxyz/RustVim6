//! Generate `docs/COVERAGE.md`: the three coverage tables required by
//! docs/PLAN.md (Ex commands / options / builtin functions), kept honest by
//! deriving every number from the code instead of typing it by hand.

use std::fmt::Write as _;
use std::path::Path;

use rvim_ex::implemented::IMPLEMENTED;
use rvim_ex::table::{AddrKind, COMMANDS};

use crate::freq::{self, Usage};

fn pct(numerator: usize, denominator: usize) -> String {
    if denominator == 0 {
        "n/a".to_string()
    } else {
        format!("{:.1}%", numerator as f64 * 100.0 / denominator as f64)
    }
}

fn addr_label(addr: AddrKind) -> &'static str {
    match addr {
        AddrKind::Lines => "lines",
        AddrKind::Windows => "windows",
        AddrKind::Arguments => "arguments",
        AddrKind::LoadedBuffers => "loaded-buffers",
        AddrKind::Buffers => "buffers",
        AddrKind::Tabs => "tabs",
        AddrKind::TabsRelative => "tabs-relative",
        AddrKind::QuickfixValid => "quickfix-valid",
        AddrKind::Quickfix => "quickfix",
        AddrKind::Unsigned => "unsigned",
        AddrKind::Other => "other",
        AddrKind::None => "none",
    }
}

/// Render the full coverage document.
pub fn render(usage: &Usage, all_runtime: &Usage) -> String {
    let total = COMMANDS.len();
    let implemented: Vec<&str> = COMMANDS
        .iter()
        .filter(|c| IMPLEMENTED.contains(&c.name))
        .map(|c| c.name)
        .collect();
    let (impl_weight, total_weight) = freq::weighted_rate(usage, IMPLEMENTED);
    let (all_impl_weight, all_total_weight) = freq::weighted_rate(all_runtime, IMPLEMENTED);
    // M2 gate: log-bucketed frequency weighting over editor commands (the M3
    // scripting commands are excluded, see freq::SCRIPTING_M3).
    let editing = |n: &str| !freq::SCRIPTING_M3.contains(&n);
    let (b_weight, b_total) = freq::bucketed_rate(usage, IMPLEMENTED, editing);
    let (breadth_done, breadth_used) = freq::breadth_rate(usage, IMPLEMENTED, editing);

    let mut out = String::new();
    out.push_str(
        "# 覆盖率表（COVERAGE）\n\n\
         > **自动生成，请勿手改**：`cargo run -p rvim-tools -- coverage`。\n\
         > 分母固定为基线 vim/vim `v8.2.5172`：589 条 Ex 命令、429 个 option、350+ 内置函数。\n\
         > 数据源：`crates/rvim-ex/src/cmd_table.rs`（由 `ex_cmds.h` 生成）+ `crates/rvim-ex/src/implemented.rs`（已实现清单）+ `reference/vim/runtime` 运行时脚本实测调用频率。\n\n",
    );

    let _ = writeln!(out, "## 1. Ex 命令（分母 589）\n");
    let _ = writeln!(out, "| 指标 | 数值 |\n|---|---|");
    let _ = writeln!(
        out,
        "| 已实现（声明表口径） | {} / {}（{}） |",
        implemented.len(),
        total,
        pct(implemented.len(), total)
    );
    let _ = writeln!(
        out,
        "| 已实现（**用户默认语料对数桶加权**，M2 出口口径） | {} / {}（**{}**） |",
        b_weight,
        b_total,
        pct(b_weight as usize, b_total as usize)
    );
    let _ = writeln!(
        out,
        "| 广度（语料实际用到的编辑命令中有多少已实现） | {} / {}（{}） |",
        breadth_done,
        breadth_used,
        pct(breadth_done, breadth_used)
    );
    let _ = writeln!(
        out,
        "| 已实现（用户默认语料原始调用次数加权） | {} / {}（{}） |",
        impl_weight,
        total_weight,
        pct(impl_weight as usize, total_weight as usize)
    );
    let _ = writeln!(
        out,
        "| 已实现（全 runtime 语料原始加权，参考） | {} / {}（{}） |",
        all_impl_weight,
        all_total_weight,
        pct(all_impl_weight as usize, all_total_weight as usize)
    );
    let _ = writeln!(
        out,
        "| 加权语料 | {} 个脚本、{} 条调用（全 runtime 语料：{} 个脚本、{} 条） |",
        usage.files, usage.total, all_runtime.files, all_runtime.total
    );
    let _ = writeln!(out, "| 目标（M2 出口） | 对数桶加权 ≥ 60% |\n");
    let _ = writeln!(
        out,
        "> 加权口径：对数桶 `1+floor(log2(n))`，语料为 `defaults.vim` + `plugin/*` 等用户默认脚本；\n\
         > 排除 M3 VimL 解释器类命令（`:call` `:if` `:let` 等，见 `rvim-tools/src/freq.rs` 的 `SCRIPTING_M3`）。\n\
         > 原始调用次数加权与全 runtime 语料数字一并给出，避免口径过于讨巧。\n"
    );

    let _ = writeln!(out, "## 2. 已实现命令（{} 条）\n", implemented.len());
    let _ = writeln!(out, "按命令表顺序：\n");
    let _ = writeln!(out, "```text");
    let mut line = String::new();
    for name in &implemented {
        if line.len() + name.len() + 2 > 76 {
            let _ = writeln!(out, "{}", line.trim_end());
            line.clear();
        }
        line.push(':');
        line.push_str(name);
        line.push(' ');
    }
    if !line.is_empty() {
        let _ = writeln!(out, "{}", line.trim_end());
    }
    let _ = writeln!(out, "```\n");

    let _ = writeln!(out, "## 3. 未实现命令中权重最高的 30 条\n");
    let _ = writeln!(
        out,
        "| rank | 命令 | runtime 调用次数 | 地址类型 |\n|---|---|---|---|"
    );
    let by_weight: Vec<(&str, u64)> = usage.by_weight();
    for (rank, (name, count)) in by_weight
        .iter()
        .filter(|(n, _)| !IMPLEMENTED.contains(n) && !freq::SCRIPTING_M3.contains(n))
        .take(30)
        .enumerate()
    {
        let addr = COMMANDS
            .iter()
            .find(|c| c.name == *name)
            .map(|c| addr_label(c.addr))
            .unwrap_or("?");
        let _ = writeln!(out, "| {} | `:{name}` | {count} | {addr} |", rank + 1);
    }

    let _ = writeln!(out, "\n## 4. 其它兼容表\n");
    let func_total = rvim_viml::FUNC_COUNT;
    let names = implemented_builtins();
    let func_done = names.len();
    let _ = writeln!(
        out,
        "| 类别 | 已实现 | 分母 | 说明 |\n|---|---|---|---|\n\
         | Options | {} | 429 | 全局选项子集（window/buffer 作用域在 M5） |\n\
         | 内置函数 | {} | {} | 纯函数在 `rvim-viml`，宿主相关在 `rvim-ex`；M3 目标 ≥70% |\n",
        rvim_editor::Options::table_size(),
        func_done,
        func_total
    );
    let _ = writeln!(
        out,
        "\n内置函数覆盖率：{}/{}（{}）。\n",
        func_done,
        func_total,
        pct(func_done, func_total)
    );
    out
}

/// Names with a pure or host implementation, matching the coverage count.
pub fn implemented_builtins() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = rvim_viml::builtins::implemented_funcs();
    names.extend_from_slice(rvim_ex::script::HOST_BUILTINS);
    names.extend_from_slice(rvim_ex::host_win::FUNCS);
    names.extend_from_slice(rvim_ex::host_meta::FUNCS);
    names.extend_from_slice(rvim_ex::host_more::FUNCS);
    names.extend_from_slice(rvim_ex::host_extra::FUNCS);
    names.sort_unstable();
    names.dedup();
    names
}

/// Builtins from the 543-entry denominator that have no implementation yet.
pub fn missing_builtins() -> Vec<&'static str> {
    let implemented = implemented_builtins();
    rvim_viml::FUNCTIONS
        .iter()
        .map(|func| func.name)
        .filter(|name| !implemented.contains(name))
        .collect()
}

/// Print the missing builtin names (used by the maintenance CLI).
pub fn report_missing() -> Result<(), String> {
    let missing = missing_builtins();
    println!(
        "missing builtins: {}/{}",
        missing.len(),
        rvim_viml::FUNC_COUNT
    );
    for name in missing {
        println!("{name}");
    }
    Ok(())
}

pub fn run(root: &Path, check: bool) -> Result<(), String> {
    // The coverage corpus is the pinned Vim runtime, which is not versioned
    // (~27 MB). Fail with an actionable message instead of rendering numbers
    // from an empty corpus.
    let runtime = root.join("reference/vim/runtime");
    if !runtime.is_dir() {
        return Err(format!(
            "{} is missing: the Vim baseline is not versioned. Run `pwsh -NoProfile -File scripts/fetch-reference.ps1` first.",
            runtime.display()
        ));
    }
    let user = freq::scan_runtime(root, freq::Corpus::UserDefault)?;
    let all = freq::scan_runtime(root, freq::Corpus::All)?;
    let rendered = render(&user, &all);
    let path = root.join("docs/COVERAGE.md");
    if check {
        let existing = std::fs::read_to_string(&path).unwrap_or_default();
        if existing != rendered {
            return Err(format!(
                "{} is out of date; run `cargo run -p rvim-tools -- coverage`",
                path.display()
            ));
        }
        println!("{} is up to date", path.display());
        return Ok(());
    }
    std::fs::write(&path, rendered).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    println!("wrote {}", path.display());
    Ok(())
}
