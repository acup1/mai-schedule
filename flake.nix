{
  description = "CLI and TUI for the MAI student schedule";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      systems = [
        "aarch64-darwin"
        "aarch64-linux"
        "x86_64-darwin"
        "x86_64-linux"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
    in
    {
      packages = forAllSystems (system:
        let
          pkgs = import nixpkgs { inherit system; };
          package = pkgs.python3Packages.buildPythonApplication {
            pname = "mai-schedule";
            version = "1.0.0";
            pyproject = true;
            src = ./.;
            build-system = [ pkgs.python3Packages.setuptools ];
            doCheck = false;
            meta.mainProgram = "mai-schedule";
          };
        in
        {
          default = package;
          mai-schedule = package;
        });

      apps = forAllSystems (system: {
        default = {
          type = "app";
          program = "${self.packages.${system}.default}/bin/mai-schedule";
        };
      });
    };
}
