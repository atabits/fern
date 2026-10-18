# Fern

An open-source task manager inspired by TickTick: a fast native desktop app written in Rust,
with mobile clients and sync planned.

> Status: early development. The desktop app works locally; there is no sync yet.

## Features

- Inbox, lists, and smart views: All, Today, Tomorrow, Next 7 Days, Completed
- Tasks and notes with subtasks, due dates, priorities and colored tags
- Quick add that understands dates, times, tags, priority and lists:
  `Buy milk tomorrow at 6pm #home !1 ^errands`
- A task details panel with a calendar, priority and tag pickers
- English and Russian UI, switchable at runtime
- Local SQLite storage

## Keyboard shortcuts

`Ctrl` on Linux and Windows, `Cmd` on macOS.

| Keys | Action |
|---|---|
| `Ctrl+N` | New task |
| `Ctrl+F` | Search |
| `Ctrl+\` | Show or hide the lists column |
| `Ctrl+1` … `Ctrl+4` | Inbox, Today, Tomorrow, Next 7 Days |
| `↑`/`↓` or `k`/`j` | Move the selection in the task list |
| `Space` | Complete or reopen the selected task |
| `Delete` | Move the selected task to the trash |
| `Esc` in the quick-add field | Move focus to the task list |

Bindings live in `assets/keymaps/default.json`.

## Repository layout

```
crates/core      shared core: data model, SQLite storage, quick-add parser
crates/ui        UI components (Zed-style), with crates/theme, crates/icons, crates/assets
apps/desktop     desktop app built with GPUI (the UI framework behind the Zed editor)
docs/            architecture notes and feature parity with TickTick
```

The core is meant to be shared by every client: the desktop app uses it directly, and the
Android app will use it through UniFFI.

## Building

Requirements: Rust stable (see `mise.toml`) and, on Linux, the libraries GPUI needs
(Wayland or X11, xkbcommon, Vulkan, fontconfig).

```sh
cargo run -p fern_desktop        # run the desktop app
cargo test --workspace           # run the tests
```

GPUI is taken straight from the [Zed repository](https://github.com/zed-industries/zed); the
first build compiles it and takes a while.

Data is stored in `$XDG_DATA_HOME/fern-tasks/tasks.db` (usually
`~/.local/share/fern-tasks/tasks.db`).

## Contributing

Code, comments, docs and commit messages are in English. UI strings live in
`apps/desktop/src/i18n.rs`; adding a language means filling the `Strings` struct for it.

## License

Fern is free software under the GNU Affero General Public License v3.0 or later
([LICENSE](LICENSE)). If you distribute a modified version, or run a modified version as a
network service (for example, a sync server), you must make its source code available under
the same license.

Third-party components:

- [GPUI](https://github.com/zed-industries/zed) — Apache-2.0. `apps/desktop/src/text_input.rs`
  started from GPUI's `examples/input.rs`.
- Icons from [Lucide](https://lucide.dev) — ISC, see `assets/icons/LICENSE`.
