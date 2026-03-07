use std::cmp;
use std::io;
use std::time::Duration;

use crossterm::terminal;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::markdown::{
    MdStyle, is_fenced_code_line, is_indented_code_line, is_setext_underline_line,
    markdown_styles_for_line,
};
use crate::*;

impl Editor {
    fn code_block_state_before(&self, row: usize) -> bool {
        let mut in_code_block = false;
        for line in self.doc.lines.iter().take(row) {
            if is_fenced_code_line(line) {
                in_code_block = !in_code_block;
            }
        }
        in_code_block
    }

    fn markdown_spans_for_window(
        line: &str,
        offset_x: usize,
        width: usize,
        in_code_block: bool,
        setext_heading: bool,
        line_bg: Color,
        selected_range: Option<(usize, usize)>,
    ) -> (Vec<Span<'static>>, usize, bool) {
        let chars: Vec<char> = line.chars().collect();
        let indented_code = !in_code_block && is_indented_code_line(line);
        let styles = markdown_styles_for_line(&chars, in_code_block, setext_heading, indented_code);
        let line_len = chars.len();
        let start = cmp::min(offset_x, line_len);
        let end = cmp::min(start + width, line_len);
        let mut spans = Vec::new();
        let mut rendered = 0usize;

        if start < end {
            let mut current_style = style_for_markdown_char(styles[start], line_bg);
            if selected_range.is_some_and(|(s, e)| (s..e).contains(&start)) {
                current_style = apply_selection_style(current_style);
            }
            let mut segment = String::new();

            for idx in start..end {
                let mut style = style_for_markdown_char(styles[idx], line_bg);
                if selected_range.is_some_and(|(s, e)| (s..e).contains(&idx)) {
                    style = apply_selection_style(style);
                }
                if style != current_style {
                    let text = std::mem::take(&mut segment);
                    rendered += text.chars().count();
                    spans.push(Span::styled(text, current_style));
                    current_style = style;
                }
                segment.push(chars[idx]);
            }

            if !segment.is_empty() {
                rendered += segment.chars().count();
                spans.push(Span::styled(segment, current_style));
            }
        }

        let next_in_code_block = if is_fenced_code_line(line) {
            !in_code_block
        } else {
            in_code_block
        };
        (spans, rendered, next_in_code_block)
    }

    fn build_editor_lines(
        &self,
        text_height: usize,
        gutter: usize,
        body_width: usize,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::with_capacity(text_height);
        let mut in_code_block = self.code_block_state_before(self.offset.y);

        for screen_row in 0..text_height {
            let file_row = self.offset.y + screen_row;
            let line_bg = if file_row == self.cursor.y {
                CRT_LINE_BG
            } else {
                CRT_BG
            };
            let mut spans = vec![Span::styled(
                format!(
                    "{:>width$} ",
                    file_row + 1,
                    width = gutter.saturating_sub(1)
                ),
                Style::default().fg(CRT_DIM_FG).bg(line_bg),
            )];

            if let Some(line) = self.doc.line(file_row) {
                let next_line = self.doc.line(file_row + 1).map(String::as_str);
                let setext_heading = !in_code_block
                    && !line.trim().is_empty()
                    && next_line.is_some_and(is_setext_underline_line);
                let (mut content_spans, rendered, next_state) = Self::markdown_spans_for_window(
                    line,
                    self.offset.x,
                    body_width,
                    in_code_block,
                    setext_heading,
                    line_bg,
                    self.selection_range_for_line(file_row, line.chars().count()),
                );
                spans.append(&mut content_spans);
                if rendered < body_width {
                    spans.push(Span::styled(
                        " ".repeat(body_width - rendered),
                        Style::default().fg(CRT_FG).bg(line_bg),
                    ));
                }
                in_code_block = next_state;
            } else if body_width > 0 {
                spans.push(Span::styled(
                    "~",
                    Style::default().fg(CRT_DIM_FG).bg(line_bg),
                ));
                if body_width > 1 {
                    spans.push(Span::styled(
                        " ".repeat(body_width - 1),
                        Style::default().fg(CRT_FG).bg(line_bg),
                    ));
                }
            }

            lines.push(Line::from(spans));
        }

        lines
    }

