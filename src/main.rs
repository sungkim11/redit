use std::cmp;
use std::env;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{self, Stdout, Write, stdout};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::execute;
use crossterm::style::ResetColor;
use crossterm::terminal::{
    self, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

fn main() -> io::Result<()> {
    let file_arg = env::args().nth(1).map(PathBuf::from);
    let mut editor = Editor::new(file_arg)?;
    editor.run()
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct Position {
    x: usize,
    y: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct SelectionRange {
    start: Position,
    end: Position,
}

struct StatusMessage {
    text: String,
    created_at: Instant,
}

impl StatusMessage {
    fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            created_at: Instant::now(),
        }
    }
}

struct Document {
    lines: Vec<String>,
    file_path: Option<PathBuf>,
    modified: bool,
}

impl Document {
    fn new_empty(file_path: Option<PathBuf>) -> Self {
        Self {
            lines: vec![String::new()],
            file_path,
            modified: false,
        }
    }

    fn open(path: PathBuf) -> io::Result<Self> {
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

    fn line_count(&self) -> usize {
        self.lines.len()
    }

    fn word_count(&self) -> usize {
        self.lines
            .iter()
            .map(|line| line.split_whitespace().count())
            .sum()
    }

    fn line(&self, index: usize) -> Option<&String> {
        self.lines.get(index)
    }

    fn line_char_len(&self, index: usize) -> usize {
        self.line(index).map_or(0, |line| line.chars().count())
    }

    fn insert_char(&mut self, pos: Position, ch: char) {
        if pos.y >= self.lines.len() {
            self.lines.push(String::new());
        }
        if let Some(line) = self.lines.get_mut(pos.y) {
            let idx = byte_index_for_char(line, pos.x);
            line.insert(idx, ch);
            self.modified = true;
        }
    }

    fn insert_newline(&mut self, pos: Position) {
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

    fn backspace(&mut self, pos: Position) -> Option<Position> {
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

    fn delete_forward(&mut self, pos: Position) {
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

    fn save(&mut self) -> io::Result<PathBuf> {
        let path = self
            .file_path
            .clone()
            .unwrap_or_else(|| PathBuf::from("redit.md"));
        self.save_as(path)
    }

    fn save_as(&mut self, path: PathBuf) -> io::Result<PathBuf> {
        let mut text = self.lines.join("\n");
        if text.is_empty() {
            text.push('\n');
        }
        fs::write(&path, text)?;
        self.file_path = Some(path.clone());
        self.modified = false;
        Ok(path)
    }

    fn file_name_or_default(&self) -> String {
        self.file_path
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .map_or_else(|| "[No Name]".to_string(), String::from)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MenuKind {
    File,
    Edit,
    Search,
    Help,
}

#[derive(Clone, Copy)]
struct MenuEntry {
    label: &'static str,
    mnemonic: char,
    action: MenuAction,
}

#[derive(Clone, Copy)]
struct MenuRect {
    x: usize,
    y: usize,
    width: usize,
    height: usize,
}

#[derive(Clone, PartialEq, Eq)]
struct EditorSnapshot {
    lines: Vec<String>,
    cursor: Position,
    offset: Position,
    selection_anchor: Option<Position>,
    modified: bool,
}

#[derive(Clone, Copy)]
enum MenuAction {
    Save,
    SaveAs,
    Quit,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    Find,
    Replace,
    TogglePreview,
    Keybindings,
    About,
}

const MENU_ITEMS: &[(MenuKind, &str)] = &[
    (MenuKind::File, "File"),
    (MenuKind::Edit, "Edit"),
    (MenuKind::Search, "Search"),
    (MenuKind::Help, "Help"),
];

const APP_NAME: &str = "redit";
const APP_VERSION: &str = "0.1";

const FILE_MENU_ENTRIES: &[MenuEntry] = &[
    MenuEntry {
        label: "Save        Ctrl+S",
        mnemonic: 's',
        action: MenuAction::Save,
    },
    MenuEntry {
        label: "Save As...  Ctrl+Shift+S",
        mnemonic: 'a',
        action: MenuAction::SaveAs,
    },
    MenuEntry {
        label: "Quit        Ctrl+Q",
        mnemonic: 'q',
        action: MenuAction::Quit,
    },
];

const EDIT_MENU_ENTRIES: &[MenuEntry] = &[
    MenuEntry {
        label: "Undo        Ctrl+Z",
        mnemonic: 'u',
        action: MenuAction::Undo,
    },
    MenuEntry {
        label: "Redo        Ctrl+Y",
        mnemonic: 'r',
        action: MenuAction::Redo,
    },
    MenuEntry {
        label: "Cut         Ctrl+X",
        mnemonic: 't',
        action: MenuAction::Cut,
    },
    MenuEntry {
        label: "Copy        Ctrl+C",
        mnemonic: 'c',
        action: MenuAction::Copy,
    },
    MenuEntry {
        label: "Paste       Ctrl+V",
        mnemonic: 'p',
        action: MenuAction::Paste,
    },
];

const SEARCH_MENU_ENTRIES: &[MenuEntry] = &[
    MenuEntry {
        label: "Find        Ctrl+F",
        mnemonic: 'f',
        action: MenuAction::Find,
    },
    MenuEntry {
        label: "Replace     Ctrl+R",
        mnemonic: 'r',
        action: MenuAction::Replace,
    },
];

const HELP_MENU_ENTRIES: &[MenuEntry] = &[
    MenuEntry {
        label: "Keybindings F1",
        mnemonic: 'k',
        action: MenuAction::Keybindings,
    },
    MenuEntry {
        label: "Toggle Preview Ctrl+P",
        mnemonic: 'p',
        action: MenuAction::TogglePreview,
    },
    MenuEntry {
        label: "About redit v0.1",
        mnemonic: 'a',
        action: MenuAction::About,
    },
];

const PREVIEW_MIN_TOTAL_WIDTH: usize = 56;
const PREVIEW_SEPARATOR_WIDTH: usize = 1;

#[derive(Clone, Copy, PartialEq, Eq)]
enum PreviewBackend {
    Glow,
    Fallback,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SearchPopupMode {
    Find,
    Replace,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SearchPopupField {
    Find,
    Replace,
}

#[derive(Clone)]
struct SearchPopupState {
    mode: SearchPopupMode,
    find_input: String,
    replace_input: String,
    active_field: SearchPopupField,
}

#[derive(Clone)]
struct SearchPopupRender {
    rect: Rect,
    title: String,
    lines: Vec<Line<'static>>,
    cursor: (u16, u16),
}

#[derive(Clone)]
struct SaveAsPopupState {
    path_input: String,
    cursor: usize,
    select_all: bool,
}

#[derive(Clone)]
struct SaveAsPopupRender {
    rect: Rect,
    title: String,
    lines: Vec<Line<'static>>,
    cursor: (u16, u16),
}

impl SearchPopupState {
    fn find(initial_query: &str) -> Self {
        Self {
            mode: SearchPopupMode::Find,
            find_input: initial_query.to_string(),
            replace_input: String::new(),
            active_field: SearchPopupField::Find,
        }
    }

    fn replace(initial_query: &str) -> Self {
        Self {
            mode: SearchPopupMode::Replace,
            find_input: initial_query.to_string(),
            replace_input: String::new(),
            active_field: SearchPopupField::Find,
        }
    }

    fn active_field_mut(&mut self) -> &mut String {
        match self.active_field {
            SearchPopupField::Find => &mut self.find_input,
            SearchPopupField::Replace => &mut self.replace_input,
        }
    }
}

const CRT_BG: Color = Color::Rgb(2, 12, 4);
const CRT_FG: Color = Color::Rgb(255, 255, 255);
const CRT_DIM_FG: Color = Color::Rgb(55, 148, 79);
const CRT_BAR_BG: Color = Color::Rgb(16, 62, 30);
const CRT_BAR_FG: Color = Color::Rgb(185, 255, 205);
const CRT_ACTIVE_BG: Color = Color::Rgb(170, 255, 170);
const CRT_ACTIVE_FG: Color = Color::Black;
const CRT_MENU_BG: Color = Color::Rgb(7, 36, 18);
const CRT_MENU_FG: Color = Color::Rgb(152, 245, 176);
const CRT_HEADING_FG: Color = Color::Rgb(116, 170, 255);
const CRT_HTML_TAG_FG: Color = Color::Rgb(132, 190, 255);
const CRT_MARKER_FG: Color = Color::Rgb(92, 166, 255);
const CRT_MARKER_BG: Color = Color::Rgb(7, 18, 44);
const CRT_LINE_BG: Color = Color::Rgb(9, 24, 12);
const CRT_PANEL_BORDER: Color = Color::Rgb(64, 164, 94);
const CRT_PREVIEW_SEP: Color = Color::Rgb(47, 120, 68);
const CRT_MESSAGE_BG: Color = Color::Rgb(11, 33, 17);
const CRT_INPUT_BG: Color = Color::Rgb(4, 24, 12);
const CRT_INPUT_ACTIVE_BG: Color = Color::Rgb(7, 18, 44);
const CRT_SELECTION_BG: Color = Color::Rgb(18, 72, 184);
const CRT_SELECTION_FG: Color = Color::Rgb(255, 255, 255);

#[derive(Clone, Copy, PartialEq, Eq)]
enum MdStyle {
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

#[derive(Clone)]
struct MarkdownContinuation {
    prefix: String,
}

struct Editor {
    terminal: TerminalGuard,
    doc: Document,
    cursor: Position,
    offset: Position,
    status: StatusMessage,
    active_menu: Option<MenuKind>,
    active_menu_index: usize,
    preview_mode: bool,
    preview_cache_lines: Vec<String>,
    preview_cache_width: usize,
    preview_cache_revision: u64,
    preview_backend: PreviewBackend,
    preview_error: Option<String>,
    clipboard: String,
    undo_stack: Vec<EditorSnapshot>,
    redo_stack: Vec<EditorSnapshot>,
    selection_anchor: Option<Position>,
    mouse_drag_anchor: Option<Position>,
    save_as_popup: Option<SaveAsPopupState>,
    search_popup: Option<SearchPopupState>,
    last_search_query: String,
    should_quit: bool,
    quit_warning_countdown: u8,
}

impl Editor {
    fn new(file_arg: Option<PathBuf>) -> io::Result<Self> {
        let doc = match file_arg {
            Some(path) if Path::new(&path).exists() => Document::open(path)?,
            Some(path) => Document::new_empty(Some(path)),
            None => Document::new_empty(None),
        };

        Ok(Self {
            terminal: TerminalGuard::new()?,
            doc,
            cursor: Position::default(),
            offset: Position::default(),
            status: StatusMessage::new(
                "Alt-F/E/S/H menus | Ctrl-S save | Ctrl-Q quit | Shift+Arrows select | F1 help",
            ),
            active_menu: None,
            active_menu_index: 0,
            preview_mode: false,
            preview_cache_lines: Vec::new(),
            preview_cache_width: 0,
            preview_cache_revision: 0,
            preview_backend: PreviewBackend::Fallback,
            preview_error: None,
            clipboard: String::new(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            selection_anchor: None,
            mouse_drag_anchor: None,
            save_as_popup: None,
            search_popup: None,
            last_search_query: String::new(),
            should_quit: false,
            quit_warning_countdown: 1,
        })
    }

    fn run(&mut self) -> io::Result<()> {
        while !self.should_quit {
            self.refresh_screen()?;
            if event::poll(Duration::from_millis(250))? {
                self.process_event()?;
            }
        }
        Ok(())
    }

    fn process_event(&mut self) -> io::Result<()> {
        match event::read()? {
            Event::Key(key) => {
                if key.kind == KeyEventKind::Release {
                    return Ok(());
                }
                self.handle_key(key);
            }
            Event::Mouse(mouse) => self.handle_mouse(mouse),
            _ => {}
        }
        Ok(())
    }

    fn handle_key(&mut self, key: KeyEvent) {
        if self.save_as_popup.is_some() {
            if self.handle_save_as_popup_key(key) {
                self.scroll();
                return;
            }
        }

        if self.search_popup.is_some() {
            if self.handle_search_popup_key(key) {
                self.scroll();
                return;
            }
        }

        if let Some(menu) = self.active_menu {
            if self.handle_menu_mode_key(menu, key) {
                self.scroll();
                return;
            }
            self.active_menu = None;
            self.active_menu_index = 0;
        }

        match key {
            KeyEvent {
                code: KeyCode::Char(c),
                modifiers,
                ..
            } if modifiers.contains(KeyModifiers::ALT) => self.handle_alt_menu(c),
            KeyEvent {
                code: KeyCode::Esc, ..
            } => {
                self.clear_selection();
            }
            KeyEvent {
                code: KeyCode::Char('q'),
                modifiers,
                ..
            } if modifiers.contains(KeyModifiers::CONTROL) => {
                self.invoke_menu_action(MenuAction::Quit)
            }
            KeyEvent {
                code: KeyCode::Char(c),
                modifiers,
                ..
            } if modifiers.contains(KeyModifiers::CONTROL)
                && modifiers.contains(KeyModifiers::SHIFT)
                && c.eq_ignore_ascii_case(&'s') =>
            {
                self.invoke_menu_action(MenuAction::SaveAs)
            }
            KeyEvent {
                code: KeyCode::Char(c),
                modifiers,
                ..
            } if modifiers.contains(KeyModifiers::CONTROL) && c.eq_ignore_ascii_case(&'s') => {
                self.invoke_menu_action(MenuAction::Save)
            }
            KeyEvent {
                code: KeyCode::Char('z'),
                modifiers,
                ..
            } if modifiers.contains(KeyModifiers::CONTROL) => {
                self.invoke_menu_action(MenuAction::Undo)
            }
            KeyEvent {
                code: KeyCode::Char('y'),
                modifiers,
                ..
            } if modifiers.contains(KeyModifiers::CONTROL) => {
                self.invoke_menu_action(MenuAction::Redo)
            }
            KeyEvent {
                code: KeyCode::Char('x'),
                modifiers,
                ..
            } if modifiers.contains(KeyModifiers::CONTROL) => {
                self.invoke_menu_action(MenuAction::Cut)
            }
            KeyEvent {
                code: KeyCode::Char('c'),
                modifiers,
                ..
            } if modifiers.contains(KeyModifiers::CONTROL) => {
                self.invoke_menu_action(MenuAction::Copy)
            }
            KeyEvent {
                code: KeyCode::Char('v'),
                modifiers,
                ..
            } if modifiers.contains(KeyModifiers::CONTROL) => {
                self.invoke_menu_action(MenuAction::Paste)
            }
            KeyEvent {
                code: KeyCode::Char('f'),
                modifiers,
                ..
            } if modifiers.contains(KeyModifiers::CONTROL) => {
                self.invoke_menu_action(MenuAction::Find)
            }
            KeyEvent {
                code: KeyCode::Char('r'),
                modifiers,
                ..
            } if modifiers.contains(KeyModifiers::CONTROL) => {
                self.invoke_menu_action(MenuAction::Replace)
            }
            KeyEvent {
                code: KeyCode::Char('p'),
                modifiers,
                ..
            } if modifiers.contains(KeyModifiers::CONTROL) => {
                self.invoke_menu_action(MenuAction::TogglePreview)
            }
            KeyEvent {
                code: KeyCode::F(1),
                ..
            } => self.invoke_menu_action(MenuAction::Keybindings),
            KeyEvent {
                code: KeyCode::Left,
                modifiers,
                ..
            } => self.move_cursor_left_with_selection(shift_only(modifiers)),
            KeyEvent {
                code: KeyCode::Right,
                modifiers,
                ..
            } => self.move_cursor_right_with_selection(shift_only(modifiers)),
            KeyEvent {
                code: KeyCode::Up,
                modifiers,
                ..
            } => self.move_cursor_up_with_selection(shift_only(modifiers)),
            KeyEvent {
                code: KeyCode::Down,
                modifiers,
                ..
            } => self.move_cursor_down_with_selection(shift_only(modifiers)),
            KeyEvent {
                code: KeyCode::PageUp,
                modifiers,
                ..
            } => self.page_up_with_selection(shift_only(modifiers)),
            KeyEvent {
                code: KeyCode::PageDown,
                modifiers,
                ..
            } => self.page_down_with_selection(shift_only(modifiers)),
            KeyEvent {
                code: KeyCode::Home,
                modifiers,
                ..
            } => self.home_with_selection(shift_only(modifiers)),
            KeyEvent {
                code: KeyCode::End,
                modifiers,
                ..
            } => self.end_with_selection(shift_only(modifiers)),
            KeyEvent {
                code: KeyCode::Backspace,
                ..
            } => {
                self.begin_edit();
                self.backspace();
            }
            KeyEvent {
                code: KeyCode::Delete,
                ..
            } => {
                self.begin_edit();
                self.delete_forward();
            }
            KeyEvent {
                code: KeyCode::Enter,
                ..
            } => {
                self.begin_edit();
                self.insert_newline();
            }
            KeyEvent {
                code: KeyCode::Tab, ..
            } => {
                self.begin_edit();
                for _ in 0..4 {
                    self.insert_char(' ');
                }
            }
            KeyEvent {
                code: KeyCode::Char(c),
                modifiers,
                ..
            } if !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
                self.begin_edit();
                self.insert_char(c)
            }
            _ => {}
        }
        self.scroll();
    }

    fn handle_menu_mode_key(&mut self, menu: MenuKind, key: KeyEvent) -> bool {
        match key {
            KeyEvent {
                code: KeyCode::Esc, ..
            } => {
                self.active_menu = None;
                self.active_menu_index = 0;
                true
            }
            KeyEvent {
                code: KeyCode::Left,
                ..
            } => {
                self.open_menu(self.prev_menu(menu));
                true
            }
            KeyEvent {
                code: KeyCode::Right,
                ..
            } => {
                self.open_menu(self.next_menu(menu));
                true
            }
            KeyEvent {
                code: KeyCode::Up, ..
            } => {
                self.select_prev_menu_item(menu);
                true
            }
            KeyEvent {
                code: KeyCode::Down,
                ..
            } => {
                self.select_next_menu_item(menu);
                true
            }
            KeyEvent {
                code: KeyCode::Enter,
                ..
            } => {
                self.activate_selected_menu_item(menu);
                self.active_menu = None;
                self.active_menu_index = 0;
                true
            }
            KeyEvent {
                code: KeyCode::Char(c),
                modifiers,
                ..
            } if modifiers.contains(KeyModifiers::ALT) => {
                self.handle_alt_menu(c);
                true
            }
            KeyEvent {
                code: KeyCode::Char(c),
                modifiers,
                ..
            } if !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
                let c = c.to_ascii_lowercase();
                if let Some(target_menu) = Self::menu_from_mnemonic(c) {
                    self.open_menu(target_menu);
                    return true;
                }
                if let Some(index) = self.menu_item_index_by_mnemonic(menu, c) {
                    self.active_menu_index = index;
                    self.activate_selected_menu_item(menu);
                    self.active_menu = None;
                    self.active_menu_index = 0;
                    return true;
                }
                false
            }
            _ => false,
        }
    }

    fn handle_alt_menu(&mut self, c: char) {
        if let Some(menu) = Self::menu_from_mnemonic(c.to_ascii_lowercase()) {
            self.open_menu(menu);
        }
    }

    fn open_menu(&mut self, menu: MenuKind) {
        self.active_menu = Some(menu);
        self.active_menu_index = 0;
        self.status = StatusMessage::new(match menu {
            MenuKind::File => "File menu opened",
            MenuKind::Edit => "Edit menu opened",
            MenuKind::Search => "Search menu opened",
            MenuKind::Help => "Help menu opened",
        });
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) {
        if self.search_popup.is_some() || self.save_as_popup.is_some() {
            return;
        }

        let col = usize::from(mouse.column);
        let row = usize::from(mouse.row);
        let (cols, rows) = terminal::size().unwrap_or((80, 24));
        let cols = usize::from(cols);
        let rows = usize::from(rows);

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if row == 0 {
                    if let Some(menu) = self.menu_at_column(col) {
                        self.open_menu(menu);
                    } else {
                        self.active_menu = None;
                    }
                    self.mouse_drag_anchor = None;
                    return;
                }

                if let Some(menu) = self.active_menu {
                    if let Some(index) = self.dropdown_item_at(menu, col, row, cols, rows) {
                        self.active_menu_index = index;
                        self.activate_selected_menu_item(menu);
                        self.active_menu = None;
                        self.active_menu_index = 0;
                        self.mouse_drag_anchor = None;
                        return;
                    }
                    self.active_menu = None;
                    self.active_menu_index = 0;
                }

                if let Some(pos) = self.editor_position_from_mouse(col, row, cols, rows) {
                    self.cursor = pos;
                    self.clear_selection();
                    self.mouse_drag_anchor = Some(pos);
                    self.scroll();
                } else {
                    self.mouse_drag_anchor = None;
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(anchor) = self.mouse_drag_anchor {
                    if let Some(pos) = self.editor_position_from_mouse(col, row, cols, rows) {
                        self.cursor = pos;
                        self.selection_anchor = Some(anchor);
                        self.normalize_selection_anchor();
                        self.active_menu = None;
                        self.active_menu_index = 0;
                        self.scroll();
                    }
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.mouse_drag_anchor = None;
            }
            _ => {}
        }
    }

    fn editor_position_from_mouse(
        &self,
        column: usize,
        row: usize,
        cols: usize,
        rows: usize,
    ) -> Option<Position> {
        let inner_width = cols.saturating_sub(2);
        let text_height = rows.saturating_sub(5);
        if text_height == 0 {
            return None;
        }
        if row < 2 || row >= 2 + text_height {
            return None;
        }

        let gutter = self.gutter_width();
        let body_width = self.editor_body_width(inner_width);
        let editor_end = 1 + gutter + body_width;
        if column < 1 || column >= editor_end {
            return None;
        }

        let file_row = cmp::min(
            self.offset.y + (row - 2),
            self.doc.line_count().saturating_sub(1),
        );
        let visual_x = if column <= gutter {
            0
        } else {
            self.offset.x + column - (gutter + 1)
        };
        let line_len = self.doc.line_char_len(file_row);
        Some(Position {
            x: cmp::min(visual_x, line_len),
            y: file_row,
        })
    }

    fn menu_at_column(&self, column: usize) -> Option<MenuKind> {
        for (menu, x, width) in Self::menu_item_bounds() {
            let end = x + width;
            if (x..end).contains(&column) {
                return Some(menu);
            }
        }
        None
    }

    fn menu_item_bounds() -> Vec<(MenuKind, usize, usize)> {
        let mut x = format!(" {APP_NAME} v{APP_VERSION} ").chars().count();
        let mut bounds = Vec::with_capacity(MENU_ITEMS.len());
        for (index, (menu, label)) in MENU_ITEMS.iter().enumerate() {
            x += if index == 0 { 1 } else { 2 };
            let width = format!(" {label} ").chars().count();
            bounds.push((*menu, x, width));
            x += width;
        }
        bounds
    }

    fn menu_entries(menu: MenuKind) -> &'static [MenuEntry] {
        match menu {
            MenuKind::File => FILE_MENU_ENTRIES,
            MenuKind::Edit => EDIT_MENU_ENTRIES,
            MenuKind::Search => SEARCH_MENU_ENTRIES,
            MenuKind::Help => HELP_MENU_ENTRIES,
        }
    }

    fn menu_from_mnemonic(c: char) -> Option<MenuKind> {
        match c {
            'f' => Some(MenuKind::File),
            'e' => Some(MenuKind::Edit),
            's' => Some(MenuKind::Search),
            'h' => Some(MenuKind::Help),
            _ => None,
        }
    }

    fn menu_index(menu: MenuKind) -> usize {
        MENU_ITEMS
            .iter()
            .position(|(kind, _)| *kind == menu)
            .unwrap_or(0)
    }

    fn menu_by_index(index: usize) -> MenuKind {
        MENU_ITEMS[index % MENU_ITEMS.len()].0
    }

    fn prev_menu(&self, menu: MenuKind) -> MenuKind {
        let idx = Self::menu_index(menu);
        let prev = if idx == 0 {
            MENU_ITEMS.len() - 1
        } else {
            idx - 1
        };
        Self::menu_by_index(prev)
    }

    fn next_menu(&self, menu: MenuKind) -> MenuKind {
        let idx = Self::menu_index(menu);
        Self::menu_by_index((idx + 1) % MENU_ITEMS.len())
    }

    fn select_prev_menu_item(&mut self, menu: MenuKind) {
        let len = Self::menu_entries(menu).len();
        if len == 0 {
            return;
        }
        self.active_menu_index = if self.active_menu_index == 0 {
            len - 1
        } else {
            self.active_menu_index - 1
        };
    }

    fn select_next_menu_item(&mut self, menu: MenuKind) {
        let len = Self::menu_entries(menu).len();
        if len == 0 {
            return;
        }
        self.active_menu_index = (self.active_menu_index + 1) % len;
    }

    fn menu_label_bounds(&self, menu: MenuKind) -> Option<(usize, usize)> {
        for (kind, x, width) in Self::menu_item_bounds() {
            if kind == menu {
                return Some((x, width));
            }
        }
        None
    }

    fn dropdown_rect(&self, menu: MenuKind, cols: usize, rows: usize) -> Option<MenuRect> {
        let entries = Self::menu_entries(menu);
        if entries.is_empty() {
            return None;
        }

        let (x, _) = self.menu_label_bounds(menu)?;
        if x >= cols {
            return None;
        }

        let content_width = entries
            .iter()
            .map(|entry| entry.label.len())
            .max()
            .unwrap_or(0);
        let width = cmp::min(content_width + 4, cols - x);
        let max_height = rows.saturating_sub(3);
        let entry_rows = cmp::min(entries.len(), max_height.saturating_sub(2));
        let height = entry_rows + 2;
        if width < 4 || entry_rows == 0 {
            return None;
        }

        Some(MenuRect {
            x,
            y: 1,
            width,
            height,
        })
    }

    fn dropdown_item_at(
        &self,
        menu: MenuKind,
        column: usize,
        row: usize,
        cols: usize,
        rows: usize,
    ) -> Option<usize> {
        let rect = self.dropdown_rect(menu, cols, rows)?;
        if row <= rect.y || row >= rect.y + rect.height - 1 {
            return None;
        }
        if column <= rect.x || column >= rect.x + rect.width - 1 {
            return None;
        }
        Some(row - rect.y - 1)
    }

    fn menu_item_index_by_mnemonic(&self, menu: MenuKind, mnemonic: char) -> Option<usize> {
        Self::menu_entries(menu)
            .iter()
            .position(|entry| entry.mnemonic == mnemonic)
    }

    fn activate_selected_menu_item(&mut self, menu: MenuKind) {
        let entries = Self::menu_entries(menu);
        if entries.is_empty() {
            return;
        }
        let idx = cmp::min(self.active_menu_index, entries.len() - 1);
        self.active_menu_index = idx;
        self.invoke_menu_action(entries[idx].action);
    }

    fn invoke_menu_action(&mut self, action: MenuAction) {
        match action {
            MenuAction::Save => self.save(),
            MenuAction::SaveAs => self.open_save_as_popup(),
            MenuAction::Quit => self.quit(),
            MenuAction::Undo => self.undo(),
            MenuAction::Redo => self.redo(),
            MenuAction::Cut => self.cut_current_line(),
            MenuAction::Copy => self.copy_current_line(),
            MenuAction::Paste => self.paste_clipboard(),
            MenuAction::Find => self.open_find_popup(),
            MenuAction::Replace => self.open_replace_popup(),
            MenuAction::TogglePreview => self.toggle_preview(),
            MenuAction::Keybindings => {
                self.status = StatusMessage::new(
                    "Shortcuts: Ctrl-S save, Ctrl-Shift-S save as, Ctrl-Q quit, Ctrl-P preview.",
                );
            }
            MenuAction::About => {
                self.status = StatusMessage::new(format!(
                    "{APP_NAME} v{APP_VERSION}: terminal markup editor prototype in Rust."
                ));
            }
        }
    }

    fn current_snapshot(&self) -> EditorSnapshot {
        EditorSnapshot {
            lines: self.doc.lines.clone(),
            cursor: self.cursor,
            offset: self.offset,
            selection_anchor: self.selection_anchor,
            modified: self.doc.modified,
        }
    }

    fn restore_snapshot(&mut self, snapshot: EditorSnapshot) {
        self.doc.lines = if snapshot.lines.is_empty() {
            vec![String::new()]
        } else {
            snapshot.lines
        };
        self.doc.modified = snapshot.modified;
        self.cursor = snapshot.cursor;
        self.offset = snapshot.offset;
        self.selection_anchor = snapshot.selection_anchor;
        self.mouse_drag_anchor = None;
        self.clamp_cursor_to_document();
        self.normalize_selection_anchor();
    }

    fn push_undo_snapshot(&mut self) {
        let snapshot = self.current_snapshot();
        if self.undo_stack.last() == Some(&snapshot) {
            return;
        }
        self.undo_stack.push(snapshot);
    }

    fn begin_edit(&mut self) {
        self.push_undo_snapshot();
        self.redo_stack.clear();
    }

    fn clear_selection(&mut self) {
        self.selection_anchor = None;
    }

    fn normalized_selection(&self) -> Option<SelectionRange> {
        let anchor = self.selection_anchor?;
        if anchor == self.cursor {
            return None;
        }
        if (anchor.y, anchor.x) <= (self.cursor.y, self.cursor.x) {
            Some(SelectionRange {
                start: anchor,
                end: self.cursor,
            })
        } else {
            Some(SelectionRange {
                start: self.cursor,
                end: anchor,
            })
        }
    }

    fn normalize_selection_anchor(&mut self) {
        if self.selection_anchor == Some(self.cursor) {
            self.selection_anchor = None;
        }
    }

    fn selection_range_for_line(&self, line_idx: usize, line_len: usize) -> Option<(usize, usize)> {
        let selection = self.normalized_selection()?;
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
        (start < end).then_some((cmp::min(start, line_len), cmp::min(end, line_len)))
    }

    fn selected_text(&self) -> Option<String> {
        let selection = self.normalized_selection()?;
        if selection.start.y == selection.end.y {
            let line = self.doc.line(selection.start.y)?;
            return Some(slice_chars(line, selection.start.x, selection.end.x));
        }

        let mut parts = Vec::new();
        let first_line = self.doc.line(selection.start.y)?;
        parts.push(slice_chars(
            first_line,
            selection.start.x,
            first_line.chars().count(),
        ));
        for line_idx in selection.start.y + 1..selection.end.y {
            parts.push(self.doc.line(line_idx)?.clone());
        }
        let last_line = self.doc.line(selection.end.y)?;
        parts.push(slice_chars(last_line, 0, selection.end.x));
        Some(parts.join("\n"))
    }

    fn delete_selection(&mut self) -> bool {
        let Some(selection) = self.normalized_selection() else {
            return false;
        };

        if selection.start.y == selection.end.y {
            if let Some(line) = self.doc.lines.get_mut(selection.start.y) {
                let start = byte_index_for_char(line, selection.start.x);
                let end = byte_index_for_char(line, selection.end.x);
                line.replace_range(start..end, "");
            }
        } else {
            let first_prefix = self
                .doc
                .line(selection.start.y)
                .map(|line| slice_chars(line, 0, selection.start.x))
                .unwrap_or_default();
            let last_suffix = self
                .doc
                .line(selection.end.y)
                .map(|line| slice_chars(line, selection.end.x, line.chars().count()))
                .unwrap_or_default();

            if let Some(line) = self.doc.lines.get_mut(selection.start.y) {
                *line = format!("{first_prefix}{last_suffix}");
            }

            if selection.start.y < selection.end.y && selection.end.y < self.doc.lines.len() {
                self.doc
                    .lines
                    .drain(selection.start.y + 1..=selection.end.y);
            }
        }

        if self.doc.lines.is_empty() {
            self.doc.lines.push(String::new());
        }
        self.doc.modified = true;
        self.cursor = selection.start;
        self.clear_selection();
        self.clamp_cursor_to_document();
        true
    }

    fn apply_selection_for_move(&mut self, old_cursor: Position, extend: bool) {
        if extend {
            if self.selection_anchor.is_none() {
                self.selection_anchor = Some(old_cursor);
            }
            self.normalize_selection_anchor();
        } else {
            self.clear_selection();
        }
    }

    fn clamp_cursor_to_document(&mut self) {
        let bottom = self.doc.line_count().saturating_sub(1);
        self.cursor.y = cmp::min(self.cursor.y, bottom);
        self.cursor.x = cmp::min(self.cursor.x, self.doc.line_char_len(self.cursor.y));
    }

    fn undo(&mut self) {
        let Some(snapshot) = self.undo_stack.pop() else {
            self.status = StatusMessage::new("Nothing to undo.");
            return;
        };

        self.redo_stack.push(self.current_snapshot());
        self.restore_snapshot(snapshot);
        self.status = StatusMessage::new("Undo.");
    }

    fn redo(&mut self) {
        let Some(snapshot) = self.redo_stack.pop() else {
            self.status = StatusMessage::new("Nothing to redo.");
            return;
        };

        self.undo_stack.push(self.current_snapshot());
        self.restore_snapshot(snapshot);
        self.status = StatusMessage::new("Redo.");
    }

    fn copy_current_line(&mut self) {
        if let Some(selected) = self.selected_text() {
            self.clipboard = selected;
            self.status = StatusMessage::new("Copied selection.");
            return;
        }

        let Some(line) = self.doc.line(self.cursor.y) else {
            self.status = StatusMessage::new("No line to copy.");
            return;
        };
        self.clipboard = line.clone();
        self.status = StatusMessage::new("Copied current line.");
    }

    fn cut_current_line(&mut self) {
        if let Some(selected) = self.selected_text() {
            self.begin_edit();
            self.clipboard = selected;
            self.delete_selection();
            self.status = StatusMessage::new("Cut selection.");
            return;
        }

        if self.doc.line_count() == 0 {
            self.status = StatusMessage::new("No line to cut.");
            return;
        }

        self.begin_edit();
        let removed = if self.doc.line_count() == 1 {
            let value = self.doc.lines[0].clone();
            self.doc.lines[0].clear();
            value
        } else {
            self.doc.lines.remove(self.cursor.y)
        };
        self.clipboard = removed;
        if self.cursor.y >= self.doc.line_count() {
            self.cursor.y = self.doc.line_count().saturating_sub(1);
        }
        self.clamp_cursor_x();
        self.doc.modified = true;
        self.status = StatusMessage::new("Cut current line.");
    }

    fn paste_clipboard(&mut self) {
        if self.clipboard.is_empty() {
            self.status = StatusMessage::new("Clipboard is empty.");
            return;
        }

        self.begin_edit();
        self.delete_selection();
        let text = self.clipboard.clone();
        self.insert_text_at_cursor(&text);
        self.status = StatusMessage::new("Pasted clipboard.");
    }

    fn insert_text_at_cursor(&mut self, text: &str) {
        for ch in text.chars() {
            if ch == '\n' {
                self.doc.insert_newline(self.cursor);
                self.cursor.y += 1;
                self.cursor.x = 0;
            } else {
                self.doc.insert_char(self.cursor, ch);
                self.cursor.x += 1;
            }
        }
    }

    fn open_save_as_popup(&mut self) {
        let default_path = self
            .doc
            .file_path
            .as_ref()
            .map_or_else(|| "redit.md".to_string(), |path| path.display().to_string());
        let default_cursor = default_path.chars().count();
        self.save_as_popup = Some(SaveAsPopupState {
            path_input: default_path,
            cursor: default_cursor,
            select_all: true,
        });
        self.search_popup = None;
        self.active_menu = None;
        self.active_menu_index = 0;
    }

    fn handle_save_as_popup_key(&mut self, key: KeyEvent) -> bool {
        if self.save_as_popup.is_none() {
            return false;
        }

        match key {
            KeyEvent {
                code: KeyCode::Esc, ..
            } => {
                self.save_as_popup = None;
                self.status = StatusMessage::new("Save As cancelled.");
            }
            KeyEvent {
                code: KeyCode::Backspace,
                ..
            } => {
                self.save_as_backspace();
            }
            KeyEvent {
                code: KeyCode::Delete,
                ..
            } => {
                self.save_as_delete();
            }
            KeyEvent {
                code: KeyCode::Left,
                ..
            } => {
                if let Some(popup) = self.save_as_popup.as_mut() {
                    popup.select_all = false;
                    popup.cursor = popup.cursor.saturating_sub(1);
                }
            }
            KeyEvent {
                code: KeyCode::Right,
                ..
            } => {
                if let Some(popup) = self.save_as_popup.as_mut() {
                    popup.select_all = false;
                    popup.cursor = cmp::min(popup.cursor + 1, popup.path_input.chars().count());
                }
            }
            KeyEvent {
                code: KeyCode::Home,
                ..
            } => {
                if let Some(popup) = self.save_as_popup.as_mut() {
                    popup.select_all = false;
                    popup.cursor = 0;
                }
            }
            KeyEvent {
                code: KeyCode::End, ..
            } => {
                if let Some(popup) = self.save_as_popup.as_mut() {
                    popup.select_all = false;
                    popup.cursor = popup.path_input.chars().count();
                }
            }
            KeyEvent {
                code: KeyCode::Enter,
                ..
            } => {
                self.submit_save_as_popup();
            }
            KeyEvent {
                code: KeyCode::Char('v'),
                modifiers,
                ..
            } if modifiers.contains(KeyModifiers::CONTROL) => {
                let paste_text = self.clipboard.clone();
                if !paste_text.is_empty() {
                    self.save_as_insert_text(&paste_text);
                }
            }
            KeyEvent {
                code: KeyCode::Char('a'),
                modifiers,
                ..
            } if modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(popup) = self.save_as_popup.as_mut() {
                    popup.select_all = true;
                    popup.cursor = popup.path_input.chars().count();
                }
            }
            KeyEvent {
                code: KeyCode::Char(c),
                modifiers,
                ..
            } if !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
                let mut buffer = String::new();
                buffer.push(c);
                self.save_as_insert_text(&buffer);
            }
            _ => {}
        }

        true
    }

    fn save_as_insert_text(&mut self, text: &str) {
        if let Some(popup) = self.save_as_popup.as_mut() {
            if popup.select_all {
                popup.path_input.clear();
                popup.cursor = 0;
                popup.select_all = false;
            }

            let byte_idx = byte_index_for_char(&popup.path_input, popup.cursor);
            popup.path_input.insert_str(byte_idx, text);
            popup.cursor += text.chars().count();
        }
    }

    fn save_as_backspace(&mut self) {
        if let Some(popup) = self.save_as_popup.as_mut() {
            if popup.select_all {
                popup.path_input.clear();
                popup.cursor = 0;
                popup.select_all = false;
                return;
            }

            if popup.cursor > 0 {
                remove_char_at(&mut popup.path_input, popup.cursor - 1);
                popup.cursor -= 1;
            }
        }
    }

    fn save_as_delete(&mut self) {
        if let Some(popup) = self.save_as_popup.as_mut() {
            if popup.select_all {
                popup.path_input.clear();
                popup.cursor = 0;
                popup.select_all = false;
                return;
            }

            remove_char_at(&mut popup.path_input, popup.cursor);
        }
    }

    fn submit_save_as_popup(&mut self) {
        let Some(popup) = self.save_as_popup.clone() else {
            return;
        };
        let path_text = popup.path_input.trim();
        if path_text.is_empty() {
            self.status = StatusMessage::new("Save As path cannot be empty.");
            return;
        }

        match self.doc.save_as(PathBuf::from(path_text)) {
            Ok(path) => {
                self.status = StatusMessage::new(format!("Saved {}", path.display()));
                self.quit_warning_countdown = 1;
                self.save_as_popup = None;
            }
            Err(err) => {
                self.status = StatusMessage::new(format!("Save As failed: {err}"));
            }
        }
    }

    fn open_find_popup(&mut self) {
        self.search_popup = Some(SearchPopupState::find(&self.last_search_query));
        self.save_as_popup = None;
        self.active_menu = None;
        self.active_menu_index = 0;
    }

    fn open_replace_popup(&mut self) {
        self.search_popup = Some(SearchPopupState::replace(&self.last_search_query));
        self.save_as_popup = None;
        self.active_menu = None;
        self.active_menu_index = 0;
    }

    fn handle_search_popup_key(&mut self, key: KeyEvent) -> bool {
        if self.search_popup.is_none() {
            return false;
        }

        match key {
            KeyEvent {
                code: KeyCode::Esc, ..
            } => {
                self.search_popup = None;
                self.status = StatusMessage::new("Search cancelled.");
            }
            KeyEvent {
                code: KeyCode::Tab, ..
            }
            | KeyEvent {
                code: KeyCode::Up, ..
            }
            | KeyEvent {
                code: KeyCode::Down,
                ..
            } => {
                if let Some(popup) = self.search_popup.as_mut() {
                    if popup.mode == SearchPopupMode::Replace {
                        popup.active_field = match popup.active_field {
                            SearchPopupField::Find => SearchPopupField::Replace,
                            SearchPopupField::Replace => SearchPopupField::Find,
                        };
                    }
                }
            }
            KeyEvent {
                code: KeyCode::Backspace,
                ..
            } => {
                if let Some(popup) = self.search_popup.as_mut() {
                    popup.active_field_mut().pop();
                }
            }
            KeyEvent {
                code: KeyCode::Enter,
                ..
            } => {
                self.submit_search_popup();
            }
            KeyEvent {
                code: KeyCode::Char('v'),
                modifiers,
                ..
            } if modifiers.contains(KeyModifiers::CONTROL) => {
                let paste_text = self.clipboard.clone();
                if let Some(popup) = self.search_popup.as_mut() {
                    popup.active_field_mut().push_str(&paste_text);
                }
            }
            KeyEvent {
                code: KeyCode::Char(c),
                modifiers,
                ..
            } if !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
                if let Some(popup) = self.search_popup.as_mut() {
                    popup.active_field_mut().push(c);
                }
            }
            _ => {}
        }

        true
    }

    fn submit_search_popup(&mut self) {
        let Some(popup) = self.search_popup.clone() else {
            return;
        };

        match popup.mode {
            SearchPopupMode::Find => {
                if popup.find_input.trim().is_empty() {
                    self.status = StatusMessage::new("Find text cannot be empty.");
                    return;
                }
                self.last_search_query = popup.find_input.clone();
                self.find_next_occurrence(&popup.find_input);
                self.search_popup = None;
            }
            SearchPopupMode::Replace => {
                if popup.find_input.trim().is_empty() {
                    self.status = StatusMessage::new("Find text cannot be empty.");
                    return;
                }
                if popup.active_field == SearchPopupField::Find {
                    if let Some(state) = self.search_popup.as_mut() {
                        state.active_field = SearchPopupField::Replace;
                    }
                    return;
                }

                self.last_search_query = popup.find_input.clone();
                self.replace_all_occurrences(&popup.find_input, &popup.replace_input);
                self.search_popup = None;
            }
        }
    }

    fn find_next_occurrence(&mut self, query: &str) {
        let query_chars: Vec<char> = query.chars().collect();
        if query_chars.is_empty() {
            self.status = StatusMessage::new("Find text cannot be empty.");
            return;
        }

        let start_line = self.cursor.y;
        let start_col = self.cursor.x.saturating_add(1);
        let found = self
            .find_from(query, start_line, start_col)
            .or_else(|| self.find_from(query, 0, 0));

        if let Some(pos) = found {
            self.clear_selection();
            self.cursor = pos;
            self.scroll();
            self.status = StatusMessage::new(format!(
                "Found \"{query}\" at Ln {}, Col {}.",
                pos.y + 1,
                pos.x + 1
            ));
        } else {
            self.status = StatusMessage::new(format!("No matches for \"{query}\"."));
        }
    }

    fn find_from(&self, query: &str, start_line: usize, start_col: usize) -> Option<Position> {
        if query.is_empty() {
            return None;
        }

        for line_idx in start_line..self.doc.line_count() {
            let line = self.doc.line(line_idx)?;
            let col_start = if line_idx == start_line { start_col } else { 0 };
            if let Some(col) = find_substring_at_char(line, query, col_start) {
                return Some(Position {
                    x: col,
                    y: line_idx,
                });
            }
        }
        None
    }

    fn replace_all_occurrences(&mut self, query: &str, replacement: &str) {
        if query.is_empty() {
            self.status = StatusMessage::new("Find text cannot be empty.");
            return;
        }

        let mut total = 0usize;
        for line in &self.doc.lines {
            total += line.match_indices(query).count();
        }
        if total == 0 {
            self.status = StatusMessage::new(format!("No matches for \"{query}\"."));
            return;
        }

        self.begin_edit();
        for line in &mut self.doc.lines {
            if line.contains(query) {
                *line = line.replace(query, replacement);
            }
        }
        self.doc.modified = true;
        self.clamp_cursor_to_document();
        self.status = StatusMessage::new(format!("Replaced {total} occurrence(s) of \"{query}\"."));
    }

    fn toggle_preview(&mut self) {
        self.preview_mode = !self.preview_mode;
        if self.preview_mode {
            self.preview_cache_lines.clear();
            self.preview_cache_width = 0;
            self.preview_cache_revision = 0;
            let (cols, _) = terminal::size().unwrap_or((80, 24));
            let inner_cols = usize::from(cols).saturating_sub(2);
            if self.preview_layout(inner_cols).is_some() {
                self.status = StatusMessage::new("Preview enabled (Ctrl-P to hide).");
            } else {
                self.status =
                    StatusMessage::new("Preview enabled. Widen terminal to show split preview.");
            }
        } else {
            self.status = StatusMessage::new("Preview hidden.");
        }
        self.scroll();
    }

    fn preview_layout(&self, cols: usize) -> Option<(usize, usize, usize)> {
        if !self.preview_mode {
            return None;
        }

        let gutter = self.gutter_width();
        let total_body = cols.saturating_sub(gutter);
        if total_body < PREVIEW_MIN_TOTAL_WIDTH {
            return None;
        }

        let editor_width = total_body.saturating_sub(PREVIEW_SEPARATOR_WIDTH) / 2;
        let separator_x = gutter + editor_width;
        let preview_x = separator_x + PREVIEW_SEPARATOR_WIDTH;
        let preview_width = cols.saturating_sub(preview_x);
        if editor_width == 0 || preview_width == 0 {
            return None;
        }
        Some((separator_x, preview_x, preview_width))
    }

    fn editor_body_width(&self, cols: usize) -> usize {
        let gutter = self.gutter_width();
        if let Some((separator_x, _, _)) = self.preview_layout(cols) {
            separator_x.saturating_sub(gutter)
        } else {
            cols.saturating_sub(gutter)
        }
    }

    fn preview_revision(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.doc.lines.hash(&mut hasher);
        hasher.finish()
    }

    fn ensure_preview_cache(&mut self, width: usize) {
        let revision = self.preview_revision();
        if self.preview_cache_width == width && self.preview_cache_revision == revision {
            return;
        }

        let (lines, backend, error) = self.build_preview_lines(width);
        self.preview_cache_lines = lines;
        self.preview_cache_width = width;
        self.preview_cache_revision = revision;
        self.preview_backend = backend;

        if error != self.preview_error {
            if let Some(err) = &error {
                self.status = StatusMessage::new(err.clone());
            }
            self.preview_error = error;
        }
    }

    fn build_preview_lines(&self, width: usize) -> (Vec<String>, PreviewBackend, Option<String>) {
        if let Some(glow_path) = find_glow_command() {
            match self.render_with_glow(&glow_path, width) {
                Ok(lines) => return (lines, PreviewBackend::Glow, None),
                Err(err) => {
                    let lines = self.render_fallback_preview(width);
                    return (
                        lines,
                        PreviewBackend::Fallback,
                        Some(format!("Glow preview unavailable: {err}. Using fallback.")),
                    );
                }
            }
        }

        (
            self.render_fallback_preview(width),
            PreviewBackend::Fallback,
            Some("Glow not found. Install glow to enable GitHub-style preview.".to_string()),
        )
    }

    fn render_with_glow(&self, glow_path: &Path, width: usize) -> Result<Vec<String>, String> {
        let mut command = Command::new(glow_path);
        command
            .arg("-s")
            .arg("dark")
            .arg("-w")
            .arg(width.to_string())
            .arg("-");

        let source = self.preview_source_text();
        let output = run_command_with_stdin(&mut command, Some(source.as_bytes()))
            .map_err(|err| err.to_string())?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let detail = if stderr.is_empty() {
                format!("exit status {}", output.status)
            } else {
                stderr
            };
            return Err(detail);
        }

        let text = String::from_utf8_lossy(&output.stdout);
        let lines = text
            .lines()
            .map(|line| line.to_string())
            .collect::<Vec<_>>();
        Ok(lines)
    }

    fn render_fallback_preview(&self, width: usize) -> Vec<String> {
        self.preview_source_lines()
            .iter()
            .map(|line| clip_to_char_width(line, width))
            .collect()
    }

    fn preview_source_lines(&self) -> Vec<String> {
        self.doc
            .lines
            .iter()
            .map(|line| html_heading_to_markdown(line).unwrap_or_else(|| line.clone()))
            .collect()
    }

    fn preview_source_text(&self) -> String {
        self.preview_source_lines().join("\n")
    }

    fn insert_char(&mut self, c: char) {
        self.delete_selection();
        self.doc.insert_char(self.cursor, c);
        self.cursor.x += 1;
    }

    fn insert_newline(&mut self) {
        self.delete_selection();
        let continuation = self.markdown_continuation();
        self.doc.insert_newline(self.cursor);
        self.cursor.y += 1;
        self.cursor.x = 0;
        if let Some(continuation) = continuation {
            for ch in continuation.prefix.chars() {
                self.insert_char(ch);
            }
        }
    }

    fn markdown_continuation(&self) -> Option<MarkdownContinuation> {
        let line = self.doc.line(self.cursor.y)?;
        let before_cursor: String = line.chars().take(self.cursor.x).collect();
        markdown_list_continuation(&before_cursor)
    }

    fn backspace(&mut self) {
        if self.delete_selection() {
            return;
        }
        if let Some(pos) = self.doc.backspace(self.cursor) {
            self.cursor = pos;
        }
    }

    fn delete_forward(&mut self) {
        if self.delete_selection() {
            return;
        }
        self.doc.delete_forward(self.cursor);
    }

    fn quit(&mut self) {
        if self.doc.modified && self.quit_warning_countdown > 0 {
            self.status = StatusMessage::new("Unsaved changes. Press Ctrl-Q again to force quit.");
            self.quit_warning_countdown -= 1;
            return;
        }
        self.should_quit = true;
    }

    fn save(&mut self) {
        match self.doc.save() {
            Ok(path) => {
                self.status = StatusMessage::new(format!("Saved {}", path.display()));
                self.quit_warning_countdown = 1;
            }
            Err(err) => {
                self.status = StatusMessage::new(format!("Save failed: {err}"));
            }
        }
    }

    fn move_cursor_left(&mut self) {
        if self.cursor.x > 0 {
            self.cursor.x -= 1;
            return;
        }
        if self.cursor.y > 0 {
            self.cursor.y -= 1;
            self.cursor.x = self.doc.line_char_len(self.cursor.y);
        }
    }

    fn move_cursor_left_with_selection(&mut self, extend: bool) {
        let old = self.cursor;
        self.move_cursor_left();
        self.apply_selection_for_move(old, extend);
    }

    fn move_cursor_right(&mut self) {
        let line_len = self.doc.line_char_len(self.cursor.y);
        if self.cursor.x < line_len {
            self.cursor.x += 1;
            return;
        }
        if self.cursor.y + 1 < self.doc.line_count() {
            self.cursor.y += 1;
            self.cursor.x = 0;
        }
    }

    fn move_cursor_right_with_selection(&mut self, extend: bool) {
        let old = self.cursor;
        self.move_cursor_right();
        self.apply_selection_for_move(old, extend);
    }

    fn move_cursor_up(&mut self) {
        if self.cursor.y > 0 {
            self.cursor.y -= 1;
            self.clamp_cursor_x();
        }
    }

    fn move_cursor_up_with_selection(&mut self, extend: bool) {
        let old = self.cursor;
        self.move_cursor_up();
        self.apply_selection_for_move(old, extend);
    }

    fn move_cursor_down(&mut self) {
        if self.cursor.y + 1 < self.doc.line_count() {
            self.cursor.y += 1;
            self.clamp_cursor_x();
        }
    }

    fn move_cursor_down_with_selection(&mut self, extend: bool) {
        let old = self.cursor;
        self.move_cursor_down();
        self.apply_selection_for_move(old, extend);
    }

    fn page_up(&mut self) {
        let height = self.text_area_height();
        self.cursor.y = self.cursor.y.saturating_sub(height);
        self.clamp_cursor_x();
    }

    fn page_up_with_selection(&mut self, extend: bool) {
        let old = self.cursor;
        self.page_up();
        self.apply_selection_for_move(old, extend);
    }

    fn page_down(&mut self) {
        let height = self.text_area_height();
        let bottom = self.doc.line_count().saturating_sub(1);
        self.cursor.y = cmp::min(self.cursor.y + height, bottom);
        self.clamp_cursor_x();
    }

    fn page_down_with_selection(&mut self, extend: bool) {
        let old = self.cursor;
        self.page_down();
        self.apply_selection_for_move(old, extend);
    }

    fn home_with_selection(&mut self, extend: bool) {
        let old = self.cursor;
        self.cursor.x = 0;
        self.apply_selection_for_move(old, extend);
    }

    fn end_with_selection(&mut self, extend: bool) {
        let old = self.cursor;
        self.cursor.x = self.doc.line_char_len(self.cursor.y);
        self.apply_selection_for_move(old, extend);
    }

    fn clamp_cursor_x(&mut self) {
        self.cursor.x = cmp::min(self.cursor.x, self.doc.line_char_len(self.cursor.y));
    }

    fn text_area_height(&self) -> usize {
        let (_, rows) = terminal::size().unwrap_or((80, 24));
        usize::from(rows.saturating_sub(5))
    }

    fn gutter_width(&self) -> usize {
        let digits = self.doc.line_count().max(1).to_string().len();
        digits + 1
    }

    fn scroll(&mut self) {
        let (cols, rows) = terminal::size().unwrap_or((80, 24));
        let cols = usize::from(cols).saturating_sub(2);
        let text_height = usize::from(rows.saturating_sub(5));
        let text_width = self.editor_body_width(cols);

        if self.cursor.y < self.offset.y {
            self.offset.y = self.cursor.y;
        }
        if text_height > 0 && self.cursor.y >= self.offset.y + text_height {
            self.offset.y = self.cursor.y + 1 - text_height;
        }

        if self.cursor.x < self.offset.x {
            self.offset.x = self.cursor.x;
        }
        if text_width > 0 && self.cursor.x >= self.offset.x + text_width {
            self.offset.x = self.cursor.x + 1 - text_width;
        }
    }

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

    fn refresh_screen(&mut self) -> io::Result<()> {
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

struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalGuard {
    fn new() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;
        Ok(Self { terminal })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = self.terminal.show_cursor();
        let _ = execute!(
            self.terminal.backend_mut(),
            ResetColor,
            DisableMouseCapture,
            LeaveAlternateScreen
        );
        let _ = disable_raw_mode();
    }
}

fn byte_index_for_char(line: &str, char_idx: usize) -> usize {
    line.char_indices()
        .nth(char_idx)
        .map_or(line.len(), |(idx, _)| idx)
}

fn remove_char_at(line: &mut String, char_idx: usize) -> Option<char> {
    let start = byte_index_for_char(line, char_idx);
    let end = byte_index_for_char(line, char_idx + 1);
    if start >= end {
        return None;
    }
    let removed = line[start..end].chars().next();
    line.replace_range(start..end, "");
    removed
}

fn slice_chars(line: &str, start: usize, end: usize) -> String {
    if start >= end {
        return String::new();
    }
    line.chars().skip(start).take(end - start).collect()
}

fn shift_only(modifiers: KeyModifiers) -> bool {
    modifiers.contains(KeyModifiers::SHIFT)
        && !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
}

fn find_substring_at_char(line: &str, query: &str, start_char: usize) -> Option<usize> {
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

fn markdown_styles_for_line(
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

fn is_setext_underline_line(line: &str) -> bool {
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

fn is_fenced_code_line(line: &str) -> bool {
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

fn is_indented_code_line(line: &str) -> bool {
    line.starts_with("    ") || line.starts_with('\t')
}

fn markdown_list_continuation(before_cursor: &str) -> Option<MarkdownContinuation> {
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
        return Some(MarkdownContinuation {
            prefix: format!("{indent}{} ", chars[start]),
        });
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
            return Some(MarkdownContinuation {
                prefix: format!("{indent}{}. ", value + 1),
            });
        }
    }

    None
}

fn find_glow_command() -> Option<PathBuf> {
    if command_is_available("glow") {
        return Some(PathBuf::from("glow"));
    }
    let candidate = env::var("HOME")
        .ok()
        .map(PathBuf::from)
        .map(|home| home.join(".local/bin/glow"))?;
    if command_is_available(&candidate) {
        Some(candidate)
    } else {
        None
    }
}

fn command_is_available(command: impl AsRef<std::ffi::OsStr>) -> bool {
    Command::new(command)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn run_command_with_stdin(command: &mut Command, input: Option<&[u8]>) -> io::Result<Output> {
    if let Some(bytes) = input {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(bytes)?;
        }
        child.wait_with_output()
    } else {
        command.output()
    }
}

fn clip_to_char_width(text: &str, width: usize) -> String {
    text.chars().take(width).collect()
}

fn pad_or_clip_to_char_width(text: &str, width: usize) -> String {
    let clipped = clip_to_char_width(text, width);
    let used = clipped.chars().count();
    if used < width {
        format!("{clipped}{}", " ".repeat(width - used))
    } else {
        clipped
    }
}

fn strip_ansi_escape_codes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            if chars.peek().copied() == Some('[') {
                chars.next();
                for next in chars.by_ref() {
                    if ('@'..='~').contains(&next) {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(ch);
    }
    out
}

fn line_char_width(line: &Line<'_>) -> usize {
    line.spans
        .iter()
        .map(|span| span.content.chars().count())
        .sum()
}

fn ansi_to_line_clipped(text: &str, width: usize) -> Line<'static> {
    if width == 0 {
        return Line::raw(String::new());
    }

    let chars: Vec<char> = text.chars().collect();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut current_style = Style::default().fg(CRT_FG).bg(CRT_BG);
    let mut buffer = String::new();
    let mut visible = 0usize;
    let mut i = 0usize;

    let flush = |spans: &mut Vec<Span<'static>>, buffer: &mut String, style: Style| {
        if !buffer.is_empty() {
            let content = std::mem::take(buffer);
            spans.push(Span::styled(content, style));
        }
    };

    while i < chars.len() && visible < width {
        if chars[i] == '\u{1b}' && i + 1 < chars.len() && chars[i + 1] == '[' {
            flush(&mut spans, &mut buffer, current_style);
            i += 2;
            let mut seq = String::new();
            while i < chars.len() {
                let c = chars[i];
                if c.is_ascii_alphabetic() {
                    if c == 'm' {
                        apply_sgr_sequence(&seq, &mut current_style);
                    }
                    i += 1;
                    break;
                }
                seq.push(c);
                i += 1;
            }
            continue;
        }

        let ch = chars[i];
        i += 1;
        if ch == '\n' || ch == '\r' {
            continue;
        }
        buffer.push(ch);
        visible += 1;
    }
    flush(&mut spans, &mut buffer, current_style);
    Line::from(spans)
}

fn apply_sgr_sequence(seq: &str, style: &mut Style) {
    let params: Vec<Option<u16>> = if seq.is_empty() {
        vec![Some(0)]
    } else {
        seq.split(';')
            .map(|part| {
                if part.is_empty() {
                    None
                } else {
                    part.parse::<u16>().ok()
                }
            })
            .collect()
    };

    let mut i = 0usize;
    while i < params.len() {
        let code = params[i].unwrap_or(0);
        match code {
            0 => *style = Style::default().fg(CRT_FG).bg(CRT_BG),
            1 => *style = style.add_modifier(Modifier::BOLD),
            2 => *style = style.add_modifier(Modifier::DIM),
            3 => *style = style.add_modifier(Modifier::ITALIC),
            4 => *style = style.add_modifier(Modifier::UNDERLINED),
            22 => *style = style.remove_modifier(Modifier::BOLD | Modifier::DIM),
            23 => *style = style.remove_modifier(Modifier::ITALIC),
            24 => *style = style.remove_modifier(Modifier::UNDERLINED),
            30..=37 => *style = style.fg(Color::Indexed((code - 30) as u8)),
            90..=97 => *style = style.fg(Color::Indexed((code - 90 + 8) as u8)),
            39 => *style = style.fg(CRT_FG),
            40..=47 => *style = style.bg(Color::Indexed((code - 40) as u8)),
            100..=107 => *style = style.bg(Color::Indexed((code - 100 + 8) as u8)),
            49 => *style = style.bg(CRT_BG),
            38 | 48 => {
                let is_fg = code == 38;
                if i + 1 < params.len() {
                    let mode = params[i + 1].unwrap_or(0);
                    if mode == 5 && i + 2 < params.len() {
                        if let Some(idx) = params[i + 2] {
                            if is_fg {
                                *style = style.fg(Color::Indexed(idx as u8));
                            } else {
                                *style = style.bg(Color::Indexed(idx as u8));
                            }
                        }
                        i += 2;
                    } else if mode == 2 && i + 4 < params.len() {
                        if let (Some(r), Some(g), Some(b)) =
                            (params[i + 2], params[i + 3], params[i + 4])
                        {
                            let color = Color::Rgb(r as u8, g as u8, b as u8);
                            if is_fg {
                                *style = style.fg(color);
                            } else {
                                *style = style.bg(color);
                            }
                        }
                        i += 4;
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
}

fn html_heading_to_markdown(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if !trimmed.starts_with("<h") {
        return None;
    }

    let level_char = trimmed.chars().nth(2)?;
    if !('1'..='6').contains(&level_char) {
        return None;
    }

    let open_end = trimmed.find('>')?;
    let close_tag = format!("</h{level_char}>");
    if !trimmed.ends_with(&close_tag)
        || open_end + 1 > trimmed.len().saturating_sub(close_tag.len())
    {
        return None;
    }

    let content = trimmed[open_end + 1..trimmed.len() - close_tag.len()].trim();
    let level = level_char.to_digit(10)? as usize;
    if level == 1 {
        Some(format!("# {content}"))
    } else {
        // Use bold for h2+ because Glow's dark theme renders ATX h2+ with
        // a visible "##" prefix, which looks like raw markdown syntax.
        Some(format!("**{content}**"))
    }
}
