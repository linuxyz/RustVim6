//! Persistent Vim9 runtime state (script registry, modules, imports).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;

use crate::value::Value;
use crate::vim9::ast::{LambdaDef, Vim9Func, VimClass, VimEnum};
use crate::vim9::types::Vim9Type;

/// A lambda instance: the compiled body plus values captured at definition.
#[derive(Debug)]
pub struct Vim9Lambda {
    pub def: Rc<LambdaDef>,
    pub captured: HashMap<String, Value>,
    pub sid: usize,
}

/// Runtime state for one class: its definition and static members.
#[derive(Debug)]
pub struct VimClassState {
    pub def: Rc<VimClass>,
    pub statics: RefCell<HashMap<String, Value>>,
}

/// A sourced script that can be imported by name.
#[derive(Debug)]
pub struct Module {
    pub path: PathBuf,
    pub sid: usize,
    /// Autoload prefix (e.g. 'dist#ft'), empty for ordinary scripts.
    pub prefix: String,
    /// Exported function name -> canonical function name.
    pub funcs: HashMap<String, String>,
    /// Exported variable name -> declared type.
    pub vars: HashMap<String, Vim9Type>,
    /// Exported class name -> canonical name.
    pub classes: HashMap<String, String>,
    /// Exported enum name -> canonical name.
    pub enums: HashMap<String, String>,
    /// Exported type alias name -> canonical name.
    pub aliases: HashMap<String, String>,
}

/// What a name in an importing script refers to.
#[derive(Clone, Debug)]
pub enum ImportBinding {
    /// 'import ./file.vim as ns'
    Module(Rc<Module>),
    /// 'import autoload dist/ft.vim as ns'
    Autoload(String),
    /// 'import {Func} from ...'
    Func(String),
    /// 'import {var} from ...'
    Var {
        sid: usize,
        name: String,
        ty: Vim9Type,
    },
    /// 'import {Class} from ...'
    Class(String),
    /// 'import {Enum} from ...'
    Enum(String),
    /// 'import {Alias} from ...'
    Alias(String),
}

/// Vim9 state that lives as long as the editor session.
#[derive(Debug, Default)]
pub struct Vim9State {
    /// Every compiled function, by canonical name.
    pub funcs: HashMap<String, Rc<Vim9Func>>,
    /// Sourced scripts, by canonical absolute file name.
    pub modules: HashMap<String, Rc<Module>>,
    /// Per-script import bindings, keyed by script id.
    pub imports: HashMap<usize, HashMap<String, ImportBinding>>,
    /// Script-local variable types, keyed by script id.
    pub script_types: HashMap<usize, HashMap<String, Vim9Type>>,
    /// Autoload prefix per script id (empty for ordinary scripts).
    pub script_prefix: HashMap<usize, String>,
    /// Constants (key: script id + name) that cannot be reassigned.
    pub constants: HashSet<String>,
    /// Scripts already loaded through 'import', to avoid duplicate sourcing.
    pub loaded: HashSet<String>,
    /// Classes by canonical name.
    pub classes: HashMap<String, Rc<VimClassState>>,
    /// Enums by canonical name.
    pub enums: HashMap<String, Rc<VimEnum>>,
    /// Type aliases by canonical name.
    pub type_aliases: HashMap<String, Vim9Type>,
    /// Object dictionaries, keyed by the address of their Rc.
    pub objects: HashMap<usize, Rc<VimClassState>>,
}

impl Vim9State {
    pub fn function(&self, name: &str) -> Option<Rc<Vim9Func>> {
        self.funcs.get(name).cloned()
    }

    pub fn add_function(&mut self, func: Rc<Vim9Func>) {
        self.funcs.insert(func.canonical.clone(), func);
    }

    pub fn class(&self, name: &str) -> Option<Rc<VimClassState>> {
        self.classes.get(name).cloned()
    }

    /// Register an object dictionary and return its class.
    pub fn register_object(&mut self, dict: &Value, class: Rc<VimClassState>) {
        if let Value::Dict(rc) = dict {
            self.objects.insert(Rc::as_ptr(rc) as usize, class);
        }
    }

    pub fn object_class(&self, value: &Value) -> Option<Rc<VimClassState>> {
        let Value::Dict(rc) = value else {
            return None;
        };
        self.objects.get(&(Rc::as_ptr(rc) as usize)).cloned()
    }

    pub fn import_binding(&self, sid: usize, name: &str) -> Option<ImportBinding> {
        self.imports.get(&sid).and_then(|m| m.get(name)).cloned()
    }
}

/// Canonical key for a script path (case-insensitive on Windows for stability).
pub fn module_key(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    if cfg!(windows) {
        normalized.to_ascii_lowercase()
    } else {
        normalized
    }
}
