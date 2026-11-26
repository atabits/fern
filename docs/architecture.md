# Architecture

Fern is organized as a layered Cargo workspace of focused crates on top of GPUI so feature screens compile in parallel, shared layers remain reusable across desktop and `wasm32`, and core logic stays completely independent of UI frameworks.

```text
assets/                  Bundled icons, fonts, and default keymaps
crates/
  core/                  Data model, SQLite storage & migrations, Quick Add, recurrence,
                         Horizons, Things queries, E2EE sync protocol (no GPUI dependency)
  icons/                 IconName: one enum variant per SVG in assets/icons
  assets/                AssetSource embedding assets/ (read from disk in debug builds)
  theme/                 Theme, semantic ThemeColors, StatusColors, and JSONC ThemeRegistry
  ui/                    Reusable RenderOnce GPUI primitives (Button, Icon, Label, Checkbox,
                         Chip, ListItem, Modal, Popover, Scrollbar); import via ui::prelude::*
  i18n/                  Static translation tables (EN, RU, ZH_CN) and CurrentLocale global
  desktop_common/        Shared actions, keymap, text_input, calendar helpers, audio synthesis,
                         export, undo, portable paths, DataVersion, StickyOpener, layout tokens
  desktop_sync/          Background sync runner, two-way CalDAV client, and iCal subscriptions
  task_ui/               Shared task UI (rows, menus, due picker, drag-and-drop, lists panel,
                         list/tag dialogs, search modal, print, task details)
  plugins/               Sandboxed QuickJS-NG (ES2022) runtime, manifest validation, permissions,
                         HTTP egress guard, and virtual "fern" / "fern:ui" modules
  plugin_ui/             Native GPUI renderer for declarative UiNode trees & Settings -> Plugins
  calendar_ui/           Calendar views (Day, Week, Multi-Day, Month, Year, Agenda)
  countdown_ui/          Countdowns, anniversaries, and holidays
  focus_ui/              Pomodoro & Stopwatch timers, Focus Mini, and Focus/Task statistics
  habit_ui/              Habit tracking, check-in modes, streaks, and logs
  horizons_ui/           Panoramic Horizons planner (Inbox, Today, Week..Life, rollover)
  life_clock_ui/         Split-screen kinetic Life Clock timeline
  matrix_ui/             Eisenhower Matrix with custom profiles and rule editor
  sticky_ui/             Floating desktop sticky notes
  summary_ui/            Auto-generated Markdown Summary reports & templates
  things_ui/             Optional Things-style workspace (Beta)
  todo_ui/               Task list, Kanban boards, and Timeline views
apps/
  desktop/               Thin native desktop shell (main, window frame, sidebar, settings window,
                         quick add window, reminders, global hotkeys, tray, updater, user themes)
  server/                fern-server — self-hosted HTTP sync server (Protocol v2)
```

## Crate Boundaries

- Keep `apps/desktop` thin: feature UI belongs in its corresponding `crates/<feature>_ui` crate.
- Sibling `*_ui` crates do not depend on each other (except `todo_ui -> summary_ui, horizons_ui`).
- Cross-view actions flow through GPUI `EventEmitter` + `cx.subscribe` / `cx.subscribe_in` in `apps/desktop/src/app_view.rs`, and cross-window refresh uses `desktop_common::data_version::data_changed`.
- `crates/core` has no GPUI dependency, enabling reuse across `apps/server`, WebAssembly targets, and future mobile clients (via UniFFI).

## Sync, Fern Cloud & End-to-End Encryption

Devices can stay 100% local, sync through **Fern Cloud**, or sync through a self-hosted `fern-server` row by row (`crates/core/src/sync.rs`, `crates/core/src/cloud.rs`, and `crates/desktop_sync/`):

