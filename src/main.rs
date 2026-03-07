use std::cmp;
use std::collections::HashSet;
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

mod document;
mod history;
mod markdown;
mod render;
mod selection;

use document::{Document, Position, byte_index_for_char, find_substring_at_char, remove_char_at};
use history::UndoRedoHistory;
use markdown::markdown_list_continuation;
use selection::{
    SelectionRange, delete_selection_in_lines, normalized_selection, selected_text_in_lines,
    selection_range_for_line,
};

fn main() -> io::Result<()> {
    let file_arg = env::args().nth(1).map(PathBuf::from);
    let mut editor = Editor::new(file_arg)?;
    editor.run()
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum MenuKind {
    File,
    Edit,
    Search,
    View,
    Tools,
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
    ToggleTerminal,
    Palette,
    Keybindings,
    About,
}

const MENU_ITEMS: &[(MenuKind, &str)] = &[
    (MenuKind::File, "File"),
    (MenuKind::Edit, "Edit"),
    (MenuKind::Search, "Search"),
    (MenuKind::View, "View"),
    (MenuKind::Tools, "Tools"),
    (MenuKind::Help, "Help"),
];

const APP_NAME: &str = "redit";
const APP_VERSION: &str = "0.1";
const SETTINGS_FILE: &str = "settings.conf";

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

const VIEW_MENU_ENTRIES: &[MenuEntry] = &[
    MenuEntry {
        label: "Markdown     Ctrl+P",
        mnemonic: 'p',
        action: MenuAction::TogglePreview,
    },
    MenuEntry {
        label: "Terminal     F3",
        mnemonic: 't',
        action: MenuAction::ToggleTerminal,
    },
];

const TOOLS_MENU_ENTRIES: &[MenuEntry] = &[MenuEntry {
    label: "Palette",
    mnemonic: 'p',
    action: MenuAction::Palette,
}];

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

const PREVIEW_MIN_TOTAL_WIDTH: usize = 56;
const PREVIEW_SEPARATOR_WIDTH: usize = 1;
const EXPLORER_WIDTH: usize = 28;
const EXPLORER_MIN_WIDTH: usize = 8;
const EXPLORER_SEPARATOR_WIDTH: usize = 1;
const TOP_BAR_ROWS: usize = 1;
const FOOTER_ROWS: usize = 2;
const MIN_EDITOR_OUTER_HEIGHT: usize = 3;
const MIN_TERMINAL_OUTER_HEIGHT: usize = 3;

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

#[derive(Clone)]
struct InfoPopupState {
    title: String,
    lines: Vec<String>,
}

#[derive(Clone)]
struct InfoPopupRender {
    rect: Rect,
    title: String,
    lines: Vec<Line<'static>>,
}

#[derive(Clone)]
struct PalettePopupState {
    selected: usize,
}

#[derive(Clone)]
struct PalettePopupRender {
    rect: Rect,
    title: String,
    lines: Vec<Line<'static>>,
}

#[derive(Clone)]
struct ShellPopupState {
    input: String,
    cursor: usize,
    output_lines: Vec<String>,
}

#[derive(Clone)]
struct ExplorerEntry {
    rendered_label: String,
    path: PathBuf,
    is_dir: bool,
    expanded: bool,
    parent: Option<PathBuf>,
    is_parent_link: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ActivePane {
    Editor,
    Explorer,
    Shell,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PaletteTheme {
    MainframeGreen,
    BlackWhite,
    Amber,
    OceanBlue,
    LightPaper,
}

#[derive(Clone, Copy)]
struct Theme {
    bg: Color,
    fg: Color,
    dim_fg: Color,
    bar_bg: Color,
    bar_fg: Color,
    active_bg: Color,
    active_fg: Color,
    menu_bg: Color,
    menu_fg: Color,
    heading_fg: Color,
    html_tag_fg: Color,
    marker_fg: Color,
    marker_bg: Color,
    line_bg: Color,
    panel_border: Color,
    preview_sep: Color,
    message_bg: Color,
    input_bg: Color,
    input_active_bg: Color,
    selection_bg: Color,
    selection_fg: Color,
}

impl PaletteTheme {
    const ALL: [PaletteTheme; 5] = [
        PaletteTheme::MainframeGreen,
        PaletteTheme::BlackWhite,
        PaletteTheme::Amber,
        PaletteTheme::OceanBlue,
        PaletteTheme::LightPaper,
    ];

    fn name(self) -> &'static str {
        match self {
            PaletteTheme::MainframeGreen => "Mainframe Green",
            PaletteTheme::BlackWhite => "Black & White",
            PaletteTheme::Amber => "Amber",
            PaletteTheme::OceanBlue => "Ocean Blue",
            PaletteTheme::LightPaper => "Light Paper",
        }
    }

    fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|theme| *theme == self)
            .unwrap_or(0)
    }

    fn from_index(index: usize) -> Self {
        Self::ALL[index % Self::ALL.len()]
    }

    fn settings_value(self) -> &'static str {
        match self {
            PaletteTheme::MainframeGreen => "mainframe_green",
            PaletteTheme::BlackWhite => "black_white",
            PaletteTheme::Amber => "amber",
            PaletteTheme::OceanBlue => "ocean_blue",
            PaletteTheme::LightPaper => "light_paper",
        }
    }

    fn from_settings_value(value: &str) -> Option<Self> {
        match value {
            "mainframe_green" => Some(PaletteTheme::MainframeGreen),
            "black_white" => Some(PaletteTheme::BlackWhite),
            "amber" => Some(PaletteTheme::Amber),
            "ocean_blue" => Some(PaletteTheme::OceanBlue),
            "light_paper" => Some(PaletteTheme::LightPaper),
            _ => None,
        }
    }

    fn theme(self) -> Theme {
        match self {
            PaletteTheme::MainframeGreen => Theme {
                bg: CRT_BG,
                fg: CRT_FG,
                dim_fg: CRT_DIM_FG,
                bar_bg: CRT_BAR_BG,
                bar_fg: CRT_BAR_FG,
                active_bg: CRT_ACTIVE_BG,
                active_fg: CRT_ACTIVE_FG,
                menu_bg: CRT_MENU_BG,
                menu_fg: CRT_MENU_FG,
                heading_fg: CRT_HEADING_FG,
                html_tag_fg: CRT_HTML_TAG_FG,
                marker_fg: CRT_MARKER_FG,
                marker_bg: CRT_MARKER_BG,
                line_bg: CRT_LINE_BG,
                panel_border: CRT_PANEL_BORDER,
                preview_sep: CRT_PREVIEW_SEP,
                message_bg: CRT_MESSAGE_BG,
                input_bg: CRT_INPUT_BG,
                input_active_bg: CRT_INPUT_ACTIVE_BG,
                selection_bg: CRT_SELECTION_BG,
                selection_fg: CRT_SELECTION_FG,
            },
            PaletteTheme::BlackWhite => Theme {
                bg: Color::Black,
                fg: Color::White,
                dim_fg: Color::Rgb(170, 170, 170),
                bar_bg: Color::Rgb(32, 32, 32),
                bar_fg: Color::White,
                active_bg: Color::White,
                active_fg: Color::Black,
                menu_bg: Color::Rgb(18, 18, 18),
                menu_fg: Color::White,
                heading_fg: Color::Rgb(170, 170, 255),
                html_tag_fg: Color::Rgb(180, 180, 255),
                marker_fg: Color::Rgb(170, 170, 255),
                marker_bg: Color::Rgb(30, 30, 30),
                line_bg: Color::Rgb(12, 12, 12),
                panel_border: Color::Rgb(200, 200, 200),
                preview_sep: Color::Rgb(110, 110, 110),
                message_bg: Color::Rgb(24, 24, 24),
                input_bg: Color::Rgb(16, 16, 16),
                input_active_bg: Color::Rgb(30, 30, 30),
                selection_bg: Color::Rgb(70, 70, 70),
                selection_fg: Color::White,
            },
            PaletteTheme::Amber => Theme {
                bg: Color::Rgb(20, 10, 2),
                fg: Color::Rgb(255, 236, 190),
                dim_fg: Color::Rgb(201, 147, 70),
                bar_bg: Color::Rgb(78, 41, 10),
                bar_fg: Color::Rgb(255, 221, 160),
                active_bg: Color::Rgb(255, 204, 120),
                active_fg: Color::Black,
                menu_bg: Color::Rgb(44, 22, 6),
                menu_fg: Color::Rgb(255, 210, 133),
                heading_fg: Color::Rgb(255, 189, 102),
                html_tag_fg: Color::Rgb(255, 174, 79),
                marker_fg: Color::Rgb(255, 160, 54),
                marker_bg: Color::Rgb(59, 27, 5),
                line_bg: Color::Rgb(27, 13, 3),
                panel_border: Color::Rgb(201, 132, 38),
                preview_sep: Color::Rgb(150, 95, 25),
                message_bg: Color::Rgb(35, 17, 5),
                input_bg: Color::Rgb(28, 14, 4),
                input_active_bg: Color::Rgb(49, 24, 6),
                selection_bg: Color::Rgb(134, 80, 8),
                selection_fg: Color::Rgb(255, 244, 224),
            },
            PaletteTheme::OceanBlue => Theme {
                bg: Color::Rgb(4, 10, 22),
                fg: Color::Rgb(230, 242, 255),
                dim_fg: Color::Rgb(112, 156, 214),
                bar_bg: Color::Rgb(12, 39, 82),
                bar_fg: Color::Rgb(196, 226, 255),
                active_bg: Color::Rgb(133, 188, 255),
                active_fg: Color::Rgb(4, 20, 51),
                menu_bg: Color::Rgb(7, 24, 51),
                menu_fg: Color::Rgb(173, 212, 255),
                heading_fg: Color::Rgb(122, 198, 255),
                html_tag_fg: Color::Rgb(129, 184, 255),
                marker_fg: Color::Rgb(107, 168, 255),
                marker_bg: Color::Rgb(10, 28, 64),
                line_bg: Color::Rgb(7, 18, 40),
                panel_border: Color::Rgb(91, 151, 222),
                preview_sep: Color::Rgb(59, 116, 191),
                message_bg: Color::Rgb(9, 22, 46),
                input_bg: Color::Rgb(8, 20, 43),
                input_active_bg: Color::Rgb(14, 33, 70),
                selection_bg: Color::Rgb(43, 93, 170),
                selection_fg: Color::Rgb(245, 250, 255),
            },
            PaletteTheme::LightPaper => Theme {
                bg: Color::Rgb(248, 247, 241),
                fg: Color::Rgb(34, 39, 46),
                dim_fg: Color::Rgb(101, 109, 124),
                bar_bg: Color::Rgb(217, 221, 230),
                bar_fg: Color::Rgb(35, 39, 46),
                active_bg: Color::Rgb(86, 132, 210),
                active_fg: Color::Rgb(255, 255, 255),
                menu_bg: Color::Rgb(232, 235, 242),
                menu_fg: Color::Rgb(47, 57, 69),
                heading_fg: Color::Rgb(38, 104, 189),
                html_tag_fg: Color::Rgb(46, 117, 211),
                marker_fg: Color::Rgb(47, 108, 189),
                marker_bg: Color::Rgb(220, 229, 242),
                line_bg: Color::Rgb(237, 239, 244),
                panel_border: Color::Rgb(123, 140, 163),
                preview_sep: Color::Rgb(151, 167, 188),
                message_bg: Color::Rgb(227, 231, 239),
                input_bg: Color::Rgb(231, 235, 243),
                input_active_bg: Color::Rgb(212, 222, 240),
                selection_bg: Color::Rgb(72, 124, 204),
                selection_fg: Color::Rgb(255, 255, 255),
            },
        }
    }
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
    palette_theme: PaletteTheme,
    theme: Theme,
    explorer_root: PathBuf,
    explorer_entries: Vec<ExplorerEntry>,
    explorer_selected: usize,
    explorer_offset: usize,
    explorer_expanded_dirs: HashSet<PathBuf>,
    active_pane: ActivePane,
    last_explorer_click: Option<(usize, Instant)>,
    clipboard: String,
    history: UndoRedoHistory<EditorSnapshot>,
    selection_anchor: Option<Position>,
    mouse_drag_anchor: Option<Position>,
    save_as_popup: Option<SaveAsPopupState>,
    search_popup: Option<SearchPopupState>,
    info_popup: Option<InfoPopupState>,
    palette_popup: Option<PalettePopupState>,
    shell_popup: Option<ShellPopupState>,
    last_search_query: String,
    should_quit: bool,
    quit_warning_countdown: u8,
}

