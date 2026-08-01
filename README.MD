# telegram-loader

A Telegram bot that saves every photo, video and voice message it receives to a
folder on disk, and a web UI for that same folder: a paginated gallery with
image and video playback, downloads, and uploads back into the folder.

## Layout

A Cargo workspace with three crates:

```
telegram-loader/
├── Cargo.toml           # workspace
├── crates/
│   ├── backend/         # Axum web server + the Telegram bot (bin: telegram-loader)
│   ├── frontend/        # Leptos (CSR) app, built by trunk
│   └── shared/          # types on the wire, used by both
├── assets/              # styles.css, picked up by trunk
├── migrations/          # placeholder — there is no database yet
└── dist/web/            # trunk output, served by the backend
```

`shared` holds the JSON types (`MediaItem`, `MediaPage`, `UploadResponse`) plus
the extension→category mapping, so the backend and the UI cannot disagree about
what a "video" is. `frontend` compiles to wasm only, which is why the workspace
`default-members` excludes it — a plain `cargo build` builds just the server.

## What the bot saves

| Message | Saved as |
| --- | --- |
| Photo | largest available size only |
| Video, video note, animation (GIF) | the file as sent |
| Voice, audio | the file as sent |
| Document | the file as sent (photos/videos sent "as a file") |

Files are named `{kind}_{file_unique_id}.{ext}`, e.g. `voice_AgADXwEAAqcyIUs.oga`.
The extension comes from Telegram's own file path. Because `file_unique_id` is
stable, sending the same file twice is detected and skipped instead of
re-downloaded. Downloads land in a `.part` file and are renamed on success, so an
interrupted transfer never leaves a truncated file behind. Uploads from the web
UI use the same `.part` dance and keep their original name.

## Bot commands

| Command | Reply |
| --- | --- |
| `/webpage` | the URL of the web UI on the local network, e.g. `http://192.168.1.73:8080` |
| `/help`, `/start` | what the bot does, and this list |

The commands are registered with Telegram at startup, so they show up in the
client's `/` menu.

`/webpage` works out the address it hands out like this:

1. `WEB_URL`, if set — the only reliable answer behind a reverse proxy or in a
   container, where the address the server sees is not the address a phone can
   open;
2. otherwise the port from `BIND_ADDR`, plus the host from `BIND_ADDR` when that
   is a real address;
3. and when `BIND_ADDR` is the `0.0.0.0` wildcard, the address of the interface
   holding the default route — the machine's own LAN address. This is looked up
   from the routing table; nothing is sent anywhere.

**In Docker, set `WEB_URL`.** The container's own address (`172.17.x.x`) is what
step 3 finds, and that is not reachable from a phone:

```bash
docker run ... -e WEB_URL=http://192.168.1.73:8080 telegram-loader
```

## The web UI

`http://<host>:8080` serves a gallery of everything in `SAVE_DIR`:

- paginated grid (12/24/48/96 per page), newest first by default
- filter by images / video / audio / other files, search by file name, and sort
  by date, name or size
- click a card to open it fullscreen — images, playing video, playing audio —
  with `←`/`→` to move through the page and `Esc` to close
- download any file, individually, from the card or the viewer
- upload by dropping files on the page or picking them; new files never
  overwrite an existing name, they get a `_1`, `_2` … suffix

The filter, page and search live in the URL, so a view can be bookmarked or
shared.

### API

The UI is a client of these endpoints; they are also usable on their own.

| Endpoint | Purpose |
| --- | --- |
| `GET /api/media?page=&per_page=&kind=&sort=&q=` | one page of the listing, as JSON |
| `GET /api/media/{name}` | streams the file inline; supports range requests, so video seeks |
| `GET /api/media/{name}?download=1` | same file as an attachment |
| `POST /api/media` | multipart upload of one or more files |
| `GET /api/health` | liveness check |

`{name}` is a plain file name. Anything with a path separator, a leading dot or
a `..` is rejected before it reaches the filesystem, and the resolved path is
checked against `SAVE_DIR` a second time to catch symlinks.

## Configuration

Everything is read from the environment at startup; there is nothing to edit in
the source.

