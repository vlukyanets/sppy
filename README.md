# sppy

Synchronized video watching for small groups — a Syncplay alternative written in Rust.

A server keeps the authoritative playback state; every participant runs their own
player (mpv for now) that sppy keeps in sync. Media comes from the server's library
(streamed or preloaded into a local cache), from a matching copy on the participant's
own disk, or from any URL mpv can open.

Status: stage 1 (mpv only).

## Install

Download the archive for your system from
[Releases](https://github.com/vlukyanets/sppy/releases) and unpack it: `sppy` is the app,
`sppy-server` the optional headless server.

To check a download: `sha256sum -c SHA256SUMS --ignore-missing` (Linux) or
`Get-FileHash sppy-*.zip` in PowerShell, compared with the line in `SHA256SUMS`.

sppy drives a separately installed [mpv](https://mpv.io):

- Windows: `winget install mpv` (or download mpv and set its path in ⚙ Settings).
- Linux: install `mpv` from your distribution.

[yt-dlp](https://github.com/yt-dlp/yt-dlp) is needed for web links in the playlist.

To build from source instead: `cargo build --release -p sppy-app -p sppy-server-cli`.

## Quick start: host from the app

1. Run `sppy`, enter a nickname, open ⚙ Settings → *My media folders* and add a folder.
   Save, then click **Become host**. While you host, your media folders are the library
   everyone picks from, and you play them straight from your disk.
2. Click **Copy invite link** and send it to your friends (`sppy://host:8740/<token>`).
   The list next to it picks the address in the link: one of this computer's
   addresses (LAN, VPN such as Tailscale) or the *Public address* from Settings.
3. Friends paste the link into **Invite link**, enter a nickname and click **Connect**.
4. Add files from **Library** (or a URL) and press **Play**. Pause and seek either in the
   window or directly in mpv — everyone follows.

When an episode ends the room moves on to the next one by itself. For library files the
server knows their length (read from MKV, WebM, MP4, MOV and AVI headers), so this happens
even if nobody has a player open. For links and other formats the room moves on once every
participant's player has reached the end.

## Headless server

```sh
sppy-server print-tokens          # creates the tokens, prints them and ready links
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

# The host can add and remove folders from the app (Settings → Server folders);
# they are saved into this file. Set to false to allow changes only here.
# allow_remote_media_changes = true
```

Invite link: `sppy://mybox.tailnet.ts.net:8740/<invite token>`. Behind a TLS reverse
proxy use `sppys://…`. Connecting with the host token instead
(`sppy://mybox.tailnet.ts.net:8740/<host token>`) makes you the host: you control the
room and manage the shared folders in ⚙ Settings → *Server folders*. Keep that link private. Other commands: `rotate-invite`, `rotate-host-token`.
State (tokens, playlist, position) lives in `~/.local/share/sppy/server/`.

Built-in limits keep one participant from spoiling the room for the others: the playlist
holds up to 1000 items, chat allows a burst of 10 messages and then one a second, a
connection that floods the server with messages or stops reading is dropped, and at
most 32 connections may wait to join at once (the rest get HTTP 503).

## Delivery modes

- **Stream** — mpv plays the file from the server over HTTP (Range requests).
- **Preload** — the file is downloaded into the cache first (resumable; the next playlist
  item is fetched in advance). After repeated download failures sppy falls back to streaming.
- **My media folders** — if a file with the same fingerprint is found there (any name,
  any folder), it is played directly. A file can also be chosen by hand (📁).

Downloads go to the cache: `~/.cache/sppy/media` on Linux,
`%LOCALAPPDATA%\sppy\cache\media` on Windows (`$SPPY_HOME/.cache/media` when `SPPY_HOME`
is set). Its size limit and **Clear cache** are in Settings.

## Subtitles

External subtitles (srt, ass, ssa, vtt, VobSub idx+sub, …) are found next to a video or in a
`Subs/`, `Subtitles/` or `Sub/` folder beside it, including `Subs/<video name>/*.srt`.
Names are compared ignoring case and punctuation: `Movie (2019) [EN].srt` belongs to
`Movie.2019.mkv` with the label `en`. The player shows the subtitle matching the first of
your *Subtitle languages* (Settings), otherwise an unlabeled one; the others are available
as tracks (`j` in mpv).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
