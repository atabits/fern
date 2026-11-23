# AGENTS.md — Development Guide for AI Agents

This file defines the architecture, build workflow, and coding rules for working on **Fern** (a native Rust + GPUI desktop task manager and sync server). Follow these rules to keep builds fast and CI green across Windows, macOS, and Linux.

---

## 1. Workspace Architecture & Crate Boundaries

Fern uses a layered Cargo workspace so crates compile in parallel and edits to one screen do not recompile the entire app:

- **Core & Design System**
  - `crates/core` (`fern_core`): domain models, SQLite storage, recurrence, quick-add parser, sync protocol, summary/stats logic. No GPUI dependency.
  - `crates/icons`, `crates/assets`, `crates/theme`, `crates/ui`: design tokens, bundled fonts/icons, reusable GPUI primitives.
  - `crates/i18n` (`fern_i18n`): static translation tables (`EN`, `RU`, `ZH_CN`) and `CurrentLocale` global. Every new UI string must be added to all locales in `Strings`.
- **Shared Desktop Layers**
  - `crates/desktop_common`: app paths (`paths`), global actions (`actions`), `keymap`, date/time formatting (`calendar`), `text_input`, `touch`/`touch_keyboard`, `markdown_toolbar`, `export`, `sync`, `subscriptions`, `undo`, `DataVersion`, `StickyOpener`, layout constants, and small cross-view presenters (`colors`, `habit_helpers`, `countdown_helpers`).
  - `crates/task_ui`: reusable task components shared across views (`drag`, `task_rows`, `task_menu`, `due_picker`, `lists_panel`, `list_dialog`, `tag_dialog`, `print`, `task_detail`).
- **Parallel Feature Crates** *(independent siblings — compile in parallel)*
  - `crates/countdown_ui`
  - `crates/habit_ui`
  - `crates/matrix_ui`
  - `crates/focus_ui`
  - `crates/sticky_ui`
  - `crates/calendar_ui`
  - `crates/horizons_ui`
  - `crates/summary_ui`
  - `crates/todo_ui` *(depends on `summary_ui`, `horizons_ui`)*
- **Thin Binaries**
  - `apps/desktop` (`fern_desktop`): thin window shell (`main.rs`, `app_view.rs`, `sidebar.rs`, `title_bar.rs`, `settings_window.rs`, `search_modal.rs`, `quick_add_window.rs`, `reminders.rs`, `hotkeys.rs`, `tray.rs`, `updater.rs`, `user_themes.rs`, `release_notes.rs`).
  - `apps/server` (`fern_server`): optional self-hosted HTTP sync server.

### Strict Architectural Rules
1. **Keep `apps/desktop` thin.** Put feature UI code in its corresponding `crates/<feature>_ui` crate (or create a new crate if adding a major section). Never dump large new views into `apps/desktop`.
2. **No dependencies between sibling `*_ui` crates** (except `todo_ui -> summary_ui, horizons_ui`).
   - Wire cross-view actions via GPUI `EventEmitter` + `cx.subscribe` / `cx.subscribe_in` in `apps/desktop/src/app_view.rs`.
   - Use shared globals/callbacks in `desktop_common` (such as `DataVersion` for cross-window refresh and `StickyOpener` for opening sticky notes) rather than importing another feature crate.
3. **New crate conventions:**
   - Use `[lib] path = "src/<crate_name>.rs"` (no `mod.rs` files).
   - Inherit `edition.workspace = true`, `version.workspace = true`, `license.workspace = true`, and `[lints] workspace = true`.
   - Register the crate in root `Cargo.toml` under both `[workspace] members` and `[workspace.dependencies]`.
   - Export only the crate's own public API (`pub`). Use `pub(crate) use` internally if needed, and ensure zero `unused_imports` warnings.

---

## 2. Dependency & Build Profile Hygiene

