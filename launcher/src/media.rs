//! Offline launcher illustrations and explicitly selected local PNG previews.
//!
//! No account lookup, network access, or bundled game assets. A skin preview
//! never changes the identity or skin used by the game. Invalid custom paths
//! return errors; callers may explicitly request `None` to show a labeled
//! original illustration instead, rather than silently impersonating a skin.

use eframe::egui;
use std::fs::{self, File, Metadata};
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub const FALLBACK_HEAD_LABEL: &str = "Solaris avatar · original illustration (not your skin)";
pub const DEFAULT_BACKGROUND_LABEL: &str = "Solaris landscape · original illustration";
pub const LOCAL_SKIN_NOTICE: &str = "Local preview only — does not change your in-game skin.";

const MAX_PNG_BYTES: usize = 16 * 1024 * 1024;
const MAX_RGBA_BYTES: usize = 64 * 1024 * 1024;
const MAX_PNG_CHUNKS: usize = 4096;
const MAX_METADATA_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy)]
enum ImageKind {
    Head,
    Background,
}

fn validate_dimensions(width: u32, height: u32, kind: ImageKind) -> Result<usize, String> {
    if width == 0 || height == 0 || width > 4096 || height > 4096 {
        return Err("PNG dimensions must be between 1 and 4096 pixels on each side".into());
    }
    let bytes = u64::from(width) * u64::from(height) * 4;
    if bytes > MAX_RGBA_BYTES as u64 {
        return Err("Decoded PNG exceeds the 64 MiB RGBA output limit".into());
    }
    if matches!(kind, ImageKind::Head)
        && (width > 1024 || !width.is_multiple_of(64) || (height != width && height * 2 != width))
    {
        return Err(
            "Skin PNG must be 64×64 or 64×32, or an integer HD multiple up to 1024 pixels wide"
                .into(),
        );
    }
    Ok(bytes as usize)
}

fn validate_png_envelope(bytes: &[u8]) -> Result<(), String> {
    if bytes.len() > MAX_PNG_BYTES {
        return Err("PNG file exceeds the 16 MiB input limit".into());
    }
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err("Selected image is not a PNG file".into());
    }
    // Require one complete static PNG, not a valid prefix of arbitrary data.
    // The png decoder still owns CRC, compression, and semantic validation.
    let mut pos = 8;
    let mut chunks = 0;
    let mut metadata_bytes = 0;
    while let Some(header) = bytes.get(pos..pos + 8) {
        chunks += 1;
        if chunks > MAX_PNG_CHUNKS {
            return Err("PNG exceeds the 4096-chunk limit".into());
        }
        let length = u32::from_be_bytes(header[..4].try_into().expect("chunk length")) as usize;
        let end = pos
            .checked_add(12)
            .and_then(|n| n.checked_add(length))
            .filter(|&n| n <= bytes.len())
            .ok_or("Truncated PNG chunk")?;
        let kind = &header[4..8];
        if kind[0].is_ascii_lowercase() {
            metadata_bytes += length + 12;
            if metadata_bytes > MAX_METADATA_BYTES {
                return Err("PNG metadata exceeds the 1 MiB limit".into());
            }
        }
        if pos == 8 && (kind != b"IHDR" || length != 13) {
            return Err("PNG must start with a 13-byte IHDR".into());
        }
        if kind == b"acTL" || kind == b"fcTL" || kind == b"fdAT" {
            return Err("Animated PNG is not supported; select a static PNG".into());
        }
        if kind == b"IEND" {
            return if length == 0 && end == bytes.len() {
                Ok(())
            } else {
                Err("PNG has an invalid IEND or trailing bytes".into())
            };
        }
        pos = end;
    }
    Err("PNG is truncated or missing IEND".into())
}

fn decode_png(bytes: &[u8], kind: ImageKind) -> Result<Raster, String> {
    validate_png_envelope(bytes)?;
    let mut decoder = png::Decoder::new_with_limits(
        Cursor::new(bytes),
        png::Limits {
            bytes: MAX_RGBA_BYTES,
        },
    );
    decoder.ignore_checksums(false);
    // We consume pixels, not user text or color-management profiles. Skip
    // their allocations in addition to bounding all chunks in the envelope.
    decoder.set_ignore_text_chunk(true);
    decoder.set_ignore_iccp_chunk(true);
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let header = decoder
        .read_header_info()
        .map_err(|e| format!("Invalid PNG header: {e}"))?;
    let size = [header.width as usize, header.height as usize];
    // Before read_info's row buffers or any decoded image allocation.
    let rgba_bytes = validate_dimensions(header.width, header.height, kind)?;
    let mut reader = decoder
        .read_info()
        .map_err(|e| format!("Invalid PNG: {e}"))?;
    let output_bytes = reader
        .output_buffer_size()
        .ok_or("PNG output size overflow")?;
    if output_bytes > rgba_bytes {
        return Err("PNG output exceeds the preflight RGBA budget".into());
    }
    let mut rgba = vec![0; rgba_bytes];
    let output = reader
        .next_frame(&mut rgba)
        .map_err(|e| format!("Cannot decode PNG: {e}"))?;
    reader
        .finish()
        .map_err(|e| format!("Incomplete or corrupt PNG: {e}"))?;
    if output.bit_depth != png::BitDepth::Eight
        || [output.width as usize, output.height as usize] != size
    {
        return Err("PNG decoder returned an unexpected frame format".into());
    }
    let channels = match output.color_type {
        png::ColorType::Rgba => 4,
        png::ColorType::Rgb => 3,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Grayscale => 1,
        png::ColorType::Indexed => return Err("PNG palette was not expanded".into()),
    };
    if output.buffer_size() != size[0] * size[1] * channels {
        return Err("PNG decoder returned an unexpected pixel count".into());
    }
    // Expand backwards in the same bounded allocation, preserving all source
    // pixels (including grayscale/palette alpha) without a second RGBA buffer.
    if channels != 4 {
        for p in (0..size[0] * size[1]).rev() {
            let i = p * channels;
            let pixel = match channels {
                1 => [rgba[i], rgba[i], rgba[i], 255],
                2 => [rgba[i], rgba[i], rgba[i], rgba[i + 1]],
                3 => [rgba[i], rgba[i + 1], rgba[i + 2], 255],
                _ => unreachable!(),
            };
            rgba[p * 4..p * 4 + 4].copy_from_slice(&pixel);
        }
    }
    Ok(Raster { size, rgba })
}

