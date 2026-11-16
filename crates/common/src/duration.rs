//! Media duration from container headers, without decoding: Matroska/WebM (`Segment/Info/
//! Duration`), MP4/MOV (`moov/mvhd`, or `mvex/mehd` when fragmented) and AVI (frames of
//! the video stream, OpenDML total for big files). Other formats (MPEG-TS, …) give `None`:
//! the server then relies on what the players report.
//!
//! Only headers are read, never the media data. Sizes and loop counts are bounded, so a
//! broken file gives `None` rather than a long read.

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
}
