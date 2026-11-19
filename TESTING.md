# Testing sppy

## Automated

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

Tests that need mpv (`sppy-player` mpv tests) are skipped when `mpv` is not on `PATH`.

Sync quality with real players is measured by hand (it depends on the machine's load, so
CI does not run it): two clients with headless mpv must stay within 80 ms of the room.

```sh
cargo test -p sppy-client-core --test real_mpv -- --ignored --nocapture
```

## Manual: two clients on one machine

1. `cargo run -p sppy-client-core --example two_clients` (optionally pass a media file).
   This starts an embedded server and two clients, each with its own mpv window, and
   prints the drift of both every second.
2. Wait for playback to start in both windows. The printed drift should stay within ±80 ms.
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
16. Look: rows light up under the pointer, the current item has an accent bar, row actions
    (My file…, ▲▼, ✕) appear on hover; transport buttons sit on one line; no squares
    instead of icons anywhere; Settings opens centred with cards.

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
