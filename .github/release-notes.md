## Downloads

| Platform | Download |
|---|---|
| 🍏 macOS (Apple Silicon) | [fern-{version}-macos-arm64.dmg](https://github.com/{repo}/releases/download/v{version}/fern-{version}-macos-arm64.dmg) |
| 🍏 macOS (Intel) | [fern-{version}-macos-x86_64.dmg](https://github.com/{repo}/releases/download/v{version}/fern-{version}-macos-x86_64.dmg) |
| 🪟 Windows (x86_64) | [fern-{version}-windows-x86_64.zip](https://github.com/{repo}/releases/download/v{version}/fern-{version}-windows-x86_64.zip) |
| 🐧 Linux (x86_64) | [AppImage](https://github.com/{repo}/releases/download/v{version}/fern-{version}-linux-x86_64.AppImage) · [tar.gz](https://github.com/{repo}/releases/download/v{version}/fern-{version}-linux-x86_64.tar.gz) |
| 🐧 Linux (ARM64) | [AppImage](https://github.com/{repo}/releases/download/v{version}/fern-{version}-linux-arm64.AppImage) · [tar.gz](https://github.com/{repo}/releases/download/v{version}/fern-{version}-linux-arm64.tar.gz) |

The `fern-server-*` archives hold the optional self-hosted sync server.

## Installation

**macOS** — open the DMG and drag Fern into Applications. The app is not notarized: the first
time, right-click it and choose **Open**, or run
`xattr -d com.apple.quarantine /Applications/Fern.app`.

**Windows** — unpack the ZIP and run `fern.exe`. SmartScreen may warn about an unknown
publisher: choose **More info → Run anyway**.

**Linux** — `chmod +x fern-{version}-linux-*.AppImage` and run it, or unpack the tar.gz and run
`fern` (copy `fern-tasks.desktop` and `fern.svg` into your applications and icons folders for a
launcher entry).
