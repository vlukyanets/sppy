//! The line protocol between sppy and its Lua module inside VLC (`sppy.lua`).
//!
//! Fields are separated by tabs, one line per command and one per reply. Numbers are
//! integers: VLC applies the system locale, under which Lua reads and prints fractions
//! with a comma.

use std::path::Path;

/// Version the module sends in its hello; bumped with every change of the protocol.
pub const MODULE_VERSION: &str = "2";

/// VLC's input state (`input_state_e`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputState {
    Opening,
    Playing,
    Paused,
    Ended,
    Error,
}

/// What the module reports about the current input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub time_us: u64,
    /// 0 when VLC does not know the length (streams).
    pub length_us: u64,
    pub state: InputState,
    /// Playback rate in millionths (1.0 = 1_000_000).
    pub rate_ppm: u64,
    /// Input cache fill in thousandths; below 1000 while VLC buffers.
    pub cache_permille: u64,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WireError {
    #[error("unexpected reply from VLC: {0}")]
    Unexpected(String),
    #[error("VLC: {0}")]
    Vlc(String),
    #[error("cannot pass to VLC (control characters): {0}")]
    BadField(String),
}

/// A command for the module.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Status,
    /// Replace the playlist with one item, opened paused.
    Load {
        uri: String,
        audio_languages: String,
        subtitle_languages: String,
        /// Shown instead of the URL's last part (e.g. `videoplayback`); empty: VLC's own.
        title: String,
        /// Further item options, `name=value` (e.g. `input-slave=…`).
        options: Vec<String>,
    },
    /// Add a subtitle to the open input; the last one added is shown.
    Sub(String),
    Seek {
        us: u64,
    },
    Play,
    Pause,
    Rate {
        ppm: u64,
    },
    Osd {
        us: u64,
        text: String,
    },
    /// Empty the playlist: nothing plays while a link is being resolved.
    Clear,
    Quit,
}

fn field(s: &str) -> Result<&str, WireError> {
    if s.chars().any(char::is_control) {
        return Err(WireError::BadField(s.to_owned()));
    }
    Ok(s)
}

/// Text that cannot split a command: control characters become spaces.
fn one_line(s: &str) -> String {
    s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect()
}

impl Command {
    /// The line to send, without the newline.
    pub fn encode(&self) -> Result<String, WireError> {
        Ok(match self {
            Command::Status => "status".into(),
            Command::Load { uri, audio_languages, subtitle_languages, title, options } => {
                let mut line = format!(
                    "load\t{}\t{}\t{}\t{}",
                    field(uri)?,
                    field(audio_languages)?,
                    field(subtitle_languages)?,
                    one_line(title)
                );
                for o in options {
                    line.push('\t');
                    line.push_str(field(o)?);
                }
                line
            }
            Command::Sub(uri) => format!("sub\t{}", field(uri)?),
            Command::Seek { us } => format!("seek\t{us}"),
            Command::Play => "play".into(),
            Command::Pause => "pause".into(),
            Command::Rate { ppm } => format!("rate\t{ppm}"),
            // A message is shown on one line anyway.
            Command::Osd { us, text } => format!("osd\t{us}\t{}", one_line(text)),
            Command::Clear => "clear".into(),
            Command::Quit => "quit".into(),
        })
    }
}

/// The module's hello: `hello <secret> <version>`.
pub fn parse_hello(line: &str) -> Option<(&str, &str)> {
    let mut f = line.split('\t');
    (f.next() == Some("hello")).then_some(())?;
    let secret = f.next()?;
    let version = f.next()?;
    Some((secret, version))
}

/// The reply to anything but `status`.
pub fn parse_ack(line: &str) -> Result<(), WireError> {
    if line == "ok" {
        return Ok(());
    }
    match line.strip_prefix("error\t") {
        Some(msg) => Err(WireError::Vlc(msg.to_owned())),
        None => Err(WireError::Unexpected(line.to_owned())),
    }
}

/// The reply to `status`: `None` when nothing is open.
pub fn parse_status(line: &str) -> Result<Option<Status>, WireError> {
    if let Some(msg) = line.strip_prefix("error\t") {
        return Err(WireError::Vlc(msg.to_owned()));
    }
    let bad = || WireError::Unexpected(line.to_owned());
    let rest = line.strip_prefix("status\t").ok_or_else(bad)?;
    if rest == "none" {
        return Ok(None);
    }
    let mut f = rest.splitn(6, '\t');
    let mut num = || -> Result<i64, WireError> {
        f.next().and_then(|s| s.parse::<i64>().ok()).ok_or_else(bad)
    };
    let time = num()?;
    let length = num()?;
    let state = match num()? {
        0 | 1 => InputState::Opening,
        2 => InputState::Playing,
        3 => InputState::Paused,
        4 => InputState::Ended,
        _ => InputState::Error,
    };
    let rate = num()?;
    let cache = num()?;
    let title = f.next().unwrap_or("").to_owned();
    let clamp = |v: i64| u64::try_from(v).unwrap_or(0);
    Ok(Some(Status {
        time_us: clamp(time),
        length_us: clamp(length),
        state,
        rate_ppm: clamp(rate),
        cache_permille: clamp(cache),
        title,
    }))
}

pub fn speed_to_ppm(speed: f64) -> u64 {
    if speed.is_finite() && speed > 0.0 { (speed * 1_000_000.0).round() as u64 } else { 1_000_000 }
}

pub fn ppm_to_speed(ppm: u64) -> f64 {
    ppm as f64 / 1_000_000.0
}

/// Characters a URI path keeps as they are.
fn keep(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'/')
}

