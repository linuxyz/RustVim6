//! Message history, backing `:messages` (reference: message.c).

use std::cell::RefCell;

/// Where `:redir` sends messages.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RedirTarget {
    /// `:redir => {var}`.
    Variable(String),
    /// `:redir @{register}`.
    Register(char),
    /// `:redir > {file}` (append when true).
    File(std::path::PathBuf, bool),
}

#[derive(Clone, Debug)]
struct RedirState {
    target: RedirTarget,
    text: String,
}

thread_local! {
    static REDIR: RefCell<Option<RedirState>> = const { RefCell::new(None) };
}

/// Begin `:redir`; returns false when a redirection is already active.
pub fn start_redir(target: RedirTarget) -> bool {
    REDIR.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_some() {
            return false;
        }
        *slot = Some(RedirState {
            target,
            text: String::new(),
        });
        true
    })
}

/// True while `:redir` is capturing.
pub fn is_redirecting() -> bool {
    REDIR.with(|slot| slot.borrow().is_some())
}

/// Stop `:redir` and return the target and captured text.
pub fn stop_redir() -> Option<(RedirTarget, String)> {
    REDIR.with(|slot| {
        slot.borrow_mut()
            .take()
            .map(|state| (state.target, state.text))
    })
}

fn capture(line: &str) {
    REDIR.with(|slot| {
        if let Some(state) = slot.borrow_mut().as_mut() {
            state.text.push_str(line);
            state.text.push('\n');
        }
    });
}

#[derive(Clone, Debug, Default)]
pub struct MessageLog {
    lines: Vec<String>,
    /// Transient status-line text set by `:echo`, cleared on next input.
    last_status: Option<String>,
}

impl MessageLog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn echo(&mut self, text: impl Into<String>) {
        let line = text.into();
        capture(&line);
        self.last_status = Some(line);
    }

    pub fn emsg(&mut self, text: impl Into<String>) {
        let line = text.into();
        capture(&line);
        self.last_status = Some(line.clone());
        self.lines.push(line);
    }

    pub fn info(&mut self, text: impl Into<String>) {
        let line = text.into();
        capture(&line);
        self.lines.push(line.clone());
        self.last_status = Some(line);
    }

    pub fn take_status(&mut self) -> Option<String> {
        self.last_status.take()
    }

    pub fn history(&self) -> &[String] {
        &self.lines
    }

    /// Truncate persistent history to `len` entries (used by :silent).
    pub fn truncate_history(&mut self, len: usize) {
        self.lines.truncate(len);
    }

    pub fn clear(&mut self) {
        self.lines.clear();
        self.last_status = None;
    }
}
