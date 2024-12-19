{
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/release-24.11";

  outputs =
    { nixpkgs, ... }:
    let
      forAllSystems =
        f: nixpkgs.lib.genAttrs nixpkgs.lib.systems.flakeExposed (s: f nixpkgs.legacyPackages.${s});
    in
    {
      packages = forAllSystems (pkgs: rec {
        default = alien;

        alien = pkgs.buildRustPackage {
          pname = "alien";
          version = "0.1.0";
        };
      });

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [
            cargo
            rustc
            rustfmt

            dpkg
            rpm
          ];
        };
      });
    };
}
