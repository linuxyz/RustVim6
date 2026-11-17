//! Variable scopes and the host interface.
//!
//! Scopes (`:help internal-variables`): `g:` global, `b:` buffer, `w:` window,
//! `t:` tab, `v:` Vim, `s:` script, `l:` function-local, `a:` function args.
//! Buffer/window/tab scopes belong to the editor, so they are reached through
//! the [`Host`] trait; the rest live here.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use rvim_core::{VimError, VimResult};

use crate::value::{Value, ValueResult, VimDict};

/// One function-call frame.
#[derive(Debug, Default, Clone)]
pub struct Frame {
    pub args: HashMap<String, Value>,
    pub locals: HashMap<String, Value>,
    /// Script id the function was defined in (`<SID>`), indexing `Env::scripts`.
    pub sid: usize,
}

/// Identity of the built-in scope dictionaries that can be evaluated bare
/// (`g:`, `s:`, `l:`, `a:`, `v:`). Reference: evalvars.c:3203-3236.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ScopeKey {
    Global,
    Vim,
    Script,
    Local,
    Args,
}

fn scope_key(name: &str) -> Option<ScopeKey> {
    match name {
        "g:" => Some(ScopeKey::Global),
        "v:" => Some(ScopeKey::Vim),
        "s:" => Some(ScopeKey::Script),
        "l:" => Some(ScopeKey::Local),
        "a:" => Some(ScopeKey::Args),
        _ => None,
    }
}

/// Variable environment for one evaluation.
#[derive(Debug)]
pub struct Env {
    pub globals: HashMap<String, Value>,
    pub vvars: HashMap<String, Value>,
    /// One map per sourced script (`s:` variables); the last one is current.
    pub scripts: Vec<HashMap<String, Value>>,
    pub frames: Vec<Frame>,
    /// Current script id for `<SID>` resolution.
    pub current_sid: usize,
    /// Depth guard for function calls (E132).
    pub call_depth: usize,
    /// A `:throw` (or error converted to an exception) that crossed a function
    /// call boundary; `call_user_func` parks the original value here so the
    /// caller can re-throw it unchanged instead of wrapping it in E605.
    pub pending_throw: Option<Value>,
    /// Number of active `:try` conditionals (Vim's `trylevel`). While this is
    /// non-zero errors are converted to exceptions instead of being reported
    /// and continuing (ex_eval.c:144-260 cause_errthrow()).
    pub try_depth: usize,
    /// True while executing the body of a function declared with `abort`.
    pub aborting_func: bool,
    /// Cache of live scope dictionaries. A bare `g:`/`s:`/`l:`/`a:`/`v:` must
    /// be a *live* dictionary: `filter(g:, ...)` or `let g:d = g:` followed by
    /// `let g:d.x = 1` changes the real scope (evalvars.c:3203-3236).
    scope_cache: RefCell<HashMap<ScopeKey, Rc<RefCell<VimDict>>>>,
}

impl Default for Env {
    fn default() -> Self {
        Self::new()
    }
}

impl Env {
    pub fn new() -> Self {
        let mut env = Self {
            globals: HashMap::new(),
            vvars: HashMap::new(),
            scripts: vec![HashMap::new()],
            frames: Vec::new(),
            current_sid: 0,
            call_depth: 0,
            pending_throw: None,
            try_depth: 0,
            aborting_func: false,
            scope_cache: RefCell::new(HashMap::new()),
        };
        env.init_vvars();
        env
    }

