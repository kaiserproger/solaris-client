//! Verified resource-pack providers; no host font substitution or fabricated glyph.
use crate::protocol::{Result, content::Content};
use serde_json::Value;
use std::{
    cell::Cell,
    collections::BTreeMap,
    collections::BTreeSet,
    io::{Cursor, Read},
    rc::Rc,
    sync::Arc,
};
#[derive(Clone, Debug)]
pub enum GlyphImage {
    Texture {
        name: String,
        uv: [f32; 4],
    },
    Bits {
        rows: [u32; 16],
        left: u8,
        right: u8,
        width: u8,
    },
    Raster(Arc<super::ttf::RasterGlyph>),
    Missing,
}
#[derive(Clone, Debug)]
pub struct Glyph {
    pub image: Option<GlyphImage>,
    pub advance: f32,
    pub width: f32,
    pub height: f32,
    pub ascent: f32,
    pub bearing_x: f32,
}
impl Glyph {
    pub(super) fn bold_offset(&self) -> f32 {
        if matches!(&self.image, Some(GlyphImage::Bits { .. })) {
            0.5
        } else {
            1.
        }
    }
    pub(super) fn shadow_offset(&self) -> f32 {
        self.bold_offset()
    }
    pub(super) fn styled_advance(&self, bold: bool) -> f32 {
        self.advance + if bold { self.bold_offset() } else { 0. }
    }
}
pub struct Font {
    pub glyphs: BTreeMap<char, Arc<Glyph>>,
    budget: Rc<FontBudget>,
}
struct FontBudget {
    raster: Rc<super::ttf::RasterBudget>,
    resource_bytes: Cell<usize>,
    provider_count: Cell<usize>,
    glyph_entries: Cell<usize>,
    missing: Glyph,
}
impl FontBudget {
    fn admit_glyph(&self) -> Result<()> {
        let count = self
            .glyph_entries
            .get()
            .checked_add(1)
            .ok_or("font aggregate glyph overflow")?;
        if count > 1_000_000 {
            return Err("font aggregate glyph bound".into());
        }
        self.glyph_entries.set(count);
        Ok(())
    }
}
fn resource(kind: &str, id: &str) -> Result<String> {
    let canonical = crate::protocol::parse_identifier(id)?;
    let (ns, name) = canonical
        .split_once(':')
        .expect("canonical protocol identifier");
    Ok(format!("assets/{ns}/{kind}/{name}"))
}

