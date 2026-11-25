//! Vim9 script engine (reference: vim9script.c, vim9type.c, vim9compile.c,
//! vim9expr.c, vim9execute.c).  M9.
//!
//! The engine is split into a lexer, an expression/statement parser, a type
//! system and a tree-walking evaluator.  Legacy VimL remains in the parent
//! modules; both can call each other through the [Host] bridge.

pub mod ast;
pub mod eval;
pub mod lexer;
pub mod parser;
pub mod runtime;
pub mod types;

pub use ast::{Expr, ImportKind, LambdaDef, Param, Stmt, Vim9Func, Vim9Script};
pub use eval::{
    Flow, call_vim9, call_vim9_lambda, handle_import, run_command_line, run_import_line, run_script,
};
pub use parser::{
    ExprParser, SourceLine, StmtParser, parse_def_header, parse_expression, parse_function,
    parse_script, prepare_lines, split_top_level,
};
pub use runtime::Vim9State;
pub use types::Vim9Type;
