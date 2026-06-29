{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };
  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
      ...
    }@inputs:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
        };
      in
      rec {
        packages.default = pkgs.stdenvNoCC.mkDerivation {
            name = "test-certs";
            src = ./.;
            nativeBuildInputs = with pkgs; [
              gnutls
              xxd
            ];
            buildPhase = ''
              make
            '';
            installPhase = ''
              cp -r ./out $out
            '';
        };
      }
    );
}