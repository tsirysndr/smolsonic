module CI.Build (buildArgs, linuxRelease) where

import Prelude
import CI.Package as Package
import Dagger (Client)
import Dagger.Container as Container
import Dagger.Directory as Directory
import Dagger.Modules as Module
import Dagger.Query as Query
import Dagger.Types (Container, Directory, Platform(..))
import Data.Maybe (Maybe(..))
import Effect.Aff (Aff, throwError)
import Effect.Exception (error)
import Data.String (trim)

type BuildArgs = { source :: Directory, target :: String, version :: String }

buildArgs :: Module.Arguments BuildArgs
buildArgs = ado
  source <- Module.argument "source"
  target <- Module.argument "target"
  version <- Module.argument "version"
  in { source, target, version }

withSource :: Directory -> Container -> Container
withSource source = Container.withDirectoryWith "/src" source
  (Container.defaultWithDirectoryOpts
    { exclude = Just [ ".git", ".dagger", ".source", ".fluentci", "target", "**/node_modules", "**/.env", "**/.env.*", "result", "result-*", "dist/*.tar.gz*", "dist/*.deb*", "dist/*.rpm*", "dist/staging", "dist/rpmbuild", "*.db*", "covers", "smolsonic.toml" ] })

withCargoCaches :: Client -> String -> Container -> Container
withCargoCaches client target container = container
  # Container.withEnvVariable "CARGO_HOME" "/cargo"
  # Container.withMountedCache "/cargo/registry" (Query.cacheVolume "smolsonic-cargo-registry" client)
  # Container.withMountedCache "/cargo/git" (Query.cacheVolume "smolsonic-cargo-git" client)
  # Container.withMountedCache "/src/target" (Query.cacheVolume ("smolsonic-target-" <> target) client)

web :: Client -> Directory -> Container
web client source = Query.container client
  # Container.from "node:24-bookworm-slim"
  # Container.withFile "/usr/local/bin/bun" (Query.container client # Container.from "oven/bun:1.4.2" # Container.file "/usr/local/bin/bun")
  # Container.withMountedCache "/root/.bun/install/cache" (Query.cacheVolume "smolsonic-bun" client)
  # Container.withDirectoryWith "/web" (Directory.directory "s3webui" source)
      (Container.defaultWithDirectoryOpts { exclude = Just [ "node_modules", "dist", ".env", ".env.*" ] })
  # Container.withWorkdir "/web"
  # Container.withExec [ "bun", "install", "--frozen-lockfile" ]
  # Container.withExec [ "bun", "run", "build" ]

linuxRelease :: Client -> BuildArgs -> Aff Directory
linuxRelease client args = do
  label <- case args.target of
    "x86_64-unknown-linux-gnu" -> pure "linux-amd64"
    "aarch64-unknown-linux-gnu" -> pure "linux-aarch64"
    "armv7-unknown-linux-gnueabihf" -> pure "linux-armhf"
    _ -> throwError (error "Unsupported Linux release target")
  let installed = Query.containerWith
        (Query.defaultContainerOpts { platform = Just (Platform "linux/amd64") }) client
        # Container.from "rust:bookworm"
        # Container.withExec [ "rustup", "target", "add", args.target ]
  sysroot <- trim <$> Container.stdout (installed # Container.withExec [ "rustc", "--print", "sysroot" ])
  let
    toolchain = installed # Container.withExec [ "cp", "-a", sysroot, "/toolchain" ]
    -- Reuse Cross.toml's compiler/sysroot images without nesting Docker.
    -- Direct toolchain binaries avoid depending on the rustup shim's host libc.
    environment = if args.target == "x86_64-unknown-linux-gnu" then toolchain
      else Query.containerWith (Query.defaultContainerOpts { platform = Just (Platform "linux/amd64") }) client
        # Container.from ("ghcr.io/cross-rs/" <> args.target <> ":0.2.5")
        # Container.withDirectory "/toolchain" (Container.directory "/toolchain" toolchain)
        # Container.withEnvVariable "PATH" "/toolchain/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
    built = environment
      # withSource args.source
      # Container.withDirectory "/src/s3webui/dist" (Container.directory "/web/dist" (web client args.source))
      # Container.withWorkdir "/src"
      # withCargoCaches client args.target
      # Container.withExec [ "cargo", "build", "--release", "--locked", "--target", args.target, "--bin", "smolsonic" ]
      -- Cache mounts cannot be exported directly: materialize the executable.
      # Container.withExec [ "mkdir", "-p", "/binary" ]
      # Container.withExec [ "cp", "target/" <> args.target <> "/release/smolsonic", "/binary/smolsonic" ]
  Package.packageRelease client
    { source: args.source
    , binary: Container.file "/binary/smolsonic" built
    , target: args.target
    , label
    , version: args.version
    }
