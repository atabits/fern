{
  description = "Fern — fast native Rust + GPUI task manager, horizons planner, calendar, habits, and sync server";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachSystem [ "x86_64-linux" "aarch64-linux" ] (system:
      let
        pkgs = import nixpkgs { inherit system; };
        fern = pkgs.callPackage ./packaging/nix/package.nix { };
        fern-server = pkgs.callPackage ./packaging/nix/package.nix { buildServerOnly = true; };
      in
      {
        packages = {
          default = fern;
          inherit fern fern-server;
        };

        apps.default = flake-utils.lib.mkApp {
          drv = fern;
          exePath = "/bin/fern";
        };

        devShells.default = pkgs.mkShell {
          inputsFrom = [ fern ];
          packages = with pkgs; [
            cargo
            rustc
            rustfmt
            clippy
            desktop-file-utils
            appstream
          ];
          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath fern.buildInputs;
        };
      });
}
