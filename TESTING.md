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

1. `cargo run -p sppy-client-core --example two_clients` (optionally pass a media file or URL).
   This starts an embedded server and two clients, each with its own mpv window, and
   prints the drift of both every second.
2. Wait for playback to start in both windows. The printed drift should stay within ±80 ms.
3. Pause in window A → B pauses on the same frame within a second. Unpause in B → both play.
4. Seek in either window (arrow keys, or click the progress bar) → the other follows.
5. Change speed in a window (`[` / `]`) → it snaps back (speed is controlled by the room).
6. Close one mpv window (`q`) → the other keeps playing; the example prints the notice.
7. Kill one mpv process (`kill -9`) → it is restarted once and catches up.
8. Press Ctrl-C → both clients get "The server is shutting down".

## Manual: headless server over Tailscale

1. On the server: create `~/.config/sppy/server.toml` with a `[[media]]` folder, run
   `sppy-server`, note the invite token (`sppy-server print-tokens`).
2. On two other machines, connect with `sppy://<tailscale-name>:8740/<invite>`.
3. Repeat steps 2–5 above in stream mode, then in preload mode.
