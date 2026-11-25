# Linux Native Integration & Distribution Guide

Fern compiles natively against Wayland and X11 using **GPUI** (`wgpu`/Vulkan + OpenGL/EGL fallback, no Chromium/Electron) and integrates cleanly across **Ubuntu / Debian / Mint**, **Fedora**, **Arch Linux**, **NixOS**, and **Flatpak**.

---

## 1. Installing on Top 5 Distributions

| Distribution | Recommended Package | Install / Run Command |
|---|---|---|
| **Ubuntu 22.04 / 24.04, Debian 12, Mint** | `.deb` release artifact | `sudo apt install ./fern-*-linux-x86_64.deb` |
| **Fedora 40 / 41+, RHEL, openSUSE** | `.rpm` release artifact (or COPR `packaging/rpm/fern-tasks.spec`) | `sudo dnf install ./fern-*-linux-x86_64.rpm` |
| **Arch Linux, EndeavourOS, CachyOS** | AUR recipe `packaging/aur/PKGBUILD` | `yay -S fern-tasks-bin` *(once pushed to AUR)* or `makepkg -si` |
| **NixOS / Nix Flakes** | Root `flake.nix` & `packaging/nix/package.nix` | `nix run github:atabits/fern` or `nix build` |
| **Universal / Sandboxed** | AppImage or Flatpak (`packaging/flatpak/io.github.atabits.fern.yml`) | `chmod +x fern-*.AppImage && ./fern-*.AppImage` |

> **Ubuntu 24.04+ / Fedora AppImage note:** Newer distros ship without FUSE 2 (`libfuse2t64` / `fuse`). If your desktop does not mount AppImages automatically, install `libfuse2t64` / `fuse` or run `fern-*.AppImage --appimage-extract-and-run` (or prefer the native `.deb` / `.rpm`).

---

## 2. Wayland, X11 & Native Desktop Behavior

- **Window Decorations (CSD vs. SSD):**
  - On compositors that negotiate **client-side decorations** (GNOME, cosmic, Sway/Hyprland CSD), Fern draws native rounded corners, subtle shadow/resize edges, and pill window controls following GNOME's `button-layout` (left or right).
  - On **server-side decorated** desktops (KDE Plasma, XFCE, Cinnamon, Openbox, tiling WMs with titlebars), Fern collapses its top header inset and hides duplicate in-window close buttons in Settings so window chrome never doubles up.
- **Wayland Global Shortcuts (`fern` CLI IPC):**
  - Wayland does not allow background apps to grab arbitrary global hotkeys via X11 passive grabs. Instead, Fern listens on a private Unix domain socket (`0o600` in `$XDG_RUNTIME_DIR`) and accepts instant forwarded commands:
    - `fern --quick-add` — open Quick Add anywhere
    - `fern --toggle` — show/hide the main window
    - `fern --focus-mini` — toggle the floating Focus Mini timer
  - Bind those commands in **GNOME Settings → Keyboard → Custom Shortcuts**, **KDE System Settings → Shortcuts → Add Command**, or `~/.config/hypr/hyprland.conf` (`bind = SUPER ALT, T, exec, fern --quick-add`).
- **System Tray & Notifications:**
  - Uses `org.kde.StatusNotifierWatcher` (`ksni`) for native panel icons and `org.freedesktop.Notifications` with `desktop-entry=fern-tasks` (or `$FLATPAK_ID` in Flatpak). On GNOME Shell, enable the standard *AppIndicator and KStatusNotifierItem Support* extension if you want minimize-to-tray.
- **Per-Profile Isolation & Updates:**
  - Set `FERN_DATA_DIR=/path/to/profile` (in debug or release builds) to run separate SQLite databases/profiles, and `FERN_APP_ID` to customize the Wayland `app_id` / X11 `WM_CLASS`.
  - `.deb`, `.rpm`, Arch (`usr/bin`), Nix (`/nix/store`), Flatpak, and Snap builds are recognized as externally managed packages so in-app self-replacement is never triggered; AppImage and `.tar.gz` builds support seamless in-app updates.

---

## 3. Maintainer Publishing Checklists

### AUR (`fern-tasks-bin`)
1. Generate an ED25519 SSH key registered on `aur.archlinux.org` for package `fern-tasks-bin`.
2. Add the private key as repository secret `AUR_SSH_PRIVATE_KEY` in GitHub Settings.
3. Running `.github/workflows/release.yml` with `release: true` automatically updates `packaging/aur/{PKGBUILD,.SRCINFO}` via `script/update-aur` and pushes to `ssh://aur@aur.archlinux.org/fern-tasks-bin.git`.

### Flathub (`io.github.atabits.fern`)
1. Run `bash packaging/flatpak/generate-cargo-sources.sh` to create offline `packaging/flatpak/cargo-sources.json` from `Cargo.lock`.
2. Verify locally: `flatpak-builder --force-clean --repo=repo build-dir packaging/flatpak/io.github.atabits.fern.yml`.
3. Submit `packaging/flatpak/io.github.atabits.fern.yml` and `cargo-sources.json` to `flathub/flathub`.

### Fedora COPR (`fern-tasks`)
1. Point a Fedora COPR package at `packaging/rpm/fern-tasks.spec` (SCM method: Git `atabits/fern`, subdir `.`, spec `packaging/rpm/fern-tasks.spec`, `rpkg`/mock build with network enabled during Cargo fetch or vendored source tarball).