fn source_over(base: [u8; 4], hat: [u8; 4]) -> [u8; 4] {
    // Straight-alpha source-over in source sRGBA bytes; convert to egui's
    // premultiplied representation only after compositing the two layers.
    let a = u32::from(hat[3]);
    let b = u32::from(base[3]);
    let alpha = a * 255 + b * (255 - a);
    if alpha == 0 {
        return [0; 4];
    }
    let mut result = [0; 4];
    for c in 0..3 {
        result[c] = ((u32::from(hat[c]) * a * 255 + u32::from(base[c]) * b * (255 - a) + alpha / 2)
            / alpha) as u8;
    }
    result[3] = ((alpha + 127) / 255) as u8;
    result
}

fn head_image(skin: &Raster) -> Raster {
    let scale = skin.size[0] / 64;
    let side = 8 * scale;
    let mut face = Raster::filled([side, side], [0; 4]);
    for y in 0..side {
        for x in 0..side {
            let base = skin.pixel(8 * scale + x, 8 * scale + y);
            let hat = skin.pixel(40 * scale + x, 8 * scale + y);
            face.rect(x, y, x + 1, y + 1, source_over(base, hat));
        }
    }
    face
}

#[derive(PartialEq, Eq)]
struct FileKey {
    path: PathBuf,
    modified: SystemTime,
    len: u64,
}

impl FileKey {
    fn from_metadata(path: &Path, metadata: Metadata) -> Result<Self, String> {
        if !metadata.is_file() {
            return Err("Selected PNG path is not a regular file".into());
        }
        if metadata.len() > MAX_PNG_BYTES as u64 {
            return Err("PNG file exceeds the 16 MiB input limit".into());
        }
        let modified = metadata
            .modified()
            .map_err(|e| format!("Cannot inspect PNG modification time: {e}"))?;
        Ok(Self {
            path: path.to_owned(),
            modified,
            len: metadata.len(),
        })
    }

    fn inspect(path: &Path) -> Result<Self, String> {
        let metadata = fs::metadata(path).map_err(|e| format!("Cannot inspect local PNG: {e}"))?;
        Self::from_metadata(path, metadata)
    }
}

#[derive(PartialEq, Eq)]
enum SourceKey {
    BuiltIn,
    File(FileKey),
}

struct CachedTexture {
    source: SourceKey,
    manager: usize,
    max_texture_side: usize,
    result: Result<egui::TextureHandle, String>,
}

/// Retains one current custom head/background plus two original fallback handles.
/// Falling back must not evict the error or re-upload the original every frame.
/// Returned handles are clones; consumers retaining old clones own their lifetime.
#[derive(Default)]
pub struct MediaCache {
    head: Option<CachedTexture>,
    background: Option<CachedTexture>,
    fallback_head: Option<CachedTexture>,
    fallback_background: Option<CachedTexture>,
}

impl MediaCache {
    /// `None` returns the labeled original avatar, never a claimed account skin.
    /// Invalid local paths return `Err`; the UI decides whether to show fallback.
    pub fn head(
        &mut self,
        ctx: &egui::Context,
        skin_path: Option<&str>,
    ) -> Result<egui::TextureHandle, String> {
        let slot = if skin_path.is_none() {
            &mut self.fallback_head
        } else {
            &mut self.head
        };
        cached_texture(slot, ctx, skin_path, ImageKind::Head)
    }

    /// `None` returns the authored Solaris landscape, not a gameplay screenshot.
    pub fn background(
        &mut self,
        ctx: &egui::Context,
        path: Option<&str>,
    ) -> Result<egui::TextureHandle, String> {
        let slot = if path.is_none() {
            &mut self.fallback_background
        } else {
            &mut self.background
        };
        cached_texture(slot, ctx, path, ImageKind::Background)
    }
}

