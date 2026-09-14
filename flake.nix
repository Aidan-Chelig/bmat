{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    nixgl.url = "github:nix-community/nixGL";

    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };
  outputs =
    {
      self,
      nixpkgs,
      nixgl,
      flake-utils,
      rust-overlay,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs {
          inherit system overlays;
          config.allowUnfree = true;
        };
        # 👇 new! note that it refers to the path ./rust-toolchain.toml
        rustToolchain = pkgs.pkgsBuildHost.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
      in
      with pkgs;
      {
        devShells.default = mkShell {
          shellHook = ''
            export LD_LIBRARY_PATH="$LD_LIBRARY_PATH:${
              pkgs.lib.makeLibraryPath [
                pkgs.alsa-lib
                pkgs.udev
                pkgs.vulkan-loader
                pkgs.libxkbcommon
                pkgs.wayland
                pkgs.libX11
                pkgs.libXcursor
                pkgs.libXrandr
                pkgs.libXi
                pkgs.libclang.lib
                pkgs.stdenv.cc.cc.lib
              ]
            }"
            export LIBCLANG_PATH="${pkgs.libclang.lib}/lib"
          '';

          # 👇 we can just use `rustToolchain` here:
          buildInputs = [
            cmake
            rustToolchain
            rust-analyzer
            rustfmt
            cargo-edit
            cargo-watch
            pkg-config
            alsa-lib
            jack2

            lld
            clang
            libclang.lib

            udev
            # lutris
            wayland
            wayland-protocols
            libxkbcommon
            libX11
            libXcursor
            libXrandr
            libXi
            vulkan-tools
            vulkan-headers
            vulkan-loader
            vulkan-validation-layers
            libjack2
            just
            bacon

          ];
        };
      }
    );
}