impl Editor {
    fn default_terminal_state() -> ShellPopupState {
        ShellPopupState {
            input: String::new(),
            cursor: 0,
            output_lines: Vec::new(),
        }
    }

    fn new(file_arg: Option<PathBuf>) -> io::Result<Self> {
        let opened_path = file_arg.as_ref().and_then(|path| {
            if Path::new(path).exists() {
                Some(fs::canonicalize(path).unwrap_or_else(|_| path.clone()))
            } else {
                None
            }
        });
        let doc = match file_arg {
            Some(path) if Path::new(&path).exists() => Document::open(path)?,
            Some(path) => Document::new_empty(Some(path)),
            None => Document::new_empty(None),
        };
        let explorer_root = opened_path
            .as_ref()
            .and_then(|path| path.parent().map(Path::to_path_buf))
            .filter(|path| !path.as_os_str().is_empty())
            .or_else(|| env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        let palette_theme = Self::load_palette_theme_setting();

        let mut editor = Self {
            terminal: TerminalGuard::new()?,
            doc,
            cursor: Position::default(),
            offset: Position::default(),
            status: StatusMessage::new(
                "F2: explorer | F3: terminal | Ctrl-S save | Ctrl-Q quit | F1 help",
            ),
            active_menu: None,
            active_menu_index: 0,
            preview_mode: false,
            preview_cache_lines: Vec::new(),
            preview_cache_width: 0,
            preview_cache_revision: 0,
            preview_backend: PreviewBackend::Fallback,
            preview_error: None,
            palette_theme,
            theme: palette_theme.theme(),
            explorer_root,
            explorer_entries: Vec::new(),
            explorer_selected: 0,
            explorer_offset: 0,
            explorer_expanded_dirs: HashSet::new(),
            active_pane: ActivePane::Editor,
            last_explorer_click: None,
            clipboard: String::new(),
            history: UndoRedoHistory::default(),
            selection_anchor: None,
            mouse_drag_anchor: None,
            save_as_popup: None,
            search_popup: None,
            info_popup: None,
            palette_popup: None,
            shell_popup: Some(Self::default_terminal_state()),
            last_search_query: String::new(),
            should_quit: false,
            quit_warning_countdown: 1,
        };
        editor.refresh_explorer_entries();
        Ok(editor)
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
        if self.palette_popup.is_some() {
            if self.handle_palette_popup_key(key) {
                self.scroll();
                return;
            }
        }

        if self.info_popup.is_some() {
            if self.handle_info_popup_key(key) {
                self.scroll();
                return;
            }
        }

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

        if self.active_pane == ActivePane::Shell && self.shell_popup.is_some() {
            if self.handle_shell_popup_key(key) {
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

        if self.active_pane == ActivePane::Explorer && self.handle_explorer_key(key) {
            self.scroll();
            return;
        }

        match key {
            KeyEvent {
                code: KeyCode::F(3),
                ..
            } => self.toggle_terminal_pane(),
            KeyEvent {
                code: KeyCode::F(2),
                ..
            } => {
                self.active_pane = if self.active_pane != ActivePane::Explorer {
                    self.ensure_explorer_selection_visible(self.explorer_visible_rows());
                    ActivePane::Explorer
                } else {
                    ActivePane::Editor
                };
            }
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
            } if self.active_pane == ActivePane::Editor => {
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
            MenuKind::View => "View menu opened",
            MenuKind::Tools => "Tools menu opened",
            MenuKind::Help => "Help menu opened",
        });
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) {
        if self.palette_popup.is_some() {
            if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                let col = usize::from(mouse.column);
                let row = usize::from(mouse.row);
                let (cols, rows) = terminal::size().unwrap_or((80, 24));
                let cols = usize::from(cols);
                let rows = usize::from(rows);
                if let Some(index) = self.palette_index_from_mouse(col, row, cols, rows) {
                    if let Some(popup) = self.palette_popup.as_mut() {
                        popup.selected = index;
                    }
                    self.apply_selected_palette();
                } else {
                    self.palette_popup = None;
                }
            }
            return;
        }

        if self.info_popup.is_some() {
            if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                self.info_popup = None;
            }
            return;
        }

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

                if let Some(shell_cursor) = self.shell_cursor_from_mouse(col, row, cols, rows) {
                    self.active_pane = ActivePane::Shell;
                    if let Some(shell) = self.shell_popup.as_mut() {
                        shell.cursor = cmp::min(shell_cursor, shell.input.chars().count());
                    }
                    self.mouse_drag_anchor = None;
                    return;
                }

                if let Some(index) = self.explorer_index_from_mouse(col, row, cols, rows) {
                    self.active_pane = ActivePane::Explorer;
                    self.explorer_selected = index;
                    self.ensure_explorer_selection_visible(self.explorer_visible_rows());
                    let now = Instant::now();
                    let double_clicked =
                        self.last_explorer_click
                            .is_some_and(|(last_index, last_at)| {
                                last_index == index
                                    && now.duration_since(last_at) <= Duration::from_millis(500)
                            });
                    self.last_explorer_click = Some((index, now));
                    if double_clicked {
                        self.open_explorer_selected();
                    }
                    return;
                }

                if let Some(pos) = self.editor_position_from_mouse(col, row, cols, rows) {
                    self.active_pane = ActivePane::Editor;
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

    fn palette_popup_rect(&self, cols: usize, rows: usize) -> Option<Rect> {
        if self.palette_popup.is_none() || cols < 30 || rows < 10 {
            return None;
        }

        let content_width = PaletteTheme::ALL
            .iter()
            .enumerate()
            .map(|(idx, theme)| format!(" {}. {}", idx + 1, theme.name()).chars().count())
            .max()
            .unwrap_or(0);
        let width = cmp::max(30, cmp::min(content_width + 6, cols.saturating_sub(4)));
        let desired_height = PaletteTheme::ALL.len() + 4;
        let height = cmp::max(7, cmp::min(desired_height, rows.saturating_sub(2)));
        if width >= cols || height >= rows {
            return None;
        }

        Some(Rect::new(
            ((cols - width) / 2) as u16,
            ((rows - height) / 2) as u16,
            width as u16,
            height as u16,
        ))
    }

    fn palette_index_from_mouse(
        &self,
        column: usize,
        row: usize,
        cols: usize,
        rows: usize,
    ) -> Option<usize> {
        let rect = self.palette_popup_rect(cols, rows)?;
        let left = usize::from(rect.x);
        let top = usize::from(rect.y);
        let width = usize::from(rect.width);
        let height = usize::from(rect.height);

        if column <= left
            || column >= left + width.saturating_sub(1)
            || row <= top
            || row >= top + height.saturating_sub(1)
        {
            return None;
        }

        let inner_row = row.saturating_sub(top + 1);
        if inner_row >= PaletteTheme::ALL.len() {
            return None;
        }
        Some(inner_row)
    }

    fn shell_pane_outer_height(&self, rows: usize) -> usize {
        if self.shell_popup.is_none() {
            return 0;
        }

        let available = self.pane_available_height(rows);
        if available == 0 {
            return 0;
        }

        let desired = (available + 2) / 4;
        if available >= MIN_EDITOR_OUTER_HEIGHT + MIN_TERMINAL_OUTER_HEIGHT {
            desired.clamp(
                MIN_TERMINAL_OUTER_HEIGHT,
                available.saturating_sub(MIN_EDITOR_OUTER_HEIGHT),
            )
        } else {
            desired.max(1).min(available.saturating_sub(1))
        }
    }

    fn editor_outer_height_for_rows(&self, rows: usize) -> usize {
        let available = self.pane_available_height(rows);
        if self.shell_popup.is_none() {
            available
        } else {
            available.saturating_sub(self.shell_pane_outer_height(rows))
        }
    }

    fn pane_available_height(&self, rows: usize) -> usize {
        rows.saturating_sub(TOP_BAR_ROWS + FOOTER_ROWS)
    }

    fn explorer_outer_height_for_rows(&self, rows: usize) -> usize {
        self.editor_outer_height_for_rows(rows) + self.shell_pane_outer_height(rows)
    }

    fn explorer_text_height_for_rows(&self, rows: usize) -> usize {
        self.explorer_outer_height_for_rows(rows).saturating_sub(2)
    }

    fn editor_text_height_for_rows(&self, rows: usize) -> usize {
        self.editor_outer_height_for_rows(rows).saturating_sub(2)
    }

    fn shell_cursor_from_mouse(
        &self,
        column: usize,
        row: usize,
        cols: usize,
        rows: usize,
    ) -> Option<usize> {
        let inner_width = cols.saturating_sub(2);
        let editor_start = self.editor_start_x(inner_width);
        let editor_cols = self.editor_total_width(inner_width);
        let shell_height = self.shell_pane_outer_height(rows);
        if shell_height < 2 {
            return None;
        }
        let shell_outer_y = 1 + self.editor_outer_height_for_rows(rows);
        let shell_inner_top = shell_outer_y + 1;
        let shell_input_row = shell_outer_y + shell_height.saturating_sub(2);
        let shell_inner_left = 1 + editor_start;
        let shell_inner_right = shell_inner_left + editor_cols;
        if row < shell_inner_top || row > shell_input_row {
            return None;
        }
        if row == shell_input_row {
            if column < shell_inner_left || column >= shell_inner_right {
                return Some(0);
            }
            let input_offset = column - shell_inner_left;
            let prompt_width = 2usize;
            Some(input_offset.saturating_sub(prompt_width))
        } else {
            Some(
                self.shell_popup
                    .as_ref()
                    .map_or(0, |shell| shell.input.chars().count()),
            )
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
        let text_height = self.editor_text_height_for_rows(rows);
        if text_height == 0 {
            return None;
        }
        if row < 2 || row >= 2 + text_height {
            return None;
        }

        let editor_start = self.editor_start_x(inner_width);
        let gutter = self.gutter_width();
        let editor_cols = self.editor_total_width(inner_width);
        let body_width = self.editor_body_width(editor_cols);
        let editor_left = 1 + editor_start;
        let editor_end = editor_left + gutter + body_width;
        if column < editor_left || column >= editor_end {
            return None;
        }

        let file_row = cmp::min(
            self.offset.y + (row - 2),
            self.doc.line_count().saturating_sub(1),
        );
        let visual_x = if column <= editor_left + gutter - 1 {
            0
        } else {
            self.offset.x + column - (editor_left + gutter)
        };
        let line_len = self.doc.line_char_len(file_row);
        Some(Position {
            x: cmp::min(visual_x, line_len),
            y: file_row,
        })
    }

    fn explorer_index_from_mouse(
        &self,
        column: usize,
        row: usize,
        cols: usize,
        rows: usize,
    ) -> Option<usize> {
        let inner_width = cols.saturating_sub(2);
        let text_height = self.explorer_text_height_for_rows(rows);
        if text_height == 0 || row < 2 || row >= 2 + text_height {
            return None;
        }

        let (explorer_width, _) = self.explorer_layout(inner_width)?;
        let explorer_left = 1;
        let explorer_right = explorer_left + explorer_width;
        if column < explorer_left || column >= explorer_right {
            return None;
        }

        let idx = self.explorer_offset + (row - 2);
        (idx < self.explorer_entries.len()).then_some(idx)
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
            MenuKind::View => VIEW_MENU_ENTRIES,
            MenuKind::Tools => TOOLS_MENU_ENTRIES,
            MenuKind::Help => HELP_MENU_ENTRIES,
        }
    }

    fn menu_from_mnemonic(c: char) -> Option<MenuKind> {
        match c {
            'f' => Some(MenuKind::File),
            'e' => Some(MenuKind::Edit),
            's' => Some(MenuKind::Search),
            'v' => Some(MenuKind::View),
            't' => Some(MenuKind::Tools),
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
            MenuAction::ToggleTerminal => self.toggle_terminal_pane(),
            MenuAction::Palette => self.open_palette_popup(),
            MenuAction::Keybindings => self.open_info_popup(
                " Keybindings ",
                vec![
                    "F1 Help menu",
                    "F2 Focus Files/Editor",
                    "F3 Toggle Terminal",
                    "Ctrl-S Save",
                    "Ctrl-Shift-S Save As",
                    "Ctrl-Q Quit",
                    "Ctrl-P Toggle Markdown",
                    "Ctrl-F Find",
                    "Ctrl-R Replace",
                    "Tools > Palette Theme",
                ],
            ),
            MenuAction::About => self.open_info_popup(
                " About redit ",
                vec![
                    format!("{APP_NAME} v{APP_VERSION}"),
                    "Terminal markdown editor prototype in Rust.".to_string(),
                ],
            ),
        }
    }

    fn open_info_popup<T: Into<String>>(&mut self, title: impl Into<String>, lines: Vec<T>) {
        self.info_popup = Some(InfoPopupState {
            title: title.into(),
            lines: lines.into_iter().map(Into::into).collect(),
        });
        self.palette_popup = None;
        self.save_as_popup = None;
        self.search_popup = None;
        self.active_menu = None;
        self.active_menu_index = 0;
    }

    fn handle_info_popup_key(&mut self, key: KeyEvent) -> bool {
        if self.info_popup.is_none() {
            return false;
        }

        match key.code {
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') | KeyCode::Char(' ') => {
                self.info_popup = None;
            }
            _ => {}
        }
        true
    }

    fn open_palette_popup(&mut self) {
        self.palette_popup = Some(PalettePopupState {
            selected: self.palette_theme.index(),
        });
        self.info_popup = None;
        self.save_as_popup = None;
        self.search_popup = None;
        self.active_menu = None;
        self.active_menu_index = 0;
    }

    fn handle_palette_popup_key(&mut self, key: KeyEvent) -> bool {
        if self.palette_popup.is_none() {
            return false;
        }

        let len = PaletteTheme::ALL.len();
        match key.code {
            KeyCode::Esc => {
                self.palette_popup = None;
            }
            KeyCode::Up => {
                if let Some(popup) = self.palette_popup.as_mut() {
                    popup.selected = popup.selected.saturating_sub(1);
                }
            }
            KeyCode::Down => {
                if let Some(popup) = self.palette_popup.as_mut() {
                    popup.selected = cmp::min(popup.selected + 1, len.saturating_sub(1));
                }
            }
            KeyCode::Home => {
                if let Some(popup) = self.palette_popup.as_mut() {
                    popup.selected = 0;
                }
            }
            KeyCode::End => {
                if let Some(popup) = self.palette_popup.as_mut() {
                    popup.selected = len.saturating_sub(1);
                }
            }
            KeyCode::Enter => {
                self.apply_selected_palette();
            }
            KeyCode::Char(c) if ('1'..='9').contains(&c) => {
                let idx = (c as u8 - b'1') as usize;
                if idx < len {
                    if let Some(popup) = self.palette_popup.as_mut() {
                        popup.selected = idx;
                    }
                    self.apply_selected_palette();
                }
            }
            _ => {}
        }

        true
    }

    fn apply_selected_palette(&mut self) {
        let Some(popup) = self.palette_popup.clone() else {
            return;
        };
        let selected = PaletteTheme::from_index(popup.selected);
        self.palette_theme = selected;
        self.theme = selected.theme();
        self.palette_popup = None;
        match Self::save_palette_theme_setting(selected) {
            Ok(()) => {
                self.status = StatusMessage::new(format!("Palette changed to {}.", selected.name()))
            }
            Err(err) => {
                self.status = StatusMessage::new(format!(
                    "Palette changed to {} (settings not saved: {err}).",
                    selected.name()
                ));
            }
        }
    }

    fn load_palette_theme_setting() -> PaletteTheme {
        let Some(path) = Self::settings_path() else {
            return PaletteTheme::MainframeGreen;
        };
        let Ok(contents) = fs::read_to_string(path) else {
            return PaletteTheme::MainframeGreen;
        };

        contents
            .lines()
            .find_map(|line| {
                let (key, value) = line.split_once('=')?;
                if key.trim() != "palette" {
                    return None;
                }
                PaletteTheme::from_settings_value(value.trim())
            })
            .unwrap_or(PaletteTheme::MainframeGreen)
    }

    fn save_palette_theme_setting(theme: PaletteTheme) -> io::Result<()> {
        let Some(path) = Self::settings_path() else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, format!("palette={}\n", theme.settings_value()))
    }

