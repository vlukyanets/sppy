//! The VLC backend against a real VLC 3. Skipped when VLC is not installed. Unix only: the
//! tests run VLC with its data and config in a temporary folder, which Windows cannot do.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use sppy_player::vlc::{VlcConfig, VlcLauncher, VlcPlayer};
use sppy_player::{MediaSource, Player, PlayerEvent, PlayerLauncher};

fn vlc_available() -> bool {
    std::process::Command::new("vlc").arg("--version").output().is_ok_and(|o| o.status.success())
}

struct Vlc {
    player: VlcPlayer,
    dir: tempfile::TempDir,
}

async fn launch() -> Option<Vlc> {
    launch_with(PathBuf::from("yt-dlp")).await
}

async fn launch_with(ytdlp: PathBuf) -> Option<Vlc> {
    if !vlc_available() {
        eprintln!("vlc not found, skipping");
        return None;
    }
    let dir = tempfile::tempdir().unwrap();
    let cfg = VlcConfig {
        headless: true,
        isolated_home: Some(dir.path().join("home")),
        ytdlp,
        ..VlcConfig::default()
    };
    let player = VlcLauncher::new(cfg).launch().await.expect("launch vlc");
    Some(Vlc { player, dir })
}

/// A short real clip, encoded with whatever is installed.
fn make_clip(dir: &Path, name: &str, secs: u32) -> Option<PathBuf> {
    let out = dir.join(name);
    let mpv = std::process::Command::new("mpv")
        .args(["--no-config", "--no-terminal", "--ao=null", "--of=matroska", "--ovc=mpeg4"])
        .arg(format!("av://lavfi:testsrc=duration={secs}:size=160x120:rate=10"))
        .arg(format!("--o={}", out.display()))
        .status()
        .is_ok_and(|s| s.success());
    let ok = mpv
        || std::process::Command::new("ffmpeg")
            .args(["-loglevel", "error", "-y", "-f", "lavfi", "-i"])
            .arg(format!("testsrc=duration={secs}:size=160x120:rate=10"))
            .args(["-c:v", "mpeg4", "-f", "matroska"])
            .arg(&out)
            .status()
            .is_ok_and(|s| s.success());
    if !ok {
        eprintln!("cannot encode a clip, skipping");
    }
    ok.then_some(out)
}

async fn wait_for(
    p: &mut VlcPlayer,
    secs: u64,
    pred: impl Fn(&PlayerEvent) -> bool,
) -> PlayerEvent {
    tokio::time::timeout(Duration::from_secs(secs), async {
        loop {
            let ev = p.next_event().await.expect("player alive");
            if pred(&ev) {
                return ev;
            }
        }
    })
    .await
    .expect("event did not arrive in time")
}

/// Launch VLC and open a clip of `secs` seconds.
async fn open(secs: u32) -> Option<Vlc> {
    let mut v = launch().await?;
    let clip = make_clip(v.dir.path(), "clip.mkv", secs)?;
    v.player.load(MediaSource::Path(clip), vec![]).await.unwrap();
    wait_for(&mut v.player, 10, |e| matches!(e, PlayerEvent::FileLoaded { .. })).await;
    Some(v)
}

#[tokio::test]
async fn vlc_load_reports_duration_and_title() {
    let Some(mut v) = launch().await else { return };
    let Some(clip) = make_clip(v.dir.path(), "clip.mkv", 3) else { return };
    v.player.load(MediaSource::Path(clip), vec![]).await.unwrap();
    let ev = wait_for(&mut v.player, 10, |e| matches!(e, PlayerEvent::FileLoaded { .. })).await;
    let PlayerEvent::FileLoaded { duration_ms, title } = ev else { unreachable!() };
    let d = duration_ms.expect("duration");
    assert!((2900..=3100).contains(&d), "duration {d}");
    assert_eq!(title.as_deref(), Some("clip.mkv"));
}

