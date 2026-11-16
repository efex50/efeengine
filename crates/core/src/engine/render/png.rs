//! A small PNG decoder: bytes -> RGBA8 [`Image`]. Pure Rust (zlib inflate from `miniz_oxide`,
//! chunk checksums from `crc32fast`), so it works on every target including wasm.
//!
//! Supported: non-interlaced images, 8-bit gray / gray+alpha / RGB / RGBA / palette (with
//! `tRNS` alpha), 1/2/4-bit gray and palette, all five scanline filters, any number of `IDAT`
//! chunks. Refused with a clear error: Adam7 interlacing, 16-bit samples, 1/2/4-bit RGB or
//! alpha types (not legal PNG anyway), bad signature, CRC mismatch, truncation, oversize.
//! Gray and RGB `tRNS` colour keys are ignored (only palette transparency is applied).
//! The decoder never panics on malformed input; it returns a [`PngError`].

use super::textures::Image;

const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// Largest width or height accepted.
pub const MAX_DIMENSION: u32 = 16384;
/// Largest pixel count accepted (16384 x 16384).
pub const MAX_PIXELS: u64 = 16384 * 16384;

/// Why a PNG could not be decoded.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PngError {
    #[error("not a PNG file (bad signature)")]
    BadSignature,
    #[error("PNG data ends unexpectedly ({0})")]
    Truncated(&'static str),
    #[error("CRC mismatch in {0} chunk")]
    BadCrc(String),
    #[error("invalid PNG: {0}")]
    Invalid(&'static str),
    #[error("interlaced (Adam7) PNGs are not supported")]
    Interlaced,
    #[error("16-bit PNGs are not supported")]
    SixteenBit,
    #[error("unsupported PNG format: colour type {color_type} with bit depth {bit_depth}")]
    Unsupported { color_type: u8, bit_depth: u8 },
    #[error("PNG is {width}x{height}; the limit is {MAX_DIMENSION} per side and {MAX_PIXELS} pixels")]
    TooLarge { width: u32, height: u32 },
    #[error("cannot decompress the image data: {0}")]
    Inflate(String),
    #[error("cannot read the file: {0}")]
    Io(String),
    #[error("{0}")]
    Image(#[from] super::textures::TextureError),
}

#[derive(Clone, Copy)]
struct Header {
    width: u32,
    height: u32,
    depth: u8,
    color_type: u8,
}

impl Header {
    fn channels(&self) -> usize {
        match self.color_type {
            0 | 3 => 1,
            2 => 3,
            4 => 2,
            _ => 4,
        }
    }

    fn bits_per_pixel(&self) -> usize {
        self.channels() * self.depth as usize
    }

    /// Bytes per pixel for the filters (at least 1).
    fn filter_bpp(&self) -> usize {
        self.bits_per_pixel().div_ceil(8)
    }

    /// Bytes in one scanline, without the filter byte.
    fn row_bytes(&self) -> usize {
        (self.width as usize * self.bits_per_pixel()).div_ceil(8)
    }
}

fn parse_header(d: &[u8]) -> Result<Header, PngError> {
    if d.len() != 13 {
        return Err(PngError::Invalid("IHDR has the wrong length"));
    }
    let width = u32::from_be_bytes([d[0], d[1], d[2], d[3]]);
    let height = u32::from_be_bytes([d[4], d[5], d[6], d[7]]);
    let (depth, color_type, compression, filter, interlace) = (d[8], d[9], d[10], d[11], d[12]);
    if width == 0 || height == 0 {
        return Err(PngError::Invalid("zero width or height"));
    }
    if width > MAX_DIMENSION || height > MAX_DIMENSION || width as u64 * height as u64 > MAX_PIXELS {
        return Err(PngError::TooLarge { width, height });
    }
    if compression != 0 || filter != 0 {
        return Err(PngError::Invalid("unknown compression or filter method"));
    }
    match interlace {
        0 => {}
        1 => return Err(PngError::Interlaced),
        _ => return Err(PngError::Invalid("unknown interlace method")),
    }
    if depth == 16 && matches!(color_type, 0 | 2 | 3 | 4 | 6) {
        return Err(PngError::SixteenBit);
    }
    let ok = match color_type {
        0 | 3 => matches!(depth, 1 | 2 | 4 | 8),
        2 | 4 | 6 => depth == 8,
        _ => return Err(PngError::Invalid("unknown colour type")),
    };
    if !ok {
        return Err(PngError::Unsupported { color_type, bit_depth: depth });
    }
    Ok(Header { width, height, depth, color_type })
}

/// Reverse the scanline filters in place. `raw` is `height` rows of `1 + row_bytes` bytes;
/// afterwards the filter bytes are still there (the caller skips them).
fn unfilter(raw: &mut [u8], row_bytes: usize, bpp: usize) -> Result<(), PngError> {
    let stride = row_bytes + 1;
    let mut prev_start: Option<usize> = None;
    for start in (0..raw.len()).step_by(stride) {
        let ft = raw[start];
        let cur = start + 1;
        for i in 0..row_bytes {
            let a = if i >= bpp { raw[cur + i - bpp] } else { 0 };
            let b = prev_start.map_or(0, |p| raw[p + 1 + i]);
            let c = if i >= bpp { prev_start.map_or(0, |p| raw[p + 1 + i - bpp]) } else { 0 };
            let add = match ft {
                0 => 0,
                1 => a,
                2 => b,
                3 => ((a as u16 + b as u16) / 2) as u8,
                4 => paeth(a, b, c),
                _ => return Err(PngError::Invalid("unknown scanline filter")),
            };
            raw[cur + i] = raw[cur + i].wrapping_add(add);
        }
        prev_start = Some(start);
    }
    Ok(())
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let (ia, ib, ic) = (a as i16, b as i16, c as i16);
    let p = ia + ib - ic;
    let (pa, pb, pc) = ((p - ia).abs(), (p - ib).abs(), (p - ic).abs());
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

/// Decode a PNG file into an RGBA8 image.
pub fn decode(bytes: &[u8]) -> Result<Image, PngError> {
    if bytes.len() < 8 || bytes[..8] != SIGNATURE {
        return Err(PngError::BadSignature);
    }
    let mut pos = 8usize;
    let mut header: Option<Header> = None;
    let mut palette: Vec<[u8; 3]> = Vec::new();
    let mut trns: Vec<u8> = Vec::new();
    let mut idat: Vec<u8> = Vec::new();
    let mut saw_idat = false;
    let mut ended = false;

    // ---- chunks: length, type, data, CRC(type + data) ----
    while pos < bytes.len() {
        if bytes.len() - pos < 12 {
            return Err(PngError::Truncated("chunk header"));
        }
        let len = u32::from_be_bytes([bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3]]) as usize;
        let kind: [u8; 4] = [bytes[pos + 4], bytes[pos + 5], bytes[pos + 6], bytes[pos + 7]];
        let data_start = pos + 8;
        // `len` may be up to 4 GiB: compare without overflow on 32-bit targets
        if len > bytes.len() - data_start || bytes.len() - data_start - len < 4 {
            return Err(PngError::Truncated("chunk data"));
        }
        let data = &bytes[data_start..data_start + len];
        let crc_at = data_start + len;
        let stored = u32::from_be_bytes([bytes[crc_at], bytes[crc_at + 1], bytes[crc_at + 2], bytes[crc_at + 3]]);
        let mut h = crc32fast::Hasher::new();
        h.update(&kind);
        h.update(data);
        if h.finalize() != stored {
            return Err(PngError::BadCrc(String::from_utf8_lossy(&kind).into_owned()));
        }
        pos = crc_at + 4;

        if header.is_none() && &kind != b"IHDR" {
            return Err(PngError::Invalid("the first chunk is not IHDR"));
        }
        match &kind {
            b"IHDR" => {
                if header.is_some() {
                    return Err(PngError::Invalid("duplicate IHDR"));
                }
                header = Some(parse_header(data)?);
            }
            b"PLTE" => {
                if data.len() % 3 != 0 || data.is_empty() || data.len() > 256 * 3 {
                    return Err(PngError::Invalid("bad PLTE length"));
                }
                palette = data.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect();
            }
            b"tRNS" => trns = data.to_vec(),
            b"IDAT" => {
                saw_idat = true;
                idat.extend_from_slice(data);
            }
            b"IEND" => {
                ended = true;
                break;
            }
            // unknown critical chunks (first letter uppercase) cannot be skipped safely
            k if k[0].is_ascii_uppercase() => return Err(PngError::Invalid("unknown critical chunk")),
            _ => {}
        }
    }
    let hdr = header.ok_or(PngError::Truncated("no IHDR"))?;
    if !saw_idat {
        return Err(PngError::Truncated("no IDAT"));
    }
    if !ended {
        return Err(PngError::Truncated("no IEND"));
    }
    if hdr.color_type == 3 && palette.is_empty() {
        return Err(PngError::Invalid("palette image without PLTE"));
    }

    // ---- inflate: exactly height * (1 + row_bytes) bytes (the limit stops decompression bombs) ----
    let row_bytes = hdr.row_bytes();
    let stride = row_bytes + 1;
    let expected = stride.checked_mul(hdr.height as usize).ok_or(PngError::TooLarge { width: hdr.width, height: hdr.height })?;
    let mut raw = miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(&idat, expected)
        .map_err(|e| PngError::Inflate(format!("{:?}", e.status)))?;
    if raw.len() != expected {
        return Err(PngError::Truncated("image data"));
    }
    unfilter(&mut raw, row_bytes, hdr.filter_bpp())?;

    // ---- expand every row to RGBA8 ----
    let mut out = Vec::with_capacity(hdr.width as usize * hdr.height as usize * 4);
    let depth = hdr.depth as usize;
    let mask = (1u16 << depth) - 1;
    for row in raw.chunks_exact(stride) {
        let row = &row[1..];
        match hdr.color_type {
            0 | 3 => {
                for x in 0..hdr.width as usize {
                    // sub-byte samples are packed most significant bits first
                    let bit = x * depth;
                    let v = ((row[bit / 8] as u16 >> (8 - depth - bit % 8)) & mask) as usize;
                    if hdr.color_type == 0 {
                        let g = (v * 255 / mask as usize) as u8;
                        out.extend_from_slice(&[g, g, g, 255]);
                    } else {
                        let rgb = palette.get(v).ok_or(PngError::Invalid("palette index out of range"))?;
                        out.extend_from_slice(&[rgb[0], rgb[1], rgb[2], trns.get(v).copied().unwrap_or(255)]);
                    }
                }
            }
            2 => row.chunks_exact(3).for_each(|p| out.extend_from_slice(&[p[0], p[1], p[2], 255])),
            4 => row.chunks_exact(2).for_each(|p| out.extend_from_slice(&[p[0], p[0], p[0], p[1]])),
            _ => out.extend_from_slice(row),
        }
    }
    Ok(Image::new(hdr.width, hdr.height, out)?)
}

impl Image {
    /// Decode a PNG file (see the [`png`](self) module for what is supported).
    pub fn from_png(bytes: &[u8]) -> Result<Image, PngError> {
        decode(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- a tiny PNG encoder for the tests ----

    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let mut h = crc32fast::Hasher::new();
        h.update(kind);
        h.update(data);
        out.extend_from_slice(&h.finalize().to_be_bytes());
    }

    fn filter_row(ft: u8, row: &[u8], prev: &[u8], bpp: usize) -> Vec<u8> {
        (0..row.len())
            .map(|i| {
                let a = if i >= bpp { row[i - bpp] } else { 0 };
                let b = prev[i];
                let c = if i >= bpp { prev[i - bpp] } else { 0 };
                let p = match ft {
                    0 => 0,
                    1 => a,
                    2 => b,
                    3 => ((a as u16 + b as u16) / 2) as u8,
                    _ => paeth(a, b, c),
                };
                row[i].wrapping_sub(p)
            })
            .collect()
    }

    struct Spec<'a> {
        w: u32,
        h: u32,
        depth: u8,
        color_type: u8,
        /// Unfiltered scanlines (packed), `h` rows.
        rows: Vec<Vec<u8>>,
        plte: Option<&'a [u8]>,
        trns: Option<&'a [u8]>,
        filter: u8,
        idat_parts: usize,
        interlace: u8,
    }

    impl<'a> Spec<'a> {
        fn new(w: u32, h: u32, depth: u8, color_type: u8, rows: Vec<Vec<u8>>) -> Self {
            Spec { w, h, depth, color_type, rows, plte: None, trns: None, filter: 0, idat_parts: 1, interlace: 0 }
        }

        fn encode(&self) -> Vec<u8> {
            let channels = match self.color_type { 0 | 3 => 1, 2 => 3, 4 => 2, _ => 4 };
            let bpp = (channels * self.depth as usize).div_ceil(8);
            let mut raw = Vec::new();
            let mut prev = vec![0u8; self.rows[0].len()];
            for row in &self.rows {
                raw.push(self.filter);
                raw.extend(filter_row(self.filter, row, &prev, bpp));
                prev = row.clone();
            }
            let z = miniz_oxide::deflate::compress_to_vec_zlib(&raw, 6);
            let mut ihdr = Vec::new();
            ihdr.extend_from_slice(&self.w.to_be_bytes());
            ihdr.extend_from_slice(&self.h.to_be_bytes());
            ihdr.extend_from_slice(&[self.depth, self.color_type, 0, 0, self.interlace]);
            let mut out = SIGNATURE.to_vec();
            chunk(&mut out, b"IHDR", &ihdr);
            if let Some(p) = self.plte {
                chunk(&mut out, b"PLTE", p);
            }
            if let Some(t) = self.trns {
                chunk(&mut out, b"tRNS", t);
            }
            let part = z.len().div_ceil(self.idat_parts).max(1);
            for c in z.chunks(part) {
                chunk(&mut out, b"IDAT", c);
            }
            chunk(&mut out, b"IEND", &[]);
            out
        }
    }

    /// Deterministic pseudo-random bytes.
    fn noise(n: usize, seed: u32) -> Vec<u8> {
        let mut s = seed.wrapping_mul(2654435761).wrapping_add(1);
        (0..n)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                (s >> 8) as u8
            })
            .collect()
    }

    fn rows_of(h: u32, row_bytes: usize, seed: u32) -> Vec<Vec<u8>> {
        (0..h).map(|y| noise(row_bytes, seed + y)).collect()
    }

    #[test]
    fn rgba8_every_filter() {
        for f in 0..=4u8 {
            let rows = rows_of(7, 5 * 4, 10 + f as u32);
            let mut s = Spec::new(5, 7, 8, 6, rows.clone());
            s.filter = f;
            let img = Image::from_png(&s.encode()).unwrap_or_else(|e| panic!("filter {f}: {e}"));
            assert_eq!(img.size(), (5, 7));
            assert_eq!(img.rgba8(), rows.concat().as_slice(), "filter {f}");
        }
    }

    #[test]
    fn rgb8_gray8_gray_alpha8() {
        let rows = rows_of(4, 3 * 3, 1);
        let mut s = Spec::new(3, 4, 8, 2, rows.clone());
        s.filter = 4;
        let img = Image::from_png(&s.encode()).unwrap();
        let want: Vec<u8> = rows.concat().chunks(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect();
        assert_eq!(img.rgba8(), want.as_slice());

        let rows = rows_of(3, 6, 2);
        let mut s = Spec::new(6, 3, 8, 0, rows.clone());
        s.filter = 1;
        let want: Vec<u8> = rows.concat().iter().flat_map(|&g| [g, g, g, 255]).collect();
        assert_eq!(Image::from_png(&s.encode()).unwrap().rgba8(), want.as_slice());

        let rows = rows_of(3, 4 * 2, 3);
        let mut s = Spec::new(4, 3, 8, 4, rows.clone());
        s.filter = 3;
        let want: Vec<u8> = rows.concat().chunks(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect();
        assert_eq!(Image::from_png(&s.encode()).unwrap().rgba8(), want.as_slice());
    }

    #[test]
    fn palette_with_trns_and_multiple_idat() {
        let plte = [255, 0, 0, 0, 255, 0, 0, 0, 255];
        let trns = [128u8, 0];
        let rows = vec![vec![0, 1, 2, 1], vec![2, 2, 0, 0]];
        let mut s = Spec::new(4, 2, 8, 3, rows);
        s.plte = Some(&plte);
        s.trns = Some(&trns);
        s.filter = 2;
        s.idat_parts = 3;
        let img = Image::from_png(&s.encode()).unwrap();
        assert_eq!(&img.rgba8()[..8], &[255, 0, 0, 128, 0, 255, 0, 0]);
        assert_eq!(&img.rgba8()[8..12], &[0, 0, 255, 255]); // index 2 has no tRNS entry
    }

    #[test]
    fn low_bit_depths() {
        // 2-bit palette, 5 pixels wide: indices 0 1 2 3 1 (last byte padded)
        let plte = [0, 0, 0, 10, 10, 10, 20, 20, 20, 30, 30, 30];
        let mut s = Spec::new(5, 1, 2, 3, vec![vec![0b00_01_10_11, 0b01_000000]]);
        s.plte = Some(&plte);
        let img = Image::from_png(&s.encode()).unwrap();
        let reds: Vec<u8> = img.rgba8().chunks(4).map(|p| p[0]).collect();
        assert_eq!(reds, [0, 10, 20, 30, 10]);

        // 1-bit gray: black / white
        let s = Spec::new(3, 1, 1, 0, vec![vec![0b101_00000]]);
        let reds: Vec<u8> = Image::from_png(&s.encode()).unwrap().rgba8().chunks(4).map(|p| p[0]).collect();
        assert_eq!(reds, [255, 0, 255]);

        // 4-bit gray scales to 0..255; 4-bit palette
        let s = Spec::new(3, 1, 4, 0, vec![vec![0x0F, 0x80]]);
        let reds: Vec<u8> = Image::from_png(&s.encode()).unwrap().rgba8().chunks(4).map(|p| p[0]).collect();
        assert_eq!(reds, [0, 255, 136]);
        let mut s = Spec::new(2, 1, 4, 3, vec![vec![0x10]]);
        s.plte = Some(&[1, 2, 3, 4, 5, 6]);
        assert_eq!(&Image::from_png(&s.encode()).unwrap().rgba8()[..4], &[4, 5, 6, 255]);
    }

    #[test]
    fn errors() {
        let good = Spec::new(2, 2, 8, 6, rows_of(2, 8, 0)).encode();
        assert!(Image::from_png(&good).is_ok());

        assert_eq!(Image::from_png(b"hello"), Err(PngError::BadSignature));
        assert_eq!(Image::from_png(&[]), Err(PngError::BadSignature));

        let mut s = Spec::new(2, 2, 8, 6, rows_of(2, 8, 0));
        s.interlace = 1;
        assert_eq!(Image::from_png(&s.encode()), Err(PngError::Interlaced));

        let s = Spec::new(2, 2, 16, 6, rows_of(2, 16, 0));
        assert_eq!(Image::from_png(&s.encode()), Err(PngError::SixteenBit));

        let s = Spec::new(2, 2, 4, 6, rows_of(2, 8, 0));
        assert!(matches!(Image::from_png(&s.encode()), Err(PngError::Unsupported { .. })));

        // CRC mismatch: flip a byte inside IHDR data (offset 8 + 8 = 16 is the width)
        let mut bad = good.clone();
        bad[19] ^= 1;
        assert!(matches!(Image::from_png(&bad), Err(PngError::BadCrc(k)) if k == "IHDR"));

        // truncation at every length
        for n in 0..good.len() {
            assert!(Image::from_png(&good[..n]).is_err(), "prefix of {n} bytes decoded");
        }

        // oversize: a valid-CRC header claiming a huge image
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&70000u32.to_be_bytes());
        ihdr.extend_from_slice(&1u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
        let mut big = SIGNATURE.to_vec();
        chunk(&mut big, b"IHDR", &ihdr);
        chunk(&mut big, b"IEND", &[]);
        assert!(matches!(Image::from_png(&big), Err(PngError::TooLarge { .. })));

        // fewer pixels than the header promises
        let mut s = Spec::new(2, 2, 8, 6, rows_of(2, 8, 0));
        s.h = 3;
        assert!(Image::from_png(&s.encode()).is_err());

        // palette index out of range
        let mut s = Spec::new(1, 1, 8, 3, vec![vec![5]]);
        s.plte = Some(&[0, 0, 0]);
        assert!(Image::from_png(&s.encode()).is_err());
        // palette image without PLTE
        let s = Spec::new(1, 1, 8, 3, vec![vec![0]]);
        assert!(Image::from_png(&s.encode()).is_err());
    }

    /// Split a PNG into (type, data) chunks and rebuild it with fresh CRCs.
    fn rebuild(chunks: &[([u8; 4], Vec<u8>)]) -> Vec<u8> {
        let mut out = SIGNATURE.to_vec();
        for (k, d) in chunks {
            chunk(&mut out, k, d);
        }
        out
    }

    fn split(png: &[u8]) -> Vec<([u8; 4], Vec<u8>)> {
        let mut pos = 8;
        let mut v = Vec::new();
        while pos + 12 <= png.len() {
            let len = u32::from_be_bytes(png[pos..pos + 4].try_into().unwrap()) as usize;
            v.push((png[pos + 4..pos + 8].try_into().unwrap(), png[pos + 8..pos + 8 + len].to_vec()));
            pos += 12 + len;
        }
        v
    }

    #[test]
    fn garbage_never_panics() {
        let plte = [255, 0, 0, 0, 255, 0, 0, 0, 255];
        let mut s = Spec::new(6, 6, 8, 3, rows_of(6, 6, 9).iter().map(|r| r.iter().map(|b| b % 3).collect()).collect());
        s.plte = Some(&plte);
        s.filter = 4;
        let seeds = [s.encode(), Spec::new(5, 5, 8, 6, rows_of(5, 20, 4)).encode()];
        let mut rng = noise(4000, 77).into_iter();
        let mut next = move || rng.next().unwrap_or(0) as usize;
        for seed in &seeds {
            for _ in 0..300 {
                // 1) raw byte mutation: the CRC usually catches it
                let mut b = seed.clone();
                for _ in 0..1 + next() % 3 {
                    let i = (next() << 8 | next()) % b.len();
                    b[i] = next() as u8;
                }
                let _ = Image::from_png(&b);
                // 2) mutate chunk payloads and fix the CRCs, so the decoder's own checks run
                let mut chunks = split(seed);
                let c = next() % chunks.len();
                if !chunks[c].1.is_empty() {
                    let n = chunks[c].1.len();
                    chunks[c].1[next() % n] = next() as u8;
                }
                let _ = Image::from_png(&rebuild(&chunks));
                // 3) truncate a payload
                let c = next() % chunks.len();
                let n = chunks[c].1.len();
                chunks[c].1.truncate(next() % (n + 1));
                let _ = Image::from_png(&rebuild(&chunks));
            }
        }
    }
}
