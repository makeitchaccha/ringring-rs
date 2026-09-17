{
  description = "ringring-rs";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    crane.url = "github:ipetkov/crane";
  };

  outputs =
    { nixpkgs, crane, ... }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
    in
    {
      devShells = forAllSystems (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
        in
        {
          default = pkgs.mkShell {
            packages = with pkgs; [
              cargo
              clippy
              rustc
              rustfmt
              rust-analyzer
            ];
          };
        }
      );

      packages = forAllSystems (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
          mkPackage =
            packageSet:
            import ./nix/package.nix {
              craneLib = crane.mkLib packageSet;
            };
          muslPkgs =
            if system == "x86_64-linux" then
              pkgs.pkgsCross.musl64
            else if system == "aarch64-linux" then
              pkgs.pkgsCross.aarch64-multiplatform-musl
            else
              null;
          ringring-rs = mkPackage pkgs;
          ringring-rs-musl = if muslPkgs == null then null else mkPackage muslPkgs;
          mkImage =
            package:
            import ./nix/image.nix {
              inherit package;
              inherit (pkgs) cacert dejavu_fonts dockerTools;
            };
        in
        {
          default = ringring-rs;
        }
        // nixpkgs.lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
          musl = ringring-rs-musl;
          dockerImage = mkImage ringring-rs;
          dockerImageMusl = mkImage ringring-rs-musl;
        }
      );
    };
}
