# sppy

Synchronized video watching for small groups — a Syncplay alternative written in Rust.

A server keeps the authoritative playback state; every participant runs their own
player (mpv or VLC) that sppy keeps in sync. Media comes from the server's library
(streamed or preloaded into a local cache), from a matching copy on the participant's
own disk, or from any URL the player can open.

Status: early development. Works with mpv and VLC 3 on Linux and Windows; participants
in one room may use different players. Everyone in a room needs a version of sppy with the
same protocol; the app tells you when they differ.

## Install

sppy needs three things on every participant's computer: the sppy app, a video player
that sppy drives (mpv or VLC), and yt-dlp if you want to watch links from YouTube and
other sites.

### 1. The app

**Windows.** From [Releases](https://github.com/vlukyanets/sppy/releases) download
`sppy-…-windows-x86_64.msi` and run it. It installs for your account only, needs no
administrator rights, adds sppy to the Start menu and updates an older install; remove
sppy in *Settings → Apps*. Or take `sppy-…-windows-x86_64.zip` and unpack it anywhere.

**Linux.** On Arch Linux and its derivatives sppy is in the AUR: `paru -S sppy` (or any
other AUR helper). Elsewhere download `sppy-…-linux-x86_64.tar.gz` from
[Releases](https://github.com/vlukyanets/sppy/releases), unpack it and run `sppy`.

The window is drawn with OpenGL on Linux (any Mesa or vendor driver) and with Direct3D 12
or Vulkan on Windows; nothing else is needed for it. The app can host a room by itself:
the server is needed only for an always-on machine (see [Headless server](#headless-server)).

To check a download: `sha256sum -c SHA256SUMS --ignore-missing` (Linux) or
`Get-FileHash <file>` in PowerShell, compared with the line in `SHA256SUMS`.

### 2. A player

sppy does not play video itself: it starts a player and keeps it in sync. Choose one in
⚙ Settings → *Player*. Participants in one room may use different players.

| | Windows | Linux |
|---|---|---|
| [mpv](https://mpv.io) (default) | `winget install mpv` | `mpv` from your distribution |
| [VLC](https://www.videolan.org/vlc/) 3 | the regular installer | `vlc` from your distribution |

sppy looks for `mpv` in `PATH` and for VLC also in `Program Files`. If yours is somewhere
else, enter the full path to `mpv.exe` / `vlc.exe` (or `mpv` / `vlc`) in Settings → *Player*.

### 3. yt-dlp, for links from YouTube and other sites

Files from the library and from your own disk play without it. A link to a page on a
video site (YouTube, Vimeo, Twitch and others) needs [yt-dlp](https://github.com/yt-dlp/yt-dlp),
whichever player you use: it finds the video behind the page.

- **Windows:** `winget install yt-dlp.yt-dlp`, then **restart sppy**: programs started
  before the install do not see it yet. mpv also finds `yt-dlp.exe` put next to `mpv.exe`.
- **Arch Linux:** `sudo pacman -S yt-dlp` (it brings the JavaScript runtime that YouTube
  needs along with it).
- **Other Linux:** yt-dlp from your distribution, if it is recent; otherwise the
  standalone build from its releases page, somewhere in `PATH`.

Video sites change often, and an old yt-dlp stops working with them: keep it updated
(`winget upgrade yt-dlp.yt-dlp`, or your package manager). For YouTube, yt-dlp also needs a
JavaScript runtime; if it says so in its error, install [Deno](https://deno.com)
(`winget install DenoLand.Deno` on Windows).

### 4. Check it

Run sppy, click **Become host**, add a YouTube link to the playlist and press **Play**. The
player window opens and the video starts. If not:

- *mpv not found* (or *VLC not found*): install the player, or enter its path in
  ⚙ Settings → *Player*.
- *The player could not open the media* on a link: yt-dlp is missing or too old. Run
  `yt-dlp <the link>` in a terminal to see why.

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

### Build from source

`cargo build --profile dist -p sppy-app -p sppy-server-cli` builds both programs into
`target/dist` (`--release` builds faster but bigger).

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

An item played to its end gets a ✓ (hover over an item to set or clear it by hand). Above
the playlist, **Repeat all** starts again from the first item after the last one, **Repeat
one** plays the same item again, and **Shuffle** puts the items after the current one in a
random order. The marks and the repeat mode survive a server restart.

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

## Keeping in sync

The server keeps the room's position; each player follows it. Drifts under 100 ms are left
alone. ⚙ Settings → *Catching up* picks how your player gets back when it drifts further:

- **Speed up or slow down** (default) — a few percent faster or slower until it is back
  (the pitch stays); a jump only when it is more than 1.5 s off.
- **Jump** — no change of tempo; the player jumps to the room once it is half a second off
  (*Jump threshold* in the advanced sync settings). Players that cannot change speed
  always do this.
- **Leave my player alone** — sppy opens the items but does not play, pause or move your
  player, and what you do in it does not reach the room.

The thresholds are under *Advanced sync settings*. Values in `client.toml` that are out of
their limits or contradict each other are set back to their defaults on start, and the app
says which.

The host decides what the room does about latecomers (*Room* → *Latecomers*, under the
participants):

- **Catch up alone** (default) — the room plays on.
- **Room waits** — when someone falls more than a second behind, the room goes back
  to them and the others slow down or jump back, as in Syncplay. Someone buffering does not
  pull the room back; after `stall_hold_secs` the room pauses for them as usual.

A new item starts as soon as everyone has loaded it. With *Start* **on Ready** (host, under *Room*) it also
waits until everyone has ticked **I'm ready** in the yellow waiting line; the players say
"Press Ready in sppy" and the participant list shows who is ready (the green *loaded* next
to a name only means that the player has opened the file).

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
