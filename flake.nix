{
  description = "smolsonic - A tiny Subsonic-compatible music server";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

    crane = {
      url = "github:ipetkov/crane";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
      inputs.rust-analyzer-src.follows = "";
    };

    flake-utils.url = "github:numtide/flake-utils";

    advisory-db = {
      url = "github:rustsec/advisory-db";
      flake = false;
    };
  };

  outputs = { self, nixpkgs, crane, fenix, flake-utils, advisory-db, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
        };

        inherit (pkgs) lib;

        craneLib = crane.mkLib pkgs;

        src = craneLib.cleanCargoSource ./.;

        # node_modules for the s3webui SPA. `bun install` needs the network,
        # so dependency resolution lives in a fixed-output derivation.
        #
        # The tree is NOT platform-independent: native optional deps (e.g.
        # @tailwindcss/oxide-*) are installed per-OS/CPU, so each system
        # needs its own hash.
        #
        # Updating: change package.json/bun.lock, set the current system's
        # hash below to lib.fakeHash, run `nix build`, copy the hash Nix
        # reports on mismatch back in. Repeat per platform.
        s3webuiNodeModules = pkgs.stdenv.mkDerivation {
          pname = "smolsonic-s3webui-node-modules";
          version = "0.9.0";

          src = lib.fileset.toSource {
            root = ./s3webui;
            fileset = lib.fileset.unions [
              ./s3webui/package.json
              ./s3webui/bun.lock
            ];
          };

          nativeBuildInputs = [ pkgs.bun ];

          dontConfigure = true;

          buildPhase = ''
            runHook preBuild
            export HOME=$(mktemp -d)
            bun install --frozen-lockfile --no-progress
            runHook postBuild
          '';

          installPhase = ''
            runHook preInstall
            mv node_modules $out
            runHook postInstall
          '';

          dontFixup = true;

          outputHashMode = "recursive";
          outputHashAlgo = "sha256";
          outputHash = {
            x86_64-linux = "sha256-yKUGN1F8I1S6M5GgXeVA5y/u2gm8vYjyKxGLNO1nB90=";
            aarch64-linux = lib.fakeHash;
            aarch64-darwin = "sha256-VuAM4wkTAL8kIE0KseAC79F4+qLbLzR4g3AWiQjwaT0=";
            x86_64-darwin = lib.fakeHash;
          }.${system};
        };

        # Build the React SPA. The resulting `dist/` is embedded into the
        # smolsonic binary at compile time via rust-embed.
        s3webui = pkgs.stdenv.mkDerivation {
          pname = "smolsonic-s3webui";
          version = "0.9.0";

          src = ./s3webui;

          nativeBuildInputs = [ pkgs.bun pkgs.nodejs ];

          configurePhase = ''
            runHook preConfigure
            cp -r ${s3webuiNodeModules} node_modules
            chmod -R u+w node_modules
            patchShebangs node_modules
            export HOME=$(mktemp -d)
            runHook postConfigure
          '';

          buildPhase = ''
            runHook preBuild
            bun run build
            runHook postBuild
          '';

          installPhase = ''
            runHook preInstall
            cp -r dist $out
            runHook postInstall
          '';
        };

        # Common arguments can be set here to avoid repeating them later
        commonArgs = {
          inherit src;

          pname = "smolsonic";
          version = "0.9.0";

          nativeBuildInputs = [
            pkgs.pkg-config
          ];

          buildInputs = [
            pkgs.openssl
            pkgs.openssl.dev
          ] ++ lib.optionals pkgs.stdenv.isDarwin [
            pkgs.libiconv
          ];

          # rust-embed reads s3webui/dist at compile time. cleanCargoSource
          # strips the s3webui directory, so we drop the pre-built SPA back
          # in before cargo runs.
          preBuild = ''
            mkdir -p s3webui
            cp -r ${s3webui} s3webui/dist
            chmod -R u+w s3webui/dist
          '';
        };

        craneLibLLvmTools = craneLib.overrideToolchain
          (fenix.packages.${system}.complete.withComponents [
            "cargo"
            "llvm-tools"
            "rustc"
          ]);

        # Build *just* the cargo dependencies, so we can reuse
        # all of that work (e.g. via cachix) when running in CI
        cargoArtifacts = craneLib.buildDepsOnly commonArgs;

        # Build the actual crate itself, reusing the dependency
        # artifacts from above.
        smolsonic = craneLib.buildPackage (commonArgs // {
          inherit cargoArtifacts;
        });

      in
      {
        checks = {
          # Build the crate as part of `nix flake check` for convenience
          inherit smolsonic;

          # Run clippy (and deny all warnings) on the crate source,
          # again, reusing the dependency artifacts from above.
          smolsonic-clippy = craneLib.cargoClippy (commonArgs // {
            inherit cargoArtifacts;
            cargoClippyExtraArgs = "--all-targets -- --deny warnings";
          });

          smolsonic-doc = craneLib.cargoDoc (commonArgs // {
            inherit cargoArtifacts;
          });

          # Check formatting
          smolsonic-fmt = craneLib.cargoFmt {
            inherit src;
          };

          # Audit dependencies
          smolsonic-audit = craneLib.cargoAudit {
            inherit src advisory-db;
          };

          # Run tests with cargo-nextest
          smolsonic-nextest = craneLib.cargoNextest (commonArgs // {
            inherit cargoArtifacts;
            partitions = 1;
            partitionType = "count";
          });
        } // lib.optionalAttrs (system == "x86_64-linux") {
          # NB: cargo-tarpaulin only supports x86_64 systems
          smolsonic-coverage = craneLib.cargoTarpaulin (commonArgs // {
            inherit cargoArtifacts;
          });
        };

        packages = {
          default = smolsonic;
          smolsonic-llvm-coverage = craneLibLLvmTools.cargoLlvmCov (commonArgs // {
            inherit cargoArtifacts;
          });
        };

        apps.default = flake-utils.lib.mkApp {
          drv = smolsonic;
        };

        devShells.default = pkgs.mkShell {
          inputsFrom = builtins.attrValues self.checks.${system};

          nativeBuildInputs = with pkgs; [
            cargo
            rustc
          ];
        };
      });
}
