use std::fs;
use std::io;
use std::path::PathBuf;

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub(crate) struct Position {
    pub(crate) x: usize,
    pub(crate) y: usize,
}

pub(crate) struct Document {
    pub(crate) lines: Vec<String>,
    pub(crate) file_path: Option<PathBuf>,
    pub(crate) modified: bool,
}

impl Document {
    pub(crate) fn new_empty(file_path: Option<PathBuf>) -> Self {
        Self {
            lines: vec![String::new()],
            file_path,
            modified: false,
        }
    }

    pub(crate) fn open(path: PathBuf) -> io::Result<Self> {
        let text = fs::read_to_string(&path)?;
        let mut lines: Vec<String> = text.split('\n').map(String::from).collect();
        if lines.is_empty() {
            lines.push(String::new());
        }
        Ok(Self {
            lines,
            file_path: Some(path),
            modified: false,
        })
    }

    pub(crate) fn line_count(&self) -> usize {
        self.lines.len()
    }

    pub(crate) fn word_count(&self) -> usize {
        self.lines
            .iter()
            .map(|line| line.split_whitespace().count())
            .sum()
    }

    pub(crate) fn line(&self, index: usize) -> Option<&String> {
        self.lines.get(index)
    }

    pub(crate) fn line_char_len(&self, index: usize) -> usize {
        self.line(index).map_or(0, |line| line.chars().count())
    }

    pub(crate) fn insert_char(&mut self, pos: Position, ch: char) {
        if pos.y >= self.lines.len() {
            self.lines.push(String::new());
        }
        if let Some(line) = self.lines.get_mut(pos.y) {
            let idx = byte_index_for_char(line, pos.x);
            line.insert(idx, ch);
            self.modified = true;
        }
    }

    pub(crate) fn insert_newline(&mut self, pos: Position) {
        if pos.y >= self.lines.len() {
            self.lines.push(String::new());
            self.modified = true;
            return;
        }

        let line = &mut self.lines[pos.y];
        let idx = byte_index_for_char(line, pos.x);
        let new_line = line.split_off(idx);
        self.lines.insert(pos.y + 1, new_line);
        self.modified = true;
    }

    pub(crate) fn backspace(&mut self, pos: Position) -> Option<Position> {
        if pos.y >= self.lines.len() {
            return None;
        }

        if pos.x > 0 {
            if let Some(line) = self.lines.get_mut(pos.y) {
                remove_char_at(line, pos.x - 1);
                self.modified = true;
                return Some(Position {
                    x: pos.x - 1,
                    y: pos.y,
                });
            }
            return None;
        }

        if pos.y == 0 {
            return None;
        }

        let prev_len = self.line_char_len(pos.y - 1);
        if pos.y < self.lines.len() {
            let current_line = self.lines.remove(pos.y);
            if let Some(prev_line) = self.lines.get_mut(pos.y - 1) {
                prev_line.push_str(&current_line);
                self.modified = true;
                return Some(Position {
                    x: prev_len,
                    y: pos.y - 1,
                });
            }
        }
        None
    }

    pub(crate) fn delete_forward(&mut self, pos: Position) {
        if pos.y >= self.lines.len() {
            return;
        }

        let line_len = self.line_char_len(pos.y);
        if pos.x < line_len {
            if let Some(line) = self.lines.get_mut(pos.y) {
                remove_char_at(line, pos.x);
                self.modified = true;
            }
            return;
        }

        if pos.y + 1 >= self.lines.len() {
            return;
        }

        let next_line = self.lines.remove(pos.y + 1);
        if let Some(line) = self.lines.get_mut(pos.y) {
            line.push_str(&next_line);
            self.modified = true;
        }
    }

    pub(crate) fn save(&mut self) -> io::Result<PathBuf> {
        let path = self
            .file_path
            .clone()
            .unwrap_or_else(|| PathBuf::from("redit.md"));
        self.save_as(path)
    }

