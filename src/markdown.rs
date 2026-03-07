use std::cmp;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum MdStyle {
    Normal,
    Heading,
    Quote,
    Marker,
    Code,
    Emphasis,
    Strong,
    EmphasisStrong,
    Strike,
    LinkText,
    LinkUrl,
    HtmlTag,
}

pub(crate) fn markdown_styles_for_line(
    chars: &[char],
    in_code_block: bool,
    setext_heading: bool,
    indented_code: bool,
) -> Vec<MdStyle> {
    let len = chars.len();
    let mut styles = vec![MdStyle::Normal; len];
    if len == 0 {
        return styles;
    }

    if is_fenced_code_chars(chars) {
        styles.fill(MdStyle::Code);
        return styles;
    }

    if in_code_block {
        styles.fill(MdStyle::Code);
        return styles;
    }

    if indented_code {
        styles.fill(MdStyle::Code);
        return styles;
    }

    if setext_heading {
        styles.fill(MdStyle::Heading);
    }

    if is_setext_underline_chars(chars) || is_thematic_break_chars(chars) {
        styles.fill(MdStyle::Marker);
        return styles;
    }

    let first_non_ws = chars.iter().position(|c| !c.is_whitespace()).unwrap_or(len);
    if first_non_ws < len {
        if let Some(content_start) = markdown_heading_start(chars, first_non_ws) {
            paint_style_range(&mut styles, first_non_ws, content_start, MdStyle::Marker);
            paint_style_range(&mut styles, content_start, len, MdStyle::Heading);
        } else if chars[first_non_ws] == '>' {
            let marker_end = if first_non_ws + 1 < len && chars[first_non_ws + 1] == ' ' {
                first_non_ws + 2
            } else {
                first_non_ws + 1
            };
            paint_style_range(&mut styles, first_non_ws, marker_end, MdStyle::Marker);
            paint_style_range(&mut styles, marker_end, len, MdStyle::Quote);
        } else if let Some((marker_start, marker_end)) = markdown_list_marker(chars, first_non_ws) {
            paint_style_range(&mut styles, marker_start, marker_end, MdStyle::Marker);
        }
    }

    apply_link_styles(chars, &mut styles);
    apply_html_tag_styles(chars, &mut styles);
    apply_autolink_styles(chars, &mut styles);
    apply_inline_code_styles(chars, &mut styles);
    apply_emphasis_strong_styles(chars, &mut styles);
    apply_strikethrough_styles(chars, &mut styles);
    apply_strong_styles(chars, &mut styles);
    apply_emphasis_styles(chars, &mut styles);
    styles
}

fn paint_style_range(styles: &mut [MdStyle], start: usize, end: usize, style: MdStyle) {
    let end = cmp::min(end, styles.len());
    for slot in styles.iter_mut().take(end).skip(start) {
        *slot = style;
    }
}

fn markdown_heading_start(chars: &[char], start: usize) -> Option<usize> {
    let mut idx = start;
    while idx < chars.len() && chars[idx] == '#' {
        idx += 1;
    }
    let hashes = idx.saturating_sub(start);
    if (1..=6).contains(&hashes) && idx < chars.len() && chars[idx] == ' ' {
        Some(idx + 1)
    } else {
        None
    }
}

fn markdown_list_marker(chars: &[char], start: usize) -> Option<(usize, usize)> {
    if start + 1 < chars.len() && matches!(chars[start], '-' | '*' | '+') && chars[start + 1] == ' '
    {
        return Some((start, start + 2));
    }

    let mut idx = start;
    while idx < chars.len() && chars[idx].is_ascii_digit() {
        idx += 1;
    }
    if idx > start && idx + 1 < chars.len() && chars[idx] == '.' && chars[idx + 1] == ' ' {
        return Some((start, idx + 2));
    }
    None
}

pub(crate) fn is_setext_underline_line(line: &str) -> bool {
    let chars: Vec<char> = line.chars().collect();
    is_setext_underline_chars(&chars)
}

fn is_setext_underline_chars(chars: &[char]) -> bool {
    let start = chars
        .iter()
        .position(|c| !c.is_whitespace())
        .unwrap_or(chars.len());
    if start >= chars.len() {
        return false;
    }

    let marker = chars[start];
    if marker != '=' && marker != '-' {
        return false;
    }

    let mut marker_count = 0usize;
    for c in chars.iter().skip(start) {
        if *c == marker {
            marker_count += 1;
        } else if c.is_whitespace() {
            continue;
        } else {
            return false;
        }
    }
    marker_count >= 1
}

