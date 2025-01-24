{
  inputs = {
    nixpkgs.url = "nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    # fenix = {
    #   url = "github:nix-community/fenix";
    #   inputs.nixpkgs.follows = "nixpkgs";
    # };
  };

  outputs = {
    nixpkgs,
    flake-utils,
    # fenix,
    ...
  }:
    flake-utils.lib.eachDefaultSystem
    (
      system: let
        overlays = [
          # fenix.overlays.default
        ];
        pkgs = import nixpkgs {
          inherit system overlays;
        };

        nativeBuildInputs = with pkgs; [
          # (fenix.packages.${system}.latest.withComponents [
          #   "cargo"
          #   "rust-src"
          #   "rust-std"
          #   "rustc"
          #   "rustfmt"
          #   "clippy"
          # ])
          # rust-analyzer-nightly

          pkg-config
        ];

        buildInputs = with pkgs; [
          openssl
        ];

        LD_LIBRARY_PATH = "${pkgs.openssl.out}/lib";
      in {
        devShells.default = pkgs.mkShell {
          nativeBuildInputs = nativeBuildInputs;
          buildInputs = buildInputs;
          LD_LIBRARY_PATH = LD_LIBRARY_PATH;
        };

        # packages.default = pkgs.rustPlatform.buildRustPackage {
        #   pname = "crates-language-server";
        #   version = "0.1.0";

        #   cargoLock.lockFile = ./Cargo.lock;

        #   src = pkgs.lib.cleanSource ./.;

        #   nativeBuildInputs = nativeBuildInputs;
        #   buildInputs = buildInputs;
        #   LD_LIBRARY_PATH = LD_LIBRARY_PATH;

        #   buildPhase = ''
        #     cargo build --release
        #   '';

        #   installPhase = ''
        #     mkdir -p $out/bin
        #     cp target/release/crates-language-server $out/bin/
        #   '';

        #   # we need to skip the tests that require a network connection
        #   # as nix executes them in a sandbox without network access.
        #   checkPhase = ''
        #     cargo test --release -- --skip fetch
        #   '';

        #   meta = with pkgs.lib; {
        #     description = "A language server for Cargo.toml's dependencies";
        #     homepage = "https://github.com/rotmh/crates-language-server";
        #     license = licenses.mit;
        #   };
        # };
      }
    );
}