    pub(crate) fn save_as(&mut self, path: PathBuf) -> io::Result<PathBuf> {
        let mut text = self.lines.join("\n");
        if text.is_empty() {
            text.push('\n');
        }
        fs::write(&path, text)?;
        self.file_path = Some(path.clone());
        self.modified = false;
        Ok(path)
    }

    pub(crate) fn file_name_or_default(&self) -> String {
        self.file_path
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .map_or_else(|| "[No Name]".to_string(), String::from)
    }
}

pub(crate) fn byte_index_for_char(line: &str, char_idx: usize) -> usize {
    line.char_indices()
        .nth(char_idx)
        .map_or(line.len(), |(idx, _)| idx)
}

pub(crate) fn remove_char_at(line: &mut String, char_idx: usize) -> Option<char> {
    let start = byte_index_for_char(line, char_idx);
    let end = byte_index_for_char(line, char_idx + 1);
    if start >= end {
        return None;
    }
    let removed = line[start..end].chars().next();
    line.replace_range(start..end, "");
    removed
}

pub(crate) fn slice_chars(line: &str, start: usize, end: usize) -> String {
    if start >= end {
        return String::new();
    }
    line.chars().skip(start).take(end - start).collect()
}

pub(crate) fn find_substring_at_char(line: &str, query: &str, start_char: usize) -> Option<usize> {
    if query.is_empty() {
        return None;
    }

    let haystack: Vec<char> = line.chars().collect();
    let needle: Vec<char> = query.chars().collect();
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    if start_char >= haystack.len() {
        return None;
    }

    let max_start = haystack.len().saturating_sub(needle.len());
    for idx in start_char..=max_start {
        if haystack[idx..idx + needle.len()] == needle[..] {
            return Some(idx);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_temp_path(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir().join(format!("redit-{name}-{nonce}.md"))
    }

    #[test]
    fn delete_forward_merges_lines_at_eol() {
        let mut doc = Document::new_empty(None);
        doc.lines = vec!["abc".to_string(), "def".to_string()];
        doc.delete_forward(Position { x: 3, y: 0 });
        assert_eq!(doc.lines, vec!["abcdef"]);
    }

    #[test]
    fn backspace_merges_with_previous_line() {
        let mut doc = Document::new_empty(None);
        doc.lines = vec!["abc".to_string(), "def".to_string()];
        let cursor = doc.backspace(Position { x: 0, y: 1 });
        assert_eq!(doc.lines, vec!["abcdef"]);
        assert_eq!(cursor, Some(Position { x: 3, y: 0 }));
    }

    #[test]
    fn save_as_writes_file_and_updates_path() {
        let mut doc = Document::new_empty(None);
        doc.lines = vec!["hello".to_string(), "world".to_string()];
        doc.modified = true;

        let path = unique_temp_path("save-as");
        let saved = doc.save_as(path.clone()).expect("save_as should work");
        let content = fs::read_to_string(&path).expect("saved file should be readable");

        assert_eq!(saved, path);
        assert_eq!(content, "hello\nworld");
        assert!(!doc.modified);
        assert_eq!(doc.file_path.as_ref(), Some(&saved));

        let _ = fs::remove_file(path);
    }

    #[test]
    fn save_uses_existing_file_path() {
        let path = unique_temp_path("save");
        let mut doc = Document::new_empty(Some(path.clone()));
        doc.lines = vec!["first".to_string(), "second".to_string()];
        doc.modified = true;

        let saved = doc.save().expect("save should work");
        let content = fs::read_to_string(&path).expect("saved file should be readable");

        assert_eq!(saved, path);
        assert_eq!(content, "first\nsecond");
        assert!(!doc.modified);

        let _ = fs::remove_file(saved);
    }

    #[test]
    fn find_substring_uses_character_offsets() {
        let line = "héllo hello";
        assert_eq!(find_substring_at_char(line, "hello", 0), Some(6));
        assert_eq!(find_substring_at_char(line, "hé", 0), Some(0));
    }
}