fn is_thematic_break_chars(chars: &[char]) -> bool {
    let start = chars
        .iter()
        .position(|c| !c.is_whitespace())
        .unwrap_or(chars.len());
    if start >= chars.len() {
        return false;
    }

    let marker = chars[start];
    if !matches!(marker, '-' | '*' | '_') {
        return false;
    }

    let mut marker_count = 0usize;
    for c in chars.iter().skip(start) {
        if *c == marker {
            marker_count += 1;
        } else if c.is_whitespace() {
            continue;
        } else {
            return false;
        }
    }
    marker_count >= 3
}

fn apply_link_styles(chars: &[char], styles: &mut [MdStyle]) {
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '[' || is_escaped_marker(chars, i) {
            i += 1;
            continue;
        }
        let Some(close_bracket) =
            (i + 1..chars.len()).find(|&j| chars[j] == ']' && !is_escaped_marker(chars, j))
        else {
            i += 1;
            continue;
        };
        if close_bracket + 1 >= chars.len() || chars[close_bracket + 1] != '(' {
            i += 1;
            continue;
        }
        let Some(close_paren) = (close_bracket + 2..chars.len())
            .find(|&j| chars[j] == ')' && !is_escaped_marker(chars, j))
        else {
            i += 1;
            continue;
        };

        styles[i] = MdStyle::Marker;
        if i > 0 && chars[i - 1] == '!' && !is_escaped_marker(chars, i - 1) {
            styles[i - 1] = MdStyle::Marker;
        }
        styles[close_bracket] = MdStyle::Marker;
        styles[close_bracket + 1] = MdStyle::Marker;
        styles[close_paren] = MdStyle::Marker;
        paint_style_range(styles, i + 1, close_bracket, MdStyle::LinkText);
        paint_style_range(styles, close_bracket + 2, close_paren, MdStyle::LinkUrl);
        i = close_paren + 1;
    }
}

fn apply_html_tag_styles(chars: &[char], styles: &mut [MdStyle]) {
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '<' || is_escaped_marker(chars, i) {
            i += 1;
            continue;
        }

        let mut head = i + 1;
        if head < chars.len() && chars[head] == '/' {
            head += 1;
        }
        if head >= chars.len() {
            i += 1;
            continue;
        }
        if !looks_like_html_tag_head(chars[head]) {
            i += 1;
            continue;
        }

        let Some(end) = (head + 1..chars.len()).find(|&j| chars[j] == '>') else {
            i += 1;
            continue;
        };

        paint_style_range(styles, i, end + 1, MdStyle::HtmlTag);
        i = end + 1;
    }
}

fn looks_like_html_tag_head(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '!' || c == '?'
}

fn apply_autolink_styles(chars: &[char], styles: &mut [MdStyle]) {
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '<' || is_escaped_marker(chars, i) {
            i += 1;
            continue;
        }

        let Some(end) =
            (i + 1..chars.len()).find(|&j| chars[j] == '>' && !is_escaped_marker(chars, j))
        else {
            i += 1;
            continue;
        };

        if end <= i + 1 {
            i += 1;
            continue;
        }

        if !is_autolink_target(&chars[i + 1..end]) {
            i += 1;
            continue;
        }

        styles[i] = MdStyle::Marker;
        styles[end] = MdStyle::Marker;
        paint_style_range(styles, i + 1, end, MdStyle::LinkUrl);
        i = end + 1;
    }
}

fn is_autolink_target(content: &[char]) -> bool {
    if content.is_empty() || content.iter().any(|c| c.is_whitespace()) {
        return false;
    }

    let text: String = content.iter().collect();
    if text.starts_with("http://") || text.starts_with("https://") || text.starts_with("mailto:") {
        return true;
    }

    let mut parts = text.split('@');
    let local = parts.next().unwrap_or_default();
    let domain = parts.next().unwrap_or_default();
    if parts.next().is_some() {
        return false;
    }
    !local.is_empty() && domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.')
}

fn apply_inline_code_styles(chars: &[char], styles: &mut [MdStyle]) {
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '`' || is_escaped_marker(chars, i) {
            i += 1;
            continue;
        }
        let Some(end) =
            (i + 1..chars.len()).find(|&j| chars[j] == '`' && !is_escaped_marker(chars, j))
        else {
            styles[i] = MdStyle::Marker;
            i += 1;
            continue;
        };
        styles[i] = MdStyle::Marker;
        styles[end] = MdStyle::Marker;
        paint_style_range(styles, i + 1, end, MdStyle::Code);
        i = end + 1;
    }
}

fn apply_emphasis_strong_styles(chars: &[char], styles: &mut [MdStyle]) {
    apply_triple_delimited_style(chars, styles, '*');
    apply_triple_delimited_style(chars, styles, '_');
}

fn apply_strikethrough_styles(chars: &[char], styles: &mut [MdStyle]) {
    apply_double_delimited_style(chars, styles, '~', MdStyle::Strike);
}

fn apply_strong_styles(chars: &[char], styles: &mut [MdStyle]) {
    apply_double_delimited_style(chars, styles, '*', MdStyle::Strong);
    apply_double_delimited_style(chars, styles, '_', MdStyle::Strong);
}

