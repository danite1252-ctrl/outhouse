//! Animated pictures (GIF, animated WebP, APNG): telling whether a picture
//! moves, and making a still copy of its first frame, without decoding any
//! pixels (the bytes of the first frame are copied into a one-frame file).
//!
//! Used for avatars, banners and outhouse pictures: the web client shows
//! the still copy (`?still=1`) in lists until hovered, for people who ask
//! for less motion, and everywhere when animated pictures are a premium
//! perk the picture's owner doesn't have (see api/premium.rs).

/// Does this picture have more than one frame?
pub fn is_animated(data: &[u8]) -> bool {
    if data.starts_with(b"GIF8") {
        return gif_frames(data).map(|(n, _)| n > 1).unwrap_or(false);
    }
    if is_webp(data) {
        return webp_chunks(data).iter().any(|(id, _)| id == b"ANMF");
    }
    if data.starts_with(PNG_SIG) {
        return png_chunks(data).iter().any(|(id, _)| id == b"acTL");
    }
    false
}

/// A one-frame copy of an animated picture (same format), or None when it
/// doesn't move or can't be read.
pub fn first_frame(data: &[u8]) -> Option<Vec<u8>> {
    if data.starts_with(b"GIF8") {
        let (frames, end) = gif_frames(data)?;
        if frames < 2 {
            return None;
        }
        let mut out = data[..end].to_vec();
        out.push(0x3B);
        return Some(out);
    }
    if is_webp(data) {
        return webp_first_frame(data);
    }
    if data.starts_with(PNG_SIG) {
        let chunks = png_chunks(data);
        if !chunks.iter().any(|(id, _)| id == b"acTL") {
            return None;
        }
        let mut out = PNG_SIG.to_vec();
        let mut has_idat = false;
        for (id, raw) in chunks {
            if &id == b"acTL" || &id == b"fcTL" || &id == b"fdAT" {
                continue;
            }
            has_idat |= &id == b"IDAT";
            out.extend_from_slice(raw);
        }
        return has_idat.then_some(out);
    }
    None
}

// ---- GIF ------------------------------------------------------------------------

/// (number of frames, end of the first frame's data).
fn gif_frames(d: &[u8]) -> Option<(usize, usize)> {
    if d.len() < 13 {
        return None;
    }
    let packed = d[10];
    let mut i = 13;
    if packed & 0x80 != 0 {
        i += 3 * (1usize << ((packed & 7) + 1));
    }
    let mut frames = 0;
    let mut first_end = 0;
    let skip_blocks = |mut i: usize| -> Option<usize> {
        loop {
            let len = *d.get(i)? as usize;
            i += 1;
            if len == 0 {
                return Some(i);
            }
            i += len;
        }
    };
    while i < d.len() {
        match d[i] {
            0x21 => {
                d.get(i + 1)?;
                i = skip_blocks(i + 2)?;
            }
            0x2C => {
                let packed = *d.get(i + 9)?;
                i += 10;
                if packed & 0x80 != 0 {
                    i += 3 * (1usize << ((packed & 7) + 1));
                }
                d.get(i)?; // LZW minimum code size
                i = skip_blocks(i + 1)?;
                frames += 1;
                if frames == 1 {
                    first_end = i;
                }
                if frames > 1 {
                    break;
                }
            }
            0x3B => break,
            _ => return None,
        }
    }
    (frames > 0).then_some((frames, first_end))
}

// ---- WebP -----------------------------------------------------------------------

fn is_webp(d: &[u8]) -> bool {
    d.len() > 12 && &d[..4] == b"RIFF" && &d[8..12] == b"WEBP"
}

/// RIFF chunks as (fourcc, payload).
fn riff_chunks(d: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 8 <= d.len() {
        let id = [d[i], d[i + 1], d[i + 2], d[i + 3]];
        let len = u32::from_le_bytes([d[i + 4], d[i + 5], d[i + 6], d[i + 7]]) as usize;
        let start = i + 8;
        let Some(end) = start.checked_add(len).filter(|e| *e <= d.len()) else {
            break;
        };
        out.push((id, &d[start..end]));
        i = end + (len & 1);
    }
    out
}

fn webp_chunks(d: &[u8]) -> Vec<([u8; 4], &[u8])> {
    riff_chunks(&d[12..])
}

fn push_chunk(out: &mut Vec<u8>, id: &[u8], payload: &[u8]) {
    out.extend_from_slice(id);
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(payload);
    if payload.len() & 1 == 1 {
        out.push(0);
    }
}

