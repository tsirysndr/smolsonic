{
  lib,
  stdenv,
  rustPlatform,
  fetchFromGitHub,
  bun,
  nodejs,
  nix-update-script,
}:

let
  version = "0.10.0";

  src = fetchFromGitHub {
    owner = "tsirysndr";
    repo = "smolsonic";
    tag = "v${version}";
    hash = "sha256-RzvhjLfJHVOTnwCEdSzZz/JoGU4HeGy813G1DUDGi8Q=";
  };

  s3webuiNodeModules = stdenv.mkDerivation {
    pname = "smolsonic-s3webui-node-modules";
    inherit version src;

    sourceRoot = "${src.name}/s3webui";

    nativeBuildInputs = [ bun ];

    dontConfigure = true;

    buildPhase = ''
      runHook preBuild
      export HOME=$(mktemp -d)
      bun install --frozen-lockfile --no-progress --ignore-scripts
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
    outputHash =
      {
        x86_64-linux = lib.fakeHash;
        aarch64-linux = lib.fakeHash;
        aarch64-darwin = lib.fakeHash;
        x86_64-darwin = lib.fakeHash;
      }
      .${stdenv.hostPlatform.system}
        or (throw "smolsonic: unsupported system ${stdenv.hostPlatform.system}");
  };

  s3webui = stdenv.mkDerivation {
    pname = "smolsonic-s3webui";
    inherit version src;

    sourceRoot = "${src.name}/s3webui";

    nativeBuildInputs = [
      bun
      nodejs
    ];

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
in
rustPlatform.buildRustPackage {
  pname = "smolsonic";
  inherit version src;

  cargoHash = lib.fakeHash;

  preBuild = ''
    cp -r ${s3webui} s3webui/dist
    chmod -R u+w s3webui/dist
  '';

  passthru.updateScript = nix-update-script { };

  meta = {
    description = "Tiny self-hosted music and video server speaking the Subsonic and Jellyfin APIs";
    longDescription = ''
      smolsonic is a single-binary music and video server. Point it at a folder
      of media, set a username and password in one TOML file, and any Subsonic
      or Jellyfin-compatible client can browse and stream your library. It
      stores its index in SQLite and needs no external services. Optional
      extras include an S3-compatible upload API with a web admin UI, UPnP/DLNA
      discovery, mDNS/Zeroconf announcement, and ListenBrainz scrobbling.
    '';
    homepage = "https://github.com/tsirysndr/smolsonic";
    changelog = "https://github.com/tsirysndr/smolsonic/releases/tag/v${version}";
    license = lib.licenses.mit;
    maintainers = with lib.maintainers; [ ];
    mainProgram = "smolsonic";
    platforms = lib.platforms.unix;
  };
}
