//! How an item reaches the player: stream URL, local file, or (later) a download (spec §7.1).

use std::path::PathBuf;

pub use sppy_player::MediaSource;
use sppy_proto::{Item, Source};

#[derive(Debug, Clone, PartialEq)]
pub enum Resolved {
    /// Play from the network (server media URL or any URL).
    Stream { media: MediaSource, subtitles: Vec<MediaSource> },
    /// Play a file on this machine (subtitles may still come from the server).
    Local { path: PathBuf, subtitles: Vec<MediaSource> },
    /// Not playable yet (waiting for a download).
    Wait,
    /// Refused to open (e.g. not an http(s) URL).
    Rejected { reason: String },
}

/// Whether a playlist URL may be opened by the player: only http and https.
/// Other schemes (local paths, `edl://`, `av://`, `file://`, …) would let any controller
/// make everyone's player open arbitrary local resources.
pub fn is_allowed_url(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

/// What the UI sends for a typed link: `youtube.com/…` becomes `https://youtube.com/…`.
pub fn normalize_url(input: &str) -> String {
    let t = input.trim();
    if t.contains("://") { t.to_owned() } else { format!("https://{t}") }
}

fn encode_query(v: &str) -> String {
    v.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Media URL of a library file on the server.
pub fn file_url(base: &str, file_id: &str, token: &str) -> String {
    format!("{base}/media/{file_id}?t={}", encode_query(token))
}

/// URL of a file's external subtitle.
pub fn subtitle_url(base: &str, file_id: &str, index: u32, token: &str) -> String {
    format!("{base}/media/{file_id}/sub/{index}?t={}", encode_query(token))
}

/// Streaming source of an item (a scheme-less "URL" is a local path).
pub fn stream_source(base: &str, item: &Item, session_token: &str) -> Resolved {
    match &item.source {
        Source::File { file_id } => Resolved::Stream {
            media: MediaSource::Url(file_url(base, &file_id.0, session_token)),
            subtitles: item
                .subtitles
                .iter()
                .map(|s| MediaSource::Url(subtitle_url(base, &file_id.0, s.index, session_token)))
                .collect(),
        },
        Source::Url { url } if !is_allowed_url(url) => {
            Resolved::Rejected { reason: format!("Only http(s) links can be played: {url}") }
        }
        Source::Url { url } => {
            Resolved::Stream { media: MediaSource::Url(url.clone()), subtitles: Vec::new() }
        }
    }
}

#[cfg(test)]
mod tests {
    use sppy_proto::*;

    use super::*;

    fn item(source: Source, subs: u32) -> Item {
        Item {
            item_id: ItemId(1),
            source,
            title: "t".into(),
            duration_ms: None,
            fingerprint: None,
            subtitles: (0..subs)
                .map(|index| Subtitle { index, label: None, ext: "srt".into() })
                .collect(),
            added_by: ParticipantId(1),
        }
    }

    #[test]
    fn file_item_streams_from_server() {
        let i = item(Source::File { file_id: FileId("0123456789abcdef".into()) }, 2);
        let r = stream_source("http://h:8740", &i, "tok");
        assert_eq!(
            r,
            Resolved::Stream {
                media: MediaSource::Url("http://h:8740/media/0123456789abcdef?t=tok".into()),
                subtitles: vec![
                    MediaSource::Url("http://h:8740/media/0123456789abcdef/sub/0?t=tok".into()),
                    MediaSource::Url("http://h:8740/media/0123456789abcdef/sub/1?t=tok".into()),
                ],
            }
        );
    }

    #[test]
    fn url_item_streams_directly() {
        let r =
            stream_source("http://h:1", &item(Source::Url { url: "https://y/v".into() }, 0), "t");
        assert_eq!(
            r,
            Resolved::Stream { media: MediaSource::Url("https://y/v".into()), subtitles: vec![] }
        );
    }

    #[test]
    fn only_http_urls_are_opened() {
        for bad in [
            "/srv/a.mkv",
            "youtube.com/watch?v=x",
            "edl://a.mkv",
            "av://lavfi:testsrc",
            "file:///etc/passwd",
        ] {
            let r = stream_source("http://h:1", &item(Source::Url { url: bad.into() }, 0), "t");
            assert!(matches!(r, Resolved::Rejected { .. }), "{bad}: {r:?}");
        }
        let ok =
            stream_source("http://h:1", &item(Source::Url { url: "HTTP://x/y".into() }, 0), "t");
        assert!(matches!(ok, Resolved::Stream { .. }));
    }

    #[test]
    fn normalize_url_adds_https() {
        assert_eq!(normalize_url("youtube.com/watch?v=x"), "https://youtube.com/watch?v=x");
        assert_eq!(normalize_url("  http://a/b "), "http://a/b");
    }

    #[test]
    fn token_is_url_encoded() {
        let i = item(Source::File { file_id: FileId("ab".into()) }, 0);
        let Resolved::Stream { media: MediaSource::Url(u), .. } =
            stream_source("http://h:1", &i, "a+b/c")
        else {
            panic!()
        };
        assert_eq!(u, "http://h:1/media/ab?t=a%2Bb%2Fc");
    }
}
