# sppy

Synchronized video watching for small groups — a Syncplay alternative written in Rust.

A server keeps the authoritative playback state; every participant runs their own
player (mpv for now) that sppy keeps in sync. Media is served by the server over HTTP
(stream or preload), or taken from a matching local copy.

Status: early development (stage 1).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
