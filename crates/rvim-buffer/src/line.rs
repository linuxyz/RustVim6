//! A single logical line. Text is shared via `Rc<str>` so copying a line (undo
//! history, yank, whole-line duplication) is O(1) until mutation forces a clone.

use std::rc::Rc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    text: Rc<str>,
}

impl Line {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: Rc::from(text.into().into_boxed_str()),
        }
    }

    pub fn empty() -> Self {
        Self::new(String::new())
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    pub fn len_bytes(&self) -> usize {
        self.text.len()
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn char_count(&self) -> usize {
        self.text.chars().count()
    }

    /// Byte offset of the last character, or 0 for an empty line.
    pub fn last_char_byte(&self) -> usize {
        if self.text.is_empty() {
            0
        } else {
            let mut idx = self.text.len();
            loop {
                idx -= 1;
                if self.text.is_char_boundary(idx) {
                    return idx;
                }
            }
        }
    }
}

impl Default for Line {
    fn default() -> Self {
        Self::empty()
    }
}

impl From<&str> for Line {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cow_sharing() {
        let a = Line::new("hello");
        let b = a.clone();
        assert!(Rc::ptr_eq(&a.text, &b.text));
        assert_eq!(a.last_char_byte(), 4);
        assert_eq!(Line::empty().last_char_byte(), 0);
    }
}