    fn init_vvars(&mut self) {
        // Read-only/settled Vim variables plugins commonly test.
        let defaults: &[(&str, Value)] = &[
            // Special values; the expression parser has dedicated literal
            // handling, but `exists('v:true')`, `get(v:, ...)` and `keys(v:)`
            // see the scope dictionary (evalvars.c vimvars table).
            ("true", Value::Bool(true)),
            ("false", Value::Bool(false)),
            ("null", Value::Null),
            ("none", Value::Null),
            ("version", Value::Number(802)),
            ("progname", Value::string("rvim")),
            ("progpath", Value::string("")),
            ("errmsg", Value::string("")),
            ("exception", Value::string("")),
            ("throwpoint", Value::string("")),
            ("warningmsg", Value::string("")),
            ("statusmsg", Value::string("")),
            ("shell_error", Value::Number(0)),
            ("count", Value::Number(0)),
            ("count1", Value::Number(0)),
            ("prevcount", Value::Number(0)),
            ("lnum", Value::Number(0)),
            ("key", Value::Null),
            ("val", Value::Null),
            ("char", Value::string("")),
            ("register", Value::string("\"")),
            ("operator", Value::string("")),
            ("hlsearch", Value::Number(0)),
            ("searchforward", Value::Number(1)),
            ("scrollstart", Value::string("")),
            ("beval_lnum", Value::Number(0)),
            ("beval_col", Value::Number(0)),
            ("beval_bufnr", Value::Number(0)),
            ("beval_text", Value::string("")),
            (
                "completed_item",
                Value::Dict(std::rc::Rc::new(
                    std::cell::RefCell::new(Default::default()),
                )),
            ),
            ("cmdarg", Value::string("")),
            ("cmdbang", Value::Number(0)),
            ("oldfiles", Value::list(Vec::new())),
            ("termresponse", Value::string("")),
            ("testing", Value::Number(0)),
            ("dying", Value::Number(0)),
            ("exiting", Value::Null),
            ("t_number", Value::Number(0)),
            ("ctype", Value::string("")),
            ("swapname", Value::string("")),
            ("swapchoice", Value::string("")),
            ("swapcommand", Value::string("")),
            ("servername", Value::string("")),
            ("versionlong", Value::Number(802)),
        ];
        for (name, value) in defaults {
            self.vvars.insert((*name).to_string(), value.clone());
        }
    }

    pub fn push_script(&mut self) -> usize {
        self.scripts.push(HashMap::new());
        self.current_sid = self.scripts.len() - 1;
        self.current_sid
    }

    pub fn pop_script(&mut self) -> usize {
        let sid = self.current_sid;
        if self.scripts.len() > 1 {
            self.scripts.pop();
        }
        self.current_sid = self.scripts.len().saturating_sub(1);
        sid
    }

    pub fn current_frame(&self) -> Option<&Frame> {
        self.frames.last()
    }

    pub fn current_frame_mut(&mut self) -> Option<&mut Frame> {
        self.frames.last_mut()
    }

    /// Resolve a scoped variable name (`g:foo`, `a:1`, `s:Bar`, `foo`).
    pub fn get_var(&self, name: &str) -> Option<Value> {
        // A bare scope name evaluates to that scope's live dictionary.
        if let Some(dict) = self.scope_dict(name) {
            return Some(dict);
        }
        if let Some(rest) = name.strip_prefix("v:") {
            return self.vvars.get(rest).cloned();
        }
        if let Some(rest) = name.strip_prefix("g:") {
            return self.globals.get(rest).cloned();
        }
        if let Some(rest) = name.strip_prefix("s:") {
            return self
                .scripts
                .get(self.current_sid)
                .and_then(|m| m.get(rest))
                .cloned();
        }
        if let Some(rest) = name.strip_prefix("a:") {
            // a:0 ... a:N positional varargs are stored as "0".."N".
            return self.frames.last().and_then(|f| f.args.get(rest).cloned());
        }
        if let Some(rest) = name.strip_prefix("l:") {
            return self.frames.last().and_then(|f| f.locals.get(rest).cloned());
        }
        // Unscoped: function-local, then script, then global. Function
        // arguments are *not* in scope without the a: prefix (only lambda
        // parameters are, because Vim adds them to l: as well).
        if let Some(frame) = self.frames.last() {
            if let Some(v) = frame.locals.get(name) {
                return Some(v.clone());
            }
        }
        if let Some(v) = self.scripts.get(self.current_sid).and_then(|m| m.get(name)) {
            return Some(v.clone());
        }
        self.globals.get(name).cloned()
    }