fn apply_emphasis_styles(chars: &[char], styles: &mut [MdStyle]) {
    apply_single_delimited_style(chars, styles, '*', MdStyle::Emphasis);
    apply_single_delimited_style(chars, styles, '_', MdStyle::Emphasis);
}

fn apply_triple_delimited_style(chars: &[char], styles: &mut [MdStyle], marker: char) {
    let len = chars.len();
    let mut i = 0;
    while i + 2 < len {
        if chars[i] != marker
            || chars[i + 1] != marker
            || chars[i + 2] != marker
            || is_escaped_marker(chars, i)
            || !can_restyle_span(styles, i, i + 3)
        {
            i += 1;
            continue;
        }

        if marker == '_'
            && i > 0
            && i + 3 < len
            && chars[i - 1].is_ascii_alphanumeric()
            && chars[i + 3].is_ascii_alphanumeric()
        {
            i += 1;
            continue;
        }

        let content_start = i + 3;
        if content_start >= len || chars[content_start].is_whitespace() {
            i += 1;
            continue;
        }

        let mut j = content_start;
        let mut found = None;
        while j + 2 < len {
            if chars[j] == marker
                && chars[j + 1] == marker
                && chars[j + 2] == marker
                && !is_escaped_marker(chars, j)
                && can_restyle_span(styles, j, j + 3)
                && j > content_start
                && !chars[j - 1].is_whitespace()
                && can_restyle_span(styles, content_start, j)
                && chars[content_start..j].iter().any(|c| !c.is_whitespace())
            {
                if marker == '_'
                    && j > 0
                    && j + 3 < len
                    && chars[j - 1].is_ascii_alphanumeric()
                    && chars[j + 3].is_ascii_alphanumeric()
                {
                    j += 1;
                    continue;
                }
                found = Some(j);
                break;
            }
            j += 1;
        }

        if let Some(end) = found {
            styles[i] = MdStyle::Marker;
            styles[i + 1] = MdStyle::Marker;
            styles[i + 2] = MdStyle::Marker;
            styles[end] = MdStyle::Marker;
            styles[end + 1] = MdStyle::Marker;
            styles[end + 2] = MdStyle::Marker;
            paint_style_range(styles, content_start, end, MdStyle::EmphasisStrong);
            i = end + 3;
        } else {
            i += 1;
        }
    }
}

fn apply_double_delimited_style(
    chars: &[char],
    styles: &mut [MdStyle],
    marker: char,
    fill_style: MdStyle,
) {
    let len = chars.len();
    let mut i = 0;
    while i + 1 < len {
        if chars[i] != marker
            || chars[i + 1] != marker
            || is_escaped_marker(chars, i)
            || !can_restyle_span(styles, i, i + 2)
        {
            i += 1;
            continue;
        }
        if marker == '_'
            && i > 0
            && i + 2 < len
            && chars[i - 1].is_ascii_alphanumeric()
            && chars[i + 2].is_ascii_alphanumeric()
        {
            i += 1;
            continue;
        }

        let content_start = i + 2;
        if content_start >= len || chars[content_start].is_whitespace() {
            i += 1;
            continue;
        }

        let mut j = content_start;
        let mut found = None;
        while j + 1 < len {
            if chars[j] == marker
                && chars[j + 1] == marker
                && !is_escaped_marker(chars, j)
                && can_restyle_span(styles, j, j + 2)
                && j > content_start
                && !chars[j - 1].is_whitespace()
                && can_restyle_span(styles, content_start, j)
                && chars[content_start..j].iter().any(|c| !c.is_whitespace())
            {
                if marker == '_'
                    && j > 0
                    && j + 2 < len
                    && chars[j - 1].is_ascii_alphanumeric()
                    && chars[j + 2].is_ascii_alphanumeric()
                {
                    j += 1;
                    continue;
                }
                found = Some(j);
                break;
            }
            j += 1;
        }

        if let Some(end) = found {
            styles[i] = MdStyle::Marker;
            styles[i + 1] = MdStyle::Marker;
            styles[end] = MdStyle::Marker;
            styles[end + 1] = MdStyle::Marker;
            paint_style_range(styles, content_start, end, fill_style);
            i = end + 2;
        } else {
            i += 1;
        }
    }
}

