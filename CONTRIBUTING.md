# Contributing to Fern

Thanks for taking the time to contribute to **Fern**!

## Contributor License Agreement (CLA)

Before your pull request can be merged, you must agree to the [Fern Contributor License Agreement (CLA)](CLA.md).

- You keep full copyright ownership of your contributions.
- Fern remains free and open-source software under **GNU AGPL-3.0-or-later** (which prevents third parties from taking Fern's code into closed-source commercial forks or proprietary SaaS offerings).
- Agreeing to the CLA lets the project maintainer safely publish Fern in platform app stores (which have restrictions incompatible with pure GPL/AGPL) and run the optional hosted **Fern Cloud** service that funds development.

Simply check the CLA checkbox in the pull request template when opening a PR.

## Clean-Room Policy

Fern is a strict clean-room implementation. Never inspect, decompile, copy, or port code, assets, or proprietary network traffic from closed-source task managers. Any third-party libraries or assets must use permissive licenses compatible with AGPL-3.0-or-later.

## Architecture & Development Workflow

Read [docs/architecture.md](docs/architecture.md) and [AGENTS.md](AGENTS.md) before making substantial changes.

Key conventions:
- **Keep `apps/desktop` thin** — feature UI lives in `crates/<feature>_ui`.
- **Workspace dependencies only** — declare external crates in root `Cargo.toml` under `[workspace.dependencies]`.
- **Cross-platform builds** — gate platform-specific code (`windows`, `objc2`, `ksni`) behind `#[cfg(...)]` so Linux, macOS, Windows, and `wasm32` all stay green.
- **Localization** — every new user-facing string in `crates/i18n/src/i18n.rs` must be added to all translation tables (`en.rs`, `ru.rs`, `zh_cn.rs`).
- **Code, comments, docs, and commit messages** are in English.
- **Protect your real data** — when launching local debug builds, set `FERN_DATA_DIR` to a temporary directory.

## Required Checks Before Opening a PR

```bash
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo check --workspace --bins --locked
cargo test --workspace --locked
```

If you modify shared crates (`core`, `i18n`, `desktop_common`, `desktop_sync`, `*_ui`), also verify the `wasm32` target:

```bash
cargo check --target wasm32-unknown-unknown --locked -p fern_core -p desktop_common -p desktop_sync -p fern_i18n -p todo_ui
```

## Translations & Bug Reports

- Bug reports and feature requests: use the [GitHub Issue Templates](.github/ISSUE_TEMPLATE).
- New languages: copy `crates/i18n/src/en.rs`, register the locale in `crates/i18n/src/i18n.rs`, and test layout wrapping in narrow windows.
- Security vulnerabilities: **do not open a public issue** — see [SECURITY.md](SECURITY.md).
