//! Source-observed pixel semantics; oracle receipts live in ignored .analysis/profile-trust.
use super::{ResolvedTexture, Result};
use std::{
    io::{BufReader, Cursor},
    sync::Arc,
};
const MAX_DECODED: usize = 4 << 20;

pub(super) fn decode_png(bytes: &[u8], skin: bool) -> Result<ResolvedTexture> {
    if bytes.len() > 2 << 20 {
        return Err("profile PNG compressed byte bound".into());
    }
    let mut decoder = png::Decoder::new(BufReader::new(Cursor::new(bytes)));
    decoder.set_limits(png::Limits { bytes: 8 << 20 });
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder
        .read_info()
        .map_err(|e| format!("profile PNG header: {e}"))?;
    let (width, height) = (reader.info().width, reader.info().height);
    if width == 0
        || height == 0
        || width > 1024
        || height > 1024
        || u64::from(width) * u64::from(height) * 4 > MAX_DECODED as u64
    {
        return Err("profile PNG dimensions/decoded byte bound".into());
    }
    if skin && (width != 64 || (height != 32 && height != 64)) {
        return Err(format!(
            "profile skin dimensions {width}x{height}; expected 64x32 or 64x64"
        ));
    }
    if reader.info().animation_control.is_some() {
        return Err("animated profile PNG rejected".into());
    }
    let size = reader
        .output_buffer_size()
        .filter(|size| *size <= MAX_DECODED)
        .ok_or("profile PNG decode buffer bound")?;
    let mut pixels = vec![0; size];
    let frame = reader
        .next_frame(&mut pixels)
        .map_err(|e| format!("profile PNG decode: {e}"))?;
    // Read IEND and all CRCs before trusting or caching the image, not just the first IDAT.
    reader
        .finish()
        .map_err(|e| format!("profile PNG end: {e}"))?;
    let channels = match frame.color_type {
        png::ColorType::Rgba => 4,
        png::ColorType::Rgb => 3,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Grayscale => 1,
        _ => return Err("profile PNG palette was not expanded".into()),
    };
    let mut rgba = if channels == 4 {
        pixels.truncate(frame.buffer_size());
        pixels
    } else {
        let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
        for p in pixels[..frame.buffer_size()].chunks_exact(channels) {
            rgba.extend_from_slice(&match channels {
                3 => [p[0], p[1], p[2], 255],
                2 => [p[0], p[0], p[0], p[1]],
                _ => [p[0], p[0], p[0], 255],
            });
        }
        rgba
    };
    if skin {
        normalize_skin(&mut rgba, height);
    }
    Ok(ResolvedTexture {
        id: String::new(),
        rgba: Arc::from(rgba),
        width,
        height: if skin { 64 } else { height },
    })
}
fn normalize_skin(rgba: &mut Vec<u8>, height: u32) {
    if height == 32 {
        rgba.resize(64 * 64 * 4, 0);
        // NativeImage.copyRect's third/fourth parameters are offsets, and x is mirrored.
        for (x, y, dx, dy, w, h) in [
            (4, 16, 16, 32, 4, 4),
            (8, 16, 16, 32, 4, 4),
            (0, 20, 24, 32, 4, 12),
            (4, 20, 16, 32, 4, 12),
            (8, 20, 8, 32, 4, 12),
            (12, 20, 16, 32, 4, 12),
            (44, 16, -8, 32, 4, 4),
            (48, 16, -8, 32, 4, 4),
            (40, 20, 0, 32, 4, 12),
            (44, 20, -8, 32, 4, 12),
            (48, 20, -16, 32, 4, 12),
            (52, 20, -8, 32, 4, 12),
        ] {
            for row in 0..h {
                for col in 0..w {
                    let src = ((y + row) * 64 + x + col) as usize * 4;
                    let dst = ((y + dy + row) * 64 + x + dx + w - 1 - col) as usize * 4;
                    let color: [u8; 4] = rgba[src..src + 4].try_into().unwrap();
                    rgba[dst..dst + 4].copy_from_slice(&color);
                }
            }
        }
    }
    alpha(rgba, 0, 0, 32, 16, 255);
    if height == 32 && (0..32).all(|y| (32..64).all(|x| rgba[(y * 64 + x) * 4 + 3] >= 128)) {
        alpha(rgba, 32, 0, 64, 32, 0);
    }
    alpha(rgba, 0, 16, 64, 32, 255);
    alpha(rgba, 16, 48, 48, 64, 255);
}
fn alpha(rgba: &mut [u8], x0: usize, y0: usize, x1: usize, y1: usize, value: u8) {
    for y in y0..y1 {
        for x in x0..x1 {
            rgba[(y * 64 + x) * 4 + 3] = value;
        }
    }
}
