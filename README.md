<div align="center">

<img src="assets/icon/icon-256.png" width="112" alt="Fern icon">

# Fern

[![License: AGPL v3](https://img.shields.io/badge/License-AGPL_v3-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-stable-orange.svg)](https://www.rust-lang.org/)
[![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux%20%7C%20Windows-lightgrey.svg)](https://github.com/atabits/fern/releases/latest)
[![CI](https://github.com/atabits/fern/actions/workflows/ci.yml/badge.svg)](https://github.com/atabits/fern/actions/workflows/ci.yml)

**Tasks, calendar, habits and focus in one native app.**
A free, open-source take on TickTick, written in Rust. Your data stays on your machine.

</div>

![Fern: Next 7 Days with a task's details](docs/screenshots/tasks.png)

---

## ✨ Why You'll Love It

- **⚡️ Native and fast.** Rust and [GPUI](https://github.com/zed-industries/zed), the GPU-rendered UI framework behind the Zed editor. No Electron and no web view.
- **🗂️ Everything in one place.** Lists, tags, smart views, kanban boards, a calendar, an Eisenhower matrix, habits, a Pomodoro timer, countdowns and statistics.
- **⌨️ Quick add that understands you.** `Lunch with Sam friday 1pm #personal !2 ^Work` sets the date, time, tag, priority and list as you type.
- **🔒 Local-first.** One SQLite file on your disk. No account and no cloud. If you want sync, run your own `fern-server`.
- **🌍 English and Russian.** Switch the language at runtime.
- **🆓 Free forever.** AGPL-3.0, no subscriptions, no ads.

---

## 📸 A Closer Look

| | |
|---|---|
| ![Week calendar](docs/screenshots/calendar.png) **Calendar**: day, week, month, year and agenda views. | ![Kanban board](docs/screenshots/kanban.png) **Kanban**: sections as columns. |
| ![Habits](docs/screenshots/habits.png) **Habits**: check-ins, streaks and monthly stats. | ![Focus statistics](docs/screenshots/statistics.png) **Statistics**: focus trends, timeline and records. |
| ![Eisenhower matrix](docs/screenshots/matrix.png) **Eisenhower Matrix**: sorts tasks by urgency and importance. | ![Countdowns](docs/screenshots/countdown.png) **Countdowns**: trips, birthdays, anniversaries and holidays. |

---

## 🚀 Quick Start

### Download

Every [release](https://github.com/atabits/fern/releases/latest) includes these builds:

| Platform | File |
|---|---|
| macOS (Apple Silicon / Intel) | `fern-<version>-macos-arm64.dmg` / `-macos-x86_64.dmg` |
| Windows (x86_64) | `fern-<version>-windows-x86_64.zip` |
| Linux (x86_64 / ARM64) | `fern-<version>-linux-<arch>.AppImage` or `.tar.gz` |
| Sync server (every platform) | `fern-server-<version>-<os>-<arch>.tar.gz` / `.zip` |

The builds are not signed yet:

- **macOS:** the first time, right-click `Fern.app` and choose Open. You can also run
  `xattr -d com.apple.quarantine /Applications/Fern.app`.
- **Windows:** in SmartScreen, choose "More info" → "Run anyway".
- **Linux:** `chmod +x fern-*.AppImage`, then run it.

### Build from Source

```bash
git clone https://github.com/atabits/fern.git
cd fern
cargo run --release -p fern_desktop
```

You need stable Rust plus each platform's build tools:

- **Linux:** the libraries GPUI uses (Wayland or X11, xkbcommon, Vulkan, fontconfig). On Debian and Ubuntu, `script/linux-deps` installs them.
- **macOS:** Xcode, because GPUI compiles its Metal shaders with it. The app needs macOS 11 or later.
- **Windows:** Visual Studio Build Tools with the "Desktop development with C++" workload.

The first build compiles GPUI straight from the Zed repository, so it takes a while.

---

## 📖 How to Use

**Quick add** understands:
- dates: `today`, `tomorrow`, `friday`
- times: `6pm`, `18:30`
- tags: `#tag`
- priority: `!1` (high) to `!3` (low)
- list: `^List`

**Keyboard shortcuts** use `Ctrl` on Linux and Windows and `Cmd` on macOS:

| Keys | Action |
|---|---|
| `Ctrl+N` | New task |
| `Ctrl+F` | Search |
| `Ctrl+\` | Show or hide the lists column |
| `Ctrl+1` … `Ctrl+4` | Inbox, Today, Tomorrow, Next 7 Days |
| `↑`/`↓` or `k`/`j` | Move the selection |
| `Space` | Complete or reopen the selected task |
| `Delete` | Move the selected task to the trash |

Key bindings are defined in `assets/keymaps/default.json`.

**Your data** is stored in `fern-tasks/tasks.db` in the platform's data directory:
- Linux: `~/.local/share/fern-tasks/`
- macOS: `~/Library/Application Support/fern-tasks/`
- Windows: `%APPDATA%\fern-tasks\`

**Sync between your devices** works through your own server:

```bash
FERN_TOKEN=<a long secret> fern-server --listen 0.0.0.0:8650
```

Then open Settings → Sync in the app.

---

## 🧼 Clean-Room Implementation

Fern is an independent project. Its look and behavior were recreated by using the publicly
available TickTick app as an ordinary user would. We clicked through its screens and compared
screenshots side by side. TickTick's source code, binaries, network traffic and assets were
never inspected, decompiled or copied.

Everything in this repository was written from scratch:
- the code;
- the icons, which come from [Lucide](https://lucide.dev);
- the fonts, which are [IBM Plex](https://github.com/IBM/plex).

Fern is not affiliated with, endorsed by or connected to TickTick or its developers. "TickTick" is
a trademark of its owner and is used here only to describe what Fern is inspired by.

---

## ⚠️ Important Notes

- **Early days.** Fern is young. Back up `tasks.db` before you trust it with anything important.
- **Mobile apps are planned.** The core in `crates/core` is meant to be shared with an Android app through UniFFI.
- **No TickTick import yet.**

---

## 🛠️ For Developers

```
crates/core      shared core: data model, SQLite storage, quick-add parser, sync
crates/ui        Zed-style UI components, with crates/theme, crates/icons, crates/assets
apps/server      fern-server, the self-hosted sync server
apps/desktop     the desktop app built with GPUI
docs/            architecture notes and feature parity with TickTick
```

```bash
cargo test --workspace     # run the tests
script/package <target>    # release packages into dist/, e.g. x86_64-unknown-linux-gnu
```

- CI builds and tests every push on Linux, macOS and Windows.
- Pushing a `v*` tag builds packages for all five targets and publishes a GitHub release.

Read `docs/architecture.md` before you start. The code follows Zed's conventions.

Contributions are welcome:
- Code, comments, docs and commit messages are in English.
- UI strings live in `apps/desktop/src/i18n.rs`. To add a language, fill in the `Strings` struct for it.

---

## 📄 License

Fern is free software under the [GNU Affero General Public License v3.0 or later](LICENSE).
If you distribute a modified version, or run one as a network service (for example, a sync
server), you must publish its source under the same license.

Third-party components:

- [GPUI](https://github.com/zed-industries/zed): Apache-2.0. `apps/desktop/src/text_input.rs` started from GPUI's `examples/input.rs`.
- Icons from [Lucide](https://lucide.dev): ISC. See `assets/icons/LICENSE`.
- [IBM Plex Sans](https://github.com/IBM/plex) for the UI and IBM Plex Serif for the countdown numbers: SIL Open Font License 1.1. See `assets/fonts/OFL.txt`.

---

## 💬 Get in Touch

**Built by Amin Atabiev**

- 📧 Email: [endlessspring.of@gmail.com](mailto:endlessspring.of@gmail.com)
- 🐙 GitHub: [@atabits](https://github.com/atabits)
- 🐞 Bugs and ideas: [issues](https://github.com/atabits/fern/issues)

---

<div align="center">

**⭐️ If you like this project, give it a star!**

Made with ❤️ and 🦀 Rust

</div>
