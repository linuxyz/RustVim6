//! Vim9 compiled program representation.
//!
//! A script is parsed and type-checked into these nodes; the evaluator walks
//! them. Statements carry their source line for error messages.

use std::rc::Rc;

use super::types::Vim9Type;

#[derive(Clone, Debug)]
pub enum Expr {
    Number(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    Null,
    Blob(Vec<u8>),
    /// Variable, possibly scope-qualified (g:foo, s:Bar, b:name, a:1).
    Var(String),
    Option(String),
    Env(String),
    Register(char),
    List(Vec<Expr>),
    Dict(Vec<(DictKey, Expr)>),
    Unary(&'static str, Box<Expr>),
    Binary(String, Box<Expr>, Box<Expr>),
    Ternary(Box<Expr>, Box<Expr>, Box<Expr>),
    Index(Box<Expr>, Box<Expr>),
    Slice(Box<Expr>, Option<Box<Expr>>, Option<Box<Expr>>),
    Attr(Box<Expr>, String),
    Call(Box<Expr>, Vec<Expr>),
    /// expr->Name(args): the receiver becomes the first argument.
    Method(Box<Expr>, String, Vec<Expr>),
    Lambda(Rc<LambdaDef>),
}

/// A dictionary literal key: bare/string literals or '[expr]' computed keys.
#[derive(Clone, Debug)]
pub enum DictKey {
    Literal(String),
    Expr(Expr),
}

#[derive(Clone, Debug)]
pub struct LambdaDef {
    pub params: Vec<Param>,
    pub ret: Vim9Type,
    pub body: Vec<Stmt>,
    /// True when the lambda was written as an expression (implicit return).
    pub expr_body: Option<Expr>,
    /// Names captured from the enclosing function (snapshot at definition).
    pub captured: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Param {
    pub name: String,
    pub ty: Vim9Type,
    pub default: Option<Expr>,
    /// '...': collects the rest in this list (Vim9 varargs parameter).
    pub varargs: bool,
}

#[derive(Clone, Debug)]
pub enum Stmt {
    Var {
        name: String,
        ty: Option<Vim9Type>,
        value: Option<Expr>,
        constant: bool,
        export: bool,
        line: usize,
    },
    /// 'var [a, b] = list' destructuring declaration.
    Destructure {
        names: Vec<(String, Option<Vim9Type>)>,
        value: Expr,
        constant: bool,
        line: usize,
    },
    Assign {
        target: Expr,
        op: String,
        value: Expr,
        line: usize,
    },
    /// Prefix ++/-- and postfix forms.
    IncDec {
        target: Expr,
        delta: i64,
        line: usize,
    },
    If {
        branches: Vec<(Expr, Vec<Stmt>)>,
        else_body: Option<Vec<Stmt>>,
        line: usize,
    },
    While {
        cond: Expr,
        body: Vec<Stmt>,
        line: usize,
    },
    For {
        vars: Vec<(String, Option<Vim9Type>)>,
        iter: Expr,
        body: Vec<Stmt>,
        line: usize,
    },
    Try {
        body: Vec<Stmt>,
        catches: Vec<CatchClause>,
        finally: Vec<Stmt>,
        line: usize,
    },
    Return {
        value: Option<Expr>,
        line: usize,
    },
    Throw {
        value: Expr,
        line: usize,
    },
    Break {
        line: usize,
    },
    Continue {
        line: usize,
    },
    /// An Ex command line ('setf c', 'exe "..."', 'call Func()').
    Ex {
        line: String,
        number: usize,
    },
    /// A nested :def (Vim9 allows defining functions at script level; nested
    /// definitions are accepted for compatibility).
    Def(Rc<Vim9Func>),
    /// class/interface definition (Vim 9.2 Vim9 script).
    ClassDef(Rc<VimClass>),
    /// enum definition.
    EnumDef(Rc<VimEnum>),
    /// type alias definition.
    TypeAlias(Rc<VimTypeAlias>),
    /// A legacy ':func ... :endfunc' block inside a Vim9 script. The Ex layer
    /// owns legacy functions, so the evaluator hands this to the host.
    LegacyFunc {
        header: String,
        body: Vec<String>,
        export: bool,
        line: usize,
    },
    /// An :import/:export declaration (handled while loading the script).
    Import {
        kind: ImportKind,
        line: usize,
    },
    /// A bare expression statement (function/method call, ++x, ...).
    ExprStmt {
        expr: Expr,
        line: usize,
    },
    /// A no-op marker for 'defcompile'.
    Nop,
    /// A block body (used by block lambdas).
    Block(Vec<Stmt>),
}

#[derive(Clone, Debug)]
pub enum ImportKind {
    /// import 'file.vim' [as Name]
    Script {
        path: String,
        as_name: Option<String>,
    },
    /// import autoload 'file.vim' [as Name]
    Autoload {
        path: String,
        as_name: Option<String>,
    },
    /// import {name} from 'file.vim'
    From {
        names: Vec<String>,
        path: String,
        autoload: bool,
    },
    /// Export prefix on a var/def; carries the item it applies to.
    Export(Box<Stmt>),
}

#[derive(Clone, Debug)]
pub struct CatchClause {
    /// /pattern/ or None for a bare :catch.
    pub pattern: Option<String>,
    pub body: Vec<Stmt>,
    pub line: usize,
}

#[derive(Clone, Debug)]
pub struct Vim9Func {
    /// Name as written in the :def line.
    pub name: String,
    /// Canonical name used for lookup (dist#ft#FTasm, <SNR>3_Foo, or Foo).
    pub canonical: String,
    pub params: Vec<Param>,
    /// True when the signature ends in anonymous or named varargs.
    pub varargs: bool,
    pub ret: Vim9Type,
    pub body: Vec<Stmt>,
    pub sid: usize,
    pub is_export: bool,
    pub line: usize,
}

#[derive(Clone, Debug)]
pub struct VimClassMember {
    pub name: String,
    pub ty: Vim9Type,
    pub public: bool,
    pub static_member: bool,
    pub default: Option<Expr>,
    pub line: usize,
}

#[derive(Clone, Debug)]
pub struct VimClass {
    pub name: String,
    pub canonical: String,
    pub is_export: bool,
    pub is_abstract: bool,
    pub is_interface: bool,
    pub sid: usize,
    /// Superclass name as written (resolved at run time).
    pub parent: Option<String>,
    pub members: Vec<VimClassMember>,
    pub methods: Vec<Rc<Vim9Func>>,
    /// Names of methods declared 'static def'.
    pub static_methods: Vec<String>,
    pub constructor: Option<Rc<Vim9Func>>,
    pub line: usize,
}

#[derive(Clone, Debug)]
pub struct VimEnum {
    pub name: String,
    pub canonical: String,
    pub is_export: bool,
    pub values: Vec<(String, i64)>,
    pub line: usize,
}

#[derive(Clone, Debug)]
pub struct VimTypeAlias {
    pub name: String,
    pub canonical: String,
    pub is_export: bool,
    pub ty: Vim9Type,
    pub line: usize,
}

#[derive(Clone, Debug)]
pub struct Vim9Script {
    pub path: String,
    pub sid: usize,
    /// Canonical autoload prefix (e.g. 'dist#ft'), empty for ordinary scripts.
    pub prefix: String,
    pub funcs: Vec<Rc<Vim9Func>>,
    pub classes: Vec<Rc<VimClass>>,
    pub enums: Vec<Rc<VimEnum>>,
    pub type_aliases: Vec<Rc<VimTypeAlias>>,
    /// Top-level statements, in source order.
    pub top: Vec<Stmt>,
    /// Exported names (functions and variables).
    pub exports: Vec<String>,
    pub noclear: bool,
    pub line_count: usize,
}
