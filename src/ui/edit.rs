//! UTF-8 byte-offset selection is shared by keyboard, IME, clipboard and automation.
use crate::protocol::Result;
#[derive(Clone, Debug, Default)]
pub struct EditBuffer {
    pub text: String,
    pub anchor: usize,
    pub cursor: usize,
    pub composition: String,
    pub composition_cursor: Option<[usize; 2]>,
    pub max_bytes: usize,
    /// Server dialogs limit text by UTF-16 length (`max_length`) on top of the byte bound.
    pub max_chars: Option<usize>,
    pub multiline: bool,
}
impl EditBuffer {
    pub fn new(text: String, max_bytes: usize, multiline: bool) -> Result<Self> {
        if text.len() > max_bytes {
            return Err("text_limit".into());
        }
        let cursor = text.len();
        Ok(Self {
            text,
            anchor: cursor,
            cursor,
            max_bytes,
            multiline,
            ..Self::default()
        })
    }
    pub fn select(&mut self, start: usize, end: usize) -> Result<()> {
        if !self.text.is_char_boundary(start) || !self.text.is_char_boundary(end) {
            return Err("invalid_text_selection".into());
        }
        self.anchor = start;
        self.cursor = end;
        Ok(())
    }
    pub fn selection(&self) -> &str {
        &self.text[self.anchor.min(self.cursor)..self.anchor.max(self.cursor)]
    }
    pub fn insert(&mut self, value: &str) -> Result<()> {
        if value
            .chars()
            .any(|c| c == '\0' || (!self.multiline && (c == '\n' || c == '\r')))
        {
            return Err("invalid_text_input".into());
        }
        let lo = self.anchor.min(self.cursor);
        let hi = self.anchor.max(self.cursor);
        if self.text.len() - (hi - lo) + value.len() > self.max_bytes {
            return Err("text_limit".into());
        }
        if let Some(max) = self.max_chars
            && self.text.encode_utf16().count() - self.text[lo..hi].encode_utf16().count()
                + value.encode_utf16().count()
                > max
        {
            return Err("text_limit".into());
        }
        self.text.replace_range(lo..hi, value);
        self.cursor = lo + value.len();
        self.anchor = self.cursor;
        self.composition.clear();
        self.composition_cursor = None;
        Ok(())
    }
    pub fn compose(&mut self, text: String, cursor: Option<[usize; 2]>) -> Result<()> {
        if text.len() > self.max_bytes
            || cursor.is_some_and(|[a, b]| !text.is_char_boundary(a) || !text.is_char_boundary(b))
        {
            return Err("invalid_composition".into());
        }
        self.composition = text;
        self.composition_cursor = cursor;
        Ok(())
    }
    pub fn move_cursor(&mut self, right: bool, select: bool, word: bool) {
        let mut pos = self.cursor;
        if right {
            if let Some(c) = self.text[pos..].chars().next() {
                pos += c.len_utf8();
            }
            if word {
                while let Some(c) = self.text[pos..].chars().next() {
                    if c.is_whitespace() {
                        break;
                    }
                    pos += c.len_utf8();
                }
            }
        } else {
            if let Some((i, _)) = self.text[..pos].char_indices().next_back() {
                pos = i;
            }
            if word {
                while let Some((i, c)) = self.text[..pos].char_indices().next_back() {
                    if c.is_whitespace() {
                        break;
                    }
                    pos = i;
                }
            }
        }
        self.cursor = pos;
        if !select {
            self.anchor = pos;
        }
    }
    pub fn delete(&mut self, forward: bool, word: bool) -> Result<()> {
        if self.anchor == self.cursor {
            self.move_cursor(forward, true, word);
        }
        self.insert("")
    }
    pub fn display(&self) -> std::borrow::Cow<'_, str> {
        if self.composition.is_empty() {
            return std::borrow::Cow::Borrowed(&self.text);
        }
        std::borrow::Cow::Owned(format!(
            "{}{}{}",
            &self.text[..self.anchor.min(self.cursor)],
            self.composition,
            &self.text[self.anchor.max(self.cursor)..]
        ))
    }
    pub fn display_cursor(&self) -> Option<usize> {
        if self.composition.is_empty() {
            Some(self.cursor)
        } else {
            self.composition_cursor
                .map(|[_, end]| self.anchor.min(self.cursor) + end)
        }
    }
    pub fn display_selection(&self) -> Option<[usize; 2]> {
        if self.composition.is_empty() {
            Some([self.anchor.min(self.cursor), self.anchor.max(self.cursor)])
        } else {
            self.composition_cursor.map(|[start, end]| {
                [
                    self.anchor.min(self.cursor) + start,
                    self.anchor.min(self.cursor) + end,
                ]
            })
        }
    }
}