    fn build_preview_lines_for_view(
        &self,
        text_height: usize,
        preview_width: usize,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::with_capacity(text_height);
        for screen_row in 0..text_height {
            let file_row = self.offset.y + screen_row;
            if let Some(preview_line) = self.preview_cache_lines.get(file_row) {
                if self.preview_backend == PreviewBackend::Glow {
                    let mut line = ansi_to_line_clipped(preview_line, preview_width);
                    let visible_width = line_char_width(&line);
                    if visible_width < preview_width {
                        line.spans.push(Span::styled(
                            " ".repeat(preview_width - visible_width),
                            Style::default().fg(CRT_FG).bg(CRT_BG),
                        ));
                    }
                    lines.push(line);
                } else {
                    let visible =
                        clip_to_char_width(&strip_ansi_escape_codes(preview_line), preview_width);
                    let visible_width = visible.chars().count();
                    let mut spans = vec![Span::styled(
                        visible,
                        Style::default().fg(CRT_FG).bg(CRT_BG),
                    )];
                    if visible_width < preview_width {
                        spans.push(Span::styled(
                            " ".repeat(preview_width - visible_width),
                            Style::default().fg(CRT_FG).bg(CRT_BG),
                        ));
                    }
                    lines.push(Line::from(spans));
                }
            } else if preview_width > 0 {
                let mut spans = vec![Span::styled(
                    "~",
                    Style::default().fg(CRT_DIM_FG).bg(CRT_BG),
                )];
                if preview_width > 1 {
                    spans.push(Span::styled(
                        " ".repeat(preview_width - 1),
                        Style::default().fg(CRT_FG).bg(CRT_BG),
                    ));
                }
                lines.push(Line::from(spans));
            } else {
                lines.push(Line::raw(String::new()));
            }
        }
        lines
    }

