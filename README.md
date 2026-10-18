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

## Repository layout

```
crates/core      shared core: data model, SQLite storage, quick-add parser
apps/desktop     desktop app built with GPUI (the UI framework behind the Zed editor)
docs/parity.md   feature parity with TickTick and the roadmap
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

MIT, see [LICENSE](LICENSE). Icons are from [Lucide](https://lucide.dev) (ISC), see
`apps/desktop/assets/icons/LICENSE`.
