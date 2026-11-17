//! Message history, backing `:messages` (reference: message.c).

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
        self.last_status = Some(text.into());
    }

    pub fn emsg(&mut self, text: impl Into<String>) {
        let line = text.into();
        self.last_status = Some(line.clone());
        self.lines.push(line);
    }

    pub fn info(&mut self, text: impl Into<String>) {
        let line = text.into();
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
