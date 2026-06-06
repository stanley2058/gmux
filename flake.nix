{
  description = "gmux - terminal multiplexer with persistent sessions, tabs, and panes";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs =
    { self, nixpkgs }:
    let
      lib = nixpkgs.lib;
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forAllSystems = lib.genAttrs systems;
      pkgsFor = system: import nixpkgs { inherit system; };
    in
    {
      packages = forAllSystems (
        system:
        let
          pkgs = pkgsFor system;
          gmux = pkgs.callPackage ./nix/package.nix { };
        in
        {
          inherit gmux;
          default = gmux;
        }
      );

      apps = forAllSystems (system: {
        default = {
          type = "app";
          program = "${self.packages.${system}.default}/bin/gmux";
          meta.description = "Run Gmux";
        };
      });

      checks = forAllSystems (system: {
        gmux = self.packages.${system}.default;
        default = self.checks.${system}.gmux;
      });

      devShells = forAllSystems (
        system:
        let
          pkgs = pkgsFor system;
        in
        {
          default = pkgs.mkShell {
            name = "gmux-dev";
            packages = with pkgs; [
              cargo
              cargo-nextest
              clippy
              cmake
              just
              ninja
              pkg-config
              rustc
              rustfmt
              zig_0_15
            ];

            env = {
              LIBGHOSTTY_VT_OPTIMIZE = "Debug";
              LIBGHOSTTY_VT_SIMD = "true";
            };
          };
        }
      );

      formatter = forAllSystems (system: (pkgsFor system).nixfmt);

      overlays.default = final: _prev: {
        gmux = final.callPackage ./nix/package.nix { };
      };
    };
}