    /// Assign a variable, creating it in the right scope.
    pub fn set_var(&mut self, name: &str, value: Value) -> VimResult<()> {
        if let Some(rest) = name.strip_prefix("v:") {
            // Most v: variables are read-only; the writable ones are assigned.
            if matches!(
                rest,
                "errmsg"
                    | "warningmsg"
                    | "statusmsg"
                    | "shell_error"
                    | "count"
                    | "count1"
                    | "prevcount"
                    | "dying"
            ) {
                self.vvars.insert(rest.to_string(), value);
                return Ok(());
            }
            return Err(VimError::new(
                "E46",
                format!("Cannot change read-only variable \"v:{rest}\""),
            ));
        }
        if let Some(rest) = name.strip_prefix("g:") {
            self.globals.insert(rest.to_string(), value);
            return Ok(());
        }
        if let Some(rest) = name.strip_prefix("s:") {
            let sid = self.current_sid;
            if let Some(map) = self.scripts.get_mut(sid) {
                map.insert(rest.to_string(), value);
                return Ok(());
            }
            return Err(VimError::new("E461", "Illegal variable name".to_string()));
        }
        if let Some(rest) = name.strip_prefix("a:") {
            if let Some(frame) = self.frames.last_mut() {
                frame.args.insert(rest.to_string(), value);
                return Ok(());
            }
            return Err(VimError::new("E461", "Illegal variable name".to_string()));
        }
        if let Some(rest) = name.strip_prefix("l:") {
            if let Some(frame) = self.frames.last_mut() {
                frame.locals.insert(rest.to_string(), value);
                return Ok(());
            }
            return Err(VimError::new("E461", "Illegal variable name".to_string()));
        }
        // Unscoped inside a function is function-local; at script level it is
        // a global variable (Vim's rule for legacy script).
        if let Some(frame) = self.frames.last_mut() {
            frame.locals.insert(name.to_string(), value);
        } else {
            self.globals.insert(name.to_string(), value);
        }
        Ok(())
    }

    /// Remove a variable, reporting E108 when absent.
    pub fn unlet_var(&mut self, name: &str) -> VimResult<()> {
        let removed = if let Some(rest) = name.strip_prefix("g:") {
            self.globals.remove(rest).is_some()
        } else if let Some(rest) = name.strip_prefix("v:") {
            self.vvars.remove(rest).is_some()
        } else if let Some(rest) = name.strip_prefix("s:") {
            let sid = self.current_sid;
            self.scripts
                .get_mut(sid)
                .map(|m| m.remove(rest).is_some())
                .unwrap_or(false)
        } else if let Some(rest) = name.strip_prefix("a:") {
            self.frames
                .last_mut()
                .map(|f| f.args.remove(rest).is_some())
                .unwrap_or(false)
        } else if let Some(rest) = name.strip_prefix("l:") {
            self.frames
                .last_mut()
                .map(|f| f.locals.remove(rest).is_some())
                .unwrap_or(false)
        } else {
            let mut ok = self.globals.remove(name).is_some();
            if !ok {
                if let Some(frame) = self.frames.last_mut() {
                    ok = frame.locals.remove(name).is_some();
                }
            }
            ok
        };
        if !removed {
            return Err(VimError::new("E108", format!("No such variable: {name}")));
        }
        Ok(())
    }

    /// `exists('x')` for variable scopes.
    pub fn var_exists(&self, name: &str) -> bool {
        self.get_var(name).is_some()
    }

