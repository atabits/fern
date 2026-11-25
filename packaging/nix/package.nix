{ lib
, rustPlatform
, pkg-config
, makeWrapper
, openssl
, sqlite
, fontconfig
, freetype
, dbus
, alsa-lib
, wayland
, libxkbcommon
, libxcb
, vulkan-loader
, libGL
, libX11
, libXcursor
, libXi
, buildServerOnly ? false
}:

let
  runtimeLibs = [
    wayland
    libxkbcommon
    libxcb
    vulkan-loader
    libGL
    libX11
    libXcursor
    libXi
    fontconfig
    freetype
  ];
in
rustPlatform.buildRustPackage rec {
  pname = if buildServerOnly then "fern-server" else "fern-tasks";
  version = (lib.importTOML ../../Cargo.toml).workspace.package.version;

  src = ../../.;
  cargoLock.lockFile = ../../Cargo.lock;

  cargoBuildFlags =
    if buildServerOnly then [ "-p" "fern_server" ] else [ "-p" "fern_desktop" ];

  doCheck = false;

  nativeBuildInputs = [
    pkg-config
  ] ++ lib.optionals (!buildServerOnly) [
    makeWrapper
  ];

  buildInputs = [
    openssl
    sqlite
  ] ++ lib.optionals (!buildServerOnly) ([
    dbus
    alsa-lib
  ] ++ runtimeLibs);

  postInstall = lib.optionalString (!buildServerOnly) ''
    install -Dm0644 assets/linux/fern-tasks.desktop "$out/share/applications/fern-tasks.desktop"
    install -Dm0644 assets/linux/io.github.atabits.fern.metainfo.xml "$out/share/metainfo/io.github.atabits.fern.metainfo.xml"
    install -Dm0644 assets/linux/fern.svg "$out/share/icons/hicolor/scalable/apps/fern.svg"
    install -Dm0644 assets/icon/icon-256.png "$out/share/icons/hicolor/256x256/apps/fern.png"

    wrapProgram "$out/bin/fern" \
      --prefix LD_LIBRARY_PATH : "${lib.makeLibraryPath runtimeLibs}"
  '';

  meta = with lib; {
    description = "Local-first native Rust and GPUI task manager, calendar, habits, focus, and sync server";
    homepage = "https://fern.atabits.com";
    license = licenses.agpl3Plus;
    platforms = platforms.linux;
    mainProgram = if buildServerOnly then "fern-server" else "fern";
  };
}
