# smolsonic
[![Release](https://github.com/tsirysndr/smolsonic/actions/workflows/release.yml/badge.svg)](https://github.com/tsirysndr/smolsonic/actions/workflows/release.yml)

A tiny, self-contained [Subsonic](http://www.subsonic.org/pages/api.jsp)-compatible
music server written in Rust. Point it at a folder of music, give it a username
and a password in a TOML file, and any [Subsonic client](https://www.navidrome.org/apps/) can browse and stream
your library.

```
                     _                  _
 ___ _ __ ___   ___ | |___  ___  _ __  (_) ___
/ __| '_ ` _ \ / _ \| / __|/ _ \| '_ \ | |/ __|
\__ \ | | | | | (_) | \__ \ (_) | | | || | (__
|___/_| |_| |_|\___/|_|___/\___/|_| |_||_|\___|
                a tiny Subsonic server in Rust
```

## Features

- One binary, one TOML file, one SQLite database. No external services.
- Built on **actix-web 4** with **sqlx** (SQLite) for storage.
- Library scanner powered by **lofty** — extracts ID3/Vorbis/MP4/etc. tags and
  embedded cover art from `mp3`, `flac`, `ogg`, `opus`, `m4a`, `wav`, and more.
- Falls back to `cover.jpg` / `folder.jpg` / `front.jpg` next to the audio file
  if there's no embedded picture.
- Stable IDs (`ar-…` / `al-…` / `so-…`) derived from tag content, so re-scans
  are idempotent and clients don't lose their bookmarks.
- HTTP `Range` support for proper seeking.
- Subsonic **token auth** (`t = md5(password + salt)`) and plaintext (`p=…`
  or `p=enc:<hex>`) both supported.
- CORS is permissive — works directly from web clients.
- Optional **S3-compatible API** for uploading and deleting files in your
  library with any S3 client (`aws`, `mc`, `boto3`, `rclone`, …).
- **Zeroconf / mDNS** announcement so clients on the LAN discover the
  server automatically (Subsonic, plus the S3 endpoint when enabled).

## Install

Homebrew (macOS / Linux):

```sh
brew install tsirysndr/tap/smolsonic
```

Nix flake:

```sh
# Run without installing
nix run github:tsirysndr/smolsonic -- --config smolsonic.toml

# Install into your profile
nix profile install github:tsirysndr/smolsonic

# Or drop into a dev shell with cargo + deps
nix develop github:tsirysndr/smolsonic
```

Or build from source:

```sh
cargo build --release
```

## Quick start

```sh
# 1. Create a config
cp smolsonic.example.toml smolsonic.toml
$EDITOR smolsonic.toml      # set music_dir, username, password

# 2. Run
smolsonic --config smolsonic.toml
```

On first launch smolsonic scans `music_dir`, creates the SQLite database, and
starts the HTTP server. Point any Subsonic client at
`http://<host>:<port>/rest/…` using the credentials from your TOML file.

## Configuration

`smolsonic.toml` (all keys shown):

```toml
music_dir     = "/path/to/your/music"   # required
username      = "admin"                  # required
password      = "changeme"               # required

# Optional — defaults shown
port          = 4533
host          = "0.0.0.0"
database_path = "smolsonic.db"
covers_dir    = "covers"

# Optional S3-compatible upload API. Bucket name is fixed to "music",
# region is "us-east-1".
[s3]
enabled    = true
host       = "0.0.0.0"
port       = 9000
access_key = "smolsonic"
secret_key = "changeme-please"

# Optional Zeroconf/mDNS service broadcast. Enabled by default.
[mdns]
enabled       = true
instance_name = "smolsonic"
```

| Key             | Purpose                                                   |
| --------------- | --------------------------------------------------------- |
| `music_dir`     | Root of your library. Walked recursively.                 |
| `username`      | The single Subsonic user.                                 |
| `password`      | Cleartext on disk; used for both token and plaintext auth.|
| `port`          | TCP port to bind.                                         |
| `host`          | Interface to bind (use `127.0.0.1` to keep it local).     |
| `database_path` | Path to the SQLite file. Created if missing.              |
| `covers_dir`    | Where extracted album art is cached.                      |
| `[s3]`          | Optional S3 server section (see below).                   |
| `[mdns]`        | Optional Zeroconf/mDNS broadcast (see below).             |

### S3-compatible API

`smolsonic` ships an embedded S3 gateway whose objects map 1:1 to files under
`music_dir`. Uploads land on disk, then the built-in filesystem watcher picks
them up and rescans them into the library automatically. Deletes work the same
way — removing an object removes it from the library on the next debounce.

The bucket is always `music` and the region is always `us-east-1` — they're
not exposed in the config. The endpoint URL is whatever you bind in
`[s3]`. Authentication uses AWS Signature V4 with the `access_key` and
`secret_key` you set in the TOML file.

Example with the MinIO client:

```sh
mc alias set smol http://localhost:9000 smolsonic changeme-please --api S3v4
mc cp track.flac smol/music/Artist/Album/track.flac
mc ls smol/music/
mc rm smol/music/Artist/Album/track.flac
```

Or with `aws-cli`:

```sh
aws --endpoint-url http://localhost:9000 \
    s3 cp track.flac s3://music/Artist/Album/track.flac
```

| Key          | Purpose                                                       |
| ------------ | ------------------------------------------------------------- |
| `enabled`    | Toggle the S3 server. Default `true` when the section exists. |
| `host`       | Interface to bind for S3. Default `0.0.0.0`.                  |
| `port`       | TCP port for the S3 server. Default `9000`.                   |
| `access_key` | Required. The S3 access key clients must present.             |
| `secret_key` | Required. The S3 secret key used to verify Sig V4 signatures. |

Supported operations: `ListBuckets`, `ListObjectsV2` (with `prefix` and
`delimiter`), `HeadObject`, `GetObject`, `PutObject`, `DeleteObject`. Streaming
(`STREAMING-AWS4-HMAC-SHA256-PAYLOAD`), unsigned, and SHA-256-signed payloads
are all accepted on uploads.

### Zeroconf / mDNS

`smolsonic` announces itself on the local network so clients can discover the
server without hard-coding an IP. Two service types are advertised:

- `_subsonic._tcp.local.` — the Subsonic HTTP server, on `port`.
- `_s3._tcp.local.` — the S3 gateway, on `[s3] port`. Only broadcast when
  `[s3] enabled = true`.

Only real LAN IPv4 addresses are broadcast. Loopback, link-local,
the Docker default bridge (`172.17.0.0/16`), and virtual interfaces
(`docker*`, `br-*`, `veth*`, `vboxnet*`, `vmnet*`, `virbr*`, `tun*`, `tap*`,
`utun*`, `wg*`, `tailscale*`, `awdl*`, `bridge*`, …) are filtered out.

| Key             | Purpose                                              |
| --------------- | ---------------------------------------------------- |
| `enabled`       | Toggle mDNS broadcast. Default `true`.               |
| `instance_name` | Service instance name. Default `smolsonic`.          |

Discover with `dns-sd` (macOS) or `avahi-browse` (Linux):

```sh
dns-sd -B _subsonic._tcp
avahi-browse -r _subsonic._tcp
```

## CLI

```
Usage: smolsonic [OPTIONS]

Options:
  -c, --config <CONFIG>  Path to the TOML config file [default: smolsonic.toml]
      --no-scan          Skip the startup library scan
  -h, --help             Print help
  -V, --version          Print version
```

Trigger a rescan from a running server with the standard Subsonic endpoint:

```
GET /rest/startScan.view?u=…&t=…&s=…
GET /rest/getScanStatus.view?u=…&t=…&s=…
```

## Supported endpoints

Full navidrome-style endpoint coverage. Both `.view`-suffixed and plain paths
are accepted, on both `GET` and `POST`. Responses are JSON with the Subsonic
envelope (`{"subsonic-response": …}`).

**System** — `ping`, `getUser`, `getMusicFolders`, `getScanStatus`, `startScan`

**Library (ID3 tag browsing)** — `getArtists`, `getArtist`, `getAlbum`,
`getSong`, `getAlbumList2`, `getAlbumList` (alias)

**Library (folder browsing)** — `getIndexes`, `getMusicDirectory`

**Genres** — `getGenres`, `getSongsByGenre`

**Lists** — `getRandomSongs`, `getStarred2`, `getStarred` (alias)

**Playback** — `stream`, `download`, `getCoverArt`, `scrobble`,
`getNowPlaying`, `updateNowPlaying`

**Search** — `search3`, `search2` (alias)

**Playlists** — `getPlaylists`, `getPlaylist`, `createPlaylist`,
`updatePlaylist`, `deletePlaylist`

**Starring** — `star`, `unstar`

**Artist / album info** — `getArtistInfo`, `getArtistInfo2`, `getAlbumInfo`,
`getAlbumInfo2`, `getSimilarSongs`, `getSimilarSongs2`, `getTopSongs`,
`getLyrics`. These return minimal stub shapes (no Last.fm or external lookups).

`GET /` returns a plain-text index of every endpoint and its query params.

## Tested clients

Anything that speaks Subsonic API 1.16.x and prefers tag-based browsing should
work, including Substreamer, play:sub, Symfonium, Tempo, and Sonixd.

## How auth works

Subsonic clients send credentials as query parameters:

- **Token auth** (preferred): `u=<user>&t=<token>&s=<salt>` where
  `token = md5(password + salt)`.
- **Plaintext**: `u=<user>&p=<password>` or `p=enc:<hex-encoded password>`.

`smolsonic` accepts both. The single user/password come from the TOML config.
There's no user database.

## Development

```sh
cargo run -- --config smolsonic.toml
RUST_LOG=debug cargo run -- --config smolsonic.toml
```

Project layout:

```
src/
  main.rs            entry point
  cli.rs             clap CLI + neon styling + ASCII banner
  config.rs          TOML loader
  db.rs              SqlitePool + schema migrations
  models.rs          Artist / Album / Song row types
  scanner.rs         walkdir + lofty + cover art extraction
  watcher.rs         notify-based incremental library sync
  mdns.rs            Zeroconf/mDNS service broadcast
  server/
    mod.rs           actix App + routing
    auth.rs          Subsonic token / plaintext auth
    response.rs      JSON envelope helpers
    repo.rs          sqlx queries
    handlers.rs      Subsonic endpoint handlers
  s3/
    mod.rs           actix App + routing for the S3 gateway
    sigv4.rs         AWS Signature V4 verification + chunked-stream decode
    handlers.rs      ListBuckets / ListObjectsV2 / Get / Put / Delete / Head
```

## License

MIT.