    /// Evaluate a bare scope name (`g:`, `s:`, `l:`, `a:`, `v:`) to its live
    /// dictionary, or `None` for any other name.
    ///
    /// The dictionary is cached per scope and refreshed from the real variable
    /// storage on every access, so mutations performed through it (a following
    /// [`Env::sync_scope_dict`]) and ordinary `:let` assignments stay in sync.
    /// Reference: evalvars.c:3203-3236 (`find_var_in_ht` returns the scope
    /// dictitem for an empty variable name).
    pub fn scope_dict(&self, name: &str) -> Option<Value> {
        let key = scope_key(name)?;
        // l:/a: only exist inside a function ("Undefined variable: l:" at
        // script level, evalvars.c:3225-3226 get_funccal_local_var()).
        if matches!(key, ScopeKey::Local | ScopeKey::Args) && self.frames.is_empty() {
            return None;
        }
        let rc = {
            let mut cache = self.scope_cache.borrow_mut();
            cache
                .entry(key)
                .or_insert_with(|| Rc::new(RefCell::new(VimDict::new())))
                .clone()
        };
        let mut fresh = VimDict::new();
        match key {
            ScopeKey::Global => {
                for (k, v) in &self.globals {
                    fresh.insert(k.clone(), v.clone());
                }
            }
            ScopeKey::Vim => {
                for (k, v) in &self.vvars {
                    fresh.insert(k.clone(), v.clone());
                }
            }
            ScopeKey::Script => {
                if let Some(map) = self.scripts.get(self.current_sid) {
                    for (k, v) in map {
                        fresh.insert(k.clone(), v.clone());
                    }
                }
            }
            ScopeKey::Local => {
                if let Some(frame) = self.frames.last() {
                    for (k, v) in &frame.locals {
                        fresh.insert(k.clone(), v.clone());
                    }
                }
            }
            ScopeKey::Args => {
                if let Some(frame) = self.frames.last() {
                    for (k, v) in &frame.args {
                        fresh.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        *rc.borrow_mut() = fresh;
        Some(Value::Dict(rc))
    }

    /// Write a possibly-mutated scope dictionary back into the real variable
    /// storage (`filter(g:, ...)`, `extend(g:, ...)`, `let g:d.x = 1`, ...).
    /// Does nothing when the value is not one of the cached scope dictionaries.
    pub fn sync_scope_dict(&mut self, value: &Value) {
        let Value::Dict(rc) = value else {
            return;
        };
        let key = {
            let cache = self.scope_cache.borrow();
            cache
                .iter()
                .find(|(_, dict)| Rc::ptr_eq(dict, rc))
                .map(|(key, _)| *key)
        };
        let Some(key) = key else {
            return;
        };
        let entries: Vec<(String, Value)> = rc
            .borrow()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        match key {
            ScopeKey::Global => {
                self.globals = entries.into_iter().collect();
            }
            ScopeKey::Vim => {
                self.vvars = entries.into_iter().collect();
            }
            ScopeKey::Script => {
                if let Some(map) = self.scripts.get_mut(self.current_sid) {
                    *map = entries.into_iter().collect();
                }
            }
            ScopeKey::Local => {
                if let Some(frame) = self.frames.last_mut() {
                    frame.locals = entries.into_iter().collect();
                }
            }
            ScopeKey::Args => {
                if let Some(frame) = self.frames.last_mut() {
                    frame.args = entries.into_iter().collect();
                }
            }
        }
    }
}

/// The bridge from the language to the editor/Ex layer.
///
/// Every method has a default implementation so a host only overrides what it
/// supports; `None` means "this host does not provide that builtin", letting the
/// pure builtin table take over.
pub trait Host {
    fn option_get(&mut self, _name: &str) -> Option<Value> {
        None
    }

    fn option_set(&mut self, _name: &str, _value: &Value) -> Option<VimResult<()>> {
        None
    }

    fn buffer_var_get(&mut self, _name: &str) -> Option<Value> {
        None
    }

    fn buffer_var_set(&mut self, _name: &str, _value: Value) -> Option<VimResult<()>> {
        None
    }

    fn window_var_get(&mut self, _name: &str) -> Option<Value> {
        None
    }

    fn window_var_set(&mut self, _name: &str, _value: Value) -> Option<VimResult<()>> {
        None
    }

    fn tab_var_get(&mut self, _name: &str) -> Option<Value> {
        None
    }

    fn tab_var_set(&mut self, _name: &str, _value: Value) -> Option<VimResult<()>> {
        None
    }

    /// Read a register (`@a`, `@"`) as a string.
    fn host_register(&mut self, _name: char) -> Option<ValueResult> {
        None
    }

    /// Call a user-defined or autoload function.
    fn call_user_function(
        &mut self,
        _name: &str,
        _args: Vec<Value>,
        _env: &mut Env,
    ) -> Option<ValueResult> {
        None
    }

    /// Host-provided builtins (`getline`, `expand`, `exists`, ...).
    fn call_host_builtin(
        &mut self,
        _name: &str,
        _args: Vec<Value>,
        _env: &mut Env,
    ) -> Option<ValueResult> {
        None
    }

    /// Cheap name check used by [`crate::eval::call_function`] to avoid entering
    /// the (large) host-builtin dispatchers for every user function call.
    ///
    /// The default is `true` so a host that only overrides
    /// [`Host::call_host_builtin`] keeps the historical "call and see" contract;
    /// hosts that can answer cheaply should override this.
    fn is_host_builtin(&self, _name: &str) -> bool {
        true
    }

    /// `'maxfuncdepth'` limit for E132; hosts that know the option override this.
    fn maxfuncdepth(&self) -> usize {
        100
    }

    /// Snapshot of the buffer-local variables (`b:`) as a dictionary.
    fn buffer_var_dict(&mut self) -> Option<Value> {
        None
    }

    /// Snapshot of the window-local variables (`w:`) as a dictionary.
    fn window_var_dict(&mut self) -> Option<Value> {
        None
    }

    /// Snapshot of the tab-local variables (`t:`) as a dictionary.
    fn tab_var_dict(&mut self) -> Option<Value> {
        None
    }

    /// `exists()` for non-variable arguments (`:cmd`, `+opt`, `*func`).
    fn exists_check(&mut self, _what: &str) -> Option<bool> {
        None
    }

    /// `has('feature')`.
    fn has_feature(&self, _feature: &str) -> bool {
        false
    }

    /// Execute one Ex command line (used by `:execute` inside functions).
    fn execute_command(&mut self, _line: &str, _env: &mut Env) -> Option<VimResult<()>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_resolution_order() {
        let mut env = Env::new();
        env.globals.insert("x".into(), Value::Number(1));
        env.set_var("s:y", Value::Number(2)).unwrap();
        assert_eq!(env.get_var("g:x").unwrap().to_number(), 1);
        assert_eq!(env.get_var("x").unwrap().to_number(), 1);
        assert_eq!(env.get_var("s:y").unwrap().to_number(), 2);

        env.frames.push(Frame::default());
        env.set_var("x", Value::Number(3)).unwrap(); // local shadows global
        assert_eq!(env.get_var("x").unwrap().to_number(), 3);
        assert_eq!(env.get_var("g:x").unwrap().to_number(), 1);
        assert_eq!(env.get_var("l:x").unwrap().to_number(), 3);
    }

    #[test]
    fn args_and_varargs() {
        let mut env = Env::new();
        let mut frame = Frame::default();
        frame.args.insert("1".into(), Value::string("first"));
        frame.args.insert("0".into(), Value::Number(1));
        env.frames.push(frame);
        assert_eq!(env.get_var("a:1").unwrap().to_string_value(), "first");
        assert_eq!(env.get_var("a:0").unwrap().to_number(), 1);
        env.set_var("a:2", Value::Number(9)).unwrap();
        assert_eq!(env.get_var("a:2").unwrap().to_number(), 9);
    }

    #[test]
    fn vvars_are_read_only_except_allowed() {
        let mut env = Env::new();
        assert_eq!(env.get_var("v:version").unwrap().to_number(), 802);
        // v:true/v:false/v:null exist in the v: scope dictionary.
        assert_eq!(env.get_var("v:true").unwrap().to_string_value(), "v:true");
        assert_eq!(env.get_var("v:null").unwrap().to_string_value(), "v:null");
        assert_eq!(
            env.set_var("v:version", Value::Number(1)).unwrap_err().code,
            "E46"
        );
        env.set_var("v:errmsg", Value::string("oops")).unwrap();
        assert_eq!(env.get_var("v:errmsg").unwrap().to_string_value(), "oops");
    }

    #[test]
    fn scope_dictionaries_are_live() {
        // evalvars.c:3203-3236: a bare scope name is the scope's dictionary.
        let mut env = Env::new();
        env.set_var("g:aa", Value::Number(1)).unwrap();
        let g = env.get_var("g:").unwrap();
        let Value::Dict(dict) = &g else {
            panic!("g: must be a Dictionary");
        };
        assert_eq!(dict.borrow().get("aa").map(Value::to_number), Some(1));
        // filter(g:, ...) mutates the real globals: the dictionary write is
        // synced back by Env::sync_scope_dict().
        {
            let mut d = dict.borrow_mut();
            d.remove("aa");
            d.insert("bb", Value::Number(2));
        }
        env.sync_scope_dict(&g);
        assert_eq!(env.get_var("g:aa"), None);
        assert_eq!(env.get_var("g:bb").unwrap().to_number(), 2);
        // l:/a: do not exist outside a function.
        assert!(env.get_var("l:").is_none());
        assert!(env.get_var("a:").is_none());
        env.frames.push(Frame::default());
        env.set_var("l:x", Value::Number(3)).unwrap();
        assert!(matches!(env.get_var("l:"), Some(Value::Dict(_))));
        assert!(matches!(env.get_var("a:"), Some(Value::Dict(_))));
    }

    #[test]
    fn unlet_reports_missing_variable() {
        let mut env = Env::new();
        env.set_var("g:a", Value::Number(1)).unwrap();
        env.unlet_var("g:a").unwrap();
        assert_eq!(env.unlet_var("g:a").unwrap_err().code, "E108");
    }

    #[test]
    fn script_scopes_stack() {
        let mut env = Env::new();
        let outer = env.current_sid;
        env.set_var("s:v", Value::Number(1)).unwrap();
        let inner = env.push_script();
        assert_ne!(outer, inner);
        env.set_var("s:v", Value::Number(2)).unwrap();
        assert_eq!(env.get_var("s:v").unwrap().to_number(), 2);
        env.pop_script();
        assert_eq!(env.get_var("s:v").unwrap().to_number(), 1);
    }
}
