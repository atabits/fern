Name:           fern-tasks
Version:        0.4.0
Release:        1%{?dist}
Summary:        Tasks, horizons, calendar, habits, focus, and plugins in one native app

License:        AGPL-3.0-or-later
URL:            https://github.com/atabits/fern
Source0:        %{url}/archive/refs/tags/v%{version}.tar.gz

BuildRequires:  cargo
BuildRequires:  rust
BuildRequires:  clang
BuildRequires:  gcc
BuildRequires:  pkgconf-pkg-config
BuildRequires:  cmake
BuildRequires:  desktop-file-utils
BuildRequires:  appstream
BuildRequires:  pkgconfig(alsa)
BuildRequires:  pkgconfig(dbus-1)
BuildRequires:  pkgconfig(fontconfig)
BuildRequires:  pkgconfig(freetype2)
BuildRequires:  pkgconfig(openssl)
BuildRequires:  pkgconfig(sqlite3)
BuildRequires:  pkgconfig(vulkan)
BuildRequires:  pkgconfig(wayland-client)
BuildRequires:  pkgconfig(xcb)
BuildRequires:  pkgconfig(xkbcommon)
BuildRequires:  pkgconfig(xkbcommon-x11)

Requires:       fontconfig
Requires:       freetype
Requires:       hicolor-icon-theme
Requires:       libwayland-client
Requires:       libxcb
Requires:       libxkbcommon
Requires:       libxkbcommon-x11
Requires:       mesa-libEGL
Requires:       mesa-libGL
Requires:       vulkan-loader
Recommends:     google-noto-color-emoji-fonts
Recommends:     mesa-vulkan-drivers
Recommends:     pipewire-utils

%description
Fern is a free, open-source native task manager written in Rust and GPU-rendered with GPUI.
It works offline, needs no account, and keeps all tasks, calendars, habits, and focus sessions locally in SQLite.

%prep
%autosetup -n fern-%{version}

%build
cargo build --release --locked -p fern_desktop

%install
install -Dm0755 target/release/fern %{buildroot}%{_bindir}/fern
install -Dm0644 assets/linux/fern-tasks.desktop %{buildroot}%{_datadir}/applications/fern-tasks.desktop
install -Dm0644 assets/linux/io.github.atabits.fern.metainfo.xml %{buildroot}%{_datadir}/metainfo/io.github.atabits.fern.metainfo.xml
install -Dm0644 assets/icon/icon-256.png %{buildroot}%{_datadir}/icons/hicolor/256x256/apps/fern.png
install -Dm0644 assets/linux/fern.svg %{buildroot}%{_datadir}/icons/hicolor/scalable/apps/fern.svg

%check
desktop-file-validate %{buildroot}%{_datadir}/applications/fern-tasks.desktop
appstreamcli validate --no-net %{buildroot}%{_datadir}/metainfo/io.github.atabits.fern.metainfo.xml

%files
%license LICENSE
%doc README.md
%{_bindir}/fern
%{_datadir}/applications/fern-tasks.desktop
%{_datadir}/metainfo/io.github.atabits.fern.metainfo.xml
%{_datadir}/icons/hicolor/256x256/apps/fern.png
%{_datadir}/icons/hicolor/scalable/apps/fern.svg

%changelog
* Fri Nov 27 2026 Amin Atabiev <endlessspring.of@gmail.com> - 0.4.0-1
- Add zero-knowledge Fern Cloud sync, OS keychain vault, realtime SSE, and performance/UI polish

* Tue Nov 24 2026 Amin Atabiev <endlessspring.of@gmail.com> - 0.3.0-1
- Initial Fedora/COPR package release
