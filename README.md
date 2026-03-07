# redit

`redit` is a terminal markdown editor built with Rust + Ratatui.
It provides a multi-pane TUI with a file explorer, markdown-aware editor, optional split markdown preview, integrated terminal pane, menu bar, and popup-driven workflows.

!(media/screenshot.png)

## Current Features

- Top menus: `File`, `Edit`, `Search`, `View`, `Tools`, `Help`
- File explorer pane (left):
  - Tree view for directories/files
  - Expand/collapse directories
  - Double-click support for open/toggle
  - Reliable mouse selection across the full explorer pane while terminal pane is visible
- Editor pane (center/right):
  - Line numbers
  - Markdown syntax highlighting (`.md`, `.markdown`, etc.)
  - Code/config syntax highlighting in edit pane for many file types via Syntect (for example Python, JavaScript, TypeScript, Go, Rust, TOML, YAML, `.env`)
  - Markdown list auto-continuation on `Enter`
  - Mouse text selection (drag)
- Markdown preview:
  - `View | Markdown` toggle
  - Split preview mode with Glow rendering when available
  - Fallback renderer if Glow is not installed
- Terminal pane:
  - `View | Terminal` toggle
  - Persistent bottom pane under editor
  - Interactive command input/output
- Search and replace popups:
  - `Search | Find`
  - `Search | Replace` supports replace-next (one by one) with remaining count
- File operations:
  - `File | Save`
  - `File | Save As...` popup with editable path field
- Theme palette:
  - `Tools | Palette`
  - 5 themes (including `Black & White`)
  - Keyboard and mouse selection in palette popup
  - Palette is persisted across restarts
- Help popups:
  - `Help | Keybindings`
  - `Help | About redit`

## Prerequisites

### 1) Rust toolchain (required)

Install Rust from **https://rustup.rs** (recommended and required for build/deploy).

Then verify:

```bash
rustc --version
cargo --version
```

### 2) System build tools

- Linux: install compiler/linker toolchain (for example `build-essential` on Debian/Ubuntu)
- macOS: install Xcode Command Line Tools (`xcode-select --install`)
- Windows: install Visual Studio Build Tools (C++ workload)

### 3) Glow (strongly recommended)

Glow powers high-quality markdown preview output. Without Glow, `redit` falls back to a simpler preview renderer.

Verify installation:

```bash
glow --version
```

Common install options:

- macOS (Homebrew):

```bash
brew install glow
```

- Windows (Scoop):

```powershell
scoop install glow
```

- Windows (Chocolatey):

```powershell
choco install glow
```

- Linux:
  - Use your distro package manager if available, or
  - Download a release package from Glow's GitHub releases

## Deployment / Build Instructions

### 1) Clone and enter project

```bash
git clone <your-repo-url> redit
cd redit
```

### 2) Build debug binary

```bash
cargo build
```

Binary location:

- Linux/macOS: `target/debug/redit`
- Windows: `target\debug\redit.exe`

### 3) Run directly

```bash
cargo run -- path/to/file.md
```

If no file is provided, `redit` starts with an empty buffer.

### 4) Build release binary (recommended for deployment)

```bash
cargo build --release
```

Release binary:

- Linux/macOS: `target/release/redit`
- Windows: `target\release\redit.exe`

### 5) Optional local install (Linux/macOS)

```bash
install -Dm755 target/release/redit ~/.local/bin/redit
```

Make sure `~/.local/bin` is in your `PATH`.

### 6) Cross-platform notes

- The produced binary targets the platform you build on by default.
- To build for another target, add Rust target(s) first:

```bash
rustup target add x86_64-pc-windows-gnu
rustup target add x86_64-unknown-linux-gnu
```

Then build with `--target`:

```bash
cargo build --release --target x86_64-pc-windows-gnu
```

## Runtime Data

Palette selection is stored in:

- `$XDG_CONFIG_HOME/redit/settings.conf` (if `XDG_CONFIG_HOME` is set), or
- `~/.config/redit/settings.conf`

## Keybindings

- `F1`: Help keybindings popup
- `F2`: focus/toggle explorer pane
- `F3`: toggle terminal pane
- `Ctrl-S`: save
- `Ctrl-Shift-S`: save as
- `Ctrl-Q`: quit (double press if unsaved changes)
- `Ctrl-P`: toggle split markdown preview
- `Ctrl-F`: find popup
- `Ctrl-R`: replace popup
- `Ctrl-Z` / `Ctrl-Y`: undo / redo
- `Ctrl-X` / `Ctrl-C` / `Ctrl-V`: cut / copy / paste
- `Alt-F`, `Alt-E`, `Alt-S`, `Alt-V`, `Alt-T`, `Alt-H`: open menus

Menu navigation:

- `Left/Right`: switch menus
- `Up/Down`: move menu selection
- `Enter`: activate menu action
- `Esc`: close menu/popup

## Mouse Support

- Click top menu labels to open dropdowns
- Click dropdown items to execute actions
- File explorer double-click opens files and expands/collapses directories
- Drag in editor to select text
- Palette popup supports mouse click selection and apply

## Quick Start

```bash
cargo run -- notes.md
```

Then try:

1. `View | Markdown`
2. `View | Terminal`
3. `Tools | Palette`
4. `Search | Replace`
