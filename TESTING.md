# Testing sppy

## Automated

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

Tests that need mpv (`sppy-player` mpv tests, `sppy-client-core` `real_mpv`) are skipped
when `mpv` is not on `PATH`.

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

1. A: Settings → Hosting → add a media folder (with a video and a `.srt` next to it), Save,
   **Become host**, **Copy invite link**.
2. B: paste the link, nickname, **Connect**. Both list two participants.
3. A: Library → `+` a file; Playlist shows it; both mpv windows load it; **Play**.
4. Subtitles appear in both players.
5. Reorder (▲▼), add a second file, double-click it → both switch; remove an item.
6. Chat in both directions; messages and notices also appear as OSD in mpv.
7. A: Host → *Only the host*; B pauses in mpv → it snaps back with "No permission…".
   Switch to *Selected participants*, tick `ctrl` for B → B can pause again.
8. B: switch to **Preload**, select another item → progress bar in Participants, then
   playback from the cache. Settings shows the cache size; **Clear cache** keeps playlist files.
9. B: Settings → My media folders → add a folder containing a renamed copy of a library
   file → that item plays from the local copy (Participants shows `local`).
10. Long stall: throttle B (e.g. `--cache-secs=1` in extra args plus a slow link, or
    `tc qdisc add dev lo root netem rate 1mbit`) → after 10 s everyone waits for B and
    resumes once B has buffered.
11. A: Kick B (with confirmation) → B sees "You were removed from the room".
12. Close A's window → B shows "Server unavailable, reconnecting".

## Manual: headless server over Tailscale

1. On the server: create `~/.config/sppy/server.toml` with a `[[media]]` folder, run
   `sppy-server`, note the invite token (`sppy-server print-tokens`).
2. On two other machines, connect with `sppy://<tailscale-name>:8740/<invite>`.
3. Repeat steps 2–5 above in stream mode, then in preload mode.