    fn settings_path() -> Option<PathBuf> {
        if let Some(path) = env::var_os("XDG_CONFIG_HOME") {
            return Some(PathBuf::from(path).join(APP_NAME).join(SETTINGS_FILE));
        }
        env::var_os("HOME")
            .map(PathBuf::from)
            .map(|home| home.join(".config").join(APP_NAME).join(SETTINGS_FILE))
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

    fn begin_edit(&mut self) {
        self.history.begin_edit(self.current_snapshot());
    }

    fn clear_selection(&mut self) {
        self.selection_anchor = None;
    }

    fn normalized_selection(&self) -> Option<SelectionRange> {
        normalized_selection(self.selection_anchor, self.cursor)
    }

    fn normalize_selection_anchor(&mut self) {
        if self.selection_anchor == Some(self.cursor) {
            self.selection_anchor = None;
        }
    }

    fn selection_range_for_line(&self, line_idx: usize, line_len: usize) -> Option<(usize, usize)> {
        let selection = self.normalized_selection()?;
        selection_range_for_line(selection, line_idx, line_len)
    }

    fn selected_text(&self) -> Option<String> {
        let selection = self.normalized_selection()?;
        Some(selected_text_in_lines(&self.doc.lines, selection))
    }

    fn delete_selection(&mut self) -> bool {
        let Some(selection) = self.normalized_selection() else {
            return false;
        };

        self.cursor = delete_selection_in_lines(&mut self.doc.lines, selection);
        self.doc.modified = true;
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
        let Some(snapshot) = self.history.undo(self.current_snapshot()) else {
            self.status = StatusMessage::new("Nothing to undo.");
            return;
        };

        self.restore_snapshot(snapshot);
        self.status = StatusMessage::new("Undo.");
    }

    fn redo(&mut self) {
        let Some(snapshot) = self.history.redo(self.current_snapshot()) else {
            self.status = StatusMessage::new("Nothing to redo.");
            return;
        };

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
                self.replace_next_occurrence(&popup.find_input, &popup.replace_input);
            }
        }
    }

    fn toggle_terminal_pane(&mut self) {
        self.save_as_popup = None;
        self.search_popup = None;
        self.active_menu = None;
        self.active_menu_index = 0;

        if self.shell_popup.is_some() {
            self.shell_popup = None;
            self.active_pane = ActivePane::Editor;
            self.status = StatusMessage::new("Terminal hidden.");
        } else {
            self.shell_popup = Some(Self::default_terminal_state());
            self.active_pane = ActivePane::Shell;
            self.status = StatusMessage::new("Terminal shown.");
        }
    }

    fn handle_shell_popup_key(&mut self, key: KeyEvent) -> bool {
        if self.shell_popup.is_none() {
            return false;
        }

        match key {
            KeyEvent {
                code: KeyCode::Esc, ..
            } => {
                self.active_pane = ActivePane::Editor;
                self.status = StatusMessage::new("Terminal focus cleared.");
            }
            KeyEvent {
                code: KeyCode::F(3),
                ..
            } => {
                self.toggle_terminal_pane();
            }
            KeyEvent {
                code: KeyCode::Enter,
                ..
            } => self.submit_shell_popup(),
            KeyEvent {
                code: KeyCode::Backspace,
                ..
            } => self.shell_backspace(),
            KeyEvent {
                code: KeyCode::Delete,
                ..
            } => self.shell_delete(),
            KeyEvent {
                code: KeyCode::Left,
                ..
            } => {
                if let Some(popup) = self.shell_popup.as_mut() {
                    popup.cursor = popup.cursor.saturating_sub(1);
                }
            }
            KeyEvent {
                code: KeyCode::Right,
                ..
            } => {
                if let Some(popup) = self.shell_popup.as_mut() {
                    popup.cursor = cmp::min(popup.cursor + 1, popup.input.chars().count());
                }
            }
            KeyEvent {
                code: KeyCode::Home,
                ..
            } => {
                if let Some(popup) = self.shell_popup.as_mut() {
                    popup.cursor = 0;
                }
            }
            KeyEvent {
                code: KeyCode::End, ..
            } => {
                if let Some(popup) = self.shell_popup.as_mut() {
                    popup.cursor = popup.input.chars().count();
                }
            }
            KeyEvent {
                code: KeyCode::Char('v'),
                modifiers,
                ..
            } if modifiers.contains(KeyModifiers::CONTROL) => {
                let paste_text = self.clipboard.clone();
                if !paste_text.is_empty() {
                    self.shell_insert_text(&paste_text);
                }
            }
            KeyEvent {
                code: KeyCode::Char('l'),
                modifiers,
                ..
            } if modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(popup) = self.shell_popup.as_mut() {
                    popup.output_lines.clear();
                    popup.output_lines.push("Output cleared.".to_string());
                }
            }
            KeyEvent {
                code: KeyCode::Char(c),
                modifiers,
                ..
            } if !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
                let mut text = String::new();
                text.push(c);
                self.shell_insert_text(&text);
            }
            _ => {
                if key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                    || matches!(key.code, KeyCode::F(1) | KeyCode::F(2))
                {
                    return false;
                }
            }
        }

        true
    }

    fn shell_insert_text(&mut self, text: &str) {
        if let Some(popup) = self.shell_popup.as_mut() {
            let byte_idx = byte_index_for_char(&popup.input, popup.cursor);
            popup.input.insert_str(byte_idx, text);
            popup.cursor += text.chars().count();
        }
    }

    fn shell_backspace(&mut self) {
        if let Some(popup) = self.shell_popup.as_mut() {
            if popup.cursor > 0 {
                remove_char_at(&mut popup.input, popup.cursor - 1);
                popup.cursor -= 1;
            }
        }
    }

    fn shell_delete(&mut self) {
        if let Some(popup) = self.shell_popup.as_mut() {
            remove_char_at(&mut popup.input, popup.cursor);
        }
    }

    fn submit_shell_popup(&mut self) {
        let Some(popup) = self.shell_popup.clone() else {
            return;
        };
        let command = popup.input.trim();
        if command.is_empty() {
            self.status = StatusMessage::new("Terminal command cannot be empty.");
            return;
        }

        match run_shell_command_line(command) {
            Ok(output) => {
                let mut lines = Vec::new();
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                lines.extend(stdout.lines().map(|line| line.to_string()));
                lines.extend(stderr.lines().map(|line| format!("! {line}")));
                if lines.is_empty() {
                    lines.push("(no output)".to_string());
                }

                let max_lines = 200usize;
                if lines.len() > max_lines {
                    lines.truncate(max_lines);
                    lines.push(format!("... output truncated to {max_lines} lines"));
                }

                if let Some(state) = self.shell_popup.as_mut() {
                    state.input.clear();
                    state.cursor = 0;
                    state.output_lines.push(format!("$ {command}"));
                    state.output_lines.extend(lines);
                    let keep = 500usize;
                    if state.output_lines.len() > keep {
                        let drop_count = state.output_lines.len() - keep;
                        state.output_lines.drain(0..drop_count);
                    }
                }

                if output.status.success() {
                    self.status =
                        StatusMessage::new(format!("Terminal command succeeded: {command}"));
                } else {
                    let code = output
                        .status
                        .code()
                        .map_or_else(|| "signal".to_string(), |code| code.to_string());
                    self.status = StatusMessage::new(format!(
                        "Terminal command failed (code {code}): {command}"
                    ));
                }
            }
            Err(err) => {
                if let Some(state) = self.shell_popup.as_mut() {
                    state.output_lines.push(format!("$ {command}"));
                    state.output_lines.push(format!("Terminal error: {err}"));
                    let keep = 500usize;
                    if state.output_lines.len() > keep {
                        let drop_count = state.output_lines.len() - keep;
                        state.output_lines.drain(0..drop_count);
                    }
                }
                self.status = StatusMessage::new(format!("Terminal command failed: {err}"));
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

    fn count_occurrences(&self, query: &str) -> usize {
        self.doc
            .lines
            .iter()
            .map(|line| line.match_indices(query).count())
            .sum()
    }

    fn replace_next_occurrence(&mut self, query: &str, replacement: &str) {
        if query.is_empty() {
            self.status = StatusMessage::new("Find text cannot be empty.");
            return;
        }

        let found = self
            .find_from(query, self.cursor.y, self.cursor.x)
            .or_else(|| self.find_from(query, 0, 0));
        let Some(pos) = found else {
            self.status = StatusMessage::new(format!("No matches for \"{query}\". 0 remaining."));
            return;
        };

        self.begin_edit();
        if let Some(line) = self.doc.lines.get_mut(pos.y) {
            let query_len = query.chars().count();
            let start = byte_index_for_char(line, pos.x);
            let end = byte_index_for_char(line, pos.x + query_len);
            if start <= end && end <= line.len() {
                line.replace_range(start..end, replacement);
            }
        }

        self.doc.modified = true;
        self.clear_selection();
        self.cursor = Position {
            x: pos.x + replacement.chars().count(),
            y: pos.y,
        };
        self.clamp_cursor_to_document();
        self.scroll();

        let remaining = self.count_occurrences(query);
        self.status = StatusMessage::new(format!(
            "Replaced 1 occurrence of \"{query}\". {remaining} remaining."
        ));
    }

    fn toggle_preview(&mut self) {
        self.preview_mode = !self.preview_mode;
        if self.preview_mode {
            self.preview_cache_lines.clear();
            self.preview_cache_width = 0;
            self.preview_cache_revision = 0;
            let (cols, _) = terminal::size().unwrap_or((80, 24));
            let inner_cols = usize::from(cols).saturating_sub(2);
            if self
                .preview_layout(self.editor_total_width(inner_cols))
                .is_some()
            {
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

    fn explorer_layout(&self, cols: usize) -> Option<(usize, usize)> {
        let gutter = self.gutter_width();
        let min_editor_width = 12usize;
        let available = cols.saturating_sub(gutter + EXPLORER_SEPARATOR_WIDTH);
        if available < min_editor_width + EXPLORER_MIN_WIDTH {
            return None;
        }
        let width = cmp::min(EXPLORER_WIDTH, available.saturating_sub(min_editor_width));
        Some((width, EXPLORER_SEPARATOR_WIDTH))
    }

    fn editor_start_x(&self, cols: usize) -> usize {
        self.explorer_layout(cols)
            .map_or(0, |(width, separator)| width + separator)
    }

    fn editor_total_width(&self, cols: usize) -> usize {
        cols.saturating_sub(self.editor_start_x(cols))
    }

    fn preview_layout(&self, editor_cols: usize) -> Option<(usize, usize, usize)> {
        if !self.preview_mode {
            return None;
        }

        let gutter = self.gutter_width();
        let total_body = editor_cols.saturating_sub(gutter);
        if total_body < PREVIEW_MIN_TOTAL_WIDTH {
            return None;
        }

        let editor_width = total_body.saturating_sub(PREVIEW_SEPARATOR_WIDTH) / 2;
        let separator_x = gutter + editor_width;
        let preview_x = separator_x + PREVIEW_SEPARATOR_WIDTH;
        let preview_width = editor_cols.saturating_sub(preview_x);
        if editor_width == 0 || preview_width == 0 {
            return None;
        }
        Some((separator_x, preview_x, preview_width))
    }

    fn editor_body_width(&self, editor_cols: usize) -> usize {
        let gutter = self.gutter_width();
        if let Some((separator_x, _, _)) = self.preview_layout(editor_cols) {
            separator_x.saturating_sub(gutter)
        } else {
            editor_cols.saturating_sub(gutter)
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
            for ch in continuation.chars() {
                self.insert_char(ch);
            }
        }
    }

    fn markdown_continuation(&self) -> Option<String> {
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

    fn refresh_explorer_entries(&mut self) {
        let selected_entry = self.explorer_entries.get(self.explorer_selected).cloned();
        self.rebuild_explorer_entries(
            selected_entry.as_ref().map(|entry| entry.path.clone()),
            selected_entry.is_some_and(|entry| entry.is_parent_link),
        );
    }

    fn rebuild_explorer_entries(
        &mut self,
        selected_path: Option<PathBuf>,
        select_parent_link: bool,
    ) {
        self.explorer_expanded_dirs
            .retain(|path| path.starts_with(&self.explorer_root));

        let mut entries = Vec::new();
        if let Some(parent) = self.explorer_root.parent() {
            entries.push(ExplorerEntry {
                rendered_label: "..".to_string(),
                path: parent.to_path_buf(),
                is_dir: true,
                expanded: false,
                parent: None,
                is_parent_link: true,
            });
        }

        let children = Self::read_explorer_children(&self.explorer_root);
        let child_count = children.len();
        for (idx, (path, name, is_dir)) in children.into_iter().enumerate() {
            let is_last = idx + 1 == child_count;
            self.push_explorer_tree_entry(&mut entries, path, name, is_dir, "", is_last, None);
        }

        self.explorer_entries = entries;
        self.explorer_selected = if select_parent_link {
            self.explorer_entries
                .iter()
                .position(|entry| entry.is_parent_link)
                .unwrap_or(0)
        } else if let Some(path) = selected_path {
            self.explorer_entries
                .iter()
                .position(|entry| entry.path == path)
                .unwrap_or(0)
        } else {
            0
        };
        self.ensure_explorer_selection_visible(self.explorer_visible_rows());
    }

    fn read_explorer_children(dir: &Path) -> Vec<(PathBuf, String, bool)> {
        let mut dirs = Vec::new();
        let mut files = Vec::new();
        if let Ok(read_dir) = fs::read_dir(dir) {
            for item in read_dir.flatten() {
                let path = item.path();
                let name = item.file_name().to_string_lossy().to_string();
                if name.starts_with('.') {
                    continue;
                }
                let is_dir = item
                    .file_type()
                    .map_or_else(|_| path.is_dir(), |ft| ft.is_dir());
                if is_dir {
                    dirs.push((path, name, true));
                } else {
                    files.push((path, name, false));
                }
            }
        }
        dirs.sort_by_key(|(_, name, _)| name.to_ascii_lowercase());
        files.sort_by_key(|(_, name, _)| name.to_ascii_lowercase());
        dirs.extend(files);
        dirs
    }

    fn push_explorer_tree_entry(
        &self,
        out: &mut Vec<ExplorerEntry>,
        path: PathBuf,
        name: String,
        is_dir: bool,
        prefix: &str,
        is_last: bool,
        parent: Option<PathBuf>,
    ) {
        let expanded = is_dir && self.explorer_expanded_dirs.contains(&path);
        let (branch, marker) = if is_dir {
            (
                if is_last { "`- " } else { "|- " },
                if expanded { "[-] " } else { "[+] " },
            )
        } else {
            ("", "       ")
        };
        out.push(ExplorerEntry {
            rendered_label: format!("{prefix}{branch}{marker}{name}"),
            path: path.clone(),
            is_dir,
            expanded,
            parent: parent.clone(),
            is_parent_link: false,
        });

        if is_dir && expanded {
            let children = Self::read_explorer_children(&path);
            let child_count = children.len();
            let child_prefix = format!("{prefix}{}", if is_last { "   " } else { "|  " });
            for (idx, (child_path, child_name, child_is_dir)) in children.into_iter().enumerate() {
                let child_is_last = idx + 1 == child_count;
                self.push_explorer_tree_entry(
                    out,
                    child_path,
                    child_name,
                    child_is_dir,
                    &child_prefix,
                    child_is_last,
                    Some(path.clone()),
                );
            }
        }
    }

    fn ensure_explorer_selection_visible(&mut self, visible_rows: usize) {
        if visible_rows == 0 {
            self.explorer_offset = 0;
            return;
        }
        if self.explorer_selected < self.explorer_offset {
            self.explorer_offset = self.explorer_selected;
        }
        if self.explorer_selected >= self.explorer_offset + visible_rows {
            self.explorer_offset = self.explorer_selected + 1 - visible_rows;
        }
    }

    fn explorer_visible_rows(&self) -> usize {
        let (_, rows) = terminal::size().unwrap_or((80, 24));
        self.explorer_text_height_for_rows(usize::from(rows))
    }

    fn move_explorer_up(&mut self) {
        if self.explorer_selected > 0 {
            self.explorer_selected -= 1;
        }
        self.ensure_explorer_selection_visible(self.explorer_visible_rows());
    }

    fn move_explorer_down(&mut self) {
        if self.explorer_selected + 1 < self.explorer_entries.len() {
            self.explorer_selected += 1;
        }
        self.ensure_explorer_selection_visible(self.explorer_visible_rows());
    }

    fn page_explorer_up(&mut self) {
        let step = self.explorer_visible_rows().max(1);
        self.explorer_selected = self.explorer_selected.saturating_sub(step);
        self.ensure_explorer_selection_visible(self.explorer_visible_rows());
    }

    fn page_explorer_down(&mut self) {
        let step = self.explorer_visible_rows().max(1);
        let max_idx = self.explorer_entries.len().saturating_sub(1);
        self.explorer_selected = cmp::min(self.explorer_selected + step, max_idx);
        self.ensure_explorer_selection_visible(self.explorer_visible_rows());
    }

    fn open_explorer_parent(&mut self) {
        if let Some(parent) = self.explorer_root.parent() {
            self.explorer_root = parent.to_path_buf();
            self.rebuild_explorer_entries(None, true);
            self.status = StatusMessage::new(format!("Explorer: {}", self.explorer_root.display()));
        }
    }

    fn expand_explorer_selected(&mut self) {
        let Some(entry) = self.explorer_entries.get(self.explorer_selected).cloned() else {
            return;
        };
        if entry.is_dir && !entry.expanded && !entry.is_parent_link {
            self.explorer_expanded_dirs.insert(entry.path.clone());
            self.rebuild_explorer_entries(Some(entry.path), false);
        }
    }

    fn collapse_or_select_explorer_parent(&mut self) {
        let Some(entry) = self.explorer_entries.get(self.explorer_selected).cloned() else {
            return;
        };
        if entry.is_parent_link {
            self.open_explorer_parent();
            return;
        }
        if entry.is_dir && entry.expanded {
            self.explorer_expanded_dirs.remove(&entry.path);
            self.rebuild_explorer_entries(Some(entry.path), false);
            return;
        }
        if let Some(parent) = entry.parent {
            self.rebuild_explorer_entries(Some(parent), false);
        } else {
            self.rebuild_explorer_entries(None, true);
        }
    }

    fn open_explorer_selected(&mut self) {
        let Some(entry) = self.explorer_entries.get(self.explorer_selected).cloned() else {
            return;
        };
        if entry.is_parent_link {
            self.open_explorer_parent();
            return;
        }
        if entry.is_dir {
            if entry.expanded {
                self.explorer_expanded_dirs.remove(&entry.path);
            } else {
                self.explorer_expanded_dirs.insert(entry.path.clone());
            }
            self.rebuild_explorer_entries(Some(entry.path), false);
            return;
        }

        if self.doc.modified {
            self.status = StatusMessage::new(
                "Unsaved changes. Save before opening another file from explorer.",
            );
            return;
        }

        match Document::open(entry.path.clone()) {
            Ok(doc) => {
                self.doc = doc;
                self.cursor = Position::default();
                self.offset = Position::default();
                self.clear_selection();
                self.active_pane = ActivePane::Editor;
                self.quit_warning_countdown = 1;
                self.status = StatusMessage::new(format!("Opened {}", entry.path.display()));
                self.rebuild_explorer_entries(Some(entry.path), false);
            }
            Err(err) => {
                self.status = StatusMessage::new(format!("Open failed: {err}"));
            }
        }
    }

    fn handle_explorer_key(&mut self, key: KeyEvent) -> bool {
        match key {
            KeyEvent {
                code: KeyCode::Tab, ..
            }
            | KeyEvent {
                code: KeyCode::Esc, ..
            } => {
                self.active_pane = ActivePane::Editor;
                true
            }
            KeyEvent {
                code: KeyCode::Up, ..
            } => {
                self.move_explorer_up();
                true
            }
            KeyEvent {
                code: KeyCode::Down,
                ..
            } => {
                self.move_explorer_down();
                true
            }
            KeyEvent {
                code: KeyCode::PageUp,
                ..
            } => {
                self.page_explorer_up();
                true
            }
            KeyEvent {
                code: KeyCode::PageDown,
                ..
            } => {
                self.page_explorer_down();
                true
            }
            KeyEvent {
                code: KeyCode::Home,
                ..
            } => {
                self.explorer_selected = 0;
                self.ensure_explorer_selection_visible(self.explorer_visible_rows());
                true
            }
            KeyEvent {
                code: KeyCode::End, ..
            } => {
                self.explorer_selected = self.explorer_entries.len().saturating_sub(1);
                self.ensure_explorer_selection_visible(self.explorer_visible_rows());
                true
            }
            KeyEvent {
                code: KeyCode::Left,
                ..
            }
            | KeyEvent {
                code: KeyCode::Backspace,
                ..
            } => {
                self.collapse_or_select_explorer_parent();
                true
            }
            KeyEvent {
                code: KeyCode::Right,
                ..
            } => {
                self.expand_explorer_selected();
                true
            }
            KeyEvent {
                code: KeyCode::Enter,
                ..
            } => {
                self.open_explorer_selected();
                true
            }
            _ => {
                if key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                    || matches!(key.code, KeyCode::F(1) | KeyCode::F(2))
                {
                    false
                } else {
                    true
                }
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
        self.editor_text_height_for_rows(usize::from(rows))
    }

    fn gutter_width(&self) -> usize {
        let digits = self.doc.line_count().max(1).to_string().len();
        digits + 1
    }

    fn scroll(&mut self) {
        let (cols, rows) = terminal::size().unwrap_or((80, 24));
        let cols = usize::from(cols).saturating_sub(2);
        let text_height = self.editor_text_height_for_rows(usize::from(rows));
        let text_width = self.editor_body_width(self.editor_total_width(cols));

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

fn shift_only(modifiers: KeyModifiers) -> bool {
    modifiers.contains(KeyModifiers::SHIFT)
        && !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
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

fn run_shell_command_line(command_line: &str) -> io::Result<Output> {
    #[cfg(target_os = "windows")]
    {
        Command::new("cmd").arg("/C").arg(command_line).output()
    }

    #[cfg(not(target_os = "windows"))]
    {
        Command::new("sh").arg("-lc").arg(command_line).output()
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

fn ansi_to_line_clipped(text: &str, width: usize, theme: &Theme) -> Line<'static> {
    if width == 0 {
        return Line::raw(String::new());
    }

    let chars: Vec<char> = text.chars().collect();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut current_style = Style::default().fg(theme.fg).bg(theme.bg);
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
                        apply_sgr_sequence(&seq, &mut current_style, theme);
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

fn apply_sgr_sequence(seq: &str, style: &mut Style, theme: &Theme) {
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
            0 => *style = Style::default().fg(theme.fg).bg(theme.bg),
            1 => *style = style.add_modifier(Modifier::BOLD),
            2 => *style = style.add_modifier(Modifier::DIM),
            3 => *style = style.add_modifier(Modifier::ITALIC),
            4 => *style = style.add_modifier(Modifier::UNDERLINED),
            22 => *style = style.remove_modifier(Modifier::BOLD | Modifier::DIM),
            23 => *style = style.remove_modifier(Modifier::ITALIC),
            24 => *style = style.remove_modifier(Modifier::UNDERLINED),
            30..=37 => *style = style.fg(Color::Indexed((code - 30) as u8)),
            90..=97 => *style = style.fg(Color::Indexed((code - 90 + 8) as u8)),
            39 => *style = style.fg(theme.fg),
            40..=47 => *style = style.bg(Color::Indexed((code - 40) as u8)),
            100..=107 => *style = style.bg(Color::Indexed((code - 100 + 8) as u8)),
            49 => *style = style.bg(theme.bg),
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
