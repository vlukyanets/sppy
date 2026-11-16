# Working on sppy

sppy is a synchronized video watching tool (a Syncplay alternative) in Rust: a server
(embedded in the app or headless), a desktop app (egui) and mpv as the player.

How to test: [TESTING.md](TESTING.md). User documentation: [README.md](README.md).

## Repository rules

- Everything in the repository is in English: code, comments, docs, commit messages.
- The default branch is `master`.
- Design specs and implementation plans are not kept in the repository (no `docs/` folder).
- Commit messages follow Conventional Commits (`feat(app): …`, `fix(client-core): …`,
  `ci: …`, `!` for breaking changes). The body explains why, not what the diff shows.
  An agent may add a `Co-Authored-By:` line; no other trailers (no session links).
- One logical change per commit. Every commit builds and passes the checks below.
- Pushing is done when the owner asks. Tags and GitHub Releases (`v*` tags publish a
  release) need the owner's explicit approval every time.
- Never put personal data (e-mails, names, machine paths) into the repository, logs,
  test fixtures or requests to external services.

## Before every commit

```sh
cargo fmt --all
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

CI runs the same on Linux and Windows. Code under `#[cfg(unix)]` or `#[cfg(windows)]`
must also compile cleanly on the other platform (watch for imports used only on one).
A quick Windows check for crates without TLS:
`CC_x86_64_pc_windows_msvc=clang-cl AR_x86_64_pc_windows_msvc=llvm-lib cargo clippy --target x86_64-pc-windows-msvc -p <crate>`.

## Architecture

| Crate | Role | May depend on |
|---|---|---|
| `crates/proto` (`sppy-proto`) | Wire types and messages, serde only | — |
| `crates/sync` (`sppy-sync`) | Clock estimate, drift controller, echo filter. Pure | — |
| `crates/common` (`sppy-common`) | Server config, media extensions, fingerprint, natural sort | proto |
| `crates/server` (`sppy-server`) | Room state machine, hub, HTTP (`/ws`, `/media`), library | proto, common |
| `crates/player` (`sppy-player`) | `trait Player`, mpv over JSON IPC, `FakePlayer` (feature `fake`) | — |
| `crates/client-core` | Session: connection, sync, delivery, local copies, cache | proto, sync, common, player |
| `bins/app` (`sppy`) | egui desktop app; embeds the server for "Become host" | all |
| `bins/server` (`sppy-server`) | Headless server CLI | server |

- `client-core` must not depend on `sppy-server` (only as a dev-dependency for tests).
  Shared code goes to `sppy-common`.
- The room (`crates/server/src/room`) is a pure state machine: no I/O, time is passed in,
  effects are returned. Keep it that way; I/O lives in the hub and HTTP layers.
- `sppy-sync` is pure as well. Sync tuning lives in `SyncParams`.
- The server is authoritative over playback. Clients report status and correct
  themselves; they never trust each other.
- The app keeps only visual state. Everything else goes through `UiCommand` /
  `UiEvent` / `Snapshot` of `client-core`; the UI thread never blocks.
- mpv is not bundled: users install it themselves.

## Decisions that need the owner

Ask before changing any of these, and before other choices that are hard to revert
(new protocol concepts, new heavy dependencies, data formats on disk, publishing):

- Sync policy: no micro-pauses. A participant who lags catches up alone (speed change
  of a few percent, a seek beyond 1.5 s). The room waits for everyone only on item
  start and after a stall longer than `stall_hold_secs`. Not Syncplay's
  "follow the slowest".
- Playlist and position survive a server restart (`state.json`).
- Local copies are found by fingerprint, whatever their name or folder.
- VLC support is the next player after mpv.

## Protocol

- Any change to messages or types in `sppy-proto` bumps `PROTOCOL_VERSION` and updates
  the golden snapshots (`crates/proto/tests/snapshots`, `cargo insta review`).
- Server and client of different protocol versions refuse each other with a clear error.

## How changes are made

- Find the root cause before fixing anything. Reproduce first; do not stack guesses.
- Test-driven: write the test, see it fail for the expected reason, then fix.
  Every bug fix comes with a regression test. A test that passes before the fix
  proves nothing; check that it fails on the old code.
- Tests must not depend on timing luck: wait for a condition (`eventually(…)`), not for
  a fixed sleep. A flaky test is a bug to investigate, not to retry.
- Pure logic (ranking, parsing, formatting, state machines) goes into functions that
  can be unit-tested without a UI, network or player.
- Keep scope tight: do what was asked, mention other problems found instead of
  silently fixing them in the same commit.
- Verify before claiming: run the checks, look at real output (logs, files on disk,
  CI annotations). Say plainly what was not verified (e.g. UI not looked at by eye).
- When the user-visible behaviour changes, update README.md and TESTING.md.

## Code style

- `rustfmt.toml` sets `use_small_heuristics = "Max"`; run `cargo fmt`.
- Doc comments say why and what for; no comments that repeat the code.
- No `unwrap()` outside tests; `expect("…")` only for real invariants.
- User-facing texts are short, concrete English sentences that tell what to do next.
- Files the app writes are bounded: caches have a limit, logs rotate by size.
