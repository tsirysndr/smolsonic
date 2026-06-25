# s3webui

The admin web UI for [smolsonic](../README.md)'s embedded S3 gateway. A React
SPA that lets you browse the `music` bucket, upload tracks, and delete objects
straight from the browser — no extra service needed.

The built bundle is embedded into the `smolsonic` Rust binary via
[`rust-embed`](https://docs.rs/rust-embed) and served at `/admin/` of the S3
server (see [`src/s3/admin.rs`](../src/s3/admin.rs)).

## How it works

- Talks to the S3 endpoint **directly** from the browser using the AWS SDK v3
  (`@aws-sdk/client-s3`). There is no separate JSON API — every action is a
  SigV4-signed S3 request.
- Sign in with the `access_key` / `secret_key` from your `smolsonic.toml`.
  Credentials are kept in memory (Jotai) and never sent anywhere except as
  SigV4 signatures on outgoing S3 calls.
- Routing via [`@tanstack/react-router`](https://tanstack.com/router) with
  file-based routes under `src/routes/`. Styling is Tailwind v4 + FlyonUI.

## Stack

- React 19 + TypeScript + Vite
- TanStack Router (file-based) + TanStack Query
- Jotai for client state (auth session, UI toggles)
- Tailwind CSS v4 + FlyonUI + Tabler Icons
- AWS SDK v3 (`@aws-sdk/client-s3`, `@aws-sdk/s3-request-presigner`)
- Oxlint

## Layout

```
src/
  main.tsx              app entry
  routes/               file-based routes (TanStack Router)
    __root.tsx
    _app.tsx            authed shell
    _app.index.tsx      bucket browser
    _app.browser.tsx
    _app.upload.tsx     drag-and-drop multi-file upload
    _app.settings.tsx
    login.tsx           access-key / secret-key sign-in
  components/           Sidebar, Topbar
  atoms/                Jotai atoms (auth, ui)
  lib/
    s3.ts               AWS SDK client + helpers
    format.ts
  index.css
```

## Development

Run a `smolsonic` instance with the S3 server enabled, then start the dev
server:

```sh
bun install
bun run dev
```

Vite serves the SPA on `http://localhost:5173/admin/` and proxies `/music` to
`http://localhost:9000` (override with `VITE_API_TARGET`). The proxy is
configured with `changeOrigin: false` on purpose — the AWS SDK signs each
request with the original `Host` header, so rewriting it would break the SigV4
signature. See [`vite.config.ts`](./vite.config.ts).

Sign in with the same `access_key` / `secret_key` you set under `[s3]` in
`smolsonic.toml`.

## Build

```sh
bun run build
```

`build` runs `tsr generate` (route tree), `tsc -b` (typecheck), then
`vite build`. Output lands in `s3webui/dist/`, which the Rust crate embeds at
compile time — so any change here ships with the next `cargo build` of
`smolsonic`.

## Scripts

| Script           | What it does                                       |
| ---------------- | -------------------------------------------------- |
| `bun run dev`    | Vite dev server with HMR + S3 proxy.               |
| `bun run build`  | Generate route tree, typecheck, build static SPA.  |
| `bun run preview`| Serve the production build locally.                |
| `bun run lint`   | Run Oxlint.                                        |

## License

MIT, same as the parent project.