fn apply_single_delimited_style(
    chars: &[char],
    styles: &mut [MdStyle],
    marker: char,
    fill_style: MdStyle,
) {
    let len = chars.len();
    let mut i = 0;
    while i < len {
        if chars[i] != marker || is_escaped_marker(chars, i) || !can_restyle_span(styles, i, i + 1)
        {
            i += 1;
            continue;
        }
        if (i > 0 && chars[i - 1] == marker) || (i + 1 < len && chars[i + 1] == marker) {
            i += 1;
            continue;
        }
        if marker == '_'
            && i > 0
            && i + 1 < len
            && chars[i - 1].is_ascii_alphanumeric()
            && chars[i + 1].is_ascii_alphanumeric()
        {
            i += 1;
            continue;
        }

        let content_start = i + 1;
        if content_start >= len || chars[content_start].is_whitespace() {
            i += 1;
            continue;
        }

        let mut j = content_start;
        let mut found = None;
        while j < len {
            if chars[j] == marker
                && !is_escaped_marker(chars, j)
                && can_restyle_span(styles, j, j + 1)
                && j > content_start
                && !chars[j - 1].is_whitespace()
                && can_restyle_span(styles, content_start, j)
                && chars[content_start..j].iter().any(|c| !c.is_whitespace())
                && (j == 0 || chars[j - 1] != marker)
                && (j + 1 >= len || chars[j + 1] != marker)
            {
                if marker == '_'
                    && j > 0
                    && j + 1 < len
                    && chars[j - 1].is_ascii_alphanumeric()
                    && chars[j + 1].is_ascii_alphanumeric()
                {
                    j += 1;
                    continue;
                }
                found = Some(j);
                break;
            }
            j += 1;
        }

        if let Some(end) = found {
            styles[i] = MdStyle::Marker;
            styles[end] = MdStyle::Marker;
            paint_style_range(styles, content_start, end, fill_style);
            i = end + 1;
        } else {
            i += 1;
        }
    }
}

fn can_restyle_span(styles: &[MdStyle], start: usize, end: usize) -> bool {
    if start >= end || end > styles.len() {
        return false;
    }
    styles[start..end]
        .iter()
        .all(|style| matches!(style, MdStyle::Normal | MdStyle::Heading | MdStyle::Quote))
}

fn is_escaped_marker(chars: &[char], idx: usize) -> bool {
    if idx == 0 {
        return false;
    }

    let mut backslashes = 0usize;
    let mut pos = idx;
    while pos > 0 {
        pos -= 1;
        if chars[pos] == '\\' {
            backslashes += 1;
        } else {
            break;
        }
    }
    backslashes % 2 == 1
}

pub(crate) fn is_fenced_code_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("```") || trimmed.starts_with("~~~")
}

fn is_fenced_code_chars(chars: &[char]) -> bool {
    let start = chars
        .iter()
        .position(|c| !c.is_whitespace())
        .unwrap_or(chars.len());
    if start + 2 >= chars.len() {
        return false;
    }
    (chars[start] == '`' && chars[start + 1] == '`' && chars[start + 2] == '`')
        || (chars[start] == '~' && chars[start + 1] == '~' && chars[start + 2] == '~')
}

pub(crate) fn is_indented_code_line(line: &str) -> bool {
    line.starts_with("    ") || line.starts_with('\t')
}

pub(crate) fn markdown_list_continuation(before_cursor: &str) -> Option<String> {
    let chars: Vec<char> = before_cursor.chars().collect();
    if chars.is_empty() {
        return None;
    }
    let indent_end = chars
        .iter()
        .position(|c| !c.is_whitespace())
        .unwrap_or(chars.len());
    if indent_end >= chars.len() {
        return None;
    }

    let indent: String = chars[..indent_end].iter().collect();
    let start = indent_end;

    if start + 1 < chars.len() && matches!(chars[start], '-' | '*' | '+') && chars[start + 1] == ' '
    {
        let remainder: String = chars[start + 2..].iter().collect();
        if remainder.trim().is_empty() {
            return None;
        }
        return Some(format!("{indent}{} ", chars[start]));
    }

    let mut idx = start;
    while idx < chars.len() && chars[idx].is_ascii_digit() {
        idx += 1;
    }
    if idx > start && idx + 1 < chars.len() && chars[idx] == '.' && chars[idx + 1] == ' ' {
        let remainder: String = chars[idx + 2..].iter().collect();
        if remainder.trim().is_empty() {
            return None;
        }
        let number: String = chars[start..idx].iter().collect();
        if let Ok(value) = number.parse::<usize>() {
            return Some(format!("{indent}{}. ", value + 1));
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_continuation_for_unordered_list() {
        assert_eq!(markdown_list_continuation("- item"), Some("- ".to_string()));
    }

    #[test]
    fn list_continuation_for_ordered_list() {
        assert_eq!(
            markdown_list_continuation("  3. item"),
            Some("  4. ".to_string())
        );
    }

    #[test]
    fn heading_marker_styles_markers_and_content() {
        let chars: Vec<char> = "# Title".chars().collect();
        let styles = markdown_styles_for_line(&chars, false, false, false);
        assert_eq!(styles[0], MdStyle::Marker);
        assert_eq!(styles[2], MdStyle::Heading);
    }
}