fn percent_encode(bytes: &[u8], out: &mut String) {
    for &b in bytes {
        if keep(b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
}

/// `file://` URI of an absolute Unix path; any bytes, so non-UTF-8 names open too.
pub fn unix_file_uri(path: &[u8]) -> String {
    let mut out = String::from("file://");
    percent_encode(path, &mut out);
    out
}

/// `file://` URI of an absolute Windows path (`C:\…` or `\\server\share\…`).
pub fn windows_file_uri(path: &str) -> String {
    let path = path.replace('\\', "/");
    let mut out = String::from("file:");
    if let Some(unc) = path.strip_prefix("//") {
        out.push_str("//");
        percent_encode(unc.as_bytes(), &mut out);
    } else {
        out.push_str("///");
        // The drive colon stays as it is.
        match path.split_once(':') {
            Some((drive, rest)) if drive.len() == 1 => {
                out.push_str(drive);
                out.push(':');
                percent_encode(rest.as_bytes(), &mut out);
            }
            _ => percent_encode(path.as_bytes(), &mut out),
        }
    }
    out
}

/// `file://` URI of a local path on this system.
pub fn file_uri(path: &Path) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        unix_file_uri(path.as_os_str().as_bytes())
    }
    #[cfg(not(unix))]
    {
        windows_file_uri(&path.to_string_lossy())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_are_tab_separated_integers() {
        assert_eq!(Command::Seek { us: 1_500_000 }.encode().unwrap(), "seek\t1500000");
        assert_eq!(Command::Rate { ppm: speed_to_ppm(1.05) }.encode().unwrap(), "rate\t1050000");
        let load = Command::Load {
            uri: "file:///a.mkv".into(),
            audio_languages: "ru,rus".into(),
            subtitle_languages: String::new(),
            title: String::new(),
            options: vec![],
        };
        assert_eq!(load.encode().unwrap(), "load\tfile:///a.mkv\tru,rus\t\t");
        let link = Command::Load {
            uri: "https://cdn/v.mp4".into(),
            audio_languages: String::new(),
            subtitle_languages: String::new(),
            title: "Ep 1\nfinale".into(),
            options: vec!["input-slave=https://cdn/a.m4a".into()],
        };
        assert_eq!(
            link.encode().unwrap(),
            "load\thttps://cdn/v.mp4\t\t\tEp 1 finale\tinput-slave=https://cdn/a.m4a"
        );
        let bad = Command::Load {
            uri: "https://cdn/v.mp4".into(),
            audio_languages: String::new(),
            subtitle_languages: String::new(),
            title: String::new(),
            options: vec!["a=1\tquit".into()],
        };
        assert!(matches!(bad.encode(), Err(WireError::BadField(_))));
    }

    #[test]
    fn control_characters_cannot_split_a_command() {
        let bad = Command::Sub("http://x/a\nquit".into());
        assert!(matches!(bad.encode(), Err(WireError::BadField(_))));
        let osd = Command::Osd { us: 2_000_000, text: "two\nlines\tand tab".into() };
        assert_eq!(osd.encode().unwrap(), "osd\t2000000\ttwo lines and tab");
    }

    #[test]
    fn status_with_title_containing_tabs_after_the_numbers() {
        let s = parse_status("status\t1500000\t10000000\t2\t1050420\t1000\tEp 1\tpart").unwrap();
        assert_eq!(
            s,
            Some(Status {
                time_us: 1_500_000,
                length_us: 10_000_000,
                state: InputState::Playing,
                rate_ppm: 1_050_420,
                cache_permille: 1000,
                title: "Ep 1\tpart".into(),
            })
        );
        assert_eq!(parse_status("status\tnone").unwrap(), None);
        assert!(parse_status("status\t1,5\t0\t2\t1\t1\t").is_err());
        assert!(matches!(parse_status("error\tboom"), Err(WireError::Vlc(m)) if m == "boom"));
    }

    #[test]
    fn states_map_from_vlc_numbers() {
        let state = |n: u8| {
            parse_status(&format!("status\t0\t0\t{n}\t1000000\t1000\t")).unwrap().unwrap().state
        };
        assert_eq!(state(0), InputState::Opening);
        assert_eq!(state(1), InputState::Opening);
        assert_eq!(state(3), InputState::Paused);
        assert_eq!(state(4), InputState::Ended);
        assert_eq!(state(5), InputState::Error);
    }

    #[test]
    fn hello_and_acks() {
        assert_eq!(parse_hello("hello\tabc\t1"), Some(("abc", "1")));
        assert_eq!(parse_hello("hullo\tabc\t1"), None);
        assert!(parse_ack("ok").is_ok());
        assert_eq!(
            parse_ack("error\tnothing is open"),
            Err(WireError::Vlc("nothing is open".into()))
        );
    }

    #[test]
    fn file_uris_escape_everything_but_plain_path_characters() {
        assert_eq!(
            unix_file_uri(b"/media/S01 E01 [x].mkv"),
            "file:///media/S01%20E01%20%5Bx%5D.mkv"
        );
        assert_eq!(
            unix_file_uri("/v/Сезон.mkv".as_bytes()),
            "file:///v/%D0%A1%D0%B5%D0%B7%D0%BE%D0%BD.mkv"
        );
        assert_eq!(unix_file_uri(b"/v/\xff.mkv"), "file:///v/%FF.mkv");
        assert_eq!(unix_file_uri(b"/v/100%#?.mkv"), "file:///v/100%25%23%3F.mkv");
        assert_eq!(windows_file_uri(r"C:\Videos\a b.mkv"), "file:///C:/Videos/a%20b.mkv");
        assert_eq!(windows_file_uri(r"\\nas\share\a.mkv"), "file://nas/share/a.mkv");
    }
}
