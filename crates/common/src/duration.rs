//! Media duration without decoding: from container headers for Matroska/WebM (`Segment/
//! Info/Duration`), MP4/MOV (`moov/mvhd`, or `mvex/mehd` when fragmented) and AVI (frames
//! of the video stream, OpenDML total for big files); from the timestamps at both ends for
//! MPEG-TS, which has no header that says it. Other formats give `None`: the server then
//! relies on what the players report.
//!
//! Only headers and a few small windows are read, never the whole file. Sizes and loop
//! counts are bounded, so a broken file gives `None` rather than a long read.

use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

/// More elements than any sane header has.
const MAX_ELEMENTS: usize = 10_000;
/// Header elements we read whole (Info, SeekHead, mvhd, …) are small.
const MAX_HEADER_BYTES: u64 = 1 << 20;

/// Duration in milliseconds, if the container header says it.
pub fn duration_ms(path: &Path) -> Option<u64> {
    let file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    from_reader(&mut BufReader::new(file), len)
}

/// [`duration_ms`] for any seekable source of `len` bytes.
pub fn from_reader<R: Read + Seek>(r: &mut R, len: u64) -> Option<u64> {
    let mut magic = [0u8; 12];
    r.seek(SeekFrom::Start(0)).ok()?;
    r.read_exact(&mut magic).ok()?;
    if magic[..4] == [0x1A, 0x45, 0xDF, 0xA3] {
        matroska(r, len)
    } else if &magic[..4] == b"RIFF" && &magic[8..12] == b"AVI " {
        avi(r, len)
    } else if matches!(&magic[4..8], b"ftyp" | b"moov" | b"mdat" | b"free" | b"wide" | b"skip") {
        mp4(r, len)
    } else if magic[0] == SYNC || magic[4] == SYNC {
        mpeg_ts(r, len)
    } else {
        None
    }
}

// ---- Matroska / WebM -------------------------------------------------------------------

const EBML_HEADER: u32 = 0x1A45_DFA3;
const SEGMENT: u32 = 0x1853_8067;
const SEEK_HEAD: u32 = 0x114D_9B74;
const SEEK: u32 = 0x4DBB;
const SEEK_ID: u32 = 0x53AB;
const SEEK_POSITION: u32 = 0x53AC;
const INFO: u32 = 0x1549_A966;
const TIMESTAMP_SCALE: u32 = 0x2A_D7B1;
const DURATION: u32 = 0x4489;
const CLUSTER: u32 = 0x1F43_B675;

/// An element id with its length marker kept (as the spec writes them), 1–4 bytes.
fn ebml_id<R: Read>(r: &mut R) -> Option<u32> {
    let first = read_u8(r)?;
    let len = first.leading_zeros() as usize + 1;
    if len > 4 {
        return None;
    }
    let mut id = u32::from(first);
    for _ in 1..len {
        id = (id << 8) | u32::from(read_u8(r)?);
    }
    Some(id)
}

/// An element size, 1–8 bytes; `None` inside for "unknown size" (all value bits set).
fn ebml_size<R: Read>(r: &mut R) -> Option<Option<u64>> {
    let first = read_u8(r)?;
    let len = first.leading_zeros() as usize + 1;
    if len > 8 {
        return None;
    }
    let mut value = u64::from(first) & (0xFF >> len);
    let mut all_ones = value == (0xFF >> len);
    for _ in 1..len {
        let b = read_u8(r)?;
        all_ones &= b == 0xFF;
        value = (value << 8) | u64::from(b);
    }
    Some(if all_ones { None } else { Some(value) })
}

fn matroska<R: Read + Seek>(r: &mut R, len: u64) -> Option<u64> {
    r.seek(SeekFrom::Start(0)).ok()?;
    // EBML header, then the segment.
    let mut pos = 0u64;
    for _ in 0..MAX_ELEMENTS {
        if pos >= len {
            return None;
        }
        let id = ebml_id(r)?;
        let size = ebml_size(r)?;
        let data = r.stream_position().ok()?;
        match (id, size) {
            (SEGMENT, size) => {
                let end = size.map_or(len, |s| data.saturating_add(s).min(len));
                return segment(r, data, end);
            }
            (EBML_HEADER, Some(s)) | (_, Some(s)) => {
                pos = data.checked_add(s)?;
                r.seek(SeekFrom::Start(pos)).ok()?;
            }
            (_, None) => return None,
        }
    }
    None
}

