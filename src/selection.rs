use crate::document::{Position, byte_index_for_char, slice_chars};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct SelectionRange {
    pub(crate) start: Position,
    pub(crate) end: Position,
}

pub(crate) fn normalized_selection(
    anchor: Option<Position>,
    cursor: Position,
) -> Option<SelectionRange> {
    let anchor = anchor?;
    if anchor == cursor {
        return None;
    }
    if (anchor.y, anchor.x) <= (cursor.y, cursor.x) {
        Some(SelectionRange {
            start: anchor,
            end: cursor,
        })
    } else {
        Some(SelectionRange {
            start: cursor,
            end: anchor,
        })
    }
}

pub(crate) fn selection_range_for_line(
    selection: SelectionRange,
    line_idx: usize,
    line_len: usize,
) -> Option<(usize, usize)> {
    if line_idx < selection.start.y || line_idx > selection.end.y {
        return None;
    }

    let start = if line_idx == selection.start.y {
        selection.start.x
    } else {
        0
    };
    let end = if line_idx == selection.end.y {
        selection.end.x
    } else {
        line_len
    };

    (start < end).then_some((start.min(line_len), end.min(line_len)))
}

pub(crate) fn selected_text_in_lines(lines: &[String], selection: SelectionRange) -> String {
    if selection.start.y == selection.end.y {
        return lines
            .get(selection.start.y)
            .map_or_else(String::new, |line| {
                slice_chars(line, selection.start.x, selection.end.x)
            });
    }

    let mut parts = Vec::new();
    let first_line = lines
        .get(selection.start.y)
        .map(|line| slice_chars(line, selection.start.x, line.chars().count()))
        .unwrap_or_default();
    parts.push(first_line);

    for line_idx in selection.start.y + 1..selection.end.y {
        if let Some(line) = lines.get(line_idx) {
            parts.push(line.clone());
        }
    }

    let last_line = lines
        .get(selection.end.y)
        .map(|line| slice_chars(line, 0, selection.end.x))
        .unwrap_or_default();
    parts.push(last_line);

    parts.join("\n")
}

pub(crate) fn delete_selection_in_lines(
    lines: &mut Vec<String>,
    selection: SelectionRange,
) -> Position {
    if selection.start.y == selection.end.y {
        if let Some(line) = lines.get_mut(selection.start.y) {
            let start = byte_index_for_char(line, selection.start.x);
            let end = byte_index_for_char(line, selection.end.x);
            line.replace_range(start..end, "");
        }
    } else {
        let first_prefix = lines
            .get(selection.start.y)
            .map(|line| slice_chars(line, 0, selection.start.x))
            .unwrap_or_default();
        let last_suffix = lines
            .get(selection.end.y)
            .map(|line| slice_chars(line, selection.end.x, line.chars().count()))
            .unwrap_or_default();

        if let Some(line) = lines.get_mut(selection.start.y) {
            *line = format!("{first_prefix}{last_suffix}");
        }

        if selection.start.y < selection.end.y && selection.end.y < lines.len() {
            lines.drain(selection.start.y + 1..=selection.end.y);
        }
    }

    if lines.is_empty() {
        lines.push(String::new());
    }

    selection.start
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_text_single_line() {
        let lines = vec!["hello world".to_string()];
        let selection = SelectionRange {
            start: Position { x: 0, y: 0 },
            end: Position { x: 5, y: 0 },
        };
        assert_eq!(selected_text_in_lines(&lines, selection), "hello");
    }

    #[test]
    fn selected_text_multi_line() {
        let lines = vec!["alpha".to_string(), "beta".to_string(), "gamma".to_string()];
        let selection = SelectionRange {
            start: Position { x: 2, y: 0 },
            end: Position { x: 2, y: 2 },
        };
        assert_eq!(selected_text_in_lines(&lines, selection), "pha\nbeta\nga");
    }

    #[test]
    fn delete_selection_multi_line_merges_edges() {
        let mut lines = vec!["alpha".to_string(), "beta".to_string(), "gamma".to_string()];
        let selection = SelectionRange {
            start: Position { x: 2, y: 0 },
            end: Position { x: 2, y: 2 },
        };
        let cursor = delete_selection_in_lines(&mut lines, selection);
        assert_eq!(lines, vec!["almma"]);
        assert_eq!(cursor, Position { x: 2, y: 0 });
    }

    #[test]
    fn normalize_selection_orders_points() {
        let anchor = Some(Position { x: 10, y: 3 });
        let cursor = Position { x: 2, y: 1 };
        let normalized = normalized_selection(anchor, cursor).expect("range should exist");
        assert_eq!(normalized.start, cursor);
        assert_eq!(normalized.end, Position { x: 10, y: 3 });
    }
}
