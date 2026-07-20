# smolsonic

[![npm](https://img.shields.io/npm/v/smolsonic.svg)](https://www.npmjs.com/package/smolsonic)

A tiny, self-contained music + video server written in Rust. Speaks both the
[Subsonic API](http://www.subsonic.org/pages/api.jsp) and a Jellyfin-compatible
API on a side port, so you can use either ecosystem of clients.

This package distributes the prebuilt `smolsonic` binary via npm. On install it
downloads the release build matching your platform from
[GitHub Releases](https://github.com/tsirysndr/smolsonic/releases) and verifies
its SHA-256 checksum.

## Run without installing

```sh
npx smolsonic --config smolsonic.toml
```

## Install globally

```sh
npm install -g smolsonic
# or
bun add -g smolsonic

smolsonic --help
```

## Supported platforms

| OS      | x64 | arm64 |
| ------- | :-: | :---: |
| macOS   |  ✅ |  ✅   |
| Linux   |  ✅ |  ✅   |
| FreeBSD |  ✅ |  ✅   |
| NetBSD  |  ✅ |  ✅   |
| OpenBSD |  ✅ |  —    |

On any other platform, [build from source](https://github.com/tsirysndr/smolsonic)
with `cargo build --release`.

## Notes

- Set `SMOLSONIC_SKIP_DOWNLOAD=1` to skip the download during install (the
  binary is then fetched on first run).
- Extraction uses the system `tar`, which is present on all supported targets.

Full documentation: <https://github.com/tsirysndr/smolsonic>

## License

MIT