/// Look for Info among the segment's children; once clusters start, follow the SeekHead.
fn segment<R: Read + Seek>(r: &mut R, start: u64, end: u64) -> Option<u64> {
    let mut info_at = None;
    let mut pos = start;
    for _ in 0..MAX_ELEMENTS {
        if pos >= end {
            break;
        }
        r.seek(SeekFrom::Start(pos)).ok()?;
        let id = ebml_id(r)?;
        let size = ebml_size(r)?;
        let data = r.stream_position().ok()?;
        match (id, size) {
            (INFO, Some(s)) => return info(r, data, s),
            (SEEK_HEAD, Some(s)) => info_at = info_at.or(seek_head(r, data, s, start)),
            (CLUSTER, _) | (_, None) => break,
            _ => {}
        }
        pos = data.checked_add(size?)?;
    }
    let at = info_at?;
    r.seek(SeekFrom::Start(at)).ok()?;
    let (INFO, Some(s)) = (ebml_id(r)?, ebml_size(r)?) else { return None };
    let data = r.stream_position().ok()?;
    info(r, data, s)
}

/// Absolute position of the Info element from a SeekHead.
fn seek_head<R: Read + Seek>(r: &mut R, data: u64, size: u64, segment: u64) -> Option<u64> {
    let body = read_body(r, data, size)?;
    let mut c = io::Cursor::new(body);
    while (c.position() as usize) < c.get_ref().len() {
        let id = ebml_id(&mut c)?;
        let s = ebml_size(&mut c)??;
        let child_end = c.position().checked_add(s)?;
        if id == SEEK {
            let (mut target, mut position) = (None, None);
            while c.position() < child_end {
                let cid = ebml_id(&mut c)?;
                let cs = ebml_size(&mut c)??;
                let bytes = take(&mut c, cs)?;
                match cid {
                    SEEK_ID => {
                        target = Some(bytes.iter().fold(0u32, |a, b| (a << 8) | u32::from(*b)))
                    }
                    SEEK_POSITION => position = Some(be_uint(&bytes)?),
                    _ => {}
                }
            }
            if target == Some(INFO) {
                return segment.checked_add(position?);
            }
        }
        c.set_position(child_end);
    }
    None
}

fn info<R: Read + Seek>(r: &mut R, data: u64, size: u64) -> Option<u64> {
    let body = read_body(r, data, size)?;
    let mut c = io::Cursor::new(body);
    let (mut scale, mut duration) = (1_000_000u64, None);
    while (c.position() as usize) < c.get_ref().len() {
        let id = ebml_id(&mut c)?;
        let s = ebml_size(&mut c)??;
        let bytes = take(&mut c, s)?;
        match id {
            TIMESTAMP_SCALE => scale = be_uint(&bytes)?,
            DURATION => duration = Some(be_float(&bytes)?),
            _ => {}
        }
    }
    let ms = duration? * scale as f64 / 1_000_000.0;
    (ms.is_finite() && ms > 0.0 && ms < u64::MAX as f64).then_some(ms.round() as u64)
}

// ---- MP4 / MOV -------------------------------------------------------------------------

/// Box header: (type, absolute start of the payload, absolute end).
fn mp4_box<R: Read + Seek>(r: &mut R, at: u64, limit: u64) -> Option<([u8; 4], u64, u64)> {
    r.seek(SeekFrom::Start(at)).ok()?;
    let mut head = [0u8; 8];
    r.read_exact(&mut head).ok()?;
    let size = u64::from(u32::from_be_bytes(head[..4].try_into().ok()?));
    let kind: [u8; 4] = head[4..8].try_into().ok()?;
    let (payload, end) = match size {
        0 => (at + 8, limit),
        1 => {
            let mut large = [0u8; 8];
            r.read_exact(&mut large).ok()?;
            (at + 16, at.checked_add(u64::from_be_bytes(large))?)
        }
        s if s < 8 => return None,
        s => (at + 8, at.checked_add(s)?),
    };
    (end <= limit && payload <= end).then_some((kind, payload, end))
}

/// The first child box of `kind` within `[start, end)`.
fn find_box<R: Read + Seek>(r: &mut R, start: u64, end: u64, kind: &[u8; 4]) -> Option<(u64, u64)> {
    let mut at = start;
    for _ in 0..MAX_ELEMENTS {
        if at.checked_add(8)? > end {
            return None;
        }
        let (k, payload, box_end) = mp4_box(r, at, end)?;
        if &k == kind {
            return Some((payload, box_end));
        }
        at = box_end;
    }
    None
}

