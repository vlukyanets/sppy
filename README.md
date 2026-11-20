# sppy

Synchronized video watching for small groups — a Syncplay alternative written in Rust.

A server keeps the authoritative playback state; every participant runs their own
player (mpv or VLC) that sppy keeps in sync. Media comes from the server's library
(streamed or preloaded into a local cache), from a matching copy on the participant's
own disk, or from any URL the player can open.

Status: early development. Works with mpv and VLC 3 on Linux and Windows; participants
in one room may use different players. Everyone in a room needs the same version of sppy.

## Install

Download the archive for your system from
[Releases](https://github.com/vlukyanets/sppy/releases) and unpack it: `sppy` is the app,
`sppy-server` the optional headless server.

To check a download: `sha256sum -c SHA256SUMS --ignore-missing` (Linux) or
`Get-FileHash sppy-*.zip` in PowerShell, compared with the line in `SHA256SUMS`.

sppy drives a separately installed player: [mpv](https://mpv.io) (the default) or
[VLC](https://www.videolan.org/vlc/) 3, chosen in ⚙ Settings → *Player*.

- mpv on Windows: `winget install mpv` (or download mpv and set its path in Settings).
- VLC on Windows: the regular installer; sppy finds it in `Program Files`.
- Linux: install `mpv` or `vlc` from your distribution.

[yt-dlp](https://github.com/yt-dlp/yt-dlp) is needed for web links in the playlist (YouTube and
other sites), with either player.

### VLC

Like Syncplay, sppy controls VLC through a small Lua interface module: it puts `sppy.lua`
into VLC's Lua folder (`~/.local/share/vlc/lua/intf` on Linux,
`%APPDATA%\vlc\lua\intf` on Windows) and starts VLC with it. The module talks to sppy
over a local connection only. Compared with mpv:

- Web links go through yt-dlp first, as with mpv (video and audio that a site serves
  separately are played together). Without yt-dlp, VLC tries its own scripts, which know
  fewer sites.
- VLC cannot tell how much it has buffered ahead, so a streamed item counts as ready as
  soon as VLC has opened it.
- At the end of an item VLC's window goes black instead of keeping the last frame.

To build from source instead: `cargo build --release -p sppy-app -p sppy-server-cli`.

## Quick start: host from the app

1. Run `sppy`, enter a nickname, open ⚙ Settings → *My media folders* and add a folder.
   Save, then click **Become host**. While you host, your media folders are the library
   everyone picks from, and you play them straight from your disk.
2. Click **Copy invite link** and send it to your friends (`sppy://host:8740/<token>`).
   The list next to it picks the address in the link: one of this computer's
   addresses (LAN, VPN such as Tailscale) or the *Public address* from Settings.
3. Friends paste the link into **Invite link**, enter a nickname and click **Connect**.
   With ⚙ Settings → *Invite links* → **Open sppy:// links with this app** (Linux, Windows),
   clicking such a link in a browser or messenger opens sppy with the link filled in;
   if sppy is already open, the link goes to that window. It never joins by itself.
4. Add files from **Library** (or a URL) and press **Play**. Pause and seek either in the
   window or directly in the player — everyone follows.
5. **Stop hosting** (top right) leaves the room and stops the server; if others are still
   in the room, the app asks first.

When an episode ends the room moves on to the next one by itself. For library files the
server knows their length (read from MKV, WebM, MP4, MOV and AVI headers, or from the
timestamps of MPEG-TS/M2TS files), so this happens
even if nobody has a player open. For links and other formats the room moves on once every
participant's player has reached the end.

A link someone else added plays right away if it leads to a well-known video site
(YouTube, Vimeo, Twitch, Dailymotion, Rutube, VK Video, Bilibili, SoundCloud). For any
other site the app asks first, since your player would connect to it: **Play** once,
**Always allow** the site (listed in Settings, where ✕ takes it back), or **Skip** and let
the room go on without you. Only `http` and `https` links are ever opened.

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
# they are saved into this file. Folders can be added only inside media_root, and
# none without it. Set allow_remote_media_changes to false to allow changes only here.
# media_root = "/srv/media"
# allow_remote_media_changes = true
```

Invite link: `sppy://mybox.tailnet.ts.net:8740/<invite token>`. Behind a TLS reverse
proxy use `sppys://…` (see [Encryption](#encryption)). Connecting with the host token instead
(`sppy://mybox.tailnet.ts.net:8740/<host token>`) makes you the host: you control the
room and manage the shared folders in ⚙ Settings → *Server folders*. Keep that link private. Other commands: `rotate-invite`, `rotate-host-token`.
State (tokens, playlist, position) lives in `~/.local/share/sppy/server/`.

Built-in limits keep one participant from spoiling the room for the others: the playlist
holds up to 1000 items, chat allows a burst of 10 messages and then one a second, a
connection that floods the server with messages or stops reading is dropped, and at
most 32 connections may wait to join at once (the rest get HTTP 503).

## Encryption

sppy itself does not encrypt: an `sppy://` link carries the token, the chat and the video
in the clear. That is fine on a home network and over Tailscale or another VPN, which
encrypt by themselves. Over the open internet the app shows *⚠ Not encrypted*; put the
server behind a TLS reverse proxy and use `sppys://` links there.

With [Caddy](https://caddyserver.com), which gets the certificate by itself:

```
watch.example.org {
    reverse_proxy 127.0.0.1:8740
}
```

With nginx (WebSocket upgrade, and no buffering for video):

```nginx
location / {
    proxy_pass http://127.0.0.1:8740;
    proxy_http_version 1.1;
    proxy_set_header Upgrade $http_upgrade;
    proxy_set_header Connection "upgrade";
    proxy_read_timeout 1h;
    proxy_buffering off;
}
```

In `server.toml` listen only locally, so that nobody bypasses the proxy, and put the
`sppys://` address into the invite links:

```toml
listen = "127.0.0.1:8740"
public_address = "sppys://watch.example.org:443"
```

Links then look like `sppys://watch.example.org:443/<token>`. The port has to be written
out: without one, sppy uses 8740.

## Delivery modes

- **Stream** — the player plays the file from the server over HTTP (Range requests).
- **Preload** — the file is downloaded into the cache first (resumable; the next playlist
  item is fetched in advance). After repeated download failures sppy falls back to streaming.
- **My media folders** — if a file with the same fingerprint is found there (any name,
  any folder), it is played directly. A file can also be chosen by hand (*My file…* when
  pointing at a playlist item).

While you host, the switch is hidden: the library is your own folders, so its files always
play from your disk.

Downloads go to the cache: `~/.cache/sppy/media` on Linux,
`%LOCALAPPDATA%\sppy\cache\media` on Windows (`$SPPY_HOME/.cache/media` when `SPPY_HOME`
is set). Its size limit and **Clear cache** are in Settings.

## Subtitles

External subtitles (srt, ass, ssa, vtt, VobSub idx+sub, …) are found next to a video or in a
`Subs/`, `Subtitles/` or `Sub/` folder beside it, including `Subs/<video name>/*.srt`.
Names are compared ignoring case and punctuation: `Movie (2019) [EN].srt` belongs to
`Movie.2019.mkv` with the label `en`. The player shows the subtitle matching the first of
your *Subtitle languages* (Settings), otherwise an unlabeled one; the others are available
as tracks (`j` in mpv, `v` in VLC). Without such a file, the same languages pick a subtitle track inside
the video (MKV, MP4); `en` and `eng` both work. *Audio languages* likewise pick the audio
track (e.g. `ja` for the original of a dubbed series). Changes apply from the next item.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
