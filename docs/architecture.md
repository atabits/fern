# Architecture

Fern follows the structure of the [Zed](https://github.com/zed-industries/zed) codebase: a
Cargo workspace of small, focused crates on top of GPUI.

```
assets/            icons and other bundled files (one directory for every crate)
crates/
  core/            data model, SQLite storage and migrations, quick-add parser (no UI)
  icons/           IconName: one variant per file in assets/icons
  assets/          AssetSource that embeds assets/ (read from disk in debug builds)
  theme/           Theme, semantic ThemeColors and StatusColors, read with cx.theme()
  ui/              RenderOnce components: Icon, IconButton, Label, Checkbox, Chip, ListItem,
                   Divider, popovers; import ui::prelude::* in views
apps/
  desktop/         the desktop app: views (entities), i18n, the text input, main()
```

## Conventions

- Views are GPUI entities implementing `Render`; they talk through `EventEmitter` events and
  subscriptions instead of holding references to each other.
- Reusable elements live in `crates/ui` as `RenderOnce` structs with builder methods
  (`Label::new("…").size(LabelSize::Small).color(Color::Muted)`). Views do not hard-code colors;
  they use `Color` or `cx.theme().colors()`.
- Icons are referenced by `IconName`, never by path. A new icon is a new SVG in `assets/icons`
  (snake_case file name) plus a variant; a test checks that every variant has a file.
- User-visible strings come from `apps/desktop/src/i18n.rs`.
- The core crate has no UI dependencies, so it can be shared with mobile clients.

Zed's own crates are GPL-3.0; Fern is AGPL-3.0, so adapting code from them is allowed as long as
the original copyright notices are kept and the source is credited in the README.
