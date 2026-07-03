# smolsonic
[![Release](https://github.com/tsirysndr/smolsonic/actions/workflows/release.yml/badge.svg)](https://github.com/tsirysndr/smolsonic/actions/workflows/release.yml)

A tiny, self-contained music + video server written in Rust. Speaks both the
[Subsonic API](http://www.subsonic.org/pages/api.jsp) and a Jellyfin-compatible
API on a side port, so you can use either ecosystem of clients. Point it at a
folder of music (and optionally a folder of videos), give it a username and a
password in a TOML file, and any [Subsonic client](https://www.navidrome.org/apps/)
or Jellyfin-compatible client (Finamp, Findroid, Streamyfin, Amcfy, …) can
browse and stream your library.

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
- **Embedded S3 admin web UI** at `/admin/` of the S3 server — browse,
  upload, and delete objects from your browser, no extra service needed.
- **Optional Jellyfin-compatible sidecar API** on its own port. Works with Finamp, Findroid, Streamyfin,
  Symfonium, Amcfy Music, and other native Jellyfin clients.
- **Optional video library** scanned alongside music — direct-play streaming,
  ffmpeg-based thumbnail generation when no sibling poster exists.
- **Zeroconf / mDNS** announcement so clients on the LAN discover the
  server automatically (Subsonic, S3, and Jellyfin when enabled), plus the
  Jellyfin UDP 7359 client-discovery probe.

## Install

Install script (macOS / Linux, amd64 / aarch64, plus linux armhf):

```sh
curl -fsSL https://raw.githubusercontent.com/tsirysndr/smolsonic/main/install.sh | sh
```

Pin a version or change the install directory with env vars:

```sh
curl -fsSL https://raw.githubusercontent.com/tsirysndr/smolsonic/main/install.sh \
  | SMOLSONIC_VERSION=v0.7.0 SMOLSONIC_INSTALL=$HOME/.local/bin sh
```

Homebrew (macOS / Linux):

```sh
brew install tsirysndr/tap/smolsonic
```

Debian / Ubuntu (`.deb` for `amd64`, `arm64`, `armhf`):

```sh
# From the Gemfury apt repo
echo "deb [trusted=yes] https://apt.fury.io/tsiry/ /" \
  | sudo tee /etc/apt/sources.list.d/smolsonic.list
sudo apt-get update
sudo apt-get install smolsonic

# Or download a .deb directly from the release page
curl -fsSLO https://github.com/tsirysndr/smolsonic/releases/latest/download/smolsonic_0.7.0_amd64.deb
sudo dpkg -i smolsonic_0.7.0_amd64.deb
```

Fedora / RHEL (`.rpm` for `x86_64`):

```sh
# From the Gemfury yum repo
sudo tee /etc/yum.repos.d/smolsonic.repo <<'EOF'
[smolsonic]
name=smolsonic
baseurl=https://yum.fury.io/tsiry/
enabled=1
gpgcheck=0
EOF
sudo dnf install smolsonic

# Or download an .rpm directly from the release page
curl -fsSLO https://github.com/tsirysndr/smolsonic/releases/latest/download/smolsonic-0.7.0-1.x86_64.rpm
sudo rpm -i smolsonic-0.7.0-1.x86_64.rpm
```

The `.deb` / `.rpm` packages drop the binary at `/usr/local/bin/smolsonic`,
an example config at `/usr/share/smolsonic/smolsonic.example.toml`, and a
systemd user unit at `/usr/lib/systemd/user/smolsonic.service`. After
install:

```sh
# Config was seeded for you — edit music_dir, username, password
$EDITOR ~/.config/smolsonic/smolsonic.toml

systemctl --user enable --now smolsonic.service
systemctl --user status smolsonic.service
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

# Optional Jellyfin-compatible API on a side port. Omit the block to disable.
[jellyfin]
port        = 8096           # required — presence + port enables the sidecar
host        = "0.0.0.0"      # optional
server_name = "smolsonic"    # optional — name clients display

# Optional video library — scanned independently from music_dir.
[video]
video_dir          = "/path/to/your/videos"
scan_interval_secs = 300        # optional, default 300 (0 disables)
library_name       = "Movies"   # optional, shown to Jellyfin clients

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
| `[jellyfin]`    | Optional Jellyfin-compatible API on its own port.         |
| `[video]`       | Optional video library, surfaced through the Jellyfin API.|
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
`delimiter`), `HeadBucket`, `HeadObject`, `GetObject`, `PutObject`,
`DeleteObject`. Streaming (`STREAMING-AWS4-HMAC-SHA256-PAYLOAD`), unsigned,
and SHA-256-signed payloads are all accepted on uploads.

### S3 admin web UI

`smolsonic` also ships a React SPA embedded directly in the binary and
served at `/admin/` of the S3 server. Sign in with the `access_key` /
`secret_key` from your TOML config to browse the `music` bucket, upload
new files, and delete existing ones — all from the browser. Requests are
signed with AWS Signature V4 on the client side and hit the same S3
endpoints documented above.

![S3 admin web UI](.github/assets/s3admin.png)

Open it at:

```
http://<s3-host>:<s3-port>/admin/
```

### Jellyfin-compatible API

`smolsonic` ships an optional second HTTP server that speaks enough of the
[Jellyfin API](https://api.jellyfin.org/) to look like a real Jellyfin server
to native clients. It runs on a separate port from the Subsonic API and is
disabled unless `[jellyfin]` is set in the TOML.

```toml
[jellyfin]
port = 8096
```

What it implements:

- `/System/Info` / `/System/Info/Public` — spoofs `Version: 10.11.x` so
  modern clients accept it (they refuse anything older).
- `/Users/AuthenticateByName` — bridges to the existing `username`/`password`
  in your TOML, issues an opaque token persisted in SQLite. Tokens are accepted
  via `X-Emby-Token`, `Authorization: MediaBrowser Token=…`, and `?api_key=`
  for streaming URLs.
- `/Users/Public`, `/Users/{id}`, `/Users/{id}/Views`, `/UserViews`,
  `/Library/MediaFolders`, `/Library/VirtualFolders` — library/collection
  enumeration.
- `/Items`, `/Users/{id}/Items` — main browse endpoint. Handles `parentId`,
  `includeItemTypes` (`Audio`/`MusicAlbum`/`MusicArtist`/`Movie`), `searchTerm`,
  `albumArtistIds`/`artistIds`, `ids`, pagination. Accepts both camelCase and
  PascalCase parameter names, and repeated keys
  (`?includeItemTypes=Folder&includeItemTypes=Movie`).
- `/Items/{id}` (single-item lookup), `/Items/{id}/Images/Primary` (cover art),
  `/Items/{id}/File`, `/Items/{id}/PlaybackInfo`.
- `/Audio/{id}/stream`, `/Audio/{id}/stream.{ext}`, `/Audio/{id}/universal`
  — direct-play audio streaming, Range-aware.
- `/Videos/{id}/stream`, `/Videos/{id}/stream.{ext}` — direct-play video.
- `/Search/Hints` and `/Items?searchTerm=…` — backed by the same FTS5 index
  the Subsonic side uses, plus a LIKE search for videos.
- `/Sessions/Playing/{,Progress,Stopped}`, `/Sessions/Capabilities/Full` —
  scrobble + capability registration acks.
- `/ScheduledTasks/Running/{id}` and `/Library/Refresh` — both kick off a
  background music + video rescan and return 204 immediately.
- `/Shows/NextUp`, `/Shows/Upcoming`, `/UserItems/{Resume,Latest}`,
  `/Items/{Suggestions,Resume,Latest}` — stubbed for client compatibility.

Item IDs are deterministic 32-char SHA-256-prefixed UUIDs (dashed). The
mapping from these GUIDs back to your native `ar-…`/`al-…`/`so-…`/`vi-…`
IDs is stored in `jf_guids` so reverse lookups survive restarts.

#### Service discovery

Two discovery mechanisms run concurrently when the sidecar is enabled:

- `_jellyfin._tcp.local.` mDNS broadcast with a `ID=` TXT record matching
  the server's stable GUID.
- A UDP listener on **port 7359** that replies with
  `{"Address":"http://<lan-ip>:<port>","Id":"…","Name":"…"}` to the literal
  Jellyfin client-discovery probe `"Who is JellyfinServer?"`.

#### Tested clients

| Client            | Type          | Status                                                          |
| ----------------- | ------------- | --------------------------------------------------------------- |
| Finamp            | Android/iOS   | Native music client — full browse + play + scrobble.            |
| Amcfy Music       | Android       | Full browse + play. Triggers library scans on refresh.          |
| Symfonium         | Android       | Music + video, paid; works against the Jellyfin API.            |
| Findroid          | Android       | Video-only by design (filters out music libraries client-side). |
| Streamyfin        | Android       | Movies + audio; works.                                          |
| Official Android  | Android       | Does NOT work: it's a WebView wrapping the Jellyfin web UI,     |
|                   |               | which smolsonic doesn't ship. Use one of the native clients.    |

For music, use **Finamp** (or Amcfy / Symfonium). For video, use
**Findroid** (or Streamyfin).

#### Video library

When `[video]` is configured, smolsonic walks `video_dir` for `.mkv`, `.mp4`,
`.webm`, `.mov`, `.avi`, `.m4v` and exposes them as Jellyfin movies. Title is
the cleaned filename stem. If `ffprobe` is on `PATH` it's used to extract
duration / bitrate / width / height; otherwise those fields default to 0.

Posters resolve in this order:

1. Sibling file with the same stem: `movie.{jpg,png,webp}`
2. `poster.{ext}` / `folder.{ext}` / `cover.{ext}` in the same directory
3. If neither exists and `ffmpeg` is on `PATH`, smolsonic auto-extracts a
   frame at ~10% into the video and caches it under `covers/{video_id}.jpg`.

Playback is direct-play only — clients must support the container natively.

### Zeroconf / mDNS

`smolsonic` announces itself on the local network so clients can discover the
server without hard-coding an IP. Three service types are advertised:

- `_subsonic._tcp.local.` — the Subsonic HTTP server, on `port`.
- `_s3._tcp.local.` — the S3 gateway, on `[s3] port`. Only broadcast when
  `[s3] enabled = true`.
- `_jellyfin._tcp.local.` — the Jellyfin sidecar, on `[jellyfin] port`. Only
  broadcast when `[jellyfin]` is configured. The TXT record includes the
  server's stable Jellyfin `Id`.

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

### Search backends

Free-text search (`/rest/search3`, `/rest/search2`, Jellyfin `?searchTerm=`)
defaults to SQLite's built-in FTS5 index. Add a `[typesense]` block to swap
in [Typesense](https://typesense.org/) for typo-tolerant search and better
ranking without changing any client:

```toml
[typesense]
url = "http://localhost:8108"
api_key = "changeme"
# collection_prefix = "smolsonic"   # optional
```

Boot a local Typesense in Docker:

```sh
docker run -d --name typesense -p 8108:8108 -v typesense-data:/data \
  typesense/typesense:27.1 --data-dir /data --api-key changeme
```

On startup smolsonic creates the three collections (`{prefix}_songs`,
`{prefix}_albums`, `{prefix}_artists`) and — if the songs collection is
empty — bulk-imports every row from SQLite. From then on, scanner and
watcher mirror inserts/updates/deletes into Typesense automatically.
If Typesense is unreachable or misbehaves, search transparently falls back
to FTS5 so queries keep working. Remove the `[typesense]` block and restart
to go back to FTS5-only.

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

**Subsonic side** — anything that speaks Subsonic API 1.16.x and prefers
tag-based browsing should work, including Substreamer, play:sub, Symfonium,
Tempo, and Sonixd.

**Jellyfin side** — see the table in the Jellyfin section above. Native
clients (Finamp, Findroid, Streamyfin, Amcfy, Symfonium) work. The official
Jellyfin Android app is a WebView wrapper and is not supported.

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
  models.rs          Artist / Album / Song / Video row types
  scanner.rs         walkdir + lofty + cover art extraction (music)
  video_scanner.rs   walkdir + ffprobe/ffmpeg thumbnail extraction (video)
  watcher.rs         notify-based incremental library sync
  mdns.rs            Zeroconf/mDNS service broadcast
  server/
    mod.rs           actix App + routing
    auth.rs          Subsonic token / plaintext auth
    response.rs      JSON envelope helpers
    repo.rs          sqlx queries (shared by Subsonic and Jellyfin)
    handlers.rs      Subsonic endpoint handlers
  s3/
    mod.rs           actix App + routing for the S3 gateway
    sigv4.rs         AWS Signature V4 verification + chunked-stream decode
    handlers.rs      ListBuckets / ListObjectsV2 / Get / Put / Delete / Head
    admin.rs         rust-embed handler for the /admin/ SPA
  jellyfin/
    mod.rs           actix App + route table for the Jellyfin sidecar
    auth.rs          X-Emby-Authorization parsing + token store
    dto.rs           Spec-aligned response DTOs
    mapping.rs       Native ID ↔ Jellyfin GUID translation
    handlers.rs      All Jellyfin endpoint handlers
    discovery.rs     UDP 7359 client-discovery listener
s3webui/             React + Vite admin SPA (built and embedded at release)
```

## License

MIT.
