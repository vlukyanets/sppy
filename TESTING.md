# Testing sppy

## Automated

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

Tests that need mpv (`sppy-player` mpv tests) are skipped when `mpv` is not on `PATH`;
the VLC tests (`sppy-player --test vlc_player`, Unix only) likewise when `vlc` is not, and
their link tests when `yt-dlp` is not (they serve a clip from 127.0.0.1, no internet). They
run VLC headless with its data and config in a temporary folder, so your own VLC settings
and Lua folder are not touched.

Sync quality with real players is measured by hand (it depends on the machine's load, so
CI does not run it): two clients with headless mpv must stay within the dead band (100 ms)
of the room.

```sh
cargo test -p sppy-client-core --test real_mpv -- --ignored --nocapture
```

The same for a room with one mpv and one VLC client (Unix): both must stay within 100 ms.
VLC usually sits 40–50 ms behind (its start-up delay, inside the dead band).

```sh
cargo test -p sppy-client-core --test real_vlc -- --ignored --nocapture
```

## Manual: two clients on one machine

1. `cargo run -p sppy-client-core --example two_clients` (optionally pass a media file).
   This starts an embedded server and two clients, each with its own mpv window, and
   prints the drift of both every second.
2. Wait for playback to start in both windows. The printed drift should stay within ±100 ms.
3. Pause in window A → B pauses on the same frame within a second. Unpause in B → both play.
4. Seek in either window (arrow keys, or click the progress bar) → the other follows.
5. Change speed in a window (`[` / `]`) → it snaps back (speed is controlled by the room).
6. Close one mpv window (`q`) → the other keeps playing; the example prints the notice.
7. Kill one mpv process (`kill -9`) → it is restarted once and catches up.
8. Press Ctrl-C → both clients get "The server is shutting down".

## Manual: the app (two instances on one machine)

Run each instance with its own home so they do not share config and cache:
`SPPY_HOME=/tmp/sppy-a cargo run -p sppy-app` and `SPPY_HOME=/tmp/sppy-b cargo run -p sppy-app`.

1. A: Settings → My media folders → add a folder (with a video and a `.srt` next to it), Save,
   **Become host**. Pick an address in the list next to **Copy invite link** (or
   *Other address…* to type one), then **Copy invite link**.
2. B: paste the link, nickname, **Connect**. Both list two participants.
3. A: Library → `+` a file; Playlist shows it; both mpv windows load it; **Play**.
4. Subtitles appear in both players.
5. Reorder (▲▼), add a second file, double-click it → both switch; remove an item.
6. Chat in both directions; messages and notices also appear as OSD in mpv.
   B: add a link to an unknown site (e.g. a direct `.mp4` on some server) → B plays it,
   A sees "B added a link to …" with Play / Always allow / Skip; mpv shows a hint. Skip →
   the room plays on without A. Always allow → it plays and the site is listed in
   Settings; ✕ there and Save → asked again next time. A YouTube link plays at once.
7. A: Host → *Only the host*; B pauses in mpv → it snaps back with "No permission…".
   Switch to *Selected participants*, tick `ctrl` for B → B can pause again.
8. B: switch to **Preload**, select another item → progress bar in Participants, then
   playback from the cache. Settings shows the cache size; **Clear cache** keeps playlist files.
9. A: while hosting, add a folder in Settings → My media folders and Save → it appears
   in B's Library without restarting.
10. B: Settings → My media folders → add a folder containing a renamed copy of a library
   file, Save → "Video files found" updates and that item plays from the local copy
   (Participants shows `local`). Copy another file there, press **Rescan** → it is found.
   A (the host) has no Stream/Preload switch: library files play from disk (`local`).
11. Long stall: throttle B (e.g. `--cache-secs=1` in extra args plus a slow link, or
    `tc qdisc add dev lo root netem rate 1mbit`) → after 10 s everyone waits for B and
    resumes once B has buffered.
12. A: Kick B (with confirmation) → B sees "You were removed from the room".
13. Play a library episode, seek close to its end, then close mpv in both windows
    (`q`): when the episode's time is up the playlist moves to the next one by itself.
14. Close A's window → B shows "Server unavailable, reconnecting".
15. A: **Stop hosting** with B in the room → a confirmation; confirm → B shows "Server
    unavailable", A sees the join card with **Become host** again, and port 8740 is free
    (`ss -ltn | grep 8740` prints nothing). Become host again works.
