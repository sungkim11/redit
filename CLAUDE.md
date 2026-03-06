# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Allowed Tools

git, gh, printf, cargo, curl

## Build and Run

```bash
cargo build                    # build
cargo run -- path/to/file.md   # run with a file
cargo run                      # run with empty buffer
```

No tests or linter are configured. Rust edition 2024. Core UI deps: `crossterm 0.29` and `ratatui 0.30`.

## Architecture

This is a single-file terminal markdown editor (`src/main.rs`, ~1900 lines). It uses crossterm for terminal mode/event I/O and Ratatui for frame rendering. The entire application lives in one file with no modules.

### Core Types

- **`Editor`** — Main application struct. Owns the document, cursor, scroll offset, terminal guard, menu state, preview state, undo/redo history, clipboard, and find/replace state. The `run()` method is the event loop; `refresh_screen()` renders each frame.
- **`Document`** — Holds `Vec<String>` lines and file path. Mutation methods (`insert_char`, `insert_newline`, `delete_char`, `split_off`, etc.) operate on lines by position.
- **`TerminalGuard`** — RAII wrapper for raw mode + alternate screen. Cleans up on drop.

### Rendering Pipeline

`refresh_screen()` builds editor/menu/status/message view models, then draws a full Ratatui frame: top menu bar, visible text area (with optional split preview), dropdown menu overlay, status bar, and message line.

Markdown syntax highlighting is character-level: `markdown_styles_for_line()` produces a `Vec<MdStyle>` per line, then segments of identical style are converted into Ratatui `Span`s via `md_style_to_style()`.

### Preview System

Toggle with Ctrl-P. `preview_layout()` splits the terminal into editor | separator | preview panes. Preview content is cached (`preview_cache_lines`, `preview_cache_revision`) and invalidated when the document revision (hash of all lines) changes.

Two backends: **Glow** (shells out to `glow -s dark -w <width> -` via `run_command_with_stdin`) and **Fallback** (plain text clipping). `find_glow_command()` checks PATH and `~/.local/bin/glow`.

### Menu System

Four dropdown menus (File/Edit/Search/Help) defined as `const` slices of `MenuEntry`. `MenuAction` enum maps entries to editor operations. Mouse clicks on the menu bar or Alt-key shortcuts open dropdowns; arrow keys navigate within.

### Color Palette

All colors are `const` values prefixed with `CRT_` — retro green-on-black terminal aesthetic. Heading, link, code, and HTML tag colors are distinct constants.

### Key Helper Functions (standalone)

- `markdown_styles_for_line()` / `apply_link_styles()` / `apply_inline_code_styles()` / `apply_html_tag_styles()` — character-level style computation
- `md_style_to_style()` — maps markdown style tokens to Ratatui `Style`
- `markdown_list_continuation()` — auto-continue lists on Enter
- `html_heading_to_markdown()` — converts `<h1>`–`<h6>` HTML tags to `#` headings for preview normalization
- `clip_to_char_width()` — truncate string to N characters
