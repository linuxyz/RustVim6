//! GENERATED FILE - do not edit by hand.
//!
//! Builtin-function table mirrored from the baseline `reference/vim/src/evalfunc.c`
//! `global_functions[]` (v8.2.5172): the user-callable builtin set with arity
//! bounds. Coverage is measured against this fixed denominator.
//!
//! Regenerate with: `cargo run -p rvim-tools -- gen-funcs`

/// One builtin function declaration.
#[derive(Clone, Copy, Debug)]
pub struct FuncDef {
    pub name: &'static str,
    /// Minimum accepted argument count.
    pub min_args: u32,
    /// Maximum accepted argument count.
    pub max_args: u32,
}

/// Number of builtin functions in the baseline table.
pub const FUNC_COUNT: usize = 543;

pub static FUNCTIONS: [FuncDef; FUNC_COUNT] = [
    FuncDef {
        name: "abs",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "acos",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "add",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "and",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "append",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "appendbufline",
        min_args: 3,
        max_args: 3,
    },
    FuncDef {
        name: "argc",
        min_args: 0,
        max_args: 1,
    },
    FuncDef {
        name: "argidx",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "arglistid",
        min_args: 0,
        max_args: 2,
    },
    FuncDef {
        name: "argv",
        min_args: 0,
        max_args: 2,
    },
    FuncDef {
        name: "asin",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "assert_beeps",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "assert_equal",
        min_args: 2,
        max_args: 3,
    },
    FuncDef {
        name: "assert_equalfile",
        min_args: 2,
        max_args: 3,
    },
    FuncDef {
        name: "assert_exception",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "assert_fails",
        min_args: 1,
        max_args: 5,
    },
    FuncDef {
        name: "assert_false",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "assert_inrange",
        min_args: 3,
        max_args: 4,
    },
    FuncDef {
        name: "assert_match",
        min_args: 2,
        max_args: 3,
    },
    FuncDef {
        name: "assert_nobeep",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "assert_notequal",
        min_args: 2,
        max_args: 3,
    },
    FuncDef {
        name: "assert_notmatch",
        min_args: 2,
        max_args: 3,
    },
    FuncDef {
        name: "assert_report",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "assert_true",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "atan",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "atan2",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "autocmd_add",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "autocmd_delete",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "autocmd_get",
        min_args: 0,
        max_args: 1,
    },
    FuncDef {
        name: "balloon_gettext",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "balloon_show",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "balloon_split",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "blob2list",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "browse",
        min_args: 4,
        max_args: 4,
    },
    FuncDef {
        name: "browsedir",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "bufadd",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "bufexists",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "buffer_exists",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "buffer_name",
        min_args: 0,
        max_args: 1,
    },
    FuncDef {
        name: "buffer_number",
        min_args: 0,
        max_args: 1,
    },
    FuncDef {
        name: "buflisted",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "bufload",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "bufloaded",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "bufname",
        min_args: 0,
        max_args: 1,
    },
    FuncDef {
        name: "bufnr",
        min_args: 0,
        max_args: 2,
    },
    FuncDef {
        name: "bufwinid",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "bufwinnr",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "byte2line",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "byteidx",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "byteidxcomp",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "call",
        min_args: 2,
        max_args: 3,
    },
    FuncDef {
        name: "ceil",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "ch_canread",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "ch_close",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "ch_close_in",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "ch_evalexpr",
        min_args: 2,
        max_args: 3,
    },
    FuncDef {
        name: "ch_evalraw",
        min_args: 2,
        max_args: 3,
    },
    FuncDef {
        name: "ch_getbufnr",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "ch_getjob",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "ch_info",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "ch_log",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "ch_logfile",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "ch_open",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "ch_read",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "ch_readblob",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "ch_readraw",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "ch_sendexpr",
        min_args: 2,
        max_args: 3,
    },
    FuncDef {
        name: "ch_sendraw",
        min_args: 2,
        max_args: 3,
    },
    FuncDef {
        name: "ch_setoptions",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "ch_status",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "changenr",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "char2nr",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "charclass",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "charcol",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "charidx",
        min_args: 2,
        max_args: 3,
    },
    FuncDef {
        name: "chdir",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "cindent",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "clearmatches",
        min_args: 0,
        max_args: 1,
    },
    FuncDef {
        name: "col",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "complete",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "complete_add",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "complete_check",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "complete_info",
        min_args: 0,
        max_args: 1,
    },
    FuncDef {
        name: "confirm",
        min_args: 1,
        max_args: 4,
    },
    FuncDef {
        name: "copy",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "cos",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "cosh",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "count",
        min_args: 2,
        max_args: 4,
    },
    FuncDef {
        name: "cscope_connection",
        min_args: 0,
        max_args: 3,
    },
    FuncDef {
        name: "cursor",
        min_args: 1,
        max_args: 3,
    },
    FuncDef {
        name: "debugbreak",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "deepcopy",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "delete",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "deletebufline",
        min_args: 2,
        max_args: 3,
    },
    FuncDef {
        name: "did_filetype",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "diff_filler",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "diff_hlID",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "digraph_get",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "digraph_getlist",
        min_args: 0,
        max_args: 1,
    },
    FuncDef {
        name: "digraph_set",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "digraph_setlist",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "echoraw",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "empty",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "environ",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "escape",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "eval",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "eventhandler",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "executable",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "execute",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "exepath",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "exists",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "exists_compiled",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "exp",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "expand",
        min_args: 1,
        max_args: 3,
    },
    FuncDef {
        name: "expandcmd",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "extend",
        min_args: 2,
        max_args: 3,
    },
    FuncDef {
        name: "extendnew",
        min_args: 2,
        max_args: 3,
    },
    FuncDef {
        name: "feedkeys",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "file_readable",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "filereadable",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "filewritable",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "filter",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "finddir",
        min_args: 1,
        max_args: 3,
    },
    FuncDef {
        name: "findfile",
        min_args: 1,
        max_args: 3,
    },
    FuncDef {
        name: "flatten",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "flattennew",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "float2nr",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "floor",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "fmod",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "fnameescape",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "fnamemodify",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "foldclosed",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "foldclosedend",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "foldlevel",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "foldtext",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "foldtextresult",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "foreground",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "fullcommand",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "funcref",
        min_args: 1,
        max_args: 3,
    },
    FuncDef {
        name: "function",
        min_args: 1,
        max_args: 3,
    },
    FuncDef {
        name: "garbagecollect",
        min_args: 0,
        max_args: 1,
    },
    FuncDef {
        name: "get",
        min_args: 2,
        max_args: 3,
    },
    FuncDef {
        name: "getbufinfo",
        min_args: 0,
        max_args: 1,
    },
    FuncDef {
        name: "getbufline",
        min_args: 2,
        max_args: 3,
    },
    FuncDef {
        name: "getbufvar",
        min_args: 2,
        max_args: 3,
    },
    FuncDef {
        name: "getchangelist",
        min_args: 0,
        max_args: 1,
    },
    FuncDef {
        name: "getchar",
        min_args: 0,
        max_args: 1,
    },
    FuncDef {
        name: "getcharmod",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "getcharpos",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "getcharsearch",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "getcharstr",
        min_args: 0,
        max_args: 1,
    },
    FuncDef {
        name: "getcmdcompltype",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "getcmdline",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "getcmdpos",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "getcmdscreenpos",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "getcmdtype",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "getcmdwintype",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "getcompletion",
        min_args: 2,
        max_args: 3,
    },
    FuncDef {
        name: "getcurpos",
        min_args: 0,
        max_args: 1,
    },
    FuncDef {
        name: "getcursorcharpos",
        min_args: 0,
        max_args: 1,
    },
    FuncDef {
        name: "getcwd",
        min_args: 0,
        max_args: 2,
    },
    FuncDef {
        name: "getenv",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "getfontname",
        min_args: 0,
        max_args: 1,
    },
    FuncDef {
        name: "getfperm",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "getfsize",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "getftime",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "getftype",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "getimstatus",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "getjumplist",
        min_args: 0,
        max_args: 2,
    },
    FuncDef {
        name: "getline",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "getloclist",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "getmarklist",
        min_args: 0,
        max_args: 1,
    },
    FuncDef {
        name: "getmatches",
        min_args: 0,
        max_args: 1,
    },
    FuncDef {
        name: "getmousepos",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "getpid",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "getpos",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "getqflist",
        min_args: 0,
        max_args: 1,
    },
    FuncDef {
        name: "getreg",
        min_args: 0,
        max_args: 3,
    },
    FuncDef {
        name: "getreginfo",
        min_args: 0,
        max_args: 1,
    },
    FuncDef {
        name: "getregtype",
        min_args: 0,
        max_args: 1,
    },
    FuncDef {
        name: "gettabinfo",
        min_args: 0,
        max_args: 1,
    },
    FuncDef {
        name: "gettabvar",
        min_args: 2,
        max_args: 3,
    },
    FuncDef {
        name: "gettabwinvar",
        min_args: 3,
        max_args: 4,
    },
    FuncDef {
        name: "gettagstack",
        min_args: 0,
        max_args: 1,
    },
    FuncDef {
        name: "gettext",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "getwininfo",
        min_args: 0,
        max_args: 1,
    },
    FuncDef {
        name: "getwinpos",
        min_args: 0,
        max_args: 1,
    },
    FuncDef {
        name: "getwinposx",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "getwinposy",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "getwinvar",
        min_args: 2,
        max_args: 3,
    },
    FuncDef {
        name: "glob",
        min_args: 1,
        max_args: 4,
    },
    FuncDef {
        name: "glob2regpat",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "globpath",
        min_args: 2,
        max_args: 5,
    },
    FuncDef {
        name: "has",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "has_key",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "haslocaldir",
        min_args: 0,
        max_args: 2,
    },
    FuncDef {
        name: "hasmapto",
        min_args: 1,
        max_args: 3,
    },
    FuncDef {
        name: "highlightID",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "highlight_exists",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "histadd",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "histdel",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "histget",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "histnr",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "hlID",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "hlexists",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "hlget",
        min_args: 0,
        max_args: 2,
    },
    FuncDef {
        name: "hlset",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "hostname",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "iconv",
        min_args: 3,
        max_args: 3,
    },
    FuncDef {
        name: "indent",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "index",
        min_args: 2,
        max_args: 4,
    },
    FuncDef {
        name: "input",
        min_args: 1,
        max_args: 3,
    },
    FuncDef {
        name: "inputdialog",
        min_args: 1,
        max_args: 3,
    },
    FuncDef {
        name: "inputlist",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "inputrestore",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "inputsave",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "inputsecret",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "insert",
        min_args: 2,
        max_args: 3,
    },
    FuncDef {
        name: "interrupt",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "invert",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "isabsolutepath",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "isdirectory",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "isinf",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "islocked",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "isnan",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "items",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "job_getchannel",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "job_info",
        min_args: 0,
        max_args: 1,
    },
    FuncDef {
        name: "job_setoptions",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "job_start",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "job_status",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "job_stop",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "join",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "js_decode",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "js_encode",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "json_decode",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "json_encode",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "keys",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "last_buffer_nr",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "len",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "libcall",
        min_args: 3,
        max_args: 3,
    },
    FuncDef {
        name: "libcallnr",
        min_args: 3,
        max_args: 3,
    },
    FuncDef {
        name: "line",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "line2byte",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "lispindent",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "list2blob",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "list2str",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "listener_add",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "listener_flush",
        min_args: 0,
        max_args: 1,
    },
    FuncDef {
        name: "listener_remove",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "localtime",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "log",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "log10",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "luaeval",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "map",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "maparg",
        min_args: 1,
        max_args: 4,
    },
    FuncDef {
        name: "mapcheck",
        min_args: 1,
        max_args: 3,
    },
    FuncDef {
        name: "maplist",
        min_args: 0,
        max_args: 1,
    },
    FuncDef {
        name: "mapnew",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "mapset",
        min_args: 1,
        max_args: 3,
    },
    FuncDef {
        name: "match",
        min_args: 2,
        max_args: 4,
    },
    FuncDef {
        name: "matchadd",
        min_args: 2,
        max_args: 5,
    },
    FuncDef {
        name: "matchaddpos",
        min_args: 2,
        max_args: 5,
    },
    FuncDef {
        name: "matcharg",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "matchdelete",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "matchend",
        min_args: 2,
        max_args: 4,
    },
    FuncDef {
        name: "matchfuzzy",
        min_args: 2,
        max_args: 3,
    },
    FuncDef {
        name: "matchfuzzypos",
        min_args: 2,
        max_args: 3,
    },
    FuncDef {
        name: "matchlist",
        min_args: 2,
        max_args: 4,
    },
    FuncDef {
        name: "matchstr",
        min_args: 2,
        max_args: 4,
    },
    FuncDef {
        name: "matchstrpos",
        min_args: 2,
        max_args: 4,
    },
    FuncDef {
        name: "max",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "menu_info",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "min",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "mkdir",
        min_args: 1,
        max_args: 3,
    },
    FuncDef {
        name: "mode",
        min_args: 0,
        max_args: 1,
    },
    FuncDef {
        name: "mzeval",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "nextnonblank",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "nr2char",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "or",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "pathshorten",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "perleval",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "popup_atcursor",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "popup_beval",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "popup_clear",
        min_args: 0,
        max_args: 1,
    },
    FuncDef {
        name: "popup_close",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "popup_create",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "popup_dialog",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "popup_filter_menu",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "popup_filter_yesno",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "popup_findinfo",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "popup_findpreview",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "popup_getoptions",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "popup_getpos",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "popup_hide",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "popup_list",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "popup_locate",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "popup_menu",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "popup_move",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "popup_notification",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "popup_setoptions",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "popup_settext",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "popup_show",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "pow",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "prevnonblank",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "printf",
        min_args: 1,
        max_args: 19,
    },
    FuncDef {
        name: "prompt_getprompt",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "prompt_setcallback",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "prompt_setinterrupt",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "prompt_setprompt",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "prop_add",
        min_args: 3,
        max_args: 3,
    },
    FuncDef {
        name: "prop_add_list",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "prop_clear",
        min_args: 1,
        max_args: 3,
    },
    FuncDef {
        name: "prop_find",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "prop_list",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "prop_remove",
        min_args: 1,
        max_args: 3,
    },
    FuncDef {
        name: "prop_type_add",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "prop_type_change",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "prop_type_delete",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "prop_type_get",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "prop_type_list",
        min_args: 0,
        max_args: 1,
    },
    FuncDef {
        name: "pum_getpos",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "pumvisible",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "py3eval",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "pyeval",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "pyxeval",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "rand",
        min_args: 0,
        max_args: 1,
    },
    FuncDef {
        name: "range",
        min_args: 1,
        max_args: 3,
    },
    FuncDef {
        name: "readblob",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "readdir",
        min_args: 1,
        max_args: 3,
    },
    FuncDef {
        name: "readdirex",
        min_args: 1,
        max_args: 3,
    },
    FuncDef {
        name: "readfile",
        min_args: 1,
        max_args: 3,
    },
    FuncDef {
        name: "reduce",
        min_args: 2,
        max_args: 3,
    },
    FuncDef {
        name: "reg_executing",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "reg_recording",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "reltime",
        min_args: 0,
        max_args: 2,
    },
    FuncDef {
        name: "reltimefloat",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "reltimestr",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "remote_expr",
        min_args: 2,
        max_args: 4,
    },
    FuncDef {
        name: "remote_foreground",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "remote_peek",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "remote_read",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "remote_send",
        min_args: 2,
        max_args: 3,
    },
    FuncDef {
        name: "remote_startserver",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "remove",
        min_args: 2,
        max_args: 3,
    },
    FuncDef {
        name: "rename",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "repeat",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "resolve",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "reverse",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "round",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "rubyeval",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "screenattr",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "screenchar",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "screenchars",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "screencol",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "screenpos",
        min_args: 3,
        max_args: 3,
    },
    FuncDef {
        name: "screenrow",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "screenstring",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "search",
        min_args: 1,
        max_args: 5,
    },
    FuncDef {
        name: "searchcount",
        min_args: 0,
        max_args: 1,
    },
    FuncDef {
        name: "searchdecl",
        min_args: 1,
        max_args: 3,
    },
    FuncDef {
        name: "searchpair",
        min_args: 3,
        max_args: 7,
    },
    FuncDef {
        name: "searchpairpos",
        min_args: 3,
        max_args: 7,
    },
    FuncDef {
        name: "searchpos",
        min_args: 1,
        max_args: 5,
    },
    FuncDef {
        name: "server2client",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "serverlist",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "setbufline",
        min_args: 3,
        max_args: 3,
    },
    FuncDef {
        name: "setbufvar",
        min_args: 3,
        max_args: 3,
    },
    FuncDef {
        name: "setcellwidths",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "setcharpos",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "setcharsearch",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "setcmdpos",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "setcursorcharpos",
        min_args: 1,
        max_args: 3,
    },
    FuncDef {
        name: "setenv",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "setfperm",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "setline",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "setloclist",
        min_args: 2,
        max_args: 4,
    },
    FuncDef {
        name: "setmatches",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "setpos",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "setqflist",
        min_args: 1,
        max_args: 3,
    },
    FuncDef {
        name: "setreg",
        min_args: 2,
        max_args: 3,
    },
    FuncDef {
        name: "settabvar",
        min_args: 3,
        max_args: 3,
    },
    FuncDef {
        name: "settabwinvar",
        min_args: 4,
        max_args: 4,
    },
    FuncDef {
        name: "settagstack",
        min_args: 2,
        max_args: 3,
    },
    FuncDef {
        name: "setwinvar",
        min_args: 3,
        max_args: 3,
    },
    FuncDef {
        name: "sha256",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "shellescape",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "shiftwidth",
        min_args: 0,
        max_args: 1,
    },
    FuncDef {
        name: "sign_define",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "sign_getdefined",
        min_args: 0,
        max_args: 1,
    },
    FuncDef {
        name: "sign_getplaced",
        min_args: 0,
        max_args: 2,
    },
    FuncDef {
        name: "sign_jump",
        min_args: 3,
        max_args: 3,
    },
    FuncDef {
        name: "sign_place",
        min_args: 4,
        max_args: 5,
    },
    FuncDef {
        name: "sign_placelist",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "sign_undefine",
        min_args: 0,
        max_args: 1,
    },
    FuncDef {
        name: "sign_unplace",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "sign_unplacelist",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "simplify",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "sin",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "sinh",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "slice",
        min_args: 2,
        max_args: 3,
    },
    FuncDef {
        name: "sort",
        min_args: 1,
        max_args: 3,
    },
    FuncDef {
        name: "sound_clear",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "sound_playevent",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "sound_playfile",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "sound_stop",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "soundfold",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "spellbadword",
        min_args: 0,
        max_args: 1,
    },
    FuncDef {
        name: "spellsuggest",
        min_args: 1,
        max_args: 3,
    },
    FuncDef {
        name: "split",
        min_args: 1,
        max_args: 3,
    },
    FuncDef {
        name: "sqrt",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "srand",
        min_args: 0,
        max_args: 1,
    },
    FuncDef {
        name: "state",
        min_args: 0,
        max_args: 1,
    },
    FuncDef {
        name: "str2float",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "str2list",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "str2nr",
        min_args: 1,
        max_args: 3,
    },
    FuncDef {
        name: "strcharlen",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "strcharpart",
        min_args: 2,
        max_args: 4,
    },
    FuncDef {
        name: "strchars",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "strdisplaywidth",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "strftime",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "strgetchar",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "stridx",
        min_args: 2,
        max_args: 3,
    },
    FuncDef {
        name: "string",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "strlen",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "strpart",
        min_args: 2,
        max_args: 4,
    },
    FuncDef {
        name: "strptime",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "strridx",
        min_args: 2,
        max_args: 3,
    },
    FuncDef {
        name: "strtrans",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "strwidth",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "submatch",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "substitute",
        min_args: 4,
        max_args: 4,
    },
    FuncDef {
        name: "swapinfo",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "swapname",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "synID",
        min_args: 3,
        max_args: 3,
    },
    FuncDef {
        name: "synIDattr",
        min_args: 2,
        max_args: 3,
    },
    FuncDef {
        name: "synIDtrans",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "synconcealed",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "synstack",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "system",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "systemlist",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "tabpagebuflist",
        min_args: 0,
        max_args: 1,
    },
    FuncDef {
        name: "tabpagenr",
        min_args: 0,
        max_args: 1,
    },
    FuncDef {
        name: "tabpagewinnr",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "tagfiles",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "taglist",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "tan",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "tanh",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "tempname",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "term_dumpdiff",
        min_args: 2,
        max_args: 3,
    },
    FuncDef {
        name: "term_dumpload",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "term_dumpwrite",
        min_args: 2,
        max_args: 3,
    },
    FuncDef {
        name: "term_getaltscreen",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "term_getansicolors",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "term_getattr",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "term_getcursor",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "term_getjob",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "term_getline",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "term_getscrolled",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "term_getsize",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "term_getstatus",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "term_gettitle",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "term_gettty",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "term_list",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "term_scrape",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "term_sendkeys",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "term_setansicolors",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "term_setapi",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "term_setkill",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "term_setrestore",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "term_setsize",
        min_args: 3,
        max_args: 3,
    },
    FuncDef {
        name: "term_start",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "term_wait",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "terminalprops",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "test_alloc_fail",
        min_args: 3,
        max_args: 3,
    },
    FuncDef {
        name: "test_autochdir",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "test_feedinput",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "test_garbagecollect_now",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "test_garbagecollect_soon",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "test_getvalue",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "test_gui_event",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "test_ignore_error",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "test_null_blob",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "test_null_channel",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "test_null_dict",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "test_null_function",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "test_null_job",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "test_null_list",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "test_null_partial",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "test_null_string",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "test_option_not_set",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "test_override",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "test_refcount",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "test_setmouse",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "test_settime",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "test_srand_seed",
        min_args: 0,
        max_args: 1,
    },
    FuncDef {
        name: "test_unknown",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "test_void",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "timer_info",
        min_args: 0,
        max_args: 1,
    },
    FuncDef {
        name: "timer_pause",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "timer_start",
        min_args: 2,
        max_args: 3,
    },
    FuncDef {
        name: "timer_stop",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "timer_stopall",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "tolower",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "toupper",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "tr",
        min_args: 3,
        max_args: 3,
    },
    FuncDef {
        name: "trim",
        min_args: 1,
        max_args: 3,
    },
    FuncDef {
        name: "trunc",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "type",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "typename",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "undofile",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "undotree",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "uniq",
        min_args: 1,
        max_args: 3,
    },
    FuncDef {
        name: "values",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "virtcol",
        min_args: 1,
        max_args: 2,
    },
    FuncDef {
        name: "virtcol2col",
        min_args: 3,
        max_args: 3,
    },
    FuncDef {
        name: "visualmode",
        min_args: 0,
        max_args: 1,
    },
    FuncDef {
        name: "wildmenumode",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "win_execute",
        min_args: 2,
        max_args: 3,
    },
    FuncDef {
        name: "win_findbuf",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "win_getid",
        min_args: 0,
        max_args: 2,
    },
    FuncDef {
        name: "win_gettype",
        min_args: 0,
        max_args: 1,
    },
    FuncDef {
        name: "win_gotoid",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "win_id2tabwin",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "win_id2win",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "win_move_separator",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "win_move_statusline",
        min_args: 2,
        max_args: 2,
    },
    FuncDef {
        name: "win_screenpos",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "win_splitmove",
        min_args: 2,
        max_args: 3,
    },
    FuncDef {
        name: "winbufnr",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "wincol",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "windowsversion",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "winheight",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "winlayout",
        min_args: 0,
        max_args: 1,
    },
    FuncDef {
        name: "winline",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "winnr",
        min_args: 0,
        max_args: 1,
    },
    FuncDef {
        name: "winrestcmd",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "winrestview",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "winsaveview",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "winwidth",
        min_args: 1,
        max_args: 1,
    },
    FuncDef {
        name: "wordcount",
        min_args: 0,
        max_args: 0,
    },
    FuncDef {
        name: "writefile",
        min_args: 2,
        max_args: 3,
    },
    FuncDef {
        name: "xor",
        min_args: 2,
        max_args: 2,
    },
];
