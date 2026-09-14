{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    nixgl.url = "github:nix-community/nixGL";

    arbora = {
      url = "github:Aidan-Chelig/arbora";
      inputs.nixpkgs.follows = "nixpkgs";
      inputs.flake-utils.follows = "flake-utils";
      inputs.rust-overlay.follows = "rust-overlay";
    };

    trenchbroom-chaps = {
      url = "git+ssh://git@github.com/Aidan-Chelig/TrenchBroom-Chaps.git?ref=master";
      inputs.nixpkgs.follows = "nixpkgs";
    };

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
      arbora,
      trenchbroom-chaps,
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
        trenchbroomPackage = trenchbroom-chaps.packages.${system}.default;
        trenchbroom = pkgs.writeShellScriptBin "trenchbroom" ''
          unset LD_LIBRARY_PATH
          exec ${nixgl.packages.${system}.nixGLIntel}/bin/nixGLIntel ${trenchbroomPackage}/bin/trenchbroom "$@"
        '';
        ericwTools = pkgs.stdenv.mkDerivation {
          pname = "ericw-tools-bin";
          version = "2.0.0-alpha10";

          src = pkgs.fetchzip {
            url = "https://github.com/ericwa/ericw-tools/releases/download/2.0.0-alpha10/ericw-tools-2.0.0-alpha10-Linux.zip";
            hash = "sha256-tDrH11P9DdKx9iUataCEircqWk0DAFZGKmwUBz1k87s=";
            stripRoot = false;
          };

          nativeBuildInputs = [ pkgs.autoPatchelfHook ];
          buildInputs = [
            pkgs.stdenv.cc.cc.lib
            pkgs.zlib
            pkgs.embree
          ];

          dontConfigure = true;
          dontBuild = true;

          installPhase = ''
            runHook preInstall

            mkdir -p $out/bin $out/lib
            cp qbsp vis light bsputil bspinfo $out/bin/
            cp *.so* $out/lib/

            runHook postInstall
          '';
        };
        # 👇 new! note that it refers to the path ./rust-toolchain.toml
        rustToolchain = pkgs.pkgsBuildHost.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
        arboraPackage = arbora.packages.${system}.default;
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
            arboraPackage
            rustToolchain
            rust-analyzer
            rustfmt
            cargo-edit
            cargo-watch
            tracy
            ericwTools
            trenchbroom
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
