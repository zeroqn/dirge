{
  description = "Minimal, fast pure-Rust coding agent with persistent memory";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
      ];
      forAllSystems =
        f:
        nixpkgs.lib.genAttrs systems (
          system:
          f {
            inherit system;
            pkgs = nixpkgs.legacyPackages.${system};
          }
        );
    in
    {
      packages = forAllSystems (
        { system, pkgs }:
        rec {
          dirge = pkgs.callPackage ./nix/package.nix { src = self; };
          # x86_64-linux: forked prebuilt with the microVM sandbox enabled
          # (zeroqn/dirge rolling `ds-sandbox` release, built by
          # .github/workflows/release-sandbox.yml). Other systems keep the
          # upstream prebuilt (no microVM support).
          dirge-bin =
            if system == "x86_64-linux"
            then pkgs.callPackage ./nix/bin-sandbox.nix { }
            else pkgs.callPackage ./nix/bin.nix { };
          default = dirge;
        }
      );

      homeModules = {
        dirge = import ./nix/home-manager.nix { inherit self; };
        default = self.homeModules.dirge;
      };

      apps = forAllSystems (
        { system, ... }:
        {
          default = {
            type = "app";
            program = "${self.packages.${system}.default}/bin/dirge";
          };
        }
      );

      devShells = forAllSystems (
        { pkgs, ... }:
        {
          default = pkgs.callPackage ./nix/devshell.nix { };
        }
      );

      checks = forAllSystems (
        { system, ... }:
        {
          build = self.packages.${system}.default;
        }
      );

      formatter = forAllSystems ({ pkgs, ... }: pkgs.nixfmt-rfc-style);

      overlays.default = final: prev: {
        dirge = final.callPackage ./nix/package.nix { src = self; };
        # Same x86_64-linux override as `packages.dirge-bin` above.
        dirge-bin =
          if final.stdenv.hostPlatform.system == "x86_64-linux"
          then final.callPackage ./nix/bin-sandbox.nix { }
          else final.callPackage ./nix/bin.nix { };
      };
    };
}
