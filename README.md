# sppy

Synchronized video watching for small groups — a Syncplay alternative written in Rust.

A server keeps the authoritative playback state; every participant runs their own
player (mpv for now) that sppy keeps in sync. Media comes from the server's library
(streamed or preloaded into a local cache), from a matching copy on the participant's
own disk, or from any URL mpv can open.

Status: stage 1 (mpv only).

## Requirements

- [mpv](https://mpv.io) on `PATH` (or set its path in Settings).
- [yt-dlp](https://github.com/yt-dlp/yt-dlp) for web URLs in the playlist.
- Rust (stable) to build: `cargo build --release`.

## Quick start: host from the app

1. Run `sppy`, enter a nickname, open ⚙ Settings → *Hosting* and add a media folder.
   Save, then click **Become host**.
2. Click **Copy invite link** and send it to your friends (`sppy://host:8740/<token>`).
   Set *Public address* in Settings (e.g. your Tailscale name) so the link points to it.
3. Friends paste the link into **Invite link**, enter a nickname and click **Connect**.
4. Add files from **Library** (or a URL) and press **Play**. Pause and seek either in the
   window or directly in mpv — everyone follows.

## Headless server

```sh
sppy-server print-tokens          # creates and prints the host and invite tokens
sppy-server                       # runs with ~/.config/sppy/server.toml
```

```toml
# ~/.config/sppy/server.toml
listen = "0.0.0.0:8740"
public_address = "mybox.tailnet.ts.net:8740"   # optional, for invite links
max_participants = 8
resume_grace_secs = 60
stall_hold_secs = 10        # 0 = never pause the room for a buffering participant
resume_buffer_secs = 10

[[media]]
name = "Movies"
path = "/srv/media/movies"
```

Invite link: `sppy://mybox.tailnet.ts.net:8740/<invite token>`. Behind a TLS reverse
proxy use `sppys://…`. Other commands: `rotate-invite`, `rotate-host-token`.
State (tokens, playlist, position) lives in `~/.local/share/sppy/server/`.

## Delivery modes

- **Stream** — mpv plays the file from the server over HTTP (Range requests).
- **Preload** — the file is downloaded into the cache first (resumable; the next playlist
  item is fetched in advance). After repeated download failures sppy falls back to streaming.
- **My media folders** — if a file with the same fingerprint is found there (any name,
  any folder), it is played directly. A file can also be chosen by hand (📁).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
