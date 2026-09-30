{
  description = "moon-down — terminal download manager on an embedded aria2-rust daemon";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { nixpkgs, ... }:
    let
      # One shell definition, reused per system, so the platforms cannot drift.
      mkShell =
        pkgs:
        let
          # nixpkgs' libayatana-appindicator ships the shared library but no
          # pkg-config file and no headers. The `libappindicator` crate that backs
          # the tray feature uses hand-written FFI, so it only needs the .so at
          # link time — a metadata-only .pc shim satisfies system-deps.
          appindicatorPc = pkgs.writeTextDir "pkgconfig/libayatana-appindicator3-0.1.pc" ''
            prefix=${pkgs.libayatana-appindicator}
            libdir=''${prefix}/lib
            Name: libayatana-appindicator3
            Description: Ayatana Application Indicators library
            Version: 0.5.92
            Requires: glib-2.0 gio-2.0 gtk+-3.0
            Libs: -L''${libdir} -layatana-appindicator3
          '';
        in
        pkgs.mkShell {
          name = "moon-down";

          packages = with pkgs; [
            # --- toolchain -------------------------------------------------------
            cargo
            rustc
            rustfmt
            clippy
            # cargo fetches the aria2-rust git dependency; without this in the
            # shell, `cargo build` cannot resolve it.
            git

            # --- optional `tray` feature ----------------------------------------
            # libayatana-appindicator is the StatusNotifierItem/AppIndicator
            # backend the tray icon renders through. gtk3 is its toolkit.
            gtk3
            libayatana-appindicator

            # --- runtime helpers the app shells out to -------------------------
            # Archive extraction for both .7z and .rar delegates to `7z`
            # (extract.rs resolves 7zz, then 7z; nixpkgs p7zip ships `7z`).
            p7zip

            # pkg-config shim for the tray feature (see appindicatorPc above).
            appindicatorPc
          ];

          nativeBuildInputs = with pkgs; [ pkg-config ];

          shellHook = ''
            export PKG_CONFIG_PATH="${appindicatorPc}/pkgconfig:''${PKG_CONFIG_PATH:-}"
            echo "moon-down dev shell"
            echo "  cargo test                    # default build, no GUI deps"
            echo "  cargo build --features tray   # adds the --tray status icon"
            echo "  7z=$(command -v 7z || echo missing)"
            echo "  appindicator=$(pkg-config --modversion libayatana-appindicator3-0.1 2>/dev/null || echo unresolved)"
          '';
        };

      shells = {
        x86_64-linux = mkShell nixpkgs.legacyPackages.x86_64-linux;
        aarch64-linux = mkShell nixpkgs.legacyPackages.aarch64-linux;
        darwin = mkShell nixpkgs.legacyPackages.x86_64-darwin;
      };
    in
    {
      # `nix develop` with no arguments resolves devShells.<system>.default, so
      # each system exposes itself as its own default.
      devShells = {
        x86_64-linux = { default = shells.x86_64-linux; };
        aarch64-linux = { default = shells.aarch64-linux; };
        darwin = { default = shells.darwin; };
      };
    };
}