#[tokio::test]
async fn vlc_time_follows_the_clock_between_steps() {
    let Some(mut v) = open(10).await else { return };
    v.player.play().await.unwrap();
    wait_for(&mut v.player, 5, |e| matches!(e, PlayerEvent::TimePos(ms) if *ms > 300)).await;
    let start = tokio::time::Instant::now();
    let PlayerEvent::TimePos(first) =
        wait_for(&mut v.player, 5, |e| matches!(e, PlayerEvent::TimePos(_))).await
    else {
        unreachable!()
    };
    // Positions reported 1.5 s apart differ by 1.5 s, steps or not.
    let mut last = first;
    while start.elapsed() < Duration::from_millis(1500) {
        if let PlayerEvent::TimePos(ms) =
            wait_for(&mut v.player, 5, |e| matches!(e, PlayerEvent::TimePos(_))).await
        {
            last = ms;
        }
    }
    let advanced = last - first;
    let wall = start.elapsed().as_millis() as u64;
    assert!(advanced.abs_diff(wall) < 120, "advanced {advanced} ms in {wall} ms");
}

#[tokio::test]
async fn vlc_seek_reports_seeked() {
    let Some(mut v) = open(10).await else { return };
    v.player.seek(6000).await.unwrap();
    let ev = wait_for(&mut v.player, 5, |e| matches!(e, PlayerEvent::Seeked { .. })).await;
    let PlayerEvent::Seeked { position_ms } = ev else { unreachable!() };
    assert!(position_ms.abs_diff(6000) < 100, "seeked to {position_ms}");
}

#[tokio::test]
async fn vlc_pause_and_play_are_reported() {
    let Some(mut v) = open(10).await else { return };
    v.player.play().await.unwrap();
    assert_eq!(
        wait_for(&mut v.player, 5, |e| matches!(e, PlayerEvent::Pause(_))).await,
        PlayerEvent::Pause(false)
    );
    v.player.pause().await.unwrap();
    assert_eq!(
        wait_for(&mut v.player, 5, |e| matches!(e, PlayerEvent::Pause(_))).await,
        PlayerEvent::Pause(true)
    );
}

#[tokio::test]
async fn vlc_speed_is_reported_as_asked() {
    let Some(mut v) = open(10).await else { return };
    v.player.play().await.unwrap();
    v.player.set_speed(1.05).await.unwrap();
    let ev = wait_for(&mut v.player, 5, |e| matches!(e, PlayerEvent::Speed(_))).await;
    assert_eq!(ev, PlayerEvent::Speed(1.05));
}

#[tokio::test]
async fn vlc_reports_the_end() {
    let Some(mut v) = open(3).await else { return };
    v.player.seek(2500).await.unwrap();
    v.player.play().await.unwrap();
    wait_for(&mut v.player, 10, |e| *e == PlayerEvent::Eof).await;
}

#[tokio::test]
async fn vlc_seek_after_the_end_opens_the_item_again() {
    let Some(mut v) = open(3).await else { return };
    v.player.seek(2500).await.unwrap();
    v.player.play().await.unwrap();
    wait_for(&mut v.player, 10, |e| *e == PlayerEvent::Eof).await;
    v.player.seek(1000).await.unwrap();
    let ev = wait_for(&mut v.player, 10, |e| matches!(e, PlayerEvent::Seeked { .. })).await;
    let PlayerEvent::Seeked { position_ms } = ev else { unreachable!() };
    assert!(position_ms.abs_diff(1000) < 100, "seeked to {position_ms}");
}

#[tokio::test]
async fn vlc_load_failure_reported() {
    let Some(mut v) = launch().await else { return };
    let missing = v.dir.path().join("missing.mkv");
    v.player.load(MediaSource::Path(missing), vec![]).await.unwrap();
    wait_for(&mut v.player, 10, |e| matches!(e, PlayerEvent::LoadFailed { .. })).await;
}

#[tokio::test]
async fn vlc_osd_and_subtitles_are_accepted() {
    let Some(mut v) = launch().await else { return };
    // Nothing open, so no video window: the message has nowhere to go, which is no error.
    v.player.osd("Connecting", 1000).await.unwrap();
    let Some(clip) = make_clip(v.dir.path(), "clip.mkv", 3) else { return };
    let srt = v.dir.path().join("clip.en.srt");
    std::fs::write(&srt, "1\n00:00:00,000 --> 00:00:02,000\nhello\n").unwrap();
    v.player.load(MediaSource::Path(clip), vec![MediaSource::Path(srt)]).await.unwrap();
    wait_for(&mut v.player, 10, |e| matches!(e, PlayerEvent::FileLoaded { .. })).await;
    v.player.osd("Hello from sppy", 1000).await.unwrap();
}