1. **Workspace dependencies only:** Declare every external and internal dependency in root `Cargo.toml` `[workspace.dependencies]` and reference it with `<dep>.workspace = true` in member crates. This prevents feature/version duplication.
2. **Keep `gpui/test-support` in `[dev-dependencies]` only:** Never enable `test-support` in normal `[dependencies]` — it pulls `proptest` and test harnesses into release builds.
   - Any crate with `#[gpui::test]` tests that load keymaps should declare `assets.workspace = true` and `gpui = { workspace = true, features = ["test-support"] }` under `[dev-dependencies]`.
3. **Preserve dev profile tuning in root `Cargo.toml`:**
   - Do **not** re-add global `opt-level = 2` under `[profile.dev.package."*"]`.
   - Keep `debug = 0` on `[profile.dev.package."*"]` and mirror `[profile.dev.build-override]` (`debug = 0`, `codegen-units = 16`, `split-debuginfo = "unpacked"`) so shared target/host crates are not compiled twice and Windows PDBs stay small.
   - Add `opt-level = 3` only for specific proc-macros or hot rendering/layout crates (`taffy`, `resvg`, `usvg`, `tiny-skia`, `cosmic-text`, `serde_json`).
4. **Windows linker policy:** Keep standard MSVC `link.exe` as the default in the repo's `.cargo/config.toml` (do not hardcode `rust-lld.exe` or `lld-link.exe` in the repo config). Developers may opt into `lld-link.exe` locally via `~/.cargo/config.toml`.

---

## 3. Fast Local Development Workflow

Use the narrowest Cargo command for your current iteration:

```bash
# Fast type-check of a single crate while editing
cargo check -p <crate>

# Type-check the desktop app (alias defined in .cargo/config.toml)
cargo c

# Run unit tests for ONLY the crate you modified (seconds instead of full workspace)
cargo test -p habit_ui
cargo test -p task_ui
cargo test -p fern_core

# Fast local optimized run (no LTO, parallel codegen)
cargo run-fast

# Full debug symbols when attaching a debugger
cargo build --profile dbg -p fern_desktop
```

- **Protect user data:** When launching debug builds manually, set `FERN_DATA_DIR` to a temporary directory so the user's real `tasks.db` is untouched.
- **Disk cleanup:** Avoid bare `cargo clean` if `target/` contains release binaries or QA artifacts; remove only `target/debug` when reclaiming debug cache space.

---

## 4. Coding & GPUI Conventions

- **Workspace Clippy denials (`Cargo.toml`):** `dbg_macro`, `declare_interior_mutable_const`, `let_underscore_future`, `redundant_clone`, and `todo` are all `deny`.
- **Error handling:** Propagate fallible operations with `?` or log/surface errors cleanly; avoid `.unwrap()` in production paths.
- **GPUI patterns:**
  - Inside `entity.update(cx, |this, cx| ...)` closures, always use the inner `cx` rather than the outer `cx`.
  - Keep blocking I/O, HTTP (`ureq`), and heavy exports off the foreground UI thread via `cx.background_spawn(...)`.
  - Always `.detach()`, `.detach_and_log_err(cx)`, `.await`, or store `gpui::Task` values (dropping a `Task` cancels it).
  - After mutating SQLite data that other windows (e.g., sticky notes) also display, call `desktop_common::data_version::data_changed(cx.entity_id(), cx)`.
- **Cross-platform builds:** Gate Win32/WinRT code (`windows`, `raw-window-handle`, `tray`) behind `#[cfg(windows)]` so Linux and macOS CI never break.

---

## 5. Required Verification Before Finishing

After completing changes (and updating `Cargo.lock` via `cargo check --workspace` if workspace crates/deps changed), run the exact CI checks with `--locked`:

```bash
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo check --workspace --bins --locked
cargo test --workspace --locked
```

For release-impacting changes, also verify:

```bash
cargo build --profile release-fast -p fern_desktop -p fern_server --locked
```
