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

#[derive(Clone, Copy, Default)]
struct Position {
    x: usize,
    y: usize,
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

#[derive(Clone, Copy)]
enum MenuAction {
    Save,
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

const FILE_MENU_ENTRIES: &[MenuEntry] = &[
    MenuEntry {
        label: "Save        Ctrl+S",
        mnemonic: 's',
        action: MenuAction::Save,
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
        label: "About redit",
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
                "Alt-F/E/S/H: menus | Ctrl-S save | Ctrl-Q quit | Ctrl-P preview | F1 help",
            ),
            active_menu: None,
            active_menu_index: 0,
            preview_mode: false,
            preview_cache_lines: Vec::new(),
            preview_cache_width: 0,
            preview_cache_revision: 0,
            preview_backend: PreviewBackend::Fallback,
            preview_error: None,
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
            } => {}
            KeyEvent {
                code: KeyCode::Char('q'),
                modifiers,
                ..
            } if modifiers.contains(KeyModifiers::CONTROL) => {
                self.invoke_menu_action(MenuAction::Quit)
            }
            KeyEvent {
                code: KeyCode::Char('s'),
                modifiers,
                ..
            } if modifiers.contains(KeyModifiers::CONTROL) => {
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
                ..
            } => self.move_cursor_left(),
            KeyEvent {
                code: KeyCode::Right,
                ..
            } => self.move_cursor_right(),
            KeyEvent {
                code: KeyCode::Up, ..
            } => self.move_cursor_up(),
            KeyEvent {
                code: KeyCode::Down,
                ..
            } => self.move_cursor_down(),
            KeyEvent {
                code: KeyCode::PageUp,
                ..
            } => self.page_up(),
            KeyEvent {
                code: KeyCode::PageDown,
                ..
            } => self.page_down(),
            KeyEvent {
                code: KeyCode::Home,
                ..
            } => self.cursor.x = 0,
            KeyEvent {
                code: KeyCode::End, ..
            } => self.cursor.x = self.doc.line_char_len(self.cursor.y),
            KeyEvent {
                code: KeyCode::Backspace,
                ..
            } => self.backspace(),
            KeyEvent {
                code: KeyCode::Delete,
                ..
            } => self.doc.delete_forward(self.cursor),
            KeyEvent {
                code: KeyCode::Enter,
                ..
            } => self.insert_newline(),
            KeyEvent {
                code: KeyCode::Tab, ..
            } => {
                for _ in 0..4 {
                    self.insert_char(' ');
                }
            }
            KeyEvent {
                code: KeyCode::Char(c),
                modifiers,
                ..
            } if !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
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
        if mouse.kind != MouseEventKind::Down(MouseButton::Left) {
            return;
        }

        let col = usize::from(mouse.column);
        let row = usize::from(mouse.row);
        let (cols, rows) = terminal::size().unwrap_or((80, 24));
        let cols = usize::from(cols);
        let rows = usize::from(rows);

        if row == 0 {
            if let Some(menu) = self.menu_at_column(col) {
                self.open_menu(menu);
            } else {
                self.active_menu = None;
            }
            return;
        }

        if let Some(menu) = self.active_menu {
            if let Some(index) = self.dropdown_item_at(menu, col, row, cols, rows) {
                self.active_menu_index = index;
                self.activate_selected_menu_item(menu);
                self.active_menu = None;
                self.active_menu_index = 0;
                return;
            }
            self.active_menu = None;
            self.active_menu_index = 0;
        }
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
        let mut x = " redit ".chars().count();
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
            MenuAction::Quit => self.quit(),
            MenuAction::Undo => {
                self.status = StatusMessage::new("Undo is not implemented yet.");
            }
            MenuAction::Redo => {
                self.status = StatusMessage::new("Redo is not implemented yet.");
            }
            MenuAction::Cut => {
                self.status = StatusMessage::new("Cut is not implemented yet.");
            }
            MenuAction::Copy => {
                self.status = StatusMessage::new("Copy is not implemented yet.");
            }
            MenuAction::Paste => {
                self.status = StatusMessage::new("Paste is not implemented yet.");
            }
            MenuAction::Find => {
                self.status = StatusMessage::new("Find is not implemented yet.");
            }
            MenuAction::Replace => {
                self.status = StatusMessage::new("Replace is not implemented yet.");
            }
            MenuAction::TogglePreview => self.toggle_preview(),
            MenuAction::Keybindings => {
                self.status = StatusMessage::new(
                    "Menus: Alt-F/E/S/H, arrows, Enter, Esc. Shortcuts: Ctrl-S/Q/Z/Y/X/C/V/F/R/P.",
                );
            }
            MenuAction::About => {
                self.status =
                    StatusMessage::new("redit: terminal markup editor prototype in Rust.");
            }
        }
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
        self.doc.insert_char(self.cursor, c);
        self.cursor.x += 1;
    }

    fn insert_newline(&mut self) {
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
        if let Some(pos) = self.doc.backspace(self.cursor) {
            self.cursor = pos;
        }
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

    fn move_cursor_up(&mut self) {
        if self.cursor.y > 0 {
            self.cursor.y -= 1;
            self.clamp_cursor_x();
        }
    }

    fn move_cursor_down(&mut self) {
        if self.cursor.y + 1 < self.doc.line_count() {
            self.cursor.y += 1;
            self.clamp_cursor_x();
        }
    }

    fn page_up(&mut self) {
        let height = self.text_area_height();
        self.cursor.y = self.cursor.y.saturating_sub(height);
        self.clamp_cursor_x();
    }

    fn page_down(&mut self) {
        let height = self.text_area_height();
        let bottom = self.doc.line_count().saturating_sub(1);
        self.cursor.y = cmp::min(self.cursor.y + height, bottom);
        self.clamp_cursor_x();
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
            let mut current_style = styles[start];
            let mut segment = String::new();

            for idx in start..end {
                let style = styles[idx];
                if style != current_style {
                    let text = std::mem::take(&mut segment);
                    rendered += text.chars().count();
                    spans.push(Span::styled(
                        text,
                        md_style_to_style(current_style, line_bg),
                    ));
                    current_style = style;
                }
                segment.push(chars[idx]);
            }

            if !segment.is_empty() {
                rendered += segment.chars().count();
                spans.push(Span::styled(
                    segment,
                    md_style_to_style(current_style, line_bg),
                ));
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

        let title = " redit ";
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
        let cursor_rel_x = self.cursor.x.saturating_sub(self.offset.x) + gutter;
        let cursor_rel_y = self.cursor.y.saturating_sub(self.offset.y);
        let cursor_screen_x = cursor_rel_x + 1;
        let cursor_screen_y = cursor_rel_y + 2;
        let show_cursor = cursor_rel_y < text_height && cursor_rel_x < inner_width;
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

            if show_cursor {
                frame.set_cursor_position((cursor_screen_x as u16, cursor_screen_y as u16));
            }
        })?;

        if show_cursor {
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
