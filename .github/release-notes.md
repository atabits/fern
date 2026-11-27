<!-- fern-release-template -->
Fern is a free, open-source task manager: tasks, horizons, calendar, habits, focus, and plugins in
one native app. Download it and start working: no account, no server, your data stays on your
computer. Optional zero-knowledge sync across devices is available via Fern Cloud or your own
self-hosted `fern-server`.

> [!WARNING]
> **Early preview.** Please read before installing:
> - **Unsigned builds.** Windows SmartScreen and macOS Gatekeeper warn on first launch. See
>   Installation below.
> - **Little testing on macOS.** The macOS builds pass CI, but fewer people have run them on
>   hardware yet. [Report issues](https://github.com/{repo}/issues).
> - **Back up what matters.** Fern keeps a daily copy of your data. Settings → About → Generate
>   Backup makes one on demand.
> - **Desktop & Web.** No native mobile app yet — use [fern.atabits.com](https://fern.atabits.com)
>   in a browser or install the desktop builds below.
> - **Updates.** From 0.1.5 on, Fern downloads new releases itself and installs them on
>   **Restart to Update**. Older versions need this release installed by hand once.

## Downloads

| Platform | Download |
|---|---|
| 🍏 macOS (Apple Silicon) | [fern-{version}-macos-arm64.dmg](https://github.com/{repo}/releases/download/v{version}/fern-{version}-macos-arm64.dmg) |
| 🍏 macOS (Intel) | [fern-{version}-macos-x86_64.dmg](https://github.com/{repo}/releases/download/v{version}/fern-{version}-macos-x86_64.dmg) |
| 🪟 Windows (x86_64) | [Installer](https://github.com/{repo}/releases/download/v{version}/fern-{version}-windows-x86_64-setup.exe) · [standalone ZIP](https://github.com/{repo}/releases/download/v{version}/fern-{version}-windows-x86_64.zip) |
| 🐧 Linux (x86_64) | [.deb](https://github.com/{repo}/releases/download/v{version}/fern-{version}-linux-x86_64.deb) · [.rpm](https://github.com/{repo}/releases/download/v{version}/fern-{version}-linux-x86_64.rpm) · [AppImage](https://github.com/{repo}/releases/download/v{version}/fern-{version}-linux-x86_64.AppImage) · [tar.gz](https://github.com/{repo}/releases/download/v{version}/fern-{version}-linux-x86_64.tar.gz) |
| 🐧 Linux (ARM64) | [.deb](https://github.com/{repo}/releases/download/v{version}/fern-{version}-linux-arm64.deb) · [.rpm](https://github.com/{repo}/releases/download/v{version}/fern-{version}-linux-arm64.rpm) · [AppImage](https://github.com/{repo}/releases/download/v{version}/fern-{version}-linux-arm64.AppImage) · [tar.gz](https://github.com/{repo}/releases/download/v{version}/fern-{version}-linux-arm64.tar.gz) |

The `fern-server-*` archives hold the optional self-hosted sync server.

## Installation

**macOS** — open the DMG and drag Fern into Applications. If the app is not notarized, macOS
refuses to open it the first time. Open **System Settings → Privacy & Security** and click
**Open Anyway** next to Fern. You can also run
`xattr -d com.apple.quarantine /Applications/Fern.app`.

**Windows** — run the installer. It needs no administrator rights and adds Fern to the Start
menu. The ZIP runs without installation: unpack it and run `fern.exe`. To keep your data beside
`fern.exe`, create an empty `.portable` file there before starting Fern. If SmartScreen warns
about an unknown publisher, choose **More info → Run anyway**.

**Linux** — on Debian and Ubuntu, open the `.deb` or run `sudo apt install ./fern-{version}-linux-*.deb`.
Elsewhere, `chmod +x fern-{version}-linux-*.AppImage` and run it. You can also unpack the
tar.gz and run `fern`.
