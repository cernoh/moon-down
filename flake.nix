{
  description = "moon-down — terminal download manager on an embedded aria2-rust daemon";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs, ... }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
      ];
      linuxSystems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f system nixpkgs.legacyPackages.${system});
      forLinuxSystems = f: nixpkgs.lib.genAttrs linuxSystems (system: f system nixpkgs.legacyPackages.${system});

      mkPackage =
        pkgs:
        pkgs.rustPlatform.buildRustPackage {
          pname = "moon-down";
          version = "0.1.0";
          src = ./.;

          cargoLock = {
            lockFile = ./Cargo.lock;
            outputHashes = {
              "aria2-0.3.9" = "sha256-cztBSWPIWdG2DJriTC8Gq2vms79Mt9dZEzCFVjccnBA=";
            };
          };

          nativeBuildInputs = with pkgs; [ pkg-config ];
          # Default build needs no system libs; tray feature (off by default)
          # would need gtk3 + libayatana-appindicator + pkg-config shim.

          # No extra buildFeatures — default build is the TUI without --tray.
          # Users wanting the tray can override:
          #   (pkgs.moon-down.override { withTray = true; })  or build with
          #   `nix build .#moon-down --impure` after editing, but we keep the
          #   default closure GUI-free.

          meta = with pkgs.lib; {
            description = "Terminal download manager (ratatui + embedded aria2)";
            homepage = "https://github.com/cernoh/moon-down";
            license = licenses.mit;
            mainProgram = "moon-down";
            platforms = platforms.all;
          };
        };

      # One shell definition, reused per system, so the platforms cannot drift.
      mkShell =
        pkgs:
        let
          appindicatorPc = pkgs.writeTextDir "pkgconfig/libayatana-appindicator3-0.1.pc" ''
            prefix=${pkgs.libayatana-appindicator}
            libdir=''${prefix}/lib
            Name: libayatana-appindicator3
            Description: Ayatana Application Indicators library
            Version: 0.5.92
            Requires: glib-2.0 gio-2.0 gtk+-3.0
            Libs: -L''${libdir} -layatana-appindicator3
          '';
          trayLibPath = pkgs.lib.concatStringsSep ":" (
            map (p: "${p}/lib") [ pkgs.libayatana-appindicator pkgs.gtk3 ]
          );
        in
        pkgs.mkShell {
          name = "moon-down";
          packages = with pkgs; [
            cargo
            rustc
            rustfmt
            clippy
            git
            gtk3
            libayatana-appindicator
            p7zip
            appindicatorPc
          ];
          nativeBuildInputs = with pkgs; [ pkg-config ];
          shellHook = ''
            export PKG_CONFIG_PATH="${appindicatorPc}/pkgconfig:''${PKG_CONFIG_PATH:-}"
            export LD_LIBRARY_PATH="${trayLibPath}:''${LD_LIBRARY_PATH:-}"
            echo "moon-down dev shell"
            echo "  cargo test                    # default build, no GUI deps"
            echo "  cargo build --features tray   # adds the --tray status icon"
            echo "  7z=$(command -v 7z || echo missing)"
            echo "  appindicator=$(pkg-config --modversion libayatana-appindicator3-0.1 2>/dev/null || echo unresolved)"
          '';
        };
    in
    {
      packages = forAllSystems (system: pkgs: {
        moon-down = mkPackage pkgs;
        default = self.packages.${system}.moon-down;
      });

      apps = forAllSystems (system: pkgs: {
        default = {
          type = "app";
          program = "${self.packages.${system}.moon-down}/bin/moon-down";
        };
        moon-down = self.apps.${system}.default;
      });

      overlays.default = final: prev: {
        moon-down = mkPackage final;
      };

      devShells = forLinuxSystems (_system: pkgs: {
        default = mkShell pkgs;
      });
    };
}