    fn build_separator_lines(text_height: usize) -> Vec<Line<'static>> {
        let separator = clip_to_char_width("│", PREVIEW_SEPARATOR_WIDTH);
        (0..text_height)
            .map(|_| {
                Line::styled(
                    pad_or_clip_to_char_width(&separator, PREVIEW_SEPARATOR_WIDTH),
                    Style::default().fg(CRT_PREVIEW_SEP).bg(CRT_BG),
                )
            })
            .collect()
    }

    fn build_top_menu_line(&self, cols: usize) -> Line<'static> {
        let base = Style::default().fg(CRT_BAR_FG).bg(CRT_BAR_BG);
        let active = Style::default().fg(CRT_ACTIVE_FG).bg(CRT_ACTIVE_BG);
        let mut spans = Vec::new();
        let mut used = 0usize;

        let title = format!(" {APP_NAME} v{APP_VERSION} ");
        let title_width = title.chars().count();
        if title_width <= cols {
            spans.push(Span::styled(title, base));
            used += title_width;
        }

        for (index, (kind, label)) in MENU_ITEMS.iter().enumerate() {
            if used >= cols {
                break;
            }

            let sep = if index == 0 { " " } else { "  " };
            let sep_width = sep.chars().count();
            if sep_width <= cols - used {
                spans.push(Span::styled(sep, base));
                used += sep_width;
            }

            let item = format!(" {label} ");
            let text = clip_to_char_width(&item, cols - used);
            let text_width = text.chars().count();
            if text_width == 0 {
                break;
            }
            let style = if self.active_menu == Some(*kind) {
                active
            } else {
                base
            };
            spans.push(Span::styled(text, style));
            used += text_width;
        }

        if used < cols {
            spans.push(Span::styled(" ".repeat(cols - used), base));
        }

        Line::from(spans)
    }

    fn status_bar_line(&self, cols: usize) -> String {
        let name = self.doc.file_name_or_default();
        let modified = if self.doc.modified { " (modified)" } else { "" };
        let preview = if self.preview_mode {
            match self.preview_backend {
                PreviewBackend::Glow => "Preview:Glow",
                PreviewBackend::Fallback => "Preview:Fallback",
            }
        } else {
            "Preview:OFF"
        };
        let left = format!(
            " {name} | {} lines | {} words{modified} | {preview}",
            self.doc.line_count(),
            self.doc.word_count()
        );
        let right = format!("Ln {}, Col {} ", self.cursor.y + 1, self.cursor.x + 1);
        let right_width = right.chars().count();
        if cols <= right_width {
            return clip_to_char_width(&right, cols);
        }

        let max_left = cols - right_width;
        let mut result = clip_to_char_width(&left, max_left);
        let left_width = result.chars().count();
        if left_width < max_left {
            result.push_str(&" ".repeat(max_left - left_width));
        }
        result.push_str(&right);
        pad_or_clip_to_char_width(&result, cols)
    }

    fn message_bar_line(&self, cols: usize) -> String {
        if self.status.created_at.elapsed() >= Duration::from_secs(5) {
            return " ".repeat(cols);
        }
        let msg = clip_to_char_width(&self.status.text, cols);
        pad_or_clip_to_char_width(&msg, cols)
    }

    fn dropdown_lines(&self, menu: MenuKind, rect: MenuRect) -> Vec<Line<'static>> {
        let entries = Self::menu_entries(menu);
        let inner_height = rect.height.saturating_sub(2);
        let inner_width = rect.width.saturating_sub(2);
        (0..inner_height)
            .map(|idx| {
                let is_selected = idx == self.active_menu_index;
                let style = if is_selected {
                    Style::default().fg(CRT_ACTIVE_FG).bg(CRT_ACTIVE_BG)
                } else {
                    Style::default().fg(CRT_MENU_FG).bg(CRT_MENU_BG)
                };
                let line = entries
                    .get(idx)
                    .map_or_else(|| String::new(), |entry| format!(" {} ", entry.label));
                Line::styled(pad_or_clip_to_char_width(&line, inner_width), style)
            })
            .collect()
    }

    fn build_save_as_popup_render(&self, cols: usize, rows: usize) -> Option<SaveAsPopupRender> {
        let popup = self.save_as_popup.as_ref()?;
        if cols < 28 || rows < 8 {
            return None;
        }

        let max_width = cols.saturating_sub(4);
        let width = cmp::max(28, cmp::min(72, max_width));
        let height = cmp::max(6, cmp::min(7, rows.saturating_sub(2)));
        if width >= cols || height >= rows {
            return None;
        }

        let rect = Rect::new(
            ((cols - width) / 2) as u16,
            ((rows - height) / 2) as u16,
            width as u16,
            height as u16,
        );
        let inner_width = width.saturating_sub(2);
        let inner_height = height.saturating_sub(2);
        let label_style = Style::default().fg(CRT_MENU_FG).bg(CRT_MENU_BG);
        let hint_style = Style::default().fg(CRT_DIM_FG).bg(CRT_MENU_BG);
        let input_style = Style::default().fg(CRT_FG).bg(CRT_INPUT_ACTIVE_BG);
        let selected_input_style = Style::default().fg(CRT_SELECTION_FG).bg(CRT_SELECTION_BG);
        let field_style = if popup.select_all && !popup.path_input.is_empty() {
            selected_input_style
        } else {
            input_style
        };

        let mut lines = vec![
            Line::styled(
                pad_or_clip_to_char_width(" Path:", inner_width),
                label_style,
            ),
            Line::styled(
                pad_or_clip_to_char_width(&popup.path_input, inner_width),
                field_style,
            ),
            Line::styled(
                pad_or_clip_to_char_width(
                    " Enter: save  Ctrl+A: select all  Esc: cancel",
                    inner_width,
                ),
                hint_style,
            ),
        ];
        while lines.len() < inner_height {
            lines.push(Line::styled(
                pad_or_clip_to_char_width("", inner_width),
                Style::default().bg(CRT_MENU_BG),
            ));
        }
        lines.truncate(inner_height);

        let cursor_x = rect.x + 1 + cmp::min(popup.cursor, inner_width.saturating_sub(1)) as u16;
        let cursor_y = rect.y + 2;

        Some(SaveAsPopupRender {
            rect,
            title: " Save As ".to_string(),
            lines,
            cursor: (cursor_x, cursor_y),
        })
    }

    fn build_search_popup_render(&self, cols: usize, rows: usize) -> Option<SearchPopupRender> {
        let popup = self.search_popup.as_ref()?;
        if cols < 24 || rows < 8 {
            return None;
        }

        let max_width = cols.saturating_sub(4);
        let desired_width = match popup.mode {
            SearchPopupMode::Find => 56,
            SearchPopupMode::Replace => 66,
        };
        let width = cmp::max(24, cmp::min(desired_width, max_width));
        let desired_height = match popup.mode {
            SearchPopupMode::Find => 7,
            SearchPopupMode::Replace => 9,
        };
        let height = cmp::max(6, cmp::min(desired_height, rows.saturating_sub(2)));
        if width >= cols || height >= rows {
            return None;
        }

        let rect = Rect::new(
            ((cols - width) / 2) as u16,
            ((rows - height) / 2) as u16,
            width as u16,
            height as u16,
        );
        let inner_width = width.saturating_sub(2);
        let inner_height = height.saturating_sub(2);
        let label_style = Style::default().fg(CRT_MENU_FG).bg(CRT_MENU_BG);
        let hint_style = Style::default().fg(CRT_DIM_FG).bg(CRT_MENU_BG);
        let active_style = Style::default().fg(CRT_FG).bg(CRT_INPUT_ACTIVE_BG);
        let inactive_style = Style::default().fg(CRT_FG).bg(CRT_INPUT_BG);

        let field_line = |value: &str, is_active: bool| -> Line<'static> {
            let style = if is_active {
                active_style
            } else {
                inactive_style
            };
            Line::styled(pad_or_clip_to_char_width(value, inner_width), style)
        };

        let mut lines = Vec::new();
        let (title, cursor) = match popup.mode {
            SearchPopupMode::Find => {
                lines.push(Line::styled(
                    pad_or_clip_to_char_width(" Find:", inner_width),
                    label_style,
                ));
                lines.push(field_line(&popup.find_input, true));
                lines.push(Line::styled(
                    pad_or_clip_to_char_width("", inner_width),
                    Style::default().bg(CRT_MENU_BG),
                ));
                lines.push(Line::styled(
                    pad_or_clip_to_char_width(" Enter: find next   Esc: cancel", inner_width),
                    hint_style,
                ));
                let cursor_x = rect.x
                    + 1
                    + cmp::min(
                        popup.find_input.chars().count(),
                        inner_width.saturating_sub(1),
                    ) as u16;
                let cursor_y = rect.y + 2;
                (" Find ".to_string(), (cursor_x, cursor_y))
            }
            SearchPopupMode::Replace => {
                lines.push(Line::styled(
                    pad_or_clip_to_char_width(" Find:", inner_width),
                    label_style,
                ));
                lines.push(field_line(
                    &popup.find_input,
                    popup.active_field == SearchPopupField::Find,
                ));
                lines.push(Line::styled(
                    pad_or_clip_to_char_width(" Replace:", inner_width),
                    label_style,
                ));
                lines.push(field_line(
                    &popup.replace_input,
                    popup.active_field == SearchPopupField::Replace,
                ));
                lines.push(Line::styled(
                    pad_or_clip_to_char_width("", inner_width),
                    Style::default().bg(CRT_MENU_BG),
                ));
                lines.push(Line::styled(
                    pad_or_clip_to_char_width(
                        " Tab: next field   Enter: apply   Esc: cancel",
                        inner_width,
                    ),
                    hint_style,
                ));
                let (active_value, row_offset) = match popup.active_field {
                    SearchPopupField::Find => (&popup.find_input, 2u16),
                    SearchPopupField::Replace => (&popup.replace_input, 4u16),
                };
                let cursor_x = rect.x
                    + 1
                    + cmp::min(active_value.chars().count(), inner_width.saturating_sub(1)) as u16;
                let cursor_y = rect.y + row_offset;
                (" Replace ".to_string(), (cursor_x, cursor_y))
            }
        };

        while lines.len() < inner_height {
            lines.push(Line::styled(
                pad_or_clip_to_char_width("", inner_width),
                Style::default().bg(CRT_MENU_BG),
            ));
        }
        lines.truncate(inner_height);

        Some(SearchPopupRender {
            rect,
            title,
            lines,
            cursor,
        })
    }

    pub(crate) fn refresh_screen(&mut self) -> io::Result<()> {
        let (cols, rows) = terminal::size()?;
        let cols_usize = usize::from(cols);
        let rows_usize = usize::from(rows);
        let text_height = usize::from(rows.saturating_sub(5));
        let inner_width = cols_usize.saturating_sub(2);
        let gutter = self.gutter_width();
        let body_width = self.editor_body_width(inner_width);
        let preview_layout = self.preview_layout(inner_width);
        if let Some((_, _, preview_width)) = preview_layout {
            self.ensure_preview_cache(preview_width);
        }

        let menu_line = self.build_top_menu_line(cols_usize);
        let editor_lines = self.build_editor_lines(text_height, gutter, body_width);
        let separator_lines = preview_layout.map(|_| Self::build_separator_lines(text_height));
        let preview_lines = preview_layout.map(|(_, _, preview_width)| {
            self.build_preview_lines_for_view(text_height, preview_width)
        });
        let status_line = self.status_bar_line(cols_usize);
        let message_line = self.message_bar_line(cols_usize);
        let dropdown = self.active_menu.and_then(|menu| {
            self.dropdown_rect(menu, cols_usize, rows_usize)
                .map(|rect| (rect, self.dropdown_lines(menu, rect)))
        });
        let save_as_popup = self.build_save_as_popup_render(cols_usize, rows_usize);
        let search_popup = self.build_search_popup_render(cols_usize, rows_usize);
        let cursor_rel_x = self.cursor.x.saturating_sub(self.offset.x) + gutter;
        let cursor_rel_y = self.cursor.y.saturating_sub(self.offset.y);
        let cursor_screen_x = cursor_rel_x + 1;
        let cursor_screen_y = cursor_rel_y + 2;
        let show_editor_cursor = cursor_rel_y < text_height && cursor_rel_x < inner_width;
        let cursor_position = if let Some(popup) = &save_as_popup {
            Some(popup.cursor)
        } else if let Some(popup) = &search_popup {
            Some(popup.cursor)
        } else if show_editor_cursor {
            Some((cursor_screen_x as u16, cursor_screen_y as u16))
        } else {
            None
        };
        let message_active = self.status.created_at.elapsed() < Duration::from_secs(5);

        self.terminal.terminal.draw(|frame| {
            let full_area = frame.area();
            let menu_area = Rect::new(0, 0, full_area.width, 1);
            let body_outer_height = full_area.height.saturating_sub(3);
            let body_outer_area = Rect::new(0, 1, full_area.width, body_outer_height);
            let status_area = Rect::new(0, full_area.height.saturating_sub(2), full_area.width, 1);
            let message_area = Rect::new(0, full_area.height.saturating_sub(1), full_area.width, 1);

            frame.render_widget(Clear, full_area);
            frame.render_widget(
                Paragraph::new(vec![menu_line.clone()])
                    .style(Style::default().fg(CRT_BAR_FG).bg(CRT_BAR_BG)),
                menu_area,
            );

            let mut panel_title = self.doc.file_name_or_default();
            if self.doc.modified {
                panel_title.push_str(" *");
            }
            if preview_layout.is_some() {
                panel_title.push_str(" | Split Preview");
            }
            let body_block = Block::default()
                .title(format!(" {panel_title} "))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(CRT_PANEL_BORDER).bg(CRT_BG));
            frame.render_widget(body_block.clone(), body_outer_area);
            let body_area = body_block.inner(body_outer_area);

            if body_area.height > 0 && body_area.width > 0 {
                if let Some((separator_x, preview_x, _)) = preview_layout {
                    let editor_area = Rect::new(
                        body_area.x,
                        body_area.y,
                        separator_x as u16,
                        body_area.height,
                    );
                    let separator_area = Rect::new(
                        body_area.x + separator_x as u16,
                        body_area.y,
                        PREVIEW_SEPARATOR_WIDTH as u16,
                        body_area.height,
                    );
                    let preview_area = Rect::new(
                        body_area.x + preview_x as u16,
                        body_area.y,
                        body_area.width.saturating_sub(preview_x as u16),
                        body_area.height,
                    );

                    frame.render_widget(
                        Paragraph::new(editor_lines.clone())
                            .style(Style::default().fg(CRT_FG).bg(CRT_BG)),
                        editor_area,
                    );
                    if let Some(lines) = &separator_lines {
                        frame.render_widget(
                            Paragraph::new(lines.clone())
                                .style(Style::default().fg(CRT_PREVIEW_SEP).bg(CRT_BG)),
                            separator_area,
                        );
                    }
                    if let Some(lines) = &preview_lines {
                        frame.render_widget(
                            Paragraph::new(lines.clone())
                                .style(Style::default().fg(CRT_FG).bg(CRT_BG)),
                            preview_area,
                        );
                    }
                } else {
                    frame.render_widget(
                        Paragraph::new(editor_lines.clone())
                            .style(Style::default().fg(CRT_FG).bg(CRT_BG)),
                        body_area,
                    );
                }
            }

            frame.render_widget(
                Paragraph::new(status_line.clone())
                    .style(Style::default().fg(CRT_BAR_FG).bg(CRT_BAR_BG)),
                status_area,
            );
            let message_style = if message_active {
                Style::default().fg(CRT_HEADING_FG).bg(CRT_MESSAGE_BG)
            } else {
                Style::default().fg(CRT_DIM_FG).bg(CRT_BG)
            };
            frame.render_widget(
                Paragraph::new(message_line.clone()).style(message_style),
                message_area,
            );

            if let Some((rect, lines)) = &dropdown {
                let popup = Rect::new(
                    rect.x as u16,
                    rect.y as u16,
                    rect.width as u16,
                    rect.height as u16,
                );
                frame.render_widget(Clear, popup);
                let menu_block = Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(CRT_PANEL_BORDER).bg(CRT_MENU_BG));
                frame.render_widget(menu_block.clone(), popup);
                let inner = menu_block.inner(popup);
                frame.render_widget(Paragraph::new(lines.clone()), inner);
            }

            if let Some(popup) = &search_popup {
                frame.render_widget(Clear, popup.rect);
                let popup_block = Block::default()
                    .title(popup.title.clone())
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(CRT_PANEL_BORDER).bg(CRT_MENU_BG));
                frame.render_widget(popup_block.clone(), popup.rect);
                let inner = popup_block.inner(popup.rect);
                frame.render_widget(
                    Paragraph::new(popup.lines.clone())
                        .style(Style::default().fg(CRT_MENU_FG).bg(CRT_MENU_BG)),
                    inner,
                );
            }

            if let Some(popup) = &save_as_popup {
                frame.render_widget(Clear, popup.rect);
                let popup_block = Block::default()
                    .title(popup.title.clone())
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(CRT_PANEL_BORDER).bg(CRT_MENU_BG));
                frame.render_widget(popup_block.clone(), popup.rect);
                let inner = popup_block.inner(popup.rect);
                frame.render_widget(
                    Paragraph::new(popup.lines.clone())
                        .style(Style::default().fg(CRT_MENU_FG).bg(CRT_MENU_BG)),
                    inner,
                );
            }

            if let Some((x, y)) = cursor_position {
                frame.set_cursor_position((x, y));
            }
        })?;

        if cursor_position.is_some() {
            self.terminal.terminal.show_cursor()?;
        } else {
            self.terminal.terminal.hide_cursor()?;
        }

        Ok(())
    }
}

