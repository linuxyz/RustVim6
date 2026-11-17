//! rvim-viml: legacy VimL expression engine.
//!
//! Layer split with the Ex command layer (rvim-ex):
//! * language: lexer → expression AST → evaluator, values, scopes, pure builtins;
//! * Ex layer: statements (`:if`, `:while`, `:let`, `:function`, ...), the
//!   script runner, user functions and host-dependent builtins.
//!
//! The [`env::Host`] trait is the only bridge the language needs: the Ex layer
//! implements it over the editor.

pub mod builtins;
pub mod builtins_ae;
pub mod builtins_assert;
pub mod builtins_fl;
pub mod builtins_misc2;
pub mod builtins_mr;
pub mod builtins_s1;
pub mod builtins_s2;
pub mod env;
pub mod eval;
pub mod expr;
pub mod func_table;
pub mod lexer;
pub mod regex_mvp;
pub mod value;

pub use env::{Env, Frame, Host};
pub use eval::{call_function, call_value, eval};
pub use expr::{Expr, parse_expression};
pub use func_table::{FUNC_COUNT, FUNCTIONS, FuncDef};
pub use value::{FuncRef, Lambda, Value, VimDict};