pub(super) fn bitmap_metrics(cells: [u32; 2], ink: u32, height: i32) -> [f32; 3] {
    let scale = height as f32 / cells[1] as f32;
    let oversample = 1. / scale;
    let advance = ((0.5f64 + f64::from(ink as f32 * scale)) as i32).wrapping_add(1);
    [
        advance as f32,
        cells[0] as f32 / oversample,
        cells[1] as f32 / oversample,
    ]
}
impl Font {
    pub(super) fn all_missing(&self) -> Self {
        Self {
            glyphs: BTreeMap::new(),
            budget: Rc::clone(&self.budget),
        }
    }
    pub fn load_all(
        content: &Content,
        uniform: bool,
        japanese: bool,
    ) -> Result<BTreeMap<String, Arc<Self>>> {
        let budget = Rc::new(FontBudget {
            raster: Rc::new(super::ttf::RasterBudget::new()),
            resource_bytes: Cell::new(0),
            provider_count: Cell::new(0),
            glyph_entries: Cell::new(0),
            // Pinned SpecialGlyphs.MISSING: 5x8 opaque-white border, advance 6.
            missing: Glyph {
                image: Some(GlyphImage::Missing),
                advance: 6.,
                width: 5.,
                height: 8.,
                ascent: 7.,
                bearing_x: 0.,
            },
        });
        let mut fonts = BTreeMap::new();
        for id in content.font_ids() {
            Self::load_cached(
                content,
                id,
                uniform,
                japanese,
                &mut Vec::new(),
                &mut fonts,
                &budget,
            )?;
        }
        Ok(fonts)
    }
    fn load_cached(
        content: &Content,
        id: &str,
        uniform: bool,
        japanese: bool,
        chain: &mut Vec<String>,
        fonts: &mut BTreeMap<String, Arc<Self>>,
        budget: &Rc<FontBudget>,
    ) -> Result<Arc<Self>> {
        let canonical = if id.contains(':') {
            std::borrow::Cow::Borrowed(id)
        } else {
            std::borrow::Cow::Owned(format!("minecraft:{id}"))
        };
        let id = canonical.as_ref();
        if let Some(font) = fonts.get(id) {
            return Ok(Arc::clone(font));
        }
        let mut out = Self {
            glyphs: BTreeMap::new(),
            budget: Rc::clone(budget),
        };
        out.providers(content, id, uniform, japanese, chain, fonts)?;
        let out = Arc::new(out);
        fonts.insert(id.to_owned(), Arc::clone(&out));
        Ok(out)
    }
    fn providers(
        &mut self,
        c: &Content,
        id: &str,
        uniform: bool,
        japanese: bool,
        chain: &mut Vec<String>,
        fonts: &mut BTreeMap<String, Arc<Self>>,
    ) -> Result<()> {
        if chain.len() >= 16 || chain.iter().any(|x| x == id) {
            return Err("font reference cycle/depth".into());
        }
        chain.push(id.to_owned());
        let resources = c.resource_stack(&resource("font", &format!("{id}.json"))?)?;
        for data in resources.iter().rev() {
            self.reserve(data.len())?;
            if data.len() > 1024 * 1024 {
                return Err("font definition byte bound".into());
            }
            let root: Value = serde_json::from_slice(&data).map_err(|e| e.to_string())?;
            let providers = root["providers"]
                .as_array()
                .ok_or("font providers array missing")?;
            if providers.len() > 256 {
                return Err("font providers bound".into());
            }
            for p in providers {
                let count = self
                    .budget
                    .provider_count
                    .get()
                    .checked_add(1)
                    .ok_or("font aggregate provider overflow")?;
                if count > 1024 {
                    return Err("font aggregate provider bound".into());
                }
                self.budget.provider_count.set(count);
                if p["filter"]["uniform"]
                    .as_bool()
                    .is_some_and(|v| v != uniform)
                    || p["filter"]["jp"].as_bool().is_some_and(|v| v != japanese)
                {
                    continue;
                }
                match p["type"]
                    .as_str()
                    .unwrap_or("")
                    .trim_start_matches("minecraft:")
                {
                    "reference" => {
                        let reference = Self::load_cached(
                            c,
                            p["id"].as_str().ok_or("font reference ID")?,
                            uniform,
                            japanese,
                            chain,
                            fonts,
                            &self.budget,
                        )?;
                        for (&ch, glyph) in &reference.glyphs {
                            if let std::collections::btree_map::Entry::Vacant(entry) =
                                self.glyphs.entry(ch)
                            {
                                self.budget.admit_glyph()?;
                                entry.insert(Arc::clone(glyph));
                            }
                        }
                    }
                    "space" => {
                        for (key, value) in p["advances"].as_object().ok_or("font spacing map")? {
                            let ch = one_char(key)?;
                            let advance = value.as_f64().ok_or("font spacing number")? as f32;
                            if !advance.is_finite() || advance.abs() > 1024. {
                                return Err("font advance bound".into());
                            }
                            if let std::collections::btree_map::Entry::Vacant(entry) =
                                self.glyphs.entry(ch)
                            {
                                self.budget.admit_glyph()?;
                                entry.insert(Arc::new(Glyph {
                                    image: None,
                                    advance,
                                    width: 0.,
                                    height: 0.,
                                    ascent: 0.,
                                    bearing_x: 0.,
                                }));
                            }
                        }
                    }
                    "bitmap" => self.bitmap(c, p)?,
                    "unihex" => self.unihex(c, p)?,
                    "ttf" => self.true_type(c, p)?,
                    kind => return Err(format!("unsupported resource font provider {kind}")),
                }
            }
        }
        chain.pop();
        Ok(())
    }
    fn bitmap(&mut self, c: &Content, p: &Value) -> Result<()> {
        let name = p["file"].as_str().ok_or("bitmap font file")?;
        let bytes = c.resource_bytes(&resource("textures", name)?)?;
        self.reserve(bytes.len())?;
        if bytes.len() > 32 * 1024 * 1024 {
            return Err("bitmap font compressed size bound".into());
        }
        let mut decoder = png::Decoder::new(Cursor::new(bytes));
        decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
        let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
        let size = reader
            .output_buffer_size()
            .ok_or("font image output bound")?;
        if size > 32 * 1024 * 1024 {
            return Err("font image bound".into());
        }
        self.reserve(size)?;
        let mut pixels = vec![0; size];
        let info = reader.next_frame(&mut pixels).map_err(|e| e.to_string())?;
        let rows = p["chars"].as_array().ok_or("bitmap font chars")?;
        if rows.is_empty() || rows.len() > 1024 {
            return Err("bitmap rows bound".into());
        }
        let columns = rows[0].as_str().ok_or("bitmap row string")?.chars().count();
        if columns == 0 || columns > 1024 {
            return Err("bitmap font grid columns".into());
        }
        let cw = info.width / columns as u32;
        let ch = info.height / rows.len() as u32;
        let height = p
            .get("height")
            .map(super::codec_int)
            .transpose()?
            .unwrap_or(8);
        let ascent = super::codec_int(p.get("ascent").ok_or("font ascent")?)?;
        if ascent > height {
            return Err("font ascent exceeds height".into());
        }
        let bpp = info.color_type.samples();
        // Source replaces duplicates inside one provider, but the first provider wins.
        // Visiting cells backwards preserves both rules without a second glyph map.
        for (y, row) in rows.iter().enumerate().rev() {
            let chars = row.as_str().ok_or("bitmap row string")?;
            if chars.chars().count() != columns {
                return Err("bitmap row width differs".into());
            }
            for (reverse_x, char) in chars.chars().rev().enumerate() {
                let x = columns - reverse_x - 1;
                if char == '\0' || self.glyphs.contains_key(&char) {
                    continue;
                }
                let mut ink = 0;
                for px in 0..cw {
                    for py in 0..ch {
                        let off = ((y as u32 * ch + py) * info.width + (x as u32 * cw + px))
                            as usize
                            * bpp;
                        let alpha = match info.color_type {
                            png::ColorType::Rgba => pixels[off + 3],
                            png::ColorType::GrayscaleAlpha => pixels[off + 1],
                            _ => 255, // Source BitmapProvider loads NativeImage.Format.RGBA.
                        };
                        if alpha != 0 {
                            ink = ink.max(px + 1);
                        }
                    }
                }
                let [advance, width, glyph_height] = bitmap_metrics([cw, ch], ink, height);
                self.budget.admit_glyph()?;
                self.glyphs.entry(char).or_insert_with(|| {
                    Arc::new(Glyph {
                        image: Some(GlyphImage::Texture {
                            name: name.trim_end_matches(".png").to_owned(),
                            uv: [
                                (x as u32 * cw) as f32 / info.width as f32,
                                (y as u32 * ch) as f32 / info.height as f32,
                                ((x as u32 + 1) * cw) as f32 / info.width as f32,
                                ((y as u32 + 1) * ch) as f32 / info.height as f32,
                            ],
                        }),
                        advance,
                        width,
                        height: glyph_height,
                        ascent: ascent as f32,
                        bearing_x: 0.,
                    })
                });
            }
        }
        Ok(())
    }
    fn unihex(&mut self, c: &Content, p: &Value) -> Result<()> {
        let name = p["hex_file"].as_str().ok_or("unihex file")?;
        let bytes = c.resource_bytes(&resource("", name)?.replace("//", "/"))?;
        if bytes.len() > 16 * 1024 * 1024 {
            return Err("unihex archive bound".into());
        }
        self.reserve(bytes.len())?;
        let overrides = p
            .get("size_overrides")
            .map(|v| {
                v.as_array()
                    .ok_or("unihex overrides array")?
                    .iter()
                    .map(|o| {
                        let from = one_char(o["from"].as_str().ok_or("unihex override start")?)?;
                        let to = one_char(o["to"].as_str().ok_or("unihex override end")?)?;
                        let left = o["left"].as_u64().ok_or("unihex left")?;
                        let right = o["right"].as_u64().ok_or("unihex right")?;
                        if from > to || left > right || right >= 32 {
                            return Err("unihex override bounds".into());
                        }
                        Ok((from, to, left as usize, right as usize))
                    })
                    .collect::<Result<Vec<_>>>()
            })
            .transpose()?
            .unwrap_or_default();
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| e.to_string())?;
        if archive.len() > 64 {
            return Err("font archive entry bound".into());
        }
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
            if !entry.name().ends_with(".hex") {
                continue;
            }
            if entry.size() > 32 * 1024 * 1024 {
                return Err("font hex inflated size bound".into());
            }
            let inflated = usize::try_from(entry.size()).map_err(|_| "font hex size overflow")?;
            self.reserve(inflated)?;
            let mut text = String::with_capacity(inflated);
            entry
                .by_ref()
                .take(inflated as u64 + 1)
                .read_to_string(&mut text)
                .map_err(|e| e.to_string())?;
            if text.len() != inflated {
                return Err("font hex inflated size bound".into());
            }
            for line in text.lines() {
                let decoded = super::unihex::decode(line)?;
                let Some(char) = decoded.scalar else {
                    continue;
                };
                let rows = decoded.rows;
                let width = decoded.width;
                let mut left = decoded.left;
                let mut right = decoded.right;
                for &(from, to, override_left, override_right) in &overrides {
                    if (from..=to).contains(&char) {
                        left = override_left;
                        right = override_right;
                    }
                }
                if left >= width {
                    left = 0;
                    right = 0;
                }
                if right >= width || left > right {
                    return Err("unihex crop bound".into());
                }
                let glyph_width = (right - left + 1) as f32 / 2.;
                if let std::collections::btree_map::Entry::Vacant(entry) = self.glyphs.entry(char) {
                    self.budget.admit_glyph()?;
                    entry.insert(Arc::new(Glyph {
                        image: Some(GlyphImage::Bits {
                            rows,
                            left: left as u8,
                            right: right as u8,
                            width: width as u8,
                        }),
                        advance: ((right - left + 1) / 2 + 1) as f32,
                        width: glyph_width,
                        height: 8.,
                        ascent: 7.,
                        bearing_x: 0.,
                    }));
                }
            }
        }
        Ok(())
    }
    fn reserve(&mut self, bytes: usize) -> Result<()> {
        let total = self
            .budget
            .resource_bytes
            .get()
            .checked_add(bytes)
            .ok_or("font aggregate bytes overflow")?;
        if total > 128 * 1024 * 1024 {
            return Err("font aggregate resource byte bound".into());
        }
        self.budget.resource_bytes.set(total);
        Ok(())
    }
    fn true_type(&mut self, c: &Content, p: &Value) -> Result<()> {
        let file = p["file"].as_str().ok_or("TrueType font file")?;
        let bytes = c.resource_bytes(&resource("font", file)?)?;
        self.reserve(bytes.len())?;
        let number = |key: &str, default: f32| -> Result<f32> {
            p.get(key)
                .map(|v| {
                    v.as_f64()
                        .map(|v| v as f32)
                        .ok_or("TrueType provider number".into())
                })
                .unwrap_or(Ok(default))
        };
        let shift = if let Some(v) = p.get("shift") {
            let v = v
                .as_array()
                .filter(|v| v.len() == 2)
                .ok_or("TrueType shift pair")?;
            [
                v[0].as_f64().ok_or("TrueType shift x")? as f32,
                v[1].as_f64().ok_or("TrueType shift y")? as f32,
            ]
        } else {
            [0.; 2]
        };
        let mut skip = BTreeSet::new();
        if let Some(v) = p.get("skip") {
            match v {
                Value::String(v) => skip.extend(v.chars()),
                Value::Array(v) => {
                    for value in v {
                        skip.extend(value.as_str().ok_or("TrueType skip string")?.chars());
                    }
                }
                _ => return Err("TrueType skip string/list".into()),
            }
        }
        let provider = super::ttf::TrueTypeProvider::load(
            bytes,
            number("size", 11.)?,
            number("oversample", 1.)?,
            shift,
            Rc::clone(&self.budget.raster),
        )?;
        for (ch, index) in provider.glyphs(&skip) {
            if let std::collections::btree_map::Entry::Vacant(entry) = self.glyphs.entry(ch) {
                self.budget.admit_glyph()?;
                entry.insert(Arc::new(provider.glyph(index)?));
            }
        }
        Ok(())
    }
    pub fn glyph(&self, ch: char) -> Result<&Glyph> {
        Ok(self
            .glyphs
            .get(&ch)
            .map(Arc::as_ref)
            .unwrap_or(&self.budget.missing))
    }
    pub fn visual_text(&self, text: &str) -> String {
        let bidi = unicode_bidi::BidiInfo::new(text, None);
        let mut out = String::with_capacity(text.len());
        for paragraph in &bidi.paragraphs {
            out.push_str(&bidi.reorder_line(paragraph, paragraph.range.clone()));
        }
        out
    }
}
fn one_char(s: &str) -> Result<char> {
    let mut chars = s.chars();
    let ch = chars.next().ok_or("empty font character")?;
    if chars.next().is_some() {
        return Err("font key is not one scalar".into());
    }
    Ok(ch)
}