16. Languages: a file with two audio tracks (e.g. `rus` and `jpn`) and an `eng` subtitle track
    inside. Settings → *Audio languages* `ja`, *Subtitle languages* `en`, Save, select the
    file → Japanese audio with English subtitles, without pressing `#` or `j`.
17. Invite links: Settings → *Invite links* → **Open sppy:// links with this app** → the
    card says links open this app. Click an `sppy://` link in a browser: sppy starts with
    the link in the join form (not joined). Click another while sppy is open: no second
    window; the link replaces the one in the form, and while in a room a note says so.
    **Stop** → clicking links no longer opens sppy. (Linux: `xdg-mime query default
    x-scheme-handler/sppy` prints `sppy-link.desktop` while registered.)
18. Look: rows light up under the pointer, the current item has an accent bar, row actions
    (My file…, ▲▼, ✕) appear on hover; transport buttons sit on one line; no squares
    instead of icons anywhere; Settings opens centred with cards.

## Manual: catching up

1. Two instances, both playing. In B: ⚙ Settings → *Catching up* → **Jump**, Save. Freeze
   B's player for a second without it noticing (Linux: `kill -STOP <pid>`, then
   `kill -CONT <pid>`) → B jumps back to the room; no speed change appears in mpv's OSD.
   With **Speed up or slow down** the same freeze is closed by a short speed change.
2. B → **Leave my player alone**, Save. A seeks and pauses → B's player does not follow;
   pausing or seeking in B's player does not affect A. B's drift is not shown in the list.
3. B → **Speed up or slow down**, Save → B catches up with the room again.
4. A (host) → **Wait for latecomers**. Freeze B's player for 2 s → A's player jumps back
   about 2 s with "Went back 2.0 s for B" on its OSD; B does not jump. Back to
   **Latecomers catch up alone** → the same freeze makes B catch up and A plays on.

## Manual: VLC

Needs VLC 3 with its window (the automated tests only run it headless).

1. ⚙ Settings → *Player* → **VLC**; the path becomes `vlc` and the extra arguments are
   cleared. **Save** → the mpv window closes and a VLC window opens (no "continue
   playback?" or privacy question). `sppy.lua` is in VLC's Lua folder (README).
2. Two instances as above, A with mpv and B with VLC: add a library file, **Play**. Both
   play; B's drift in the participants list stays small (under ~100 ms) after it settles.
3. In VLC: pause, seek with the slider and the arrow keys, change the speed (`]`) → A
   follows the pause and the seeks, the speed snaps back.
4. Press VLC's **Stop** in the middle → VLC opens the item again where it was and catches up.
5. Seek close to the end of an item → the room moves on to the next item in both players.
6. Messages from sppy appear in VLC's video (top left).
7. Add a YouTube link: VLC shows the video's title (not `videoplayback`), plays picture
   and sound, and stays in sync with mpv. A broken link shows yt-dlp's reason.
8. Windows: with VLC installed by its installer and the path left at `vlc`, sppy finds it;
   adding a link shows no console window.
9. Close VLC's window → the app shows the player as closed, like with mpv.

## Manual: headless server over Tailscale

1. On the server: create `~/.config/sppy/server.toml` with a `[[media]]` folder, run
   `sppy-server`, note the invite token (`sppy-server print-tokens`).
2. On two other machines, connect with `sppy://<tailscale-name>:8740/<invite>`.
3. Repeat steps 2–5 above in stream mode, then in preload mode.
4. Connect with the host token instead of the invite (`sppy://<name>:8740/<host token>`):
   you are the host. Settings → *Server folders* lists the `[[media]]` folders. Without
   `media_root` in `server.toml` it says to set one and offers no way to add. Set
   `media_root = "/srv/media"` and restart: the hint shows it. Share a folder inside it →
   it appears in everyone's Library and in `server.toml` (comments kept). Remove it
   (✕, confirm) → gone from both. A relative, missing or outside path (`/etc`,
   `/srv/media/..`) shows the reason. With `allow_remote_media_changes = false` the list
   is read-only.
5. Tailscale names and addresses show no warning. Type `sppy://example.org/x` into the
   invite link field → a yellow *Not encrypted* note appears under it; `sppys://…` does
   not show it.

## Manual: behind a TLS proxy

1. Run the server with `listen = "127.0.0.1:8740"` and
   `public_address = "sppys://<domain>:443"` behind Caddy (see README → Encryption).
2. `sppy-server print-tokens` prints `sppys://<domain>:443/…` links.
3. Connect from another machine with that link: no *Not encrypted* badge; streaming,
   preload and seeking work through the proxy.
