//! How an item reaches the player: stream URL, local file, or (later) a download (spec §7.1).

use std::path::PathBuf;

pub use sppy_player::MediaSource;
use sppy_proto::{Item, Source};

#[derive(Debug, Clone, PartialEq)]
pub enum Resolved {
    /// Play from the network (server media URL or any URL).
    Stream { media: MediaSource, subtitles: Vec<MediaSource> },
    /// Play a file on this machine.
    Local { path: PathBuf, subtitles: Vec<PathBuf> },
    /// Not playable yet (waiting for a download).
    Wait,
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
        Source::Url { url } if !url.contains("://") => {
            Resolved::Local { path: PathBuf::from(url), subtitles: Vec::new() }
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
    fn schemeless_url_is_local_path() {
        let r =
            stream_source("http://h:1", &item(Source::Url { url: "/srv/a.mkv".into() }, 0), "t");
        assert_eq!(r, Resolved::Local { path: "/srv/a.mkv".into(), subtitles: vec![] });
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
