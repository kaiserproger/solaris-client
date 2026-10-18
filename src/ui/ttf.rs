//! Main-thread FreeType faces own verified resource bytes; immutable raster runs are baked once.
use super::font::{Glyph, GlyphImage};
use crate::protocol::Result;
use freetype::{Face, Library, Matrix, Vector, bitmap::PixelMode, face::LoadFlag};
use std::{
    cell::Cell,
    collections::BTreeSet,
    fmt,
    rc::Rc,
    sync::{Arc, OnceLock},
};

pub(crate) struct RasterBudget {
    used: Cell<usize>,
}
impl RasterBudget {
    pub(crate) fn new() -> Self {
        Self { used: Cell::new(0) }
    }
    fn reserve(&self, bytes: usize) -> Result<()> {
        let total = self
            .used
            .get()
            .checked_add(bytes)
            .ok_or("font raster aggregate overflow")?;
        if total > 64 * 1024 * 1024 {
            return Err("font raster aggregate bound".into());
        }
        self.used.set(total);
        Ok(())
    }
}
pub(crate) struct TrueTypeProvider {
    face: Face<Vec<u8>>,
    oversample: f32,
    budget: Rc<RasterBudget>,
}
impl fmt::Debug for TrueTypeProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TrueTypeProvider")
            .field("oversample", &self.oversample)
            .finish_non_exhaustive()
    }
}
#[derive(Clone, Copy, Debug)]
pub struct RasterRun {
    pub bounds: [u16; 4],
    pub coverage: u8,
}
#[derive(Debug)]
pub struct Raster {
    pub width: u16,
    pub height: u16,
    pub alpha: Arc<[u8]>,
    pub runs: Box<[RasterRun]>,
}
#[derive(Debug)]
pub struct RasterGlyph {
    provider: Arc<TrueTypeProvider>,
    index: u32,
    cached: OnceLock<Result<Raster>>,
}
impl RasterGlyph {
    pub fn raster(&self) -> Result<&Raster> {
        self.cached
            .get_or_init(|| self.provider.raster(self.index))
            .as_ref()
            .map_err(Clone::clone)
    }
}
impl TrueTypeProvider {
    pub(crate) fn load(
        bytes: Vec<u8>,
        size: f32,
        oversample: f32,
        shift: [f32; 2],
        budget: Rc<RasterBudget>,
    ) -> Result<Arc<Self>> {
        if bytes.len() > 32 * 1024 * 1024 || bytes.is_empty() {
            return Err("TrueType resource byte bound".into());
        }
        if !size.is_finite()
            || !oversample.is_finite()
            || size <= 0.
            || oversample <= 0.
            || size * oversample > 1024.
            || shift.iter().any(|v| !v.is_finite() || v.abs() > 1024.)
        {
            return Err("TrueType size/oversample/shift bound".into());
        }
        let library = Library::init().map_err(|e| format!("FreeType initialization: {e}"))?;
        let face = library
            .new_memory_face2(bytes, 0)
            .map_err(|e| format!("TrueType memory face: {e}"))?;
        // Math.round(float), including negative half ties, is floor(x + 0.5).
        let pixels = (size * oversample + 0.5).floor() as u32;
        if pixels == 0 {
            return Err("TrueType pixel size is zero".into());
        }
        face.set_pixel_sizes(pixels, pixels)
            .map_err(|e| format!("TrueType pixel size: {e}"))?;
        let mut matrix = Matrix {
            xx: 65536,
            xy: 0,
            yx: 0,
            yy: 65536,
        };
        let mut delta = Vector {
            x: (shift[0] * oversample * 64. + 0.5).floor() as _,
            y: (-shift[1] * oversample * 64. + 0.5).floor() as _,
        };
        face.set_transform(&mut matrix, &mut delta);
        Ok(Arc::new(Self {
            face,
            oversample,
            budget,
        }))
    }
    pub(crate) fn glyphs<'a>(
        &'a self,
        skip: &'a BTreeSet<char>,
    ) -> impl Iterator<Item = (char, u32)> + 'a {
        self.face.chars().filter_map(|(code, index)| {
            char::from_u32(u32::try_from(code).ok()?)
                .filter(|ch| !skip.contains(ch))
                .map(|ch| (ch, index.get()))
        })
    }
    pub(crate) fn glyph(self: &Arc<Self>, index: u32) -> Result<Glyph> {
        let face = &self.face;
        // Exact pinned metrics load: FT_LOAD_NO_BITMAP | FT_LOAD_BITMAP_METRICS_ONLY.
        face.load_glyph(index, LoadFlag::from_bits_retain(4_194_312))
            .map_err(|e| format!("TrueType glyph metrics: {e}"))?;
        let slot = face.glyph();
        let bitmap = slot.bitmap();
        let width = bitmap.width();
        let height = bitmap.rows();
        if !(0..=1024).contains(&width) || !(0..=1024).contains(&height) {
            return Err("TrueType glyph dimensions bound".into());
        }
        let advance = slot.advance().x as f32 / 64. / self.oversample;
        if !advance.is_finite() || advance.abs() > 1024. {
            return Err("TrueType glyph advance bound".into());
        }
        Ok(Glyph {
            image: (width > 0 && height > 0).then(|| {
                GlyphImage::Raster(Arc::new(RasterGlyph {
                    provider: Arc::clone(self),
                    index,
                    cached: OnceLock::new(),
                }))
            }),
            advance,
            width: width as f32 / self.oversample,
            height: height as f32 / self.oversample,
            ascent: slot.bitmap_top() as f32 / self.oversample,
            bearing_x: slot.bitmap_left() as f32 / self.oversample,
        })
    }
    fn raster(&self, index: u32) -> Result<Raster> {
        let face = &self.face;
        // NativeImage.copyFromFont uses FT_LOAD_RENDER, not the metrics flags.
        face.load_glyph(index, LoadFlag::RENDER)
            .map_err(|e| format!("TrueType glyph raster: {e}"))?;
        let bitmap = face.glyph().bitmap();
        let width = u16::try_from(bitmap.width()).map_err(|_| "TrueType bitmap width")?;
        let height = u16::try_from(bitmap.rows()).map_err(|_| "TrueType bitmap height")?;
        if width > 1024
            || height > 1024
            || bitmap.pixel_mode().map_err(|e| e.to_string())? != PixelMode::Gray
        {
            return Err("TrueType bitmap dimensions/pixel mode".into());
        }
        let stride = bitmap.pitch().unsigned_abs() as usize;
        if stride < usize::from(width) {
            return Err("TrueType bitmap pitch".into());
        }
        let bytes = usize::from(width) * usize::from(height);
        self.budget.reserve(bytes)?;
        let source = bitmap.buffer();
        let mut alpha = Vec::with_capacity(bytes);
        for y in 0..usize::from(height) {
            let row = if bitmap.pitch() < 0 {
                usize::from(height) - 1 - y
            } else {
                y
            };
            let start = row.checked_mul(stride).ok_or("TrueType bitmap offset")?;
            alpha.extend_from_slice(
                source
                    .get(start..start + usize::from(width))
                    .ok_or("TrueType bitmap buffer")?,
            );
        }
        let mut runs = Vec::new();
        for y in 0..height {
            let row = &alpha
                [usize::from(y) * usize::from(width)..usize::from(y + 1) * usize::from(width)];
            let mut x = 0u16;
            while x < width {
                let coverage = row[usize::from(x)];
                let start = x;
                x += 1;
                while x < width && row[usize::from(x)] == coverage {
                    x += 1;
                }
                if coverage != 0 {
                    runs.push(RasterRun {
                        bounds: [start, y, x - start, 1],
                        coverage,
                    });
                }
            }
        }
        self.budget
            .reserve(runs.len() * std::mem::size_of::<RasterRun>())?;
        Ok(Raster {
            width,
            height,
            alpha: alpha.into(),
            runs: runs.into_boxed_slice(),
        })
    }
}