fn cached_texture(
    slot: &mut Option<CachedTexture>,
    ctx: &egui::Context,
    path: Option<&str>,
    kind: ImageKind,
) -> Result<egui::TextureHandle, String> {
    let source = match path {
        None => SourceKey::BuiltIn,
        Some(path) => match FileKey::inspect(Path::new(path)) {
            Ok(key) => SourceKey::File(key),
            Err(error) => {
                // Do not retain a previous profile's texture after an invalid
                // selection, deletion, permission failure, or oversize edit.
                *slot = None;
                return Err(error);
            }
        },
    };
    // Context clones share their manager; independent contexts must not reuse
    // a handle from another texture namespace. The cached handle keeps its
    // manager alive, so its address cannot be recycled while it is compared.
    let manager = std::sync::Arc::as_ptr(&ctx.tex_manager()) as usize;
    let max_texture_side = ctx.input(|input| input.max_texture_side);
    if let Some(cached) = slot.as_ref()
        && cached.source == source
        && cached.manager == manager
        && cached.max_texture_side == max_texture_side
    {
        return cached.result.clone();
    }
    *slot = None;
    // Retry transient I/O failures on the next request. Only deterministic
    // decoding failures belong to the path/mtime/length cache key.
    let custom_bytes = match &source {
        SourceKey::File(key) => Some(read_local_png_bytes(key)?),
        SourceKey::BuiltIn => None,
    };
    let result = (|| {
        let (image, label) = match (&source, kind) {
            (SourceKey::BuiltIn, ImageKind::Head) => (avatar(), FALLBACK_HEAD_LABEL),
            (SourceKey::BuiltIn, ImageKind::Background) => (landscape(), DEFAULT_BACKGROUND_LABEL),
            (SourceKey::File(_), kind) => {
                let image = decode_png(custom_bytes.as_deref().expect("File source bytes"), kind)?;
                match kind {
                    ImageKind::Head => (head_image(&image), "Local skin head preview"),
                    ImageKind::Background => (image, "Local PNG background"),
                }
            }
        };
        if image.size.iter().any(|&side| side > max_texture_side) {
            return Err(format!(
                "Image exceeds this device's {max_texture_side}-pixel texture limit"
            ));
        }
        Ok(ctx.load_texture(label, image.color_image(), egui::TextureOptions::NEAREST))
    })();
    *slot = Some(CachedTexture {
        source,
        manager,
        max_texture_side,
        result: result.clone(),
    });
    result
}

fn read_local_png_bytes(key: &FileKey) -> Result<Vec<u8>, String> {
    let mut file = File::open(&key.path).map_err(|e| format!("Cannot open local PNG: {e}"))?;
    let before = file
        .metadata()
        .map_err(|e| format!("Cannot inspect open PNG: {e}"))?;
    if FileKey::from_metadata(&key.path, before)? != *key {
        return Err("Local PNG changed before reading; retry after saving finishes".into());
    }
    let mut bytes = Vec::with_capacity(key.len as usize);
    // Metadata is not a read bound: cap the actual stream at exactly 16 MiB
    // too. The post-read metadata/length fences also reject concurrent growth.
    (&mut file)
        .take(MAX_PNG_BYTES as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("Cannot read local PNG: {e}"))?;
    if bytes.len() > MAX_PNG_BYTES {
        return Err("PNG file exceeds the 16 MiB input limit".into());
    }
    let after = file
        .metadata()
        .map_err(|e| format!("Cannot inspect read PNG: {e}"))?;
    if bytes.len() as u64 != key.len
        || FileKey::from_metadata(&key.path, after)? != *key
        || FileKey::inspect(&key.path)? != *key
    {
        return Err("Local PNG changed while reading; retry after saving finishes".into());
    }
    Ok(bytes)
}

#[cfg(test)]
fn read_local_png(key: &FileKey, kind: ImageKind) -> Result<Raster, String> {
    decode_png(&read_local_png_bytes(key)?, kind)
}

/// Centered normalized UVs for an aspect-preserving cover fit.
/// Degenerate/transient layouts return the full image rather than NaN UVs.
pub fn cover_uv(image: egui::Vec2, frame: egui::Vec2) -> egui::Rect {
    let unit = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    if [image.x, image.y, frame.x, frame.y]
        .iter()
        .any(|v| !v.is_finite() || *v <= 0.0)
    {
        return unit;
    }
    // f64 intermediates prevent overflow for otherwise finite f32 dimensions.
    let image_aspect = f64::from(image.x) / f64::from(image.y);
    let frame_aspect = f64::from(frame.x) / f64::from(frame.y);
    let (x, y) = if image_aspect > frame_aspect {
        (((1.0 - frame_aspect / image_aspect) / 2.0) as f32, 0.0)
    } else {
        (0.0, ((1.0 - image_aspect / frame_aspect) / 2.0) as f32)
    };
    egui::Rect::from_min_max(egui::pos2(x, y), egui::pos2(1.0 - x, 1.0 - y))
}

struct Raster {
    size: [usize; 2],
    rgba: Vec<u8>,
}

impl Raster {
    fn filled(size: [usize; 2], color: [u8; 4]) -> Self {
        Self {
            size,
            rgba: color.repeat(size[0] * size[1]),
        }
    }

    fn color_image(&self) -> egui::ColorImage {
        egui::ColorImage::from_rgba_unmultiplied(self.size, &self.rgba)
    }

