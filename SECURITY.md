# Security Policy

Security and privacy are core design pillars of **Fern** — especially around local SQLite data integrity, OS keychain session storage, sandboxed QuickJS-NG plugins, and zero-knowledge end-to-end encryption (E2EE) in Fern Cloud and self-hosted `fern-server`.

## Supported Versions

Security fixes are released on the latest minor version on `main`.

| Version | Supported |
| ------- | --------- |
| `0.3.x` | Yes       |
| `< 0.3` | No        |

## Reporting a Vulnerability

**Please do not report security vulnerabilities through public GitHub issues or discussions.**

Instead, report them privately using one of the following channels:

1. **GitHub Security Advisories (preferred):** open a private vulnerability report at  
   `https://github.com/atabits/fern/security/advisories/new`
2. **Email:** send details to **endlessspring.of@gmail.com** with the subject `[Fern Security] <short summary>`.

Please include:
- A clear description of the vulnerability and its security impact.
- Minimal reproduction steps or a proof of concept.
- Affected version, OS, and build configuration (Desktop, `fern-server`, or Fern Web/Cloud).

We aim to acknowledge reports within **48 hours** and coordinate a fix and disclosure timeline with you.

## Scope

In-scope areas include:
- **Zero-Knowledge E2EE & Key Management (`crates/core/src/cloud.rs`, `crates/core/src/e2ee.rs`):** Argon2id/HKDF key derivation, ChaCha20-Poly1305 AEAD encryption, AAD binding, recovery keys, or plaintext leakage.
- **Credential & Session Storage (`crates/desktop_common/src/cloud_vault.rs`):** OS keychain integration, DPAPI/file-permission handling, token rotation, or unintended persistence in `tasks.db` / backups / diagnostics.
- **Self-Hosted Sync Server (`apps/server`) & Fern Cloud API:** authentication bypass, cross-tenant data access, path traversal, DoS/resource exhaustion, CSRF, or SSE stream authorization.
- **Plugin Sandbox (`crates/plugins`):** QuickJS-NG sandbox escapes, capability/permission bypasses, or unauthorized filesystem/network access.
- **Local Data & Imports:** SQL injection, path traversal in backup/attachment/CalDAV imports, or arbitrary command execution.

## Out of Scope

- Missing code signing warnings on unsigned community builds (documented in [docs/code-signing.md](docs/code-signing.md)).
- Attacks requiring physical access to an already unlocked OS user session or root/Administrator privileges on the local device.
- Vulnerabilities in third-party self-hosted reverse proxies or OS configurations outside Fern.