fn mp4<R: Read + Seek>(r: &mut R, len: u64) -> Option<u64> {
    let (moov, moov_end) = find_box(r, 0, len, b"moov")?;
    let (mvhd, mvhd_end) = find_box(r, moov, moov_end, b"mvhd")?;
    let body = read_body(r, mvhd, mvhd_end - mvhd)?;
    let version = *body.first()?;
    let (timescale, duration) = match version {
        0 => (be_uint(body.get(12..16)?)?, be_uint(body.get(16..20)?)?),
        1 => (be_uint(body.get(20..24)?)?, be_uint(body.get(24..32)?)?),
        _ => return None,
    };
    let unknown = duration == 0 || duration == u64::from(u32::MAX) || duration == u64::MAX;
    let duration = if unknown { fragmented(r, moov, moov_end)? } else { duration };
    if timescale == 0 {
        return None;
    }
    Some(u64::try_from(u128::from(duration) * 1000 / u128::from(timescale)).ok()?)
        .filter(|ms| *ms > 0)
}

/// Fragmented MP4: `moov/mvex/mehd` holds the whole duration (in the movie timescale).
fn fragmented<R: Read + Seek>(r: &mut R, moov: u64, moov_end: u64) -> Option<u64> {
    let (mvex, mvex_end) = find_box(r, moov, moov_end, b"mvex")?;
    let (mehd, mehd_end) = find_box(r, mvex, mvex_end, b"mehd")?;
    let body = read_body(r, mehd, mehd_end - mehd)?;
    match *body.first()? {
        0 => be_uint(body.get(4..8)?),
        1 => be_uint(body.get(4..12)?),
        _ => None,
    }
    .filter(|d| *d > 0)
}

// ---- AVI -------------------------------------------------------------------------------

/// RIFF chunks in `body`: `(id, data)`; a LIST's data starts with its 4-byte kind.
fn riff_chunks(body: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while out.len() < MAX_ELEMENTS && at + 8 <= body.len() {
        let id: [u8; 4] = body[at..at + 4].try_into().expect("4 bytes");
        let size = u32::from_le_bytes(body[at + 4..at + 8].try_into().expect("4 bytes")) as usize;
        let Some(data) = body.get(at + 8..at + 8 + size) else { break };
        out.push((id, data));
        at += 8 + size + size % 2;
    }
    out
}

fn le_u32(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?)))
}

fn avi<R: Read + Seek>(r: &mut R, len: u64) -> Option<u64> {
    // The header list comes first, right after "RIFF size AVI ".
    let mut head = [0u8; 12];
    r.seek(SeekFrom::Start(12)).ok()?;
    r.read_exact(&mut head).ok()?;
    if &head[..4] != b"LIST" || &head[8..12] != b"hdrl" {
        return None;
    }
    let size = le_u32(&head, 4)?.checked_sub(4)?;
    if 24 + size > len {
        return None;
    }
    let hdrl = read_body(r, 24, size)?;
    let (mut us_per_frame, mut frames) = (0u64, 0u64);
    let mut video: Option<(u64, u64, u64)> = None;
    for (id, data) in riff_chunks(&hdrl) {
        match (&id, data.get(..4)) {
            (b"avih", _) => {
                us_per_frame = le_u32(data, 0)?;
                frames = frames.max(le_u32(data, 16)?);
            }
            (b"LIST", Some(b"strl")) => {
                for (cid, s) in riff_chunks(&data[4..]) {
                    if &cid == b"strh" && s.get(..4) == Some(b"vids") && video.is_none() {
                        video = Some((le_u32(s, 20)?, le_u32(s, 24)?, le_u32(s, 32)?));
                    }
                }
            }
            (b"LIST", Some(b"odml")) => {
                for (cid, d) in riff_chunks(&data[4..]) {
                    if &cid == b"dmlh" {
                        frames = frames.max(le_u32(d, 0)?);
                    }
                }
            }
            _ => {}
        }
    }
    let ms = match video {
        Some((scale, rate, length)) if scale > 0 && rate > 0 => {
            u128::from(frames.max(length)) * u128::from(scale) * 1000 / u128::from(rate)
        }
        _ => u128::from(frames) * u128::from(us_per_frame) / 1000,
    };
    u64::try_from(ms).ok().filter(|ms| *ms > 0)
}

// ---- MPEG-TS ---------------------------------------------------------------------------

const SYNC: u8 = 0x47;
/// Bytes read at each end of the file.
const TS_END_WINDOW: u64 = 1 << 20;
/// Bytes read at each point checked in between.
const TS_SAMPLE: u64 = 64 << 10;
/// Points checked in between, in files big enough for them to say something.
const TS_SAMPLES: u64 = 7;
/// PTS are 33-bit counters of a 90 kHz clock.
const PTS_WRAP: i64 = 1 << 33;
const TICKS_PER_SECOND: i64 = 90_000;
/// Frames come out of order (B-frames), but not by this much.
const MAX_BACK: i64 = TICKS_PER_SECOND;
/// Neither do neighbouring PES of one stream lie this far apart.
const MAX_GAP: i64 = 10 * TICKS_PER_SECOND;

