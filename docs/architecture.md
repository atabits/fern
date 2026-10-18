# Architecture

Fern follows the structure of the [Zed](https://github.com/zed-industries/zed) codebase: a
Cargo workspace of small, focused crates on top of GPUI.

```
assets/            icons, keymaps and other bundled files (one directory for every crate)
crates/
  core/            data model, SQLite storage and migrations, quick-add parser (no UI)
  icons/           IconName: one variant per file in assets/icons
  assets/          AssetSource that embeds assets/ (read from disk in debug builds)
  theme/           Theme, semantic ThemeColors and StatusColors, read with cx.theme()
  ui/              RenderOnce components: Icon, IconButton, Label, Checkbox, Chip, ListItem,
                   Divider, popovers; import ui::prelude::* in views
apps/
  desktop/         the desktop app: views (entities), i18n, the text input, main()
  server/          fern-server, the self-hosted sync server
```

## Sync

Devices sync through a self-hosted server, row by row (`crates/core/src/sync.rs`):

- Triggers log every write to synced data in `sync_log` (table, row key, time), so every write
  path counts. Rows that existed before the log was added count as changed at time 0.
  The log keeps only the latest entry of each row (`compact_sync_log`, after every sync and on
  start).
- A device sends `changes_since(position)`: each changed row once, as it is now, or as deleted.
- A device applies what it receives with `apply_changes`: the later change of a row wins, rows
  may arrive in any order (foreign keys are checked at commit), and applied changes are not
  logged again, so they do not bounce back. Only the synced tables and plain column names are
  accepted from outside.
- The server (`apps/server`, the `fern-server` binary) keeps the latest change of every row with
  its own sequence number in SQLite. One `POST /sync` with a bearer token (`FERN_TOKEN`) takes a
  device's changes and returns the other devices' changes after the number the device last saw.
  It speaks plain HTTP; a reverse proxy adds TLS.

Attachments' records sync like other rows; their files go through the server's file store
(`PUT`/`GET /files/<name>`, kept in `<data>.files`): after a sync the app sends the files the
server lacks and fetches the ones missing here.

Not synced: settings, task activity, habit snoozes.

## Conventions

- Views are GPUI entities implementing `Render`; they talk through `EventEmitter` events and
  subscriptions instead of holding references to each other.
- Reusable elements live in `crates/ui` as `RenderOnce` structs with builder methods
  (`Label::new("…").size(LabelSize::Small).color(Color::Muted)`). Views do not hard-code colors;
  they use `Color` or `cx.theme().colors()`.
- Icons are referenced by `IconName`, never by path. A new icon is a new SVG in `assets/icons`
  (snake_case file name) plus a variant; a test checks that every variant has a file.
- User-visible strings come from `apps/desktop/src/i18n.rs`.
- Actions are declared with `actions!` (`apps/desktop/src/actions.rs`) and handled with
  `on_action` inside a key context (`Workspace`, `TaskList`, `TextInput`). Key bindings are data in
  `assets/keymaps/default.json`, in Zed's keymap format; a test loads it and fails on unknown
  actions.
- The core crate has no UI dependencies, so it can be shared with mobile clients.

Zed's own crates are GPL-3.0; Fern is AGPL-3.0, so adapting code from them is allowed as long as
the original copyright notices are kept and the source is credited in the README.
