use std::cmp;
use std::env;
use std::fs;
use std::io::{self, Stdout, Write, stdout};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::execute;
use crossterm::queue;
use crossterm::style::{
    Attribute, Color, Print, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor,
};
use crossterm::terminal::{
    self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
    enable_raw_mode,
};

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
        label: "About redit",
        mnemonic: 'a',
        action: MenuAction::About,
    },
];

const CRT_BG: Color = Color::Rgb { r: 0, g: 12, b: 0 };
const CRT_FG: Color = Color::Rgb {
    r: 110,
    g: 255,
    b: 130,
};
const CRT_DIM_FG: Color = Color::Rgb {
    r: 50,
    g: 150,
    b: 70,
};
const CRT_BAR_BG: Color = Color::Rgb {
    r: 70,
    g: 170,
    b: 90,
};
const CRT_BAR_FG: Color = Color::Black;
const CRT_ACTIVE_BG: Color = Color::Rgb {
    r: 150,
    g: 255,
    b: 160,
};
const CRT_ACTIVE_FG: Color = Color::Black;
const CRT_MENU_BG: Color = Color::Rgb { r: 0, g: 40, b: 0 };
const CRT_MENU_FG: Color = Color::Rgb {
    r: 130,
    g: 255,
    b: 150,
};
const CRT_HEADING_FG: Color = Color::Rgb {
    r: 180,
    g: 255,
    b: 185,
};
const CRT_LINK_TEXT_FG: Color = Color::Rgb {
    r: 145,
    g: 235,
    b: 255,
};
const CRT_LINK_URL_FG: Color = Color::Rgb {
    r: 110,
    g: 200,
    b: 245,
};
const CRT_HTML_TAG_FG: Color = Color::Blue;
const CRT_CODE_FG: Color = Color::Rgb {
    r: 140,
    g: 240,
    b: 130,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum MdStyle {
    Normal,
    Heading,
    Quote,
    Marker,
    Code,
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
            status: StatusMessage::new("Alt-F/E/S/H: menus | Ctrl-S save | Ctrl-Q quit | F1 help"),
            active_menu: None,
            active_menu_index: 0,
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
        let mut x = 0;
        for (index, (menu, label)) in MENU_ITEMS.iter().enumerate() {
            if index > 0 {
                x += 2;
            }
            let end = x + label.len();
            if (x..end).contains(&column) {
                return Some(*menu);
            }
            x = end;
        }
        None
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
        let mut x = 0usize;
        for (index, (kind, label)) in MENU_ITEMS.iter().enumerate() {
            if index > 0 {
                x += 2;
            }
            if *kind == menu {
                return Some((x, label.len()));
            }
            x += label.len();
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
        let width = cmp::min(content_width + 2, cols - x);
        let max_height = rows.saturating_sub(3);
        let height = cmp::min(entries.len(), max_height);
        if width == 0 || height == 0 {
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
        if row < rect.y || row >= rect.y + rect.height {
            return None;
        }
        if column < rect.x || column >= rect.x + rect.width {
            return None;
        }
        Some(row - rect.y)
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
            MenuAction::Keybindings => {
                self.status = StatusMessage::new(
                    "Menus: Alt-F/E/S/H, arrows, Enter, Esc. Shortcuts: Ctrl-S/Q/Z/Y/X/C/V/F/R.",
                );
            }
            MenuAction::About => {
                self.status =
                    StatusMessage::new("redit: terminal markup editor prototype in Rust.");
            }
        }
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
        usize::from(rows.saturating_sub(3))
    }

    fn gutter_width(&self) -> usize {
        let digits = self.doc.line_count().max(1).to_string().len();
        digits + 1
    }

    fn scroll(&mut self) {
        let (cols, rows) = terminal::size().unwrap_or((80, 24));
        let text_height = usize::from(rows.saturating_sub(3));
        let gutter = self.gutter_width();
        let text_width = usize::from(cols).saturating_sub(gutter);

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

    fn draw_markdown_content(
        &mut self,
        line: &str,
        offset_x: usize,
        width: usize,
        in_code_block: bool,
    ) -> io::Result<bool> {
        let chars: Vec<char> = line.chars().collect();
        let styles = markdown_styles_for_line(&chars, in_code_block);
        let line_len = chars.len();
        let start = cmp::min(offset_x, line_len);
        let end = cmp::min(start + width, line_len);

        if start < end {
            let mut current_style = styles[start];
            apply_markdown_style(&mut self.terminal.stdout, current_style)?;
            let mut segment = String::new();

            for idx in start..end {
                let style = styles[idx];
                if style != current_style {
                    queue!(self.terminal.stdout, Print(&segment))?;
                    segment.clear();
                    current_style = style;
                    apply_markdown_style(&mut self.terminal.stdout, current_style)?;
                }
                segment.push(chars[idx]);
            }

            if !segment.is_empty() {
                queue!(self.terminal.stdout, Print(segment))?;
            }
            apply_markdown_style(&mut self.terminal.stdout, MdStyle::Normal)?;
        }

        let next_in_code_block = if is_fenced_code_line(line) {
            !in_code_block
        } else {
            in_code_block
        };
        Ok(next_in_code_block)
    }

    fn refresh_screen(&mut self) -> io::Result<()> {
        let (cols, rows) = terminal::size()?;
        let cols_usize = usize::from(cols);
        let text_height = usize::from(rows.saturating_sub(3));
        let gutter = self.gutter_width();
        let body_width = cols_usize.saturating_sub(gutter);

        queue!(
            self.terminal.stdout,
            Hide,
            SetBackgroundColor(CRT_BG),
            SetForegroundColor(CRT_FG),
            MoveTo(0, 0),
            Clear(ClearType::All)
        )?;

        self.draw_top_menu(cols)?;

        let mut in_code_block = self.code_block_state_before(self.offset.y);
        for screen_row in 0..text_height {
            let file_row = self.offset.y + screen_row;
            queue!(self.terminal.stdout, MoveTo(0, (screen_row + 1) as u16))?;
            let number = format!("{:>width$} ", file_row + 1, width = gutter - 1);
            let line = self.doc.line(file_row).cloned();
            if let Some(line) = line {
                queue!(self.terminal.stdout, Print(number))?;
                in_code_block =
                    self.draw_markdown_content(&line, self.offset.x, body_width, in_code_block)?;
            } else {
                queue!(
                    self.terminal.stdout,
                    Print(number),
                    SetForegroundColor(CRT_DIM_FG),
                    Print("~"),
                    SetForegroundColor(CRT_FG)
                )?;
            }
        }

        self.draw_dropdown_menu(cols, rows)?;
        self.draw_status_bar(cols)?;
        self.draw_message_bar(cols, rows)?;

        let cursor_screen_x = self.cursor.x.saturating_sub(self.offset.x) + gutter;
        let cursor_screen_y = self.cursor.y.saturating_sub(self.offset.y) + 1;
        if cursor_screen_y > 0 && cursor_screen_y <= text_height && cursor_screen_x < cols_usize {
            queue!(
                self.terminal.stdout,
                MoveTo(cursor_screen_x as u16, cursor_screen_y as u16),
                Show
            )?;
        } else {
            queue!(self.terminal.stdout, Show)?;
        }

        self.terminal.stdout.flush()
    }

    fn draw_top_menu(&mut self, cols: u16) -> io::Result<()> {
        let cols = usize::from(cols);
        let active_menu = self.active_menu;
        let mut x = 0usize;

        queue!(
            self.terminal.stdout,
            MoveTo(0, 0),
            SetBackgroundColor(CRT_BAR_BG),
            SetForegroundColor(CRT_BAR_FG),
            Print(" ".repeat(cols)),
            MoveTo(0, 0)
        )?;

        for (index, (kind, label)) in MENU_ITEMS.iter().enumerate() {
            if index > 0 {
                queue!(self.terminal.stdout, Print("  "))?;
                x += 2;
            }
            if x >= cols {
                break;
            }

            if active_menu == Some(*kind) {
                queue!(
                    self.terminal.stdout,
                    SetBackgroundColor(CRT_ACTIVE_BG),
                    SetForegroundColor(CRT_ACTIVE_FG),
                    SetAttribute(Attribute::Bold)
                )?;
            }
            queue!(self.terminal.stdout, Print(*label))?;
            if active_menu == Some(*kind) {
                queue!(
                    self.terminal.stdout,
                    SetAttribute(Attribute::NormalIntensity),
                    SetBackgroundColor(CRT_BAR_BG),
                    SetForegroundColor(CRT_BAR_FG)
                )?;
            }
            x += label.len();
        }

        queue!(
            self.terminal.stdout,
            SetAttribute(Attribute::Reset),
            SetBackgroundColor(CRT_BG),
            SetForegroundColor(CRT_FG)
        )
    }

    fn draw_dropdown_menu(&mut self, cols: u16, rows: u16) -> io::Result<()> {
        let Some(menu) = self.active_menu else {
            return Ok(());
        };

        let cols = usize::from(cols);
        let rows = usize::from(rows);
        let Some(rect) = self.dropdown_rect(menu, cols, rows) else {
            return Ok(());
        };

        let entries = Self::menu_entries(menu);
        for (idx, entry) in entries.iter().take(rect.height).enumerate() {
            let mut line = format!(" {}", entry.label);
            if line.len() < rect.width {
                line.push_str(&" ".repeat(rect.width - line.len()));
            }
            line.truncate(rect.width);
            let is_selected = idx == self.active_menu_index;
            queue!(
                self.terminal.stdout,
                MoveTo(rect.x as u16, (rect.y + idx) as u16),
                SetBackgroundColor(if is_selected {
                    CRT_ACTIVE_BG
                } else {
                    CRT_MENU_BG
                }),
                SetForegroundColor(if is_selected {
                    CRT_ACTIVE_FG
                } else {
                    CRT_MENU_FG
                }),
                Print(line),
                SetBackgroundColor(CRT_BG),
                SetForegroundColor(CRT_FG)
            )?;
        }
        Ok(())
    }

    fn draw_status_bar(&mut self, cols: u16) -> io::Result<()> {
        let cols = usize::from(cols);
        let name = self.doc.file_name_or_default();
        let modified = if self.doc.modified { " (modified)" } else { "" };
        let left = format!(
            "{name} - {} lines, {} words{modified} [Markdown]",
            self.doc.line_count(),
            self.doc.word_count()
        );
        let right = format!("Ln {}, Col {}", self.cursor.y + 1, self.cursor.x + 1);

        let mut line = left;
        if line.len() + right.len() > cols {
            line.truncate(cols.saturating_sub(right.len()));
        }
        while line.len() + right.len() < cols {
            line.push(' ');
        }
        line.push_str(&right);
        line.truncate(cols);

        let row = terminal::size()?.1.saturating_sub(2);
        queue!(
            self.terminal.stdout,
            MoveTo(0, row),
            SetBackgroundColor(CRT_BAR_BG),
            SetForegroundColor(CRT_BAR_FG),
            Print(line),
            SetAttribute(Attribute::Reset),
            SetBackgroundColor(CRT_BG),
            SetForegroundColor(CRT_FG)
        )
    }

    fn draw_message_bar(&mut self, cols: u16, rows: u16) -> io::Result<()> {
        let cols = usize::from(cols);
        let row = rows.saturating_sub(1);
        queue!(
            self.terminal.stdout,
            MoveTo(0, row),
            SetBackgroundColor(CRT_BG),
            SetForegroundColor(CRT_FG),
            Clear(ClearType::CurrentLine)
        )?;

        if self.status.created_at.elapsed() < Duration::from_secs(5) {
            let mut msg = self.status.text.clone();
            msg.truncate(cols);
            queue!(self.terminal.stdout, Print(msg))?;
        }
        Ok(())
    }
}

struct TerminalGuard {
    stdout: Stdout,
}

impl TerminalGuard {
    fn new() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture, Hide)?;
        Ok(Self { stdout })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = execute!(
            self.stdout,
            ResetColor,
            Show,
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

fn apply_markdown_style(stdout: &mut Stdout, style: MdStyle) -> io::Result<()> {
    let (fg, bold) = match style {
        MdStyle::Normal => (CRT_FG, false),
        MdStyle::Heading => (CRT_HEADING_FG, true),
        MdStyle::Quote => (CRT_DIM_FG, false),
        MdStyle::Marker => (CRT_ACTIVE_BG, true),
        MdStyle::Code => (CRT_CODE_FG, false),
        MdStyle::LinkText => (CRT_LINK_TEXT_FG, false),
        MdStyle::LinkUrl => (CRT_LINK_URL_FG, false),
        MdStyle::HtmlTag => (CRT_HTML_TAG_FG, false),
    };
    queue!(
        stdout,
        SetForegroundColor(fg),
        SetAttribute(if bold {
            Attribute::Bold
        } else {
            Attribute::NormalIntensity
        })
    )
}

fn markdown_styles_for_line(chars: &[char], in_code_block: bool) -> Vec<MdStyle> {
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
    apply_inline_code_styles(chars, &mut styles);
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

fn apply_link_styles(chars: &[char], styles: &mut [MdStyle]) {
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '[' {
            i += 1;
            continue;
        }
        let Some(close_bracket) = (i + 1..chars.len()).find(|&j| chars[j] == ']') else {
            i += 1;
            continue;
        };
        if close_bracket + 1 >= chars.len() || chars[close_bracket + 1] != '(' {
            i += 1;
            continue;
        }
        let Some(close_paren) = (close_bracket + 2..chars.len()).find(|&j| chars[j] == ')') else {
            i += 1;
            continue;
        };

        styles[i] = MdStyle::Marker;
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
        if chars[i] != '<' {
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

fn apply_inline_code_styles(chars: &[char], styles: &mut [MdStyle]) {
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '`' {
            i += 1;
            continue;
        }
        let Some(end) = (i + 1..chars.len()).find(|&j| chars[j] == '`') else {
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