/// How a PES is played, best for timing first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum EsKind {
    Video,
    Audio,
    Other,
}

/// A PES start with its presentation timestamp.
#[derive(Debug, Clone, Copy)]
struct Pes {
    pid: u16,
    kind: EsKind,
    pts: i64,
}

/// Packet size and where the sync byte is in it: plain TS (188) or M2TS/BDAV (192, with a
/// 4-byte timecode first). Three packets in a row must start right.
fn ts_layout(head: &[u8]) -> Option<(usize, usize)> {
    [(188, 0), (192, 4)]
        .into_iter()
        .find(|(size, at)| (0..3).all(|k| head.get(at + k * size) == Some(&SYNC)))
}

/// PES starts with a PTS in `buf`, which begins at a packet boundary.
fn pes_starts(buf: &[u8], (size, at): (usize, usize)) -> Vec<Pes> {
    buf.chunks_exact(size).filter_map(|p| pes_start(p.get(at..at + 188)?)).collect()
}

fn pes_start(t: &[u8]) -> Option<Pes> {
    let error = t[1] & 0x80 != 0;
    let unit_start = t[1] & 0x40 != 0;
    let scrambled = t[3] & 0xC0 != 0;
    let has_payload = t[3] & 0x10 != 0;
    if t[0] != SYNC || error || !unit_start || scrambled || !has_payload {
        return None;
    }
    let mut payload = 4;
    if t[3] & 0x20 != 0 {
        payload += 1 + usize::from(t[4]);
    }
    let pes = t.get(payload..payload + 14)?;
    if pes[..3] != [0, 0, 1] || pes[6] & 0xC0 != 0x80 || pes[7] & 0x80 == 0 {
        return None;
    }
    let kind = match pes[3] {
        0xE0..=0xEF | 0xFD => EsKind::Video,
        0xC0..=0xDF => EsKind::Audio,
        0xBD => EsKind::Other,
        _ => return None,
    };
    let b = &pes[9..14];
    if b[0] & 1 == 0 || b[2] & 1 == 0 || b[4] & 1 == 0 {
        return None;
    }
    let pts = (i64::from(b[0] >> 1) & 0x07) << 30
        | i64::from(b[1]) << 22
        | i64::from(b[2] >> 1) << 15
        | i64::from(b[3]) << 7
        | i64::from(b[4] >> 1);
    Some(Pes { pid: u16::from(t[1] & 0x1F) << 8 | u16::from(t[2]), kind, pts })
}

/// Read `size` bytes from the packet boundary at or before `pos`.
fn ts_window<R: Read + Seek>(
    r: &mut R,
    pos: u64,
    size: u64,
    len: u64,
    layout: (usize, usize),
) -> Option<Vec<u8>> {
    let start = pos - pos % layout.0 as u64;
    read_body(r, start, size.min(len - start))
}

/// From the first frame to the start of the last one (one frame short of what a player
/// shows; it does not matter for ending an item).
///
/// Glued recordings restart or jump their timestamps somewhere in the middle, and then
/// the ends say nothing about the length. So the timestamps must go steadily forward inside
/// every window read and from one point checked to the next, and the result must agree
/// with the file size at the bitrate seen at the ends; otherwise `None`, since a duration
/// that is too short would end the item early.
fn mpeg_ts<R: Read + Seek>(r: &mut R, len: u64) -> Option<u64> {
    let head = read_body(r, 0, len.min(TS_END_WINDOW))?;
    let layout = ts_layout(&head)?;
    let first = pes_starts(&head, layout);
    // The timing stream: the first video one, else audio, else any.
    let best = first.iter().map(|p| p.kind).min()?;
    let pid = first.iter().find(|p| p.kind == best)?.pid;
    let base = first.iter().find(|p| p.pid == pid)?.pts;
    // Relative to `base`, signed: the counter may wrap, and B-frames come out of order.
    let rel = |pts: i64| {
        let d = (pts - base).rem_euclid(PTS_WRAP);
        if d >= PTS_WRAP / 2 { d - PTS_WRAP } else { d }
    };
    // In file order; `None` if they jump inside the window.
    let times = |pes: &[Pes]| {
        let t: Vec<i64> = pes.iter().filter(|p| p.pid == pid).map(|p| rel(p.pts)).collect();
        t.windows(2).all(|w| (-MAX_BACK..=MAX_GAP).contains(&(w[1] - w[0]))).then_some(t)
    };
    let span = |t: &Vec<i64>| t.iter().max().zip(t.iter().min()).map_or(0, |(a, b)| a - b);

    let start: Vec<i64> = times(&first)?;
    let tail = ts_window(r, len.saturating_sub(TS_END_WINDOW), TS_END_WINDOW, len, layout)?;
    let end: Vec<i64> = times(&pes_starts(&tail, layout))?;
    let (from, to) = (*start.iter().min()?, *end.iter().max()?);
    let ticks = to - from;
    if ticks <= 0 {
        return None;
    }

    if len > 4 * TS_END_WINDOW {
        let mut last = from;
        for k in 1..=TS_SAMPLES {
            let pos = len / (TS_SAMPLES + 1) * k;
            let buf = ts_window(r, pos, TS_SAMPLE, len, layout)?;
            let Some(&t) = times(&pes_starts(&buf, layout))?.first() else { continue };
            if t < last - MAX_BACK || t > to + MAX_BACK {
                return None;
            }
            last = t;
        }
    }

    let seen_ticks = span(&start) + span(&end);
    let seen_bytes = (head.len() + tail.len()) as u64;
    if seen_ticks > 0 {
        let estimate = u128::from(len) * seen_ticks as u128 / u128::from(seen_bytes);
        if ticks as u128 > 3 * estimate {
            return None;
        }
    }
    u64::try_from(ticks * 1000 / TICKS_PER_SECOND).ok().filter(|ms| *ms > 0)
}