    fn pixel(&self, x: usize, y: usize) -> [u8; 4] {
        let i = (y * self.size[0] + x) * 4;
        self.rgba[i..i + 4].try_into().expect("RGBA pixel")
    }

    fn rect(&mut self, x: usize, y: usize, right: usize, bottom: usize, color: [u8; 4]) {
        for py in y..bottom.min(self.size[1]) {
            for px in x..right.min(self.size[0]) {
                let i = (py * self.size[0] + px) * 4;
                self.rgba[i..i + 4].copy_from_slice(&color);
            }
        }
    }
}

fn avatar() -> Raster {
    // Original small slate robot, not a reproduction of any player/game skin.
    let mut image = Raster::filled([16, 16], [22, 32, 43, 255]);
    image.rect(2, 1, 14, 15, [73, 91, 105, 255]);
    image.rect(3, 2, 13, 4, [122, 144, 155, 255]);
    image.rect(2, 4, 3, 12, [57, 73, 85, 255]);
    image.rect(3, 5, 13, 11, [29, 45, 56, 255]);
    image.rect(4, 7, 6, 9, [171, 215, 216, 255]);
    image.rect(10, 7, 12, 9, [171, 215, 216, 255]);
    image.rect(6, 12, 10, 13, [146, 166, 172, 255]);
    image.rect(11, 2, 13, 4, [235, 173, 90, 255]);
    image
}