| Variable | Default | Meaning |
| --- | --- | --- |
| `TELOXIDE_TOKEN` (or `BOT_TOKEN`) | — | bot token from @BotFather. **Unset means the bot is disabled** and only the web UI runs. |
| `SAVE_DIR` | `downloads` | folder that is written to and served. Created at startup; the resolved absolute path is logged. |
| `BIND_ADDR` | `0.0.0.0:8080` | address the web server listens on |
| `FRONTEND_DIR` | `dist/web` | compiled UI. If missing, the server logs a warning and serves the API alone. |
| `MAX_UPLOAD_BYTES` | `4294967296` (4 GiB) | per-file upload limit |
| `WEB_URL` (or `PUBLIC_URL`) | derived from `BIND_ADDR` | the address `/webpage` replies with, see above. Set it in Docker or behind a proxy. |
| `RUST_LOG` | — | log filter, see below |

> The bot token used to be hard-coded in `src/main.rs` and is therefore in this
> repository's git history. Revoke it with `/revoke` in @BotFather and pass the
> new one through `TELOXIDE_TOKEN`.

There is no authentication in front of the UI. `SAVE_DIR` is fully readable and
writable by anyone who can reach the port, so bind it to a private interface or
put it behind a reverse proxy with auth before exposing it to the internet.

## Running locally

Build the UI once (or leave `trunk watch` running), then start the server:

```bash
cargo install trunk --locked          # one-time
rustup target add wasm32-unknown-unknown

trunk build --release --config crates/frontend/Trunk.toml
TELOXIDE_TOKEN=123:abc RUST_LOG=info cargo run
```

Open <http://localhost:8080>.

For frontend work, run the two halves separately — `trunk serve` rebuilds and
reloads on every change and proxies `/api` to the backend:

```bash
TELOXIDE_TOKEN=123:abc cargo run                   # :8080
trunk serve --config crates/frontend/Trunk.toml    # :8081, open this one
```

Nothing is logged unless `RUST_LOG` is set — `pretty_env_logger` reads it at
startup:

```bash
RUST_LOG=info cargo run                        # one line per saved file, plus requests
RUST_LOG=backend=debug cargo run               # adds "skipping ..." lines
RUST_LOG=warn,backend=debug cargo run          # keep teloxide and tower itself quiet
```

Levels are `error < warn < info < debug < trace`. Note the target is the crate
name, `backend`. Plain `RUST_LOG=debug` also logs every teloxide long-poll
request, which is very noisy.

Tests cover the file-name rules that keep requests inside `SAVE_DIR`, the
command parsing, and the `/webpage` URL derivation:

```bash
cargo test
```

## Building for Ubuntu

The `Dockerfile` builds the wasm UI with trunk, then the server as a statically
linked musl binary. Because `crates/backend/Cargo.toml` selects teloxide's
`rustls` feature instead of the default `native-tls`, the binary has no OpenSSL
dependency and bundles its own root certificates — it runs on any Ubuntu release
with nothing installed alongside it.

Build the image (`--platform` is only needed when the build host and the target
differ, e.g. an Apple Silicon Mac building for an x86_64 server):

```bash
docker build --platform linux/amd64 -t telegram-loader .
```

Run it, mounting a folder from the host so the media survives the container:

```bash
docker run -d --name telegram-loader --restart unless-stopped \
  -p 8080:8080 \
  -e TELOXIDE_TOKEN=123:abc \
  -e WEB_URL=http://192.168.1.73:8080 \
  -v /srv/telegram-loader:/data \
  telegram-loader
```

Saved files appear in `/srv/telegram-loader/downloads` and the UI on port 8080.
`RUST_LOG=info` is set in the image; override it with `-e RUST_LOG=...`.

### Just the binary, without Docker on the server

The server needs the compiled UI next to it, so copy both:

```bash
docker build --platform linux/amd64 --output type=local,dest=./out .
scp out/telegram-loader user@server:/usr/local/bin/telegram-loader
scp -r out/app/web user@server:/srv/telegram-loader/web
```

It needs no packages on the target — no `libssl`, no `ca-certificates`. It writes
to `downloads` relative to the working directory it is started in.

A minimal systemd unit:

```ini
[Unit]
Description=telegram-loader
After=network-online.target

[Service]
ExecStart=/usr/local/bin/telegram-loader
WorkingDirectory=/srv/telegram-loader
Environment=RUST_LOG=info
Environment=TELOXIDE_TOKEN=123:abc
Environment=FRONTEND_DIR=/srv/telegram-loader/web
Environment=BIND_ADDR=127.0.0.1:8080
Environment=WEB_URL=http://192.168.1.73:8080
Restart=always

[Install]
WantedBy=multi-user.target
```

### ARM servers

For an ARM64 Ubuntu host (Graviton, Ampere, Raspberry Pi) swap the platform; the
Dockerfile picks the matching musl target automatically:

```bash
docker build --platform linux/arm64 -t telegram-loader .
```
