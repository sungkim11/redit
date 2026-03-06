# redit

`redit` is a cross-platform terminal markup editor prototype written in Rust.
The UI is intentionally classic terminal-editor style: line gutter, editable
buffer, top menu line, status line, and message line.
Default colors use a retro green terminal palette.

## Run

```bash
cargo run -- path/to/file.md
```

If no file is provided, it starts with an empty buffer.

## Markdown Features

- Syntax highlighting for headings, lists, blockquotes, fenced/inline code, links, and inline HTML tags
- Auto-continue Markdown lists when pressing `Enter` (unordered and ordered)
- Status bar shows Markdown document stats (lines + words)

## Keybindings

- `Ctrl-S`: save (defaults to `redit.md` when no file path is set)
- `Ctrl-Q`: quit (`Ctrl-Q` twice if there are unsaved changes)
- `Alt-F`, `Alt-E`, `Alt-S`, `Alt-H`: open top menus (`File/Edit/Search/Help`)
- Menu mode: `Left/Right` switch menus, `Up/Down` move item, `Enter` activate, `Esc` close
- Edit/Search/Help shortcuts: `Ctrl-Z`, `Ctrl-Y`, `Ctrl-X`, `Ctrl-C`, `Ctrl-V`, `Ctrl-F`, `Ctrl-R`, `F1`
- Left mouse click on top menu (`File/Edit/Search/Help`): open a dropdown
- Left mouse click on a dropdown item: run that menu action
- `Arrow keys`: move cursor
- `PageUp` / `PageDown`: move by viewport
- `Home` / `End`: line start/end
- `Enter`: newline
- `Backspace` / `Delete`: remove text
- `Tab`: insert 4 spaces

## Notes

- Requires a standard Rust-supported linker toolchain on Linux (`build-essential` on Debian/Ubuntu).