fn landscape() -> Raster {
    // Authored terraced coast at dusk. These are drawn pixels, NOT a game
    // screenshot, downloaded photograph, or extracted Minecraft texture.
    let mut image = Raster::filled([640, 360], [26, 46, 59, 255]);
    for y in 0..360 {
        let t = y.min(220) as u32;
        image.rect(
            0,
            y,
            640,
            y + 1,
            [
                (26 + t * 45 / 220) as u8,
                (46 + t * 49 / 220) as u8,
                (59 + t * 37 / 220) as u8,
                255,
            ],
        );
    }
    image.rect(458, 48, 496, 86, [228, 182, 113, 255]);
    image.rect(454, 56, 500, 78, [228, 182, 113, 255]);
    for (x, y, w) in [(42, 65, 102), (108, 74, 71), (339, 39, 68), (530, 101, 82)] {
        image.rect(x, y, x + w, y + 5, [87, 111, 120, 255]);
        image.rect(x + 15, y - 4, x + w - 12, y, [87, 111, 120, 255]);
    }
    // Low-detail stepped silhouettes keep the visual language deliberately
    // illustrative and leave room for launcher text on either side.
    for x in (0usize..640).step_by(8) {
        let distant = 148 + ((x / 8 + 7) % 23).abs_diff(11) * 3;
        image.rect(x, distant, x + 8, 254, [57, 83, 88, 255]);
        let near = 190 + ((x / 8 + 14) % 31).abs_diff(15) * 3;
        image.rect(x, near, x + 8, 300, [40, 69, 73, 255]);
    }
    image.rect(0, 254, 640, 360, [31, 63, 73, 255]);
    for y in (264..360).step_by(12) {
        let inset = (y - 254) * 2;
        image.rect(
            430usize.saturating_sub(inset),
            y,
            490 + inset / 3,
            y + 2,
            [63, 95, 99, 255],
        );
    }
    for (x, y, right, bottom) in [
        (0, 238, 136, 360),
        (0, 218, 88, 240),
        (88, 264, 184, 360),
        (496, 222, 640, 360),
        (544, 204, 640, 224),
        (456, 282, 640, 360),
    ] {
        image.rect(x, y, right, bottom, [29, 48, 52, 255]);
        image.rect(x, y, right, y + 6, [78, 106, 86, 255]);
        image.rect(x + 8, y + 10, right, y + 13, [43, 63, 62, 255]);
    }
    for (x, y, height) in [
        (49, 171, 49),
        (110, 218, 46),
        (577, 152, 54),
        (525, 180, 43),
    ] {
        image.rect(x + 8, y + 15, x + 13, y + height, [71, 76, 65, 255]);
        image.rect(x + 4, y, x + 17, y + 14, [54, 87, 76, 255]);
        image.rect(x, y + 10, x + 22, y + 26, [48, 79, 68, 255]);
        image.rect(x + 2, y + 22, x + 20, y + 34, [41, 71, 63, 255]);
    }
    image
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_envelope_rejects_missing_end_and_overflowing_chunk_lengths() {
        for bytes in [
            b"\x89PNG\r\n\x1a\n".to_vec(),
            b"\x89PNG\r\n\x1a\n\xff\xff\xff\xffIHDR".to_vec(),
            b"\x89PNG\r\n\x1a\n\0\0\0\0IEND\xae\x42\x60\x82".to_vec(),
        ] {
            assert!(validate_png_envelope(&bytes).is_err());
        }
    }

    #[test]
    fn background_respects_the_native_texture_limit_before_upload() {
        let files = TestFiles::new();
        let path = files.write(
            "too-wide.png",
            &encode_rgba(&Raster::filled([2049, 1], [12, 34, 56, 255])),
        );
        let ctx = egui::Context::default();
        let mut cache = MediaCache::default();
        let output = ctx.run(
            egui::RawInput {
                max_texture_side: Some(2048),
                ..Default::default()
            },
            |ctx| {
                assert!(
                    cache.background(ctx, Some(&path)).is_err(),
                    "Oversized texture must return an error, not reach the renderer"
                );
            },
        );
        assert!(
            !output
                .textures_delta
                .set
                .iter()
                .any(|(_, d)| d.image.width() == 2049)
        );
    }

    #[test]
    fn illustration_pixels_are_deterministic_opaque_and_not_flat() {
        for (first, second) in [(avatar(), avatar()), (landscape(), landscape())] {
            assert_eq!(first.rgba, second.rgba);
            assert_eq!(first.rgba.len(), first.size[0] * first.size[1] * 4);
            assert!(first.rgba.chunks_exact(4).all(|p| p[3] == 255));
            let colors: std::collections::BTreeSet<_> = first.rgba.chunks_exact(4).collect();
            assert!(colors.len() >= 8);
        }
        assert_eq!(landscape().pixel(470, 60), [228, 182, 113, 255]);
    }

    #[test]
    fn replacing_a_file_after_inspection_is_rejected() {
        let files = TestFiles::new();
        let path = files.write("changing.png", b"old contents");
        let key = FileKey::inspect(Path::new(&path)).unwrap();
        std::fs::write(&path, b"new and longer contents").unwrap();
        assert!(
            read_local_png(&key, ImageKind::Background)
                .err()
                .unwrap()
                .contains("changed")
        );
    }

    #[test]
    fn cached_decode_error_recovers_after_a_new_file_revision() {
        let files = TestFiles::new();
        let path = files.write("recover.png", b"invalid");
        let ctx = egui::Context::default();
        let mut cache = MediaCache::default();
        assert!(cache.background(&ctx, Some(&path)).is_err());
        assert!(cache.background(&ctx, Some(&path)).is_err());
        std::fs::write(
            &path,
            encode_rgba(&Raster::filled([3, 2], [10, 20, 30, 255])),
        )
        .unwrap();
        assert_eq!(cache.background(&ctx, Some(&path)).unwrap().size(), [3, 2]);
    }

    #[test]
    fn invalid_skin_then_fallback_does_not_upload_every_frame() {
        let files = TestFiles::new();
        let bad = files.write("broken-skin.png", b"broken png");
        let ctx = egui::Context::default();
        let mut cache = MediaCache::default();
        for path in [&bad, "missing-skin.png"] {
            assert!(cache.head(&ctx, Some(path)).is_err());
            let fallback = cache.head(&ctx, None).unwrap();
            let _ = ctx.tex_manager().write().take_delta();
            for _ in 0..4 {
                assert!(cache.head(&ctx, Some(path)).is_err());
                assert_eq!(cache.head(&ctx, None).unwrap().id(), fallback.id());
                assert!(ctx.tex_manager().write().take_delta().set.is_empty());
            }
        }
    }

    #[test]
    fn metadata_chunk_flood_is_bounded_before_and_after_image_data() {
        for after_idat in [false, true] {
            let mut bytes = Vec::new();
            {
                let encoder = png::Encoder::new(&mut bytes, 1, 1);
                let mut writer = encoder.write_header().unwrap();
                if after_idat {
                    writer.write_image_data(&[0]).unwrap();
                }
                for _ in 0..5000 {
                    writer
                        .write_chunk(png::chunk::iTXt, b"k\0\0\0\0\0")
                        .unwrap();
                }
                if !after_idat {
                    writer.write_image_data(&[0]).unwrap();
                }
                writer.finish().unwrap();
            }
            assert!(bytes.len() < MAX_PNG_BYTES);
            assert!(
                validate_png_envelope(&bytes).is_err(),
                "Chunk floods must be rejected before decoder allocation"
            );
        }
    }

    #[test]
    fn independent_contexts_receive_their_own_texture_upload() {
        let mut cache = MediaCache::default();
        let first_ctx = egui::Context::default();
        let first = cache.head(&first_ctx, None).unwrap();
        let second_ctx = egui::Context::default();
        let second = cache.head(&second_ctx, None).unwrap();
        let delta = second_ctx.tex_manager().write().take_delta();
        assert!(delta.set.iter().any(|(id, _)| *id == second.id()));
        assert_eq!(first.size(), second.size());
    }

    #[cfg(windows)]
    #[test]
    fn transient_windows_file_lock_recovers_without_changing_image_metadata() {
        use std::os::windows::fs::OpenOptionsExt;
        let files = TestFiles::new();
        let path = files.write(
            "temporarily-locked.png",
            &encode_rgba(&Raster::filled([2, 2], [10, 20, 30, 255])),
        );
        let before = FileKey::inspect(Path::new(&path)).unwrap();
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&path)
            .unwrap();
        assert!(
            FileKey::inspect(Path::new(&path)).unwrap() == before,
            "The cache key must remain readable while content is locked"
        );
        let ctx = egui::Context::default();
        let mut cache = MediaCache::default();
        assert!(cache.background(&ctx, Some(&path)).is_err());
        drop(lock);
        assert!(FileKey::inspect(Path::new(&path)).unwrap() == before);
        assert_eq!(cache.background(&ctx, Some(&path)).unwrap().size(), [2, 2]);
    }

    struct TestFiles(std::path::PathBuf);

    impl TestFiles {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let root = std::env::var_os("TMPDIR")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(std::env::temp_dir);
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = root.join(format!(
                "solaris-media-{}-{nonce}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn write(&self, name: &str, bytes: &[u8]) -> String {
            let path = self.0.join(name);
            std::fs::write(&path, bytes).unwrap();
            path.to_str().unwrap().to_owned()
        }

        fn stamp(path: &str, seconds: u64) {
            let time = std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds);
            std::fs::File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_times(std::fs::FileTimes::new().set_modified(time))
                .unwrap();
        }
    }

    impl Drop for TestFiles {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn public_head_uploads_local_face_with_nearest_filtering() {
        let files = TestFiles::new();
        let mut skin = Raster::filled([64, 64], [0; 4]);
        skin.rect(8, 8, 16, 16, [10, 20, 30, 255]);
        skin.rect(40, 8, 41, 9, [210, 120, 70, 128]);
        let path = files.write("authorized-local.png", &encode_rgba(&skin));
        let ctx = egui::Context::default();
        let mut cache = MediaCache::default();
        let head = cache.head(&ctx, Some(&path)).unwrap();
        assert_eq!(head.size(), [8, 8]);
        let delta = ctx.tex_manager().write().take_delta();
        let (_, upload) = delta.set.iter().find(|(id, _)| *id == head.id()).unwrap();
        assert_eq!(upload.options, egui::TextureOptions::NEAREST);
        let egui::ImageData::Color(image) = &upload.image;
        assert_eq!(image.pixels[0].to_array(), [110, 70, 50, 255]);
        assert_eq!(head.id(), cache.head(&ctx, Some(&path)).unwrap().id());
        assert!(ctx.tex_manager().write().take_delta().set.is_empty());
    }

    #[test]
    fn background_cache_invalidates_for_mtime_size_and_path_without_sleeping() {
        let files = TestFiles::new();
        let first_bytes = encode_rgba(&Raster::filled([2, 2], [10, 20, 30, 255]));
        let path = files.write("background.png", &first_bytes);
        TestFiles::stamp(&path, 100);
        let ctx = egui::Context::default();
        let mut cache = MediaCache::default();
        let first = cache.background(&ctx, Some(&path)).unwrap();
        assert_eq!(first.size(), [2, 2]);
        assert_eq!(
            first.id(),
            cache.background(&ctx, Some(&path)).unwrap().id()
        );
        TestFiles::stamp(&path, 200);
        let modified = cache.background(&ctx, Some(&path)).unwrap();
        assert_ne!(first.id(), modified.id());
        let bigger = encode_rgba(&Raster::filled([256, 256], [70, 80, 90, 255]));
        assert_ne!(first_bytes.len(), bigger.len());
        std::fs::write(&path, &bigger).unwrap();
        TestFiles::stamp(&path, 200);
        let resized = cache.background(&ctx, Some(&path)).unwrap();
        assert_eq!(resized.size(), [256, 256]);
        assert_ne!(modified.id(), resized.id());
        let other_path = files.write("other.png", &bigger);
        let other = cache.background(&ctx, Some(&other_path)).unwrap();
        assert_ne!(other.id(), resized.id());
        let back = cache.background(&ctx, Some(&path)).unwrap();
        assert_ne!(
            back.id(),
            resized.id(),
            "only current background is retained"
        );
    }

    #[test]
    fn head_cache_retains_only_current_source_and_background_stays_independent() {
        let files = TestFiles::new();
        let bytes = encode_rgba(&Raster::filled([64, 32], [70, 80, 90, 255]));
        let a = files.write("a.png", &bytes);
        let b = files.write("b.png", &bytes);
        let ctx = egui::Context::default();
        let mut cache = MediaCache::default();
        let background = cache.background(&ctx, None).unwrap();
        let first = cache.head(&ctx, Some(&a)).unwrap();
        assert_eq!(first.id(), cache.head(&ctx, Some(&a)).unwrap().id());
        let other = cache.head(&ctx, Some(&b)).unwrap();
        assert_ne!(first.id(), other.id());
        assert_ne!(first.id(), cache.head(&ctx, Some(&a)).unwrap().id());
        assert_eq!(background.id(), cache.background(&ctx, None).unwrap().id());
        assert_eq!(cache.head(&ctx, None).unwrap().size(), [16, 16]);
    }

    #[test]
    fn invalid_custom_paths_return_errors_instead_of_claiming_fallback_is_a_skin() {
        let files = TestFiles::new();
        let bad = files.write("not-an-image.png", b"not a PNG");
        let ctx = egui::Context::default();
        let mut cache = MediaCache::default();
        assert!(cache.head(&ctx, Some(&bad)).is_err());
        assert!(cache.background(&ctx, Some(&bad)).is_err());
        std::fs::remove_file(&bad).unwrap();
        assert!(cache.head(&ctx, Some(&bad)).is_err());
        assert!(cache.background(&ctx, Some(&bad)).is_err());
        assert!(cache.head.is_none());
        assert!(cache.background.is_none());
        assert!(cache.head(&ctx, Some("")).is_err());
        assert!(
            cache
                .background(&ctx, Some(files.0.to_str().unwrap()))
                .is_err()
        );
        assert_eq!(cache.head(&ctx, None).unwrap().size(), [16, 16]);
    }

    #[test]
    fn file_metadata_rejects_oversized_input_before_reading() {
        let files = TestFiles::new();
        let path = files.write("too-large.png", &[]);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(MAX_PNG_BYTES as u64 + 1)
            .unwrap();
        let ctx = egui::Context::default();
        assert!(
            MediaCache::default()
                .background(&ctx, Some(&path))
                .err()
                .unwrap()
                .contains("16 MiB")
        );
    }

    #[test]
    fn cover_uv_centers_crop_without_stretching() {
        let full = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        assert_eq!(
            cover_uv(egui::vec2(16.0, 9.0), egui::vec2(160.0, 90.0)),
            full
        );
        assert_eq!(
            cover_uv(egui::vec2(2.0, 1.0), egui::vec2(1.0, 1.0)),
            egui::Rect::from_min_max(egui::pos2(0.25, 0.0), egui::pos2(0.75, 1.0))
        );
        assert_eq!(
            cover_uv(egui::vec2(1.0, 2.0), egui::vec2(1.0, 1.0)),
            egui::Rect::from_min_max(egui::pos2(0.0, 0.25), egui::pos2(1.0, 0.75))
        );
        for (image, frame) in [
            (egui::vec2(640.0, 360.0), egui::vec2(240.0, 700.0)),
            (egui::vec2(300.0, 900.0), egui::vec2(800.0, 200.0)),
        ] {
            let uv = cover_uv(image, frame);
            assert!(
                (uv.width() * image.x / (uv.height() * image.y) - frame.x / frame.y).abs() < 0.0001
            );
        }
    }

    #[test]
    fn cover_uv_is_finite_for_zero_negative_and_nonfinite_layouts() {
        let unit = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        for bad in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert_eq!(
                cover_uv(egui::vec2(bad, 10.0), egui::vec2(100.0, 100.0)),
                unit
            );
            assert_eq!(
                cover_uv(egui::vec2(10.0, 10.0), egui::vec2(100.0, bad)),
                unit
            );
        }
    }

    fn encode(
        size: [usize; 2],
        color: png::ColorType,
        depth: png::BitDepth,
        pixels: &[u8],
    ) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, size[0] as u32, size[1] as u32);
            encoder.set_color(color);
            encoder.set_depth(depth);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(pixels).unwrap();
            writer.finish().unwrap();
        }
        bytes
    }

    fn encode_rgba(image: &Raster) -> Vec<u8> {
        encode(
            image.size,
            png::ColorType::Rgba,
            png::BitDepth::Eight,
            &image.rgba,
        )
    }

    #[test]
    fn face_and_hat_use_exact_uvs_without_hd_resampling() {
        for scale in [1, 2, 16] {
            for height in [32, 64] {
                let mut skin = Raster::filled([64 * scale, height * scale], [255, 0, 255, 255]);
                skin.rect(
                    8 * scale,
                    8 * scale,
                    16 * scale,
                    16 * scale,
                    [10, 20, 30, 255],
                );
                skin.rect(40 * scale, 8 * scale, 48 * scale, 16 * scale, [0; 4]);
                skin.rect(
                    40 * scale,
                    8 * scale,
                    40 * scale + 1,
                    8 * scale + 1,
                    [210, 120, 70, 128],
                );
                skin.rect(
                    48 * scale - 1,
                    16 * scale - 1,
                    48 * scale,
                    16 * scale,
                    [90, 40, 20, 255],
                );
                skin.rect(
                    8 * scale + 1,
                    8 * scale,
                    8 * scale + 2,
                    8 * scale + 1,
                    [70, 80, 90, 255],
                );
                let decoded = decode_png(&encode_rgba(&skin), ImageKind::Head).unwrap();
                let face = head_image(&decoded);
                assert_eq!(face.size, [8 * scale, 8 * scale]);
                assert_eq!(face.pixel(0, 0), [110, 70, 50, 255]);
                assert_eq!(face.pixel(1, 0), [70, 80, 90, 255]);
                assert_eq!(face.pixel(0, 1), [10, 20, 30, 255]);
                assert_eq!(face.pixel(8 * scale - 1, 8 * scale - 1), [90, 40, 20, 255]);
            }
        }
    }

    #[test]
    fn source_over_handles_transparent_base_and_hat() {
        assert_eq!(
            source_over([10, 20, 30, 255], [255, 0, 0, 0]),
            [10, 20, 30, 255]
        );
        assert_eq!(
            source_over([255, 0, 0, 0], [10, 20, 30, 128]),
            [10, 20, 30, 128]
        );
        assert_eq!(source_over([255, 0, 0, 0], [0; 4]), [0; 4]);
        assert_eq!(
            source_over([0, 0, 255, 128], [255, 0, 0, 128]),
            [170, 0, 85, 192]
        );
    }

    #[test]
    fn skin_dimensions_require_integer_scale_and_square_or_two_to_one() {
        for scale in 1..=16 {
            for height in [32, 64] {
                assert!(validate_dimensions(64 * scale, height * scale, ImageKind::Head).is_ok());
            }
        }
        for (w, h) in [
            (32, 32),
            (65, 65),
            (96, 96),
            (64, 48),
            (128, 32),
            (1088, 544),
        ] {
            assert!(
                validate_dimensions(w, h, ImageKind::Head).is_err(),
                "{w}x{h}"
            );
        }
    }

    #[test]
    fn rgba_budget_is_checked_before_allocating_frame_pixels() {
        assert_eq!(
            validate_dimensions(4096, 4096, ImageKind::Background).unwrap(),
            64 * 1024 * 1024
        );
        for (w, h) in [(0, 64), (64, 0), (4097, 1), (1, 4097), (u32::MAX, u32::MAX)] {
            assert!(validate_dimensions(w, h, ImageKind::Background).is_err());
        }
        let bytes = encode(
            [4097, 1],
            png::ColorType::Rgba,
            png::BitDepth::Eight,
            &[0, 0, 0, 255].repeat(4097),
        );
        assert!(
            decode_png(&bytes, ImageKind::Background)
                .err()
                .unwrap()
                .contains("4096")
        );
    }

    #[test]
    fn decoder_normalizes_rgb_gray_gray_alpha_and_sixteen_bit() {
        for (color, depth, pixels, expected) in [
            (
                png::ColorType::Rgb,
                png::BitDepth::Eight,
                vec![10, 20, 30],
                [10, 20, 30, 255],
            ),
            (
                png::ColorType::Grayscale,
                png::BitDepth::Eight,
                vec![45],
                [45, 45, 45, 255],
            ),
            (
                png::ColorType::GrayscaleAlpha,
                png::BitDepth::Eight,
                vec![45, 128],
                [45, 45, 45, 128],
            ),
            (
                png::ColorType::Rgba,
                png::BitDepth::Sixteen,
                vec![18, 255, 52, 255, 86, 255, 120, 255],
                [18, 52, 86, 120],
            ),
            (
                png::ColorType::Grayscale,
                png::BitDepth::One,
                vec![128],
                [255, 255, 255, 255],
            ),
        ] {
            let decoded = decode_png(
                &encode([1, 1], color, depth, &pixels),
                ImageKind::Background,
            )
            .unwrap();
            assert_eq!(decoded.pixel(0, 0), expected);
        }
    }

    #[test]
    fn decoder_expands_palette_transparency() {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, 2, 1);
            encoder.set_color(png::ColorType::Indexed);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.set_palette(vec![10, 20, 30, 40, 50, 60]);
            encoder.set_trns(vec![255, 128]);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&[0, 1]).unwrap();
            writer.finish().unwrap();
        }
        let image = decode_png(&bytes, ImageKind::Background).unwrap();
        assert_eq!(image.pixel(0, 0), [10, 20, 30, 255]);
        assert_eq!(image.pixel(1, 0), [40, 50, 60, 128]);
    }

    #[test]
    fn decoder_rejects_non_png_truncation_corruption_and_trailing_bytes() {
        let valid = encode_rgba(&Raster::filled([2, 2], [10, 20, 30, 255]));
        for bytes in [
            b"not a PNG".to_vec(),
            valid[..valid.len() - 1].to_vec(),
            [valid.as_slice(), b"trailer"].concat(),
        ] {
            assert!(decode_png(&bytes, ImageKind::Background).is_err());
        }
        let mut corrupt = valid;
        let idat = corrupt.windows(4).position(|w| w == b"IDAT").unwrap();
        let length = u32::from_be_bytes(corrupt[idat - 4..idat].try_into().unwrap()) as usize;
        corrupt[idat + 4 + length] ^= 1; // Only CRC: compressed pixels remain valid.
        assert!(decode_png(&corrupt, ImageKind::Background).is_err());
    }

    #[test]
    fn decoder_rejects_encoded_input_above_sixteen_mib() {
        let too_large = vec![0; MAX_PNG_BYTES + 1];
        assert!(
            decode_png(&too_large, ImageKind::Background)
                .err()
                .unwrap()
                .contains("16 MiB")
        );
    }

    #[test]
    fn decoder_rejects_animation_instead_of_ignoring_extra_frames() {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, 1, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.set_animated(1, 0).unwrap();
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&[0, 0, 0, 255]).unwrap();
            writer.finish().unwrap();
        }
        assert!(
            decode_png(&bytes, ImageKind::Background)
                .err()
                .unwrap()
                .contains("Animated")
        );
    }

    // New API contract, written before implementation. Cargo execution belongs
    // to the integration owner; this is not an observed behavioral RED result.
    #[test]
    fn offline_defaults_are_cached_and_clearly_identified() {
        let ctx = egui::Context::default();
        let mut cache = MediaCache::default();
        let head = cache.head(&ctx, None).unwrap();
        assert_eq!(head.size(), [16, 16]);
        assert_eq!(head.id(), cache.head(&ctx, None).unwrap().id());
        let background = cache.background(&ctx, None).unwrap();
        assert_eq!(background.size(), [640, 360]);
        assert_eq!(background.id(), cache.background(&ctx, None).unwrap().id());
        assert_ne!(head.id(), background.id());
        assert!(FALLBACK_HEAD_LABEL.contains("original"));
        assert!(FALLBACK_HEAD_LABEL.contains("not your skin"));
        assert!(DEFAULT_BACKGROUND_LABEL.contains("Solaris landscape"));
        assert!(LOCAL_SKIN_NOTICE.contains("does not change"));
    }
}
