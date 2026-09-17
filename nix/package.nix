{
  craneLib,
  fontconfig,
  freetype,
  pkg-config,
}:

let
  cargoToml = builtins.fromTOML (builtins.readFile ../Cargo.toml);
in
craneLib.buildPackage {
  pname = cargoToml.package.name;
  version = cargoToml.package.version;
  src = craneLib.cleanCargoSource ../.;
  strictDeps = true;
  cargoExtraArgs = "--bin ringring-rs";

  nativeBuildInputs = [ pkg-config ];
  buildInputs = [
    fontconfig
    freetype
  ];
}