fn webp_first_frame(d: &[u8]) -> Option<Vec<u8>> {
    let (_, frame) = webp_chunks(d).into_iter().find(|(id, _)| id == b"ANMF")?;
    if frame.len() < 16 {
        return None;
    }
    // Frame width-1 and height-1 (24-bit) at 6 and 9.
    let wh = &frame[6..12];
    let inner = riff_chunks(&frame[16..]);
    let image: Vec<_> = inner
        .into_iter()
        .filter(|(id, _)| matches!(id, b"ALPH" | b"VP8 " | b"VP8L"))
        .collect();
    if !image.iter().any(|(id, _)| id == b"VP8 " || id == b"VP8L") {
        return None;
    }
    let alpha = image.iter().any(|(id, _)| id == b"ALPH" || id == b"VP8L");
    let mut body = b"WEBP".to_vec();
    let mut vp8x = vec![if alpha { 0x10 } else { 0 }, 0, 0, 0];
    vp8x.extend_from_slice(wh);
    push_chunk(&mut body, b"VP8X", &vp8x);
    for (id, payload) in image {
        push_chunk(&mut body, &id, payload);
    }
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    Some(out)
}

// ---- PNG ------------------------------------------------------------------------

const PNG_SIG: &[u8] = b"\x89PNG\r\n\x1a\n";

/// PNG chunks as (type, the whole chunk with length and CRC).
fn png_chunks(d: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    let mut i = PNG_SIG.len();
    while i + 12 <= d.len() {
        let len = u32::from_be_bytes([d[i], d[i + 1], d[i + 2], d[i + 3]]) as usize;
        let id = [d[i + 4], d[i + 5], d[i + 6], d[i + 7]];
        let Some(end) = (i + 12).checked_add(len).filter(|e| *e <= d.len()) else {
            break;
        };
        out.push((id, &d[i..end]));
        i = end;
        if &id == b"IEND" {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny 1x1 GIF with `frames` frames.
    pub fn gif(frames: usize) -> Vec<u8> {
        let mut g = b"GIF89a\x01\x00\x01\x00\x80\x00\x00\x00\x00\x00\xff\xff\xff".to_vec();
        g.extend_from_slice(b"\x21\xff\x0bNETSCAPE2.0\x03\x01\x00\x00\x00");
        for _ in 0..frames {
            g.extend_from_slice(b"\x21\xf9\x04\x00\x0a\x00\x00\x00");
            g.extend_from_slice(b"\x2c\x00\x00\x00\x00\x01\x00\x01\x00\x00\x02\x02\x44\x01\x00");
        }
        g.push(0x3B);
        g
    }

    #[test]
    fn gifs() {
        assert!(!is_animated(&gif(1)));
        assert!(is_animated(&gif(3)));
        let still = first_frame(&gif(3)).unwrap();
        assert!(!is_animated(&still));
        assert_eq!(gif_frames(&still).unwrap().0, 1);
        assert_eq!(*still.last().unwrap(), 0x3B);
        assert!(first_frame(&gif(1)).is_none());
        assert!(!is_animated(b"GIF89a"));
    }

    fn webp(animated: bool) -> Vec<u8> {
        let vp8l = b"\x2f\x00\x00\x00\x00\x07\x10\x11\x11\x88\x88\xfe\x07\x00";
        let mut body = b"WEBP".to_vec();
        if animated {
            push_chunk(&mut body, b"VP8X", &[0x12, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            push_chunk(&mut body, b"ANIM", &[0, 0, 0, 0, 0, 0]);
            for _ in 0..2 {
                let mut f = vec![0u8; 16];
                push_chunk(&mut f, b"VP8L", vp8l);
                push_chunk(&mut body, b"ANMF", &f);
            }
        } else {
            push_chunk(&mut body, b"VP8L", vp8l);
        }
        let mut out = b"RIFF".to_vec();
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&body);
        out
    }

    #[test]
    fn webps() {
        assert!(!is_animated(&webp(false)));
        assert!(is_animated(&webp(true)));
        let still = first_frame(&webp(true)).unwrap();
        assert!(is_webp(&still));
        assert!(!is_animated(&still));
        assert!(webp_chunks(&still).iter().any(|(id, _)| id == b"VP8L"));
        assert!(first_frame(&webp(false)).is_none());
    }

    #[test]
    fn apngs() {
        let chunk = |id: &[u8], data: &[u8]| {
            let mut c = (data.len() as u32).to_be_bytes().to_vec();
            c.extend_from_slice(id);
            c.extend_from_slice(data);
            c.extend_from_slice(&[0, 0, 0, 0]);
            c
        };
        let mut png = PNG_SIG.to_vec();
        png.extend(chunk(b"IHDR", &[0; 13]));
        png.extend(chunk(b"acTL", &[0; 8]));
        png.extend(chunk(b"fcTL", &[0; 26]));
        png.extend(chunk(b"IDAT", &[1, 2, 3]));
        png.extend(chunk(b"fdAT", &[0; 8]));
        png.extend(chunk(b"IEND", &[]));
        assert!(is_animated(&png));
        let still = first_frame(&png).unwrap();
        assert!(!is_animated(&still));
        assert!(png_chunks(&still).iter().any(|(id, _)| id == b"IDAT"));
        assert!(!png_chunks(&still).iter().any(|(id, _)| id == b"fdAT"));
    }
}