// ---- helpers ---------------------------------------------------------------------------

fn read_u8<R: Read>(r: &mut R) -> Option<u8> {
    let mut b = [0u8; 1];
    r.read_exact(&mut b).ok()?;
    Some(b[0])
}

fn read_body<R: Read + Seek>(r: &mut R, at: u64, size: u64) -> Option<Vec<u8>> {
    if size > MAX_HEADER_BYTES {
        return None;
    }
    r.seek(SeekFrom::Start(at)).ok()?;
    let mut body = vec![0u8; size as usize];
    r.read_exact(&mut body).ok()?;
    Some(body)
}

fn take(c: &mut io::Cursor<Vec<u8>>, n: u64) -> Option<Vec<u8>> {
    let start = c.position() as usize;
    let end = start.checked_add(usize::try_from(n).ok()?)?;
    let bytes = c.get_ref().get(start..end)?.to_vec();
    c.set_position(end as u64);
    Some(bytes)
}

fn be_uint(bytes: &[u8]) -> Option<u64> {
    (bytes.len() <= 8).then(|| bytes.iter().fold(0u64, |a, b| (a << 8) | u64::from(*b)))
}

fn be_float(bytes: &[u8]) -> Option<f64> {
    match bytes.len() {
        4 => Some(f64::from(f32::from_be_bytes(bytes.try_into().ok()?))),
        8 => Some(f64::from_be_bytes(bytes.try_into().ok()?)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(bytes: &[u8]) -> Option<u64> {
        from_reader(&mut io::Cursor::new(bytes), bytes.len() as u64)
    }

    // -- Matroska builders --

    fn id_bytes(id: u32) -> Vec<u8> {
        id.to_be_bytes().into_iter().skip_while(|b| *b == 0).collect()
    }

    fn el(id: u32, payload: &[u8]) -> Vec<u8> {
        let mut v = id_bytes(id);
        v.push(0x01); // 8-byte size: marker byte, then 7 bytes of value
        v.extend_from_slice(&(payload.len() as u64).to_be_bytes()[1..]);
        v.extend_from_slice(payload);
        v
    }

    fn el_unknown_size(id: u32, payload: &[u8]) -> Vec<u8> {
        let mut v = id_bytes(id);
        v.extend_from_slice(&[0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);
        v.extend_from_slice(payload);
        v
    }

    fn ebml_header() -> Vec<u8> {
        el(EBML_HEADER, &el(0x4282, b"matroska"))
    }

    fn info_el(scale: Option<u64>, duration: Option<&[u8]>) -> Vec<u8> {
        let mut body = Vec::new();
        if let Some(s) = scale {
            body.extend(el(TIMESTAMP_SCALE, &s.to_be_bytes()));
        }
        if let Some(d) = duration {
            body.extend(el(DURATION, d));
        }
        el(INFO, &body)
    }

    fn mkv(segment_children: &[Vec<u8>]) -> Vec<u8> {
        let mut v = ebml_header();
        v.extend(el(SEGMENT, &segment_children.concat()));
        v
    }

    #[test]
    fn matroska_duration_in_the_default_scale() {
        let f = mkv(&[info_el(None, Some(&5_400_000.0f64.to_be_bytes()))]);
        assert_eq!(parse(&f), Some(5_400_000));
    }

    #[test]
    fn matroska_float32_and_custom_scale() {
        // 2700 ticks of 2 ms each = 5.4 s.
        let f = mkv(&[info_el(Some(2_000_000), Some(&2_700.0f32.to_be_bytes()))]);
        assert_eq!(parse(&f), Some(5_400));
    }

    #[test]
    fn matroska_info_after_the_clusters_is_found_through_the_seek_head() {
        let cluster = el(CLUSTER, &[0u8; 64]);
        let info = info_el(None, Some(&1_234.0f64.to_be_bytes()));
        // SeekHead (fixed size) | Cluster | Info; positions are relative to the segment data.
        let seek_head_len = el(
            SEEK_HEAD,
            &el(SEEK, &[el(SEEK_ID, &id_bytes(INFO)), el(SEEK_POSITION, &[0u8; 8])].concat()),
        )
        .len();
        let info_pos = (seek_head_len + cluster.len()) as u64;
        let seek_head = el(
            SEEK_HEAD,
            &el(
                SEEK,
                &[el(SEEK_ID, &id_bytes(INFO)), el(SEEK_POSITION, &info_pos.to_be_bytes())]
                    .concat(),
            ),
        );
        assert_eq!(seek_head.len(), seek_head_len);
        assert_eq!(parse(&mkv(&[seek_head, cluster, info])), Some(1_234));
    }

    #[test]
    fn matroska_live_stream_has_no_duration() {
        let mut f = ebml_header();
        f.extend(el_unknown_size(SEGMENT, &[info_el(None, None), el(CLUSTER, &[0; 8])].concat()));
        assert_eq!(parse(&f), None);
        let mut with = ebml_header();
        with.extend(el_unknown_size(SEGMENT, &info_el(None, Some(&42.0f64.to_be_bytes()))));
        assert_eq!(parse(&with), Some(42), "an unknown-size segment still has its Info");
    }

    #[test]
    fn broken_matroska_gives_none() {
        let f = mkv(&[info_el(None, Some(&5_400_000.0f64.to_be_bytes()))]);
        for cut in [5, 20, f.len() - 3] {
            assert_eq!(parse(&f[..cut]), None, "cut at {cut}");
        }
        assert_eq!(parse(&mkv(&[info_el(None, Some(&f64::NAN.to_be_bytes()))])), None);
        assert_eq!(parse(&mkv(&[info_el(None, Some(&(-5.0f64).to_be_bytes()))])), None);
        assert_eq!(parse(&mkv(&[info_el(None, Some(&[1, 2, 3]))])), None);
    }

    // -- MP4 builders --

    fn mp4_box(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut v = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(kind);
        v.extend_from_slice(payload);
        v
    }

    fn mvhd_v0(timescale: u32, duration: u32) -> Vec<u8> {
        let mut p = vec![0u8; 4 + 8];
        p.extend(timescale.to_be_bytes());
        p.extend(duration.to_be_bytes());
        p.extend([0u8; 80]);
        mp4_box(b"mvhd", &p)
    }

    fn mvhd_v1(timescale: u32, duration: u64) -> Vec<u8> {
        let mut p = vec![1u8, 0, 0, 0];
        p.extend([0u8; 16]);
        p.extend(timescale.to_be_bytes());
        p.extend(duration.to_be_bytes());
        p.extend([0u8; 80]);
        mp4_box(b"mvhd", &p)
    }

    fn ftyp() -> Vec<u8> {
        mp4_box(b"ftyp", b"isom\0\0\0\0isom")
    }

    #[test]
    fn mp4_duration_with_moov_after_the_media_data() {
        let f = [ftyp(), mp4_box(b"mdat", &[7u8; 1000]), mp4_box(b"moov", &mvhd_v0(1000, 90_000))]
            .concat();
        assert_eq!(parse(&f), Some(90_000));
    }

    #[test]
    fn mp4_version_1_and_other_timescales() {
        let f = [ftyp(), mp4_box(b"moov", &mvhd_v1(90_000, 90_000 * 3_600))].concat();
        assert_eq!(parse(&f), Some(3_600_000));
        let mov = [mp4_box(b"wide", &[]), mp4_box(b"moov", &mvhd_v0(600, 1_500))].concat();
        assert_eq!(parse(&mov), Some(2_500));
    }

    #[test]
    fn fragmented_mp4_uses_mehd() {
        let mehd = mp4_box(b"mehd", &[[0u8; 4].as_slice(), &45_000u32.to_be_bytes()].concat());
        let moov = [mvhd_v0(1000, 0), mp4_box(b"mvex", &mehd)].concat();
        assert_eq!(parse(&[ftyp(), mp4_box(b"moov", &moov)].concat()), Some(45_000));
        let no_mehd = [ftyp(), mp4_box(b"moov", &mvhd_v0(1000, 0))].concat();
        assert_eq!(parse(&no_mehd), None);
    }

    #[test]
    fn broken_or_unknown_files_give_none() {
        let f = [ftyp(), mp4_box(b"moov", &mvhd_v0(1000, 90_000))].concat();
        assert_eq!(parse(&f[..f.len() - 10]), None);
        assert_eq!(parse(&[ftyp(), mp4_box(b"moov", &mvhd_v0(0, 5))].concat()), None);
        assert_eq!(parse(&[ftyp(), mp4_box(b"moov", &mvhd_v0(1000, u32::MAX))].concat()), None);
        let mut bad_size = ftyp();
        bad_size.extend([0, 0, 0, 3, b'm', b'o', b'o', b'v']);
        assert_eq!(parse(&bad_size), None);
        assert_eq!(parse(b"RIFF\0\0\0\0WAVEfmt "), None);
        assert_eq!(parse(b"short"), None);
        assert_eq!(parse(&[]), None);
    }

    // -- AVI builders --

    fn chunk(id: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut v = id.to_vec();
        v.extend((data.len() as u32).to_le_bytes());
        v.extend_from_slice(data);
        if data.len() % 2 == 1 {
            v.push(0);
        }
        v
    }

    fn list(kind: &[u8; 4], children: &[Vec<u8>]) -> Vec<u8> {
        chunk(b"LIST", &[kind.as_slice(), &children.concat()].concat())
    }

    fn avih(us_per_frame: u32, frames: u32) -> Vec<u8> {
        let mut d = vec![0u8; 56];
        d[0..4].copy_from_slice(&us_per_frame.to_le_bytes());
        d[16..20].copy_from_slice(&frames.to_le_bytes());
        chunk(b"avih", &d)
    }

    fn strh(kind: &[u8; 4], scale: u32, rate: u32, length: u32) -> Vec<u8> {
        let mut d = vec![0u8; 56];
        d[0..4].copy_from_slice(kind);
        d[20..24].copy_from_slice(&scale.to_le_bytes());
        d[24..28].copy_from_slice(&rate.to_le_bytes());
        d[32..36].copy_from_slice(&length.to_le_bytes());
        chunk(b"strh", &d)
    }

    fn avi(hdrl: &[Vec<u8>]) -> Vec<u8> {
        let body = [b"AVI ".as_slice(), &list(b"hdrl", hdrl), &list(b"movi", &[])].concat();
        chunk(b"RIFF", &body)
    }

    #[test]
    fn avi_duration_from_the_video_stream() {
        let audio = list(b"strl", &[strh(b"auds", 1, 44_100, 330_000)]);
        let video = list(b"strl", &[strh(b"vids", 1, 25, 188)]);
        assert_eq!(parse(&avi(&[avih(40_000, 188), audio, video])), Some(7_520));
        // 30000/1001 fps (NTSC): 1001 / 30000 s per frame.
        let ntsc = list(b"strl", &[strh(b"vids", 1001, 30_000, 54_000)]);
        assert_eq!(parse(&avi(&[avih(33_367, 54_000), ntsc])), Some(1_801_800));
    }

    #[test]
    fn big_avi_counts_all_frames_from_the_opendml_header() {
        let video = list(b"strl", &[strh(b"vids", 1, 25, 1_000)]);
        let odml = list(b"odml", &[chunk(b"dmlh", &250_000u32.to_le_bytes())]);
        assert_eq!(parse(&avi(&[avih(40_000, 1_000), video, odml])), Some(10_000_000));
    }

    #[test]
    fn avi_without_a_video_stream_uses_the_main_header() {
        assert_eq!(parse(&avi(&[avih(40_000, 250)])), Some(10_000));
        assert_eq!(parse(&avi(&[avih(0, 250)])), None);
        assert_eq!(parse(&avi(&[])), None);
        let f = avi(&[avih(40_000, 250)]);
        assert_eq!(parse(&f[..30]), None);
    }

    // -- MPEG-TS builders --

    const VIDEO: u16 = 0x100;
    const AUDIO: u16 = 0x101;
    const SECOND: u64 = 90_000;

    fn pts_bytes(pts: u64) -> [u8; 5] {
        [
            0x21 | (((pts >> 30) as u8 & 0x07) << 1),
            (pts >> 22) as u8,
            (((pts >> 15) as u8) << 1) | 1,
            (pts >> 7) as u8,
            ((pts as u8) << 1) | 1,
        ]
    }

    /// One 188-byte packet starting a PES of `stream_id` with this PTS.
    fn pes_packet(pid: u16, stream_id: u8, pts: u64) -> Vec<u8> {
        let mut p = vec![0x47, 0x40 | (pid >> 8) as u8, pid as u8, 0x10];
        p.extend([0, 0, 1, stream_id, 0, 0, 0x80, 0x80, 5]);
        p.extend(pts_bytes(pts));
        p.resize(188, 0xFF);
        p
    }

    /// A packet of the null PID (stuffing).
    fn null_packet() -> Vec<u8> {
        let mut p = vec![0x47, 0x1F, 0xFF, 0x10];
        p.resize(188, 0xFF);
        p
    }

    /// Video frames every 40 ms with audio beside them, `ms` long, starting at `pts0`;
    /// `filler` null packets after each frame make the file bigger.
    fn ts(pts0: u64, ms: u64, filler: usize) -> Vec<u8> {
        let mut f = Vec::new();
        for i in 0..=ms / 40 {
            let pts = (pts0 + i * 3600) % (1 << 33);
            f.extend(pes_packet(VIDEO, 0xE0, pts));
            f.extend(pes_packet(AUDIO, 0xC0, pts));
            for _ in 0..filler {
                f.extend(null_packet());
            }
        }
        f
    }

    #[test]
    fn mpeg_ts_duration_from_the_timestamps() {
        // From the first frame to the start of the last one.
        assert_eq!(parse(&ts(10 * SECOND, 60_000, 0)), Some(60_000));
        // Bigger than the windows that are read at both ends.
        let big = ts(SECOND, 600_000, 2);
        assert!(big.len() > 3 << 20);
        assert_eq!(parse(&big), Some(600_000));
    }

    #[test]
    fn m2ts_packets_have_a_4_byte_prefix() {
        let plain = ts(SECOND, 30_000, 0);
        let m2ts: Vec<u8> =
            plain.chunks(188).flat_map(|p| [&[0, 0, 0, 0][..], p].concat()).collect();
        assert_eq!(parse(&m2ts), Some(30_000));
    }

    #[test]
    fn mpeg_ts_timestamps_may_wrap_around() {
        let start = (1 << 33) - 5 * SECOND;
        assert_eq!(parse(&ts(start, 20_000, 0)), Some(20_000));
    }

    #[test]
    fn mpeg_ts_b_frames_out_of_order() {
        // Display order 0, 3, 1, 2, …: the first PES is not the earliest frame.
        let mut f = Vec::new();
        for i in [0u64, 3, 1, 2, 6, 4, 5, 9, 7, 8] {
            f.extend(pes_packet(VIDEO, 0xE0, SECOND + i * 9000));
        }
        assert_eq!(parse(&f), Some(900));
    }

    #[test]
    fn mpeg_ts_audio_only() {
        let f: Vec<u8> = (0..=50).flat_map(|i| pes_packet(AUDIO, 0xC0, i * 9000)).collect();
        assert_eq!(parse(&f), Some(5_000));
    }

    /// Recordings glued together restart their timestamps: the difference of the ends
    /// means nothing then. Better no duration than one that ends the video early.
    #[test]
    fn mpeg_ts_with_a_timestamp_jump_gives_none() {
        let mut restarted = ts(100 * SECOND, 300_000, 2);
        restarted.extend(ts(SECOND, 300_000, 2));
        assert_eq!(parse(&restarted), None);
        let mut jumped = ts(SECOND, 300_000, 2);
        jumped.extend(ts(5 * 3600 * SECOND, 300_000, 2));
        assert_eq!(parse(&jumped), None);
        // A short recording glued before a long one: the restart is inside the first
        // window, and both ends alone would give the length of the long one.
        let mut short_first = ts(SECOND, 30_000, 0);
        short_first.extend(ts(SECOND, 600_000, 2));
        assert_eq!(parse(&short_first), None);
        let mut short_last = ts(SECOND, 600_000, 2);
        short_last.extend(ts(SECOND, 30_000, 0));
        assert_eq!(parse(&short_last), None);
    }

    #[test]
    fn not_quite_mpeg_ts_gives_none() {
        let mut bad_sync = ts(SECOND, 10_000, 0);
        bad_sync[188] = 0;
        assert_eq!(parse(&bad_sync), None);
        assert_eq!(parse(&[0x47; 100]), None);
        // Packets without timestamps.
        let nulls: Vec<u8> = (0..100).flat_map(|_| null_packet()).collect();
        assert_eq!(parse(&nulls), None);
        // Scrambled packets are not read.
        let mut scrambled = ts(SECOND, 10_000, 0);
        for p in scrambled.chunks_mut(188) {
            p[3] |= 0x80;
        }
        assert_eq!(parse(&scrambled), None);
    }
}
