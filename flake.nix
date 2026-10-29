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
          package = pkgs.rustPlatform.buildRustPackage {
            pname = "mai-schedule";
            version = "2.0.0";
            src = ./.;
            cargoLock = {
              lockFile = ./Cargo.lock;
              outputHashes."terminalib-0.1.0" =
                "sha256-jMHN7WG47ex/E/iZ1s+AnG4poEka60RxS6pteG42CFE=";
            };
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
