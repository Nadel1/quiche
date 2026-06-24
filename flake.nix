{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay.url = "github:oxalica/rust-overlay";
    self.submodules = true;
  };
  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
      rust-overlay,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ rust-overlay.overlays.default ];
        };
        cargoLock = {
          lockFile = ./Cargo.lock;
          outputHashes = {
            "wirefilter-engine-0.7.0" = "sha256-vPelFk4BBqb/YkfC8EKYp/C/clannq8iHjGYZYtOftQ=";
          };
        };
        buildRustPackage =
          name: path:
          let
            manifest = (pkgs.lib.importTOML (./. + "/${path}/Cargo.toml")).package;
          in
          pkgs.rustPlatform.buildRustPackage {
            pname = manifest.name;
            version = manifest.version;
            inherit cargoLock;
            src = self;
            buildAndTestSubdir = [ path ];
            nativeBuildInputs = with pkgs; [
              cmake # for boringssl
              pkg-config # for qlog-dancer
            ];
            buildInputs = with pkgs; [
              fontconfig # for qlog-dancer
            ];
          };
        # does not compile currently
        qlogDancerWeb =
          let
            manifest = (pkgs.lib.importTOML ./qlog-dancer/Cargo.toml).package;
          in
          pkgs.rustPlatform.buildRustPackage {
            pname = "qlog-dancer-web";
            version = manifest.version;
            inherit cargoLock;
            src = self;
            nativeBuildInputs = with pkgs; [
              wasm-pack
              pkg-config
              writableTmpDirAsHomeHook
              llvmPackages.lld
            ];
            buildInputs = with pkgs; [
              fontconfig
            ];
            buildPhase = ''
              cd qlog-dancer
              wasm-pack build --target=web
            '';
            installPhase = ''
              mkdir -p $out
              cp -r pkg $out/
              cp index.html qlog-dancer.css qlog-dancer-ui.js $out/
            '';
            doCheck = false;
          };
      in
      {
        packages = {
          apps = buildRustPackage "quiche_apps" "apps";
          buffer-pool = buildRustPackage "buffer-pool" "buffer-pool";
          datagram-socket = buildRustPackage "datagram-socket" "datagram-socket";
          h3i = buildRustPackage "h3i" "h3i";
          netlog = buildRustPackage "netlog" "netlog";
          octets = buildRustPackage "octets" "octets";
          qlog = buildRustPackage "qlog" "qlog";
          qlog-dancer = buildRustPackage "qlog-dancer" "qlog-dancer";
          qlog-dancer-web = qlogDancerWeb;
          quiche = buildRustPackage "quiche" "quiche";
          task-killswitch = buildRustPackage "task-killswitch" "task-killswitch";
          tokio-quiche = buildRustPackage "tokio-quiche" "tokio-quiche";
          default = self.packages.${system}.quiche;
        };
        devShells.default =
          let
            rust-toolchain =
              with pkgs;
              pkgs.symlinkJoin {
                name = "rust-toolchain";
                paths = [
                  rustc
                  cargo
                  rustPlatform.rustcSrc
                ];
              };
          in
          pkgs.mkShell {
            buildInputs = with pkgs; [
              clippy
              cmake # for boringssl
              rust-analyzer
              rust-toolchain
              (pkgs.rust-bin.nightly.latest.minimal.override { extensions = [ "rustfmt" ]; })
              pkg-config # for qlog-dancer
              fontconfig # for qlog-dancer
              clang # for boring-sys bindgen
            ];
            LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
            RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
          };
      }
    );
}