#[tokio::test]
async fn vlc_opens_names_with_spaces_and_non_utf8_bytes() {
    use std::os::unix::ffi::OsStrExt;
    let Some(mut v) = launch().await else { return };
    let Some(clip) = make_clip(v.dir.path(), "clip.mkv", 3) else { return };
    let odd = v.dir.path().join(std::ffi::OsStr::from_bytes(b"S01 E01 #1 \xff.mkv"));
    std::fs::rename(&clip, &odd).unwrap();
    v.player.load(MediaSource::Path(odd), vec![]).await.unwrap();
    wait_for(&mut v.player, 10, |e| matches!(e, PlayerEvent::FileLoaded { .. })).await;
}

#[tokio::test]
async fn vlc_module_goes_into_the_isolated_home() {
    let Some(v) = launch().await else { return };
    assert!(v.dir.path().join("home/data/vlc/lua/intf/sppy.lua").is_file());
}

fn ytdlp_available() -> bool {
    std::process::Command::new("yt-dlp").arg("--version").output().is_ok_and(|o| o.status.success())
}

/// Serves one file over plain HTTP on 127.0.0.1, so links work without the internet.
async fn serve_file(file: PathBuf) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let name = file.file_name().unwrap().to_string_lossy().into_owned();
    let path = format!("/{name}");
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let (file, path) = (file.clone(), path.clone());
            tokio::spawn(async move {
                let mut request = Vec::new();
                let mut buf = [0u8; 1024];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut buf).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => request.extend_from_slice(&buf[..n]),
                    }
                }
                let request = String::from_utf8_lossy(&request);
                let mut first = request.lines().next().unwrap_or("").split(' ');
                let (method, target) = (first.next().unwrap_or(""), first.next().unwrap_or(""));
                let reply = if target == path {
                    let body = std::fs::read(&file).unwrap();
                    let mut r = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: video/x-matroska\r\nContent-Length: \
                         {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .into_bytes();
                    if method != "HEAD" {
                        r.extend_from_slice(&body);
                    }
                    r
                } else {
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        .to_vec()
                };
                let _ = stream.write_all(&reply).await;
            });
        }
    });
    format!("http://127.0.0.1:{port}/{name}")
}

#[tokio::test]
async fn vlc_link_is_resolved_with_ytdlp() {
    if !ytdlp_available() {
        eprintln!("yt-dlp not found, skipping");
        return;
    }
    let Some(mut v) = launch().await else { return };
    let Some(clip) = make_clip(v.dir.path(), "clip.mkv", 3) else { return };
    let url = serve_file(clip).await;
    v.player.load(MediaSource::Link(url), vec![]).await.unwrap();
    let ev = wait_for(&mut v.player, 30, |e| matches!(e, PlayerEvent::FileLoaded { .. })).await;
    // yt-dlp's title, not the last part of the URL that VLC shows by itself.
    assert!(matches!(&ev, PlayerEvent::FileLoaded { title: Some(t), .. } if t == "clip"), "{ev:?}");
}

#[tokio::test]
async fn vlc_link_failure_reports_ytdlps_reason() {
    if !ytdlp_available() {
        eprintln!("yt-dlp not found, skipping");
        return;
    }
    let Some(mut v) = launch().await else { return };
    let Some(clip) = make_clip(v.dir.path(), "clip.mkv", 3) else { return };
    let url = serve_file(clip).await.replace("clip.mkv", "missing.mkv");
    v.player.load(MediaSource::Link(url), vec![]).await.unwrap();
    let ev = wait_for(&mut v.player, 30, |e| matches!(e, PlayerEvent::LoadFailed { .. })).await;
    assert!(
        matches!(&ev, PlayerEvent::LoadFailed { message } if message.starts_with("yt-dlp:") && message.contains("404")),
        "{ev:?}"
    );
}

#[tokio::test]
async fn vlc_opens_a_link_itself_without_ytdlp() {
    let Some(mut v) = launch_with(PathBuf::from("/nonexistent/yt-dlp")).await else { return };
    let Some(clip) = make_clip(v.dir.path(), "clip.mkv", 3) else { return };
    let url = serve_file(clip).await;
    v.player.load(MediaSource::Link(url), vec![]).await.unwrap();
    let ev = wait_for(&mut v.player, 10, |e| matches!(e, PlayerEvent::FileLoaded { .. })).await;
    assert!(
        matches!(&ev, PlayerEvent::FileLoaded { title: Some(t), .. } if t == "clip.mkv"),
        "{ev:?}"
    );
}
