//! Works out how long an uploaded sound is (MP3, OGG Vorbis/Opus or WAV),
//! so soundboard clips and entrance sounds can be kept short. Only reads
//! headers and frame sizes; nothing is decoded.

/// The file type for an audio file name, or None if it isn't one we take.
pub fn audio_type(filename: &str) -> Option<&'static str> {
    let ext = filename.rsplit('.').next()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "mp3" => "audio/mpeg",
        "ogg" | "oga" | "opus" => "audio/ogg",
        "wav" => "audio/wav",
        _ => return None,
    })
}

/// Length in milliseconds, or None if the file can't be read as that type.
pub fn duration_ms(data: &[u8], content_type: &str) -> Option<u32> {
    let secs = match content_type {
        "audio/wav" => wav_secs(data),
        "audio/ogg" => ogg_secs(data),
        "audio/mpeg" => mp3_secs(data),
        _ => None,
    }?;
    (secs.is_finite() && secs >= 0.0).then(|| (secs * 1000.0).round() as u32)
}

fn u16le(d: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(d.get(at..at + 2)?.try_into().ok()?))
}

fn u32le(d: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(at..at + 4)?.try_into().ok()?))
}

fn wav_secs(d: &[u8]) -> Option<f64> {
    if d.get(0..4)? != b"RIFF" || d.get(8..12)? != b"WAVE" {
        return None;
    }
    let mut at = 12;
    let mut byte_rate = None;
    while at + 8 <= d.len() {
        let id = &d[at..at + 4];
        let size = u32le(d, at + 4)? as usize;
        let body = at + 8;
        if id == b"fmt " {
            byte_rate = Some(u32le(d, body + 8)?);
        } else if id == b"data" {
            let rate = byte_rate.filter(|r| *r > 0)?;
            // Streamed files can leave the size as 0 or 0xFFFFFFFF.
            let available = d.len() - body;
            let len = if size == 0 || size > available {
                available
            } else {
                size
            };
            return Some(len as f64 / rate as f64);
        }
        at = body.checked_add(size)?.checked_add(size & 1)?;
    }
    None
}

fn ogg_secs(d: &[u8]) -> Option<f64> {
    if d.get(0..4)? != b"OggS" {
        return None;
    }
    // First page: header 27 bytes + segment table, then the first packet.
    let segments = *d.get(26)? as usize;
    let packet = 27 + segments;
    let head = d.get(packet..)?;
    let (rate, skip) = if head.starts_with(b"\x01vorbis") {
        (u32le(head, 12)? as f64, 0.0)
    } else if head.starts_with(b"OpusHead") {
        // Opus positions always count at 48 kHz.
        (48_000.0, u16le(head, 10)? as f64)
    } else {
        return None;
    };
    if rate <= 0.0 {
        return None;
    }
    // The last page's granule position is the total sample count.
    let mut end = d.len();
    while end >= 27 {
        let start = d[..end].windows(4).rposition(|w| w == b"OggS")?;
        if let Some(bytes) = d.get(start + 6..start + 14) {
            let granule = i64::from_le_bytes(bytes.try_into().ok()?);
            if granule >= 0 {
                return Some(((granule as f64) - skip).max(0.0) / rate);
            }
        }
        end = start;
    }
    None
}

const MP3_BITRATES: [[u32; 15]; 5] = [
    // MPEG-1 layer I, II, III
    [
        0, 32, 64, 96, 128, 160, 192, 224, 256, 288, 320, 352, 384, 416, 448,
    ],
    [
        0, 32, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384,
    ],
    [
        0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320,
    ],
    // MPEG-2/2.5 layer I, then layers II and III
    [
        0, 32, 48, 56, 64, 80, 96, 112, 128, 144, 160, 176, 192, 224, 256,
    ],
    [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160],
];

/// One MPEG audio frame header: (frame length in bytes, samples, sample rate).
fn mp3_frame(h: [u8; 4]) -> Option<(usize, u32, u32)> {
    if h[0] != 0xFF || h[1] & 0xE0 != 0xE0 {
        return None;
    }
    let version = (h[1] >> 3) & 3; // 0 = 2.5, 2 = 2, 3 = 1
    let layer = (h[1] >> 1) & 3; // 1 = III, 2 = II, 3 = I
    if version == 1 || layer == 0 {
        return None;
    }
    let bitrate_idx = (h[2] >> 4) as usize;
    let rate_idx = ((h[2] >> 2) & 3) as usize;
    if bitrate_idx == 0 || bitrate_idx == 15 || rate_idx == 3 {
        return None;
    }
    let padding = ((h[2] >> 1) & 1) as usize;
    let mpeg1 = version == 3;
    let table = match (mpeg1, layer) {
        (true, 3) => 0,
        (true, 2) => 1,
        (true, _) => 2,
        (false, 3) => 3,
        (false, _) => 4,
    };
    let bitrate = MP3_BITRATES[table][bitrate_idx] * 1000;
    let base_rate = [44_100, 48_000, 32_000][rate_idx];
    let rate = match version {
        3 => base_rate,
        2 => base_rate / 2,
        _ => base_rate / 4,
    };
    let samples = match layer {
        3 => 384,
        2 => 1152,
        _ if mpeg1 => 1152,
        _ => 576,
    };
    let len = if layer == 3 {
        (12 * bitrate / rate) as usize * 4 + padding * 4
    } else {
        (samples / 8 * bitrate / rate) as usize + padding
    };
    (len >= 4).then_some((len, samples, rate))
}

