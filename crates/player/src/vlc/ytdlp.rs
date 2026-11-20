//! Links for VLC go through yt-dlp first, as mpv's ytdl_hook does for mpv: VLC's own Lua
//! scripts know far fewer sites and break often.

/// The format mpv asks for by default, so both players get the same picture.
pub const FORMAT: &str = "bestvideo+bestaudio/best";

/// Arguments for `yt-dlp`: one JSON description of the video at `url`.
pub fn args(url: &str) -> Vec<String> {
    ["-J", "--no-playlist", "--no-warnings", "-f", FORMAT, "--", url].map(String::from).to_vec()
}

/// What VLC needs to play a link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub media: String,
    /// Separate audio (sites that serve video and audio as two streams).
    pub audio: Option<String>,
    pub title: Option<String>,
    pub user_agent: Option<String>,
    pub referer: Option<String>,
}

fn text(v: &serde_json::Value, key: &str) -> Option<String> {
    v.get(key).and_then(|x| x.as_str()).filter(|s| !s.is_empty()).map(str::to_owned)
}

fn has_codec(f: &serde_json::Value, key: &str) -> bool {
    text(f, key).is_some_and(|c| c != "none")
}

/// Read yt-dlp's JSON (`-J`).
pub fn parse(json: &str) -> Result<Resolved, String> {
    let v: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("unexpected output: {e}"))?;
    let formats: Vec<&serde_json::Value> = v
        .get("requested_formats")
        .and_then(|f| f.as_array())
        .map(|a| a.iter().collect())
        .unwrap_or_default();
    let (media_format, audio) = match formats.as_slice() {
        [] => (&v, None),
        [only] => (*only, None),
        several => {
            let video = several.iter().find(|f| has_codec(f, "vcodec")).unwrap_or(&several[0]);
            let audio = several
                .iter()
                .find(|f| !std::ptr::eq(**f, *video) && has_codec(f, "acodec"))
                .and_then(|f| text(f, "url"));
            (*video, audio)
        }
    };
    let media = text(media_format, "url").ok_or("no media URL in yt-dlp's answer")?;
    let headers = media_format.get("http_headers").or_else(|| v.get("http_headers"));
    let header = |name: &str| headers.and_then(|h| text(h, name));
    Ok(Resolved {
        media,
        audio,
        title: text(&v, "title"),
        user_agent: header("User-Agent"),
        referer: header("Referer"),
    })
}

/// The reason from yt-dlp's error output, short enough for a status line.
pub fn error_message(stderr: &str) -> String {
    let line = stderr
        .lines()
        .rev()
        .find(|l| l.starts_with("ERROR:"))
        .or_else(|| stderr.lines().rev().find(|l| !l.trim().is_empty()))
        .unwrap_or("failed");
    let line = line.strip_prefix("ERROR:").unwrap_or(line).trim();
    let mut msg: String = line.chars().take(200).collect();
    if line.chars().count() > 200 {
        msg.push('…');
    }
    msg
}

/// VLC item options for a resolved link.
pub fn item_options(r: &Resolved) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(audio) = &r.audio {
        // VLC separates several slaves with '#'.
        out.push(format!("input-slave={}", audio.replace('#', "%23")));
    }
    if let Some(ua) = &r.user_agent {
        out.push(format!("http-user-agent={ua}"));
    }
    if let Some(referer) = &r.referer {
        out.push(format!("http-referrer={referer}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_link_comes_after_the_options() {
        let a = args("-x https://example.org/v");
        assert_eq!(a.last().map(String::as_str), Some("-x https://example.org/v"));
        assert_eq!(a[a.len() - 2], "--");
    }

    #[test]
    fn a_single_file_is_played_with_its_headers() {
        let json = r#"{"_type": "video", "title": "Clip", "url": "http://h/v.mkv",
            "http_headers": {"User-Agent": "UA/1", "Accept": "*/*"}}"#;
        assert_eq!(
            parse(json).unwrap(),
            Resolved {
                media: "http://h/v.mkv".into(),
                audio: None,
                title: Some("Clip".into()),
                user_agent: Some("UA/1".into()),
                referer: None,
            }
        );
    }

    #[test]
    fn separate_video_and_audio_streams_are_both_used() {
        let json = r#"{"title": "Episode", "url": null, "requested_formats": [
            {"url": "https://cdn/a.m4a", "vcodec": "none", "acodec": "mp4a.40.2",
             "http_headers": {"User-Agent": "UA/a"}},
            {"url": "https://cdn/v.mp4", "vcodec": "avc1.64001F", "acodec": "none",
             "http_headers": {"User-Agent": "UA/v", "Referer": "https://site/watch"}}
        ]}"#;
        let r = parse(json).unwrap();
        assert_eq!(r.media, "https://cdn/v.mp4");
        assert_eq!(r.audio.as_deref(), Some("https://cdn/a.m4a"));
        assert_eq!(r.referer.as_deref(), Some("https://site/watch"));
        assert_eq!(
            item_options(&r),
            vec![
                "input-slave=https://cdn/a.m4a",
                "http-user-agent=UA/v",
                "http-referrer=https://site/watch"
            ]
        );
    }

    #[test]
    fn an_answer_without_a_url_is_an_error() {
        assert!(parse(r#"{"title": "x"}"#).is_err());
        assert!(parse("not json").is_err());
    }

    #[test]
    fn the_error_line_is_kept_short() {
        let stderr = "WARNING: something\nERROR: [generic] x: Unable to download webpage: HTTP \
                      Error 404\n";
        assert_eq!(
            error_message(stderr),
            "[generic] x: Unable to download webpage: HTTP Error 404"
        );
        assert_eq!(error_message(""), "failed");
        assert!(error_message(&format!("ERROR: {}", "a".repeat(500))).ends_with('…'));
    }
}
