/// Multiline input buffer with cursor movement and prompt history navigation.
#[derive(Debug, Default)]
pub struct InputState {
    buffer: String,
    /// Byte index into `buffer` (always on a UTF-8 char boundary).
    cursor: usize,
    history: Vec<String>,
    history_idx: Option<usize>,
    draft_before_history: String,
}

impl InputState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn text(&self) -> &str {
        &self.buffer
    }

    pub fn cursor_byte(&self) -> usize {
        self.cursor
    }

    /// Replace byte range `[start..end]` with `replacement` and place cursor right after it.
    pub fn replace_range(&mut self, start: usize, end: usize, replacement: &str) {
        if start <= end && end <= self.buffer.len() {
            self.buffer.replace_range(start..end, replacement);
            self.cursor = start + replacement.len();
            self.history_idx = None;
        }
    }

    pub fn insert_char(&mut self, ch: char) {
        self.buffer.insert(self.cursor, ch);
        self.cursor += ch.len_utf8();
        self.history_idx = None;
    }

    pub fn insert_newline(&mut self) {
        self.insert_char('\n');
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let prev = self.buffer[..self.cursor]
            .char_indices()
            .next_back()
            .map(|(i, _)| i)
            .unwrap_or(0);
        self.buffer.drain(prev..self.cursor);
        self.cursor = prev;
        self.history_idx = None;
    }

    pub fn delete(&mut self) {
        if self.cursor >= self.buffer.len() {
            return;
        }
        let next_len = self.buffer[self.cursor..]
            .chars()
            .next()
            .map(char::len_utf8)
            .unwrap_or(0);
        self.buffer.drain(self.cursor..self.cursor + next_len);
        self.history_idx = None;
    }

    pub fn move_left(&mut self) {
        if self.cursor == 0 {
            return;
        }
        if let Some((idx, _)) = self.buffer[..self.cursor].char_indices().next_back() {
            self.cursor = idx;
        }
    }

    pub fn move_right(&mut self) {
        if self.cursor >= self.buffer.len() {
            return;
        }
        if let Some(ch) = self.buffer[self.cursor..].chars().next() {
            self.cursor += ch.len_utf8();
        }
    }

    pub fn move_home(&mut self) {
        let line_start = self.buffer[..self.cursor]
            .rfind('\n')
            .map(|i| i + 1)
            .unwrap_or(0);
        self.cursor = line_start;
    }

    pub fn move_end(&mut self) {
        let line_end = self.buffer[self.cursor..]
            .find('\n')
            .map(|i| self.cursor + i)
            .unwrap_or(self.buffer.len());
        self.cursor = line_end;
    }

    /// Navigate upward into previous submitted prompts.
    pub fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let next_idx = match self.history_idx {
            None => {
                self.draft_before_history = self.buffer.clone();
                self.history.len() - 1
            }
            Some(0) => 0,
            Some(i) => i - 1,
        };
        self.history_idx = Some(next_idx);
        self.buffer = self.history[next_idx].clone();
        self.cursor = self.buffer.len();
    }

    /// Navigate downward toward newer history / current draft.
    pub fn history_next(&mut self) {
        let Some(idx) = self.history_idx else {
            return;
        };
        if idx + 1 < self.history.len() {
            let next = idx + 1;
            self.history_idx = Some(next);
            self.buffer = self.history[next].clone();
        } else {
            self.history_idx = None;
            self.buffer = std::mem::take(&mut self.draft_before_history);
        }
        self.cursor = self.buffer.len();
    }

    /// Submit the current non-empty buffer, pushing it into history and clearing input.
    pub fn take_submission(&mut self) -> Option<String> {
        let trimmed = self.buffer.trim();
        if trimmed.is_empty() {
            return None;
        }
        let submitted = self.buffer.clone();
        if self.history.last() != Some(&submitted) {
            self.history.push(submitted.clone());
        }
        self.buffer.clear();
        self.cursor = 0;
        self.history_idx = None;
        self.draft_before_history.clear();
        Some(submitted)
    }

    /// Compute `(row, col)` cursor position in display characters for rendering.
    #[allow(dead_code)]
    pub fn cursor_row_col(&self) -> (u16, u16) {
        let prefix = &self.buffer[..self.cursor];
        let row = prefix.bytes().filter(|&b| b == b'\n').count() as u16;
        let line_start = prefix.rfind('\n').map(|i| i + 1).unwrap_or(0);
        let col = prefix[line_start..].chars().count() as u16;
        (row, col)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multiline_editing_and_history_navigation() {
        let mut input = InputState::new();
        for ch in "hello".chars() {
            input.insert_char(ch);
        }
        input.insert_newline();
        for ch in "world".chars() {
            input.insert_char(ch);
        }
        assert_eq!(input.text(), "hello\nworld");
        assert_eq!(input.cursor_row_col(), (1, 5));

        let first = input.take_submission().unwrap();
        assert_eq!(first, "hello\nworld");
        assert_eq!(input.text(), "");

        input.insert_char('x');
        input.history_prev();
        assert_eq!(input.text(), "hello\nworld");
        input.history_next();
        assert_eq!(input.text(), "x");
    }
}