- SQLite triggers record every write to synced tables in `sync_log` (`table`, `row key`, `timestamp`), so all write paths are captured consistently. `compact_sync_log` retains only the latest entry per row.
- Each device sends `changes_since(position)` and applies remote changes with `apply_changes` using last-write-wins watermarks, deferred foreign-key validation at commit, and echo suppression so applied changes are not re-logged.
- **Fern Cloud Zero-Knowledge Vault (`crates/core/src/cloud.rs`):** Accounts use a split-key design. Clients run **Argon2id** (`m=19456 KiB, t=2, p=1`, unique 16-byte salt) and split the master key via **HKDF-SHA256** into an `auth_key` (sent over TLS and hashed again server-side with Argon2id) and a client-only `kek` that unwraps a random 256-bit `VaultKey` (ChaCha20-Poly1305). A 256-bit Crockford Base32 **Recovery Key** independently wraps the same `VaultKey`, enabling password changes and recovery without re-encrypting rows. On Desktop, sessions persist in an OS-protected `cloud-session.vault` whose master key is stored in the OS keychain via GPUI (Windows Credential Manager, macOS Keychain, Linux Secret Service, with DPAPI/`0600` local fallback); HTTP calls share pooled keep-alive agents behind a single-flight session coordinator, and real-time cross-device push uses single-use-ticket Server-Sent Events (`/api/v1/events`).
- **Self-Hosted Protocol v2 & E2EE (`apps/server`):** When an encryption passphrase is configured for `fern-server`, clients derive a 256-bit key via PBKDF2-HMAC-SHA256 and encrypt row payloads and attachment blobs locally with **ChaCha20-Poly1305** and domain-separated AAD before upload (see [self-hosting.md](self-hosting.md)).
- `fern-server` (`apps/server`) persists the latest change of each row with a monotonic server sequence number in SQLite (`POST /sync`), stores attachment and countdown-cover blobs in `<data>.files/` (`PUT`/`GET`/`DELETE /files/<key>`), exposes `GET /health` and `POST /gc`, and speaks plain HTTP behind a TLS reverse proxy.
- Device-local preferences (such as active views, theme mode, and habit snoozes) are intentionally not synced so each device can keep its own layout.

## Calendar Subscriptions & CalDAV

- Original iCal feeds are preserved in SQLite. Parsed events retain their source date/time representation and time zone; recurrences (`RRULE` with `UNTIL`, `COUNT`, `EXDATE`) expand in the source calendar before converting occurrences to the display time zone.
- All-day dates and floating times never pass through UTC, and unknown time zones surface explicit subscription errors rather than silently falling back to local time.
- Two-way CalDAV calendars (`crates/desktop_sync/`) support creating, editing, and deleting events with ETag (`412 Precondition Failed`) conflict recovery and `TZID` preservation.

## Plugins & Sandboxing

- Plugins live in `<FERN_DATA_DIR>/plugins/<plugin_id>/` and execute inside isolated `rquickjs` (QuickJS-NG, ES2022) runtimes on a dedicated worker thread (`crates/plugins/`).
- Capabilities are deny-by-default and verified against a manifest SHA-256 hash; outbound HTTP requests enforce host allowlists and SSRF/egress rules.
- Plugins never render HTML or access a DOM. Views return declarative `UiNode` trees mapped 1-to-1 to native GPUI elements in `crates/plugin_ui/`. See [plugins/README.md](plugins/README.md).

## Conventions

- **GPUI Views:** Views are GPUI entities implementing `Render`, communicating via `EventEmitter` events and subscriptions instead of direct sibling dependencies.
- **UI Primitives & Tokens:** Reusable elements live in `crates/ui` as `RenderOnce` builders. Views read colors strictly from `Color` or `cx.theme().colors()` — never hardcoded RGB literals.
- **Icons:** Referenced via `IconName` (`crates/icons`), backed by SVGs in `assets/icons/`. Unit tests assert every variant has a matching asset.
- **Internationalization:** All user-visible strings live in `crates/i18n/src/` across `EN`, `RU`, and `ZH_CN`.
- **Text Input:** Editable fields share `desktop_common::text_input::TextInput`, which clips text, selection, and carets to the viewport with consistent mouse/IME hit-testing and bounded flex widths (`flex_1().min_w_0()`).
- **Actions & Keymaps:** Actions are declared in `desktop_common::actions`, bound in `assets/keymaps/default.json` (with user overrides in `keymap.json`), and validated by tests so unknown actions fail CI.
- **Performance & Threading:** Blocking I/O, network calls, backups, audio synthesis, and printing run off the foreground UI thread (`cx.background_spawn`). Hidden views defer reloads via lazy `DataVersion` checks to keep edits and startup snappy.