fn md_style_to_style(style: MdStyle, bg: Color) -> Style {
    match style {
        MdStyle::Normal => Style::default().fg(CRT_FG).bg(bg),
        MdStyle::Heading => Style::default().fg(CRT_FG).bg(bg),
        MdStyle::Quote => Style::default()
            .fg(CRT_FG)
            .bg(bg)
            .add_modifier(Modifier::ITALIC),
        MdStyle::Marker => Style::default().fg(CRT_MARKER_FG).bg(CRT_MARKER_BG),
        MdStyle::Code => Style::default()
            .fg(CRT_FG)
            .bg(bg)
            .add_modifier(Modifier::DIM),
        MdStyle::Emphasis => Style::default()
            .fg(CRT_FG)
            .bg(bg)
            .add_modifier(Modifier::ITALIC),
        MdStyle::Strong => Style::default().fg(CRT_FG).bg(bg),
        MdStyle::EmphasisStrong => Style::default()
            .fg(CRT_FG)
            .bg(bg)
            .add_modifier(Modifier::ITALIC),
        MdStyle::Strike => Style::default()
            .fg(CRT_FG)
            .bg(bg)
            .add_modifier(Modifier::DIM),
        MdStyle::LinkText => Style::default().fg(CRT_FG).bg(bg),
        MdStyle::LinkUrl => Style::default()
            .fg(CRT_FG)
            .bg(bg)
            .add_modifier(Modifier::ITALIC),
        MdStyle::HtmlTag => Style::default().fg(CRT_HTML_TAG_FG).bg(bg),
    }
}

fn style_for_markdown_char(style: MdStyle, bg: Color) -> Style {
    md_style_to_style(style, bg)
}

fn apply_selection_style(style: Style) -> Style {
    style.fg(CRT_SELECTION_FG).bg(CRT_SELECTION_BG)
}