fn mp3_secs(d: &[u8]) -> Option<f64> {
    let mut at = 0;
    // ID3v2 tag at the start.
    if d.len() >= 10 && &d[0..3] == b"ID3" {
        let size = d[6..10]
            .iter()
            .fold(0usize, |acc, b| (acc << 7) | (*b as usize & 0x7F));
        at = 10 + size + if d[5] & 0x10 != 0 { 10 } else { 0 };
    }
    let mut secs = 0.0;
    let mut frames = 0;
    while at + 4 <= d.len() {
        match mp3_frame([d[at], d[at + 1], d[at + 2], d[at + 3]]) {
            // A real frame is followed by another one (or the end).
            Some((len, samples, rate))
                if at + len + 4 > d.len()
                    || mp3_frame([
                        d[at + len],
                        d[at + len + 1],
                        d[at + len + 2],
                        d[at + len + 3],
                    ])
                    .is_some()
                    || frames > 0 =>
            {
                secs += samples as f64 / rate as f64;
                frames += 1;
                at += len;
            }
            _ => at += 1,
        }
    }
    (frames > 0).then_some(secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav(seconds: f64) -> Vec<u8> {
        let rate = 8000u32;
        let samples = (rate as f64 * seconds) as u32;
        let mut d = Vec::new();
        d.extend_from_slice(b"RIFF");
        d.extend_from_slice(&(36 + samples * 2).to_le_bytes());
        d.extend_from_slice(b"WAVEfmt ");
        d.extend_from_slice(&16u32.to_le_bytes());
        d.extend_from_slice(&1u16.to_le_bytes()); // PCM
        d.extend_from_slice(&1u16.to_le_bytes()); // mono
        d.extend_from_slice(&rate.to_le_bytes());
        d.extend_from_slice(&(rate * 2).to_le_bytes());
        d.extend_from_slice(&2u16.to_le_bytes());
        d.extend_from_slice(&16u16.to_le_bytes());
        d.extend_from_slice(b"data");
        d.extend_from_slice(&(samples * 2).to_le_bytes());
        d.resize(d.len() + samples as usize * 2, 0);
        d
    }

    #[test]
    fn wav_length() {
        assert_eq!(duration_ms(&wav(1.5), "audio/wav"), Some(1500));
        assert_eq!(duration_ms(b"RIFF....WAVEjunk", "audio/wav"), None);
    }

    #[test]
    fn mp3_length() {
        // 128 kbps, 44.1 kHz MPEG-1 layer III: 417-byte frames of 1152 samples.
        let header = [0xFF, 0xFB, 0x90, 0x00];
        let mut d = b"ID3\x03\x00\x00\x00\x00\x00\x05hello".to_vec();
        for _ in 0..100 {
            d.extend_from_slice(&header);
            d.resize(d.len() + 413, 0);
        }
        let ms = duration_ms(&d, "audio/mpeg").unwrap();
        assert_eq!(ms, (100.0 * 1152.0 / 44.1f64).round() as u32);
        assert_eq!(duration_ms(b"not an mp3 at all", "audio/mpeg"), None);
    }

    #[test]
    fn ogg_opus_length() {
        let page = |granule: i64, packet: &[u8]| {
            let mut p = b"OggS\x00\x00".to_vec();
            p.extend_from_slice(&granule.to_le_bytes());
            p.extend_from_slice(&[0; 12]); // serial, sequence, checksum
            p.push(1);
            p.push(packet.len() as u8);
            p.extend_from_slice(packet);
            p
        };
        let mut head = b"OpusHead\x01\x01".to_vec();
        head.extend_from_slice(&312u16.to_le_bytes());
        head.extend_from_slice(&48_000u32.to_le_bytes());
        let mut d = page(0, &head);
        d.extend(page(-1, b"middle"));
        d.extend(page(48_000 * 3 + 312, b"end"));
        assert_eq!(duration_ms(&d, "audio/ogg"), Some(3000));
    }

    #[test]
    fn types() {
        assert_eq!(audio_type("Leeroy.MP3"), Some("audio/mpeg"));
        assert_eq!(audio_type("x.opus"), Some("audio/ogg"));
        assert_eq!(audio_type("x.flac"), None);
    }
}
