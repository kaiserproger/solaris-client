use super::{
    font::{Font, Glyph, GlyphImage},
    text::{Language, TextStyle},
};
use crate::{
    api::{DynamicGeometry, EntityMaterial, MeshGeometry, RenderLayer, Vertex},
    protocol::{
        content::{Content, TextureRegion},
        Result,
    },
};
use glam::{Mat4, Vec3};
use serde_json::Value;
use std::{collections::BTreeMap, sync::Arc};

pub struct Painter<'a> {
    pub content: &'a Content,
    pub out: &'a mut MeshGeometry,
    pub transform: Mat4,
    pub light: [f32; 2],
    pub render_layer: RenderLayer,
}
impl Painter<'_> {
    pub fn solid(&mut self, rect: [f32; 4], color: [f32; 3], alpha: f32) {
        self.quad(rect, [-1.; 4], 0, color, alpha, 0., 0.);
    }
    /// Vanilla widgets retain two-pixel borders instead of stretching their bevels.
    pub(super) fn widget(&mut self, name: &str, [x, y, w, h]: [f32; 4]) -> Result<()> {
        let t = self
            .content
            .texture(name)
            .ok_or_else(|| format!("missing verified GUI texture {name}"))?;
        let bx = 2. / t.width as f32;
        let by = 2. / t.height as f32;
        let xs = [x, x + 2., x + w - 2., x + w];
        let ys = [y, y + 2., y + h - 2., y + h];
        let us = [0., bx, 1. - bx, 1.];
        let vs = [0., by, 1. - by, 1.];
        for row in 0..3 {
            for col in 0..3 {
                self.texture(
                    t,
                    [
                        xs[col],
                        ys[row],
                        xs[col + 1] - xs[col],
                        ys[row + 1] - ys[row],
                    ],
                    [us[col], vs[row], us[col + 1], vs[row + 1]],
                    [1.; 3],
                    1.,
                    0.,
                );
            }
        }
        Ok(())
    }
    pub fn image(
        &mut self,
        name: &str,
        rect: [f32; 4],
        source: [f32; 4],
        color: [f32; 3],
        alpha: f32,
    ) -> Result<()> {
        let t = self
            .content
            .texture(name)
            .ok_or_else(|| format!("missing verified GUI texture {name}"))?;
        self.texture(t, rect, source, color, alpha, 0.);
        Ok(())
    }
    pub fn texture(
        &mut self,
        t: TextureRegion,
        rect: [f32; 4],
        source: [f32; 4],
        color: [f32; 3],
        alpha: f32,
        italic: f32,
    ) {
        let du = t.uv[2] - t.uv[0];
        let dv = t.uv[3] - t.uv[1];
        self.quad(
            rect,
            [
                t.uv[0] + source[0] * du,
                t.uv[1] + source[1] * dv,
                t.uv[0] + source[2] * du,
                t.uv[1] + source[3] * dv,
            ],
            t.layer,
            color,
            alpha,
            italic,
            0.,
        );
    }
    fn quad(
        &mut self,
        [x, y, w, h]: [f32; 4],
        [u0, v0, u1, v1]: [f32; 4],
        layer: u32,
        color: [f32; 3],
        alpha: f32,
        italic: f32,
        depth: f32,
    ) {
        let base = self.out.vertices.len() as u32;
        for (p, uv) in [
            ([x + italic, y, depth], [u0, v0]),
            ([x + w + italic, y, depth], [u1, v0]),
            ([x + w - italic, y + h, depth], [u1, v1]),
            ([x - italic, y + h, depth], [u0, v1]),
        ] {
            let point = self.transform.transform_point3(Vec3::from_array(p));
            self.out.vertices.push(Vertex {
                position: point.to_array(),
                color,
                uv,
                overlay_uv: [-1.; 2],
                overlay_tint: [1.; 3],
                alpha,
                texture_layer: layer,
                light: self.light,
                normal: [0.; 3],
            });
        }
        self.out.indices[self.render_layer as usize].extend([
            base,
            base + 1,
            base + 2,
            base,
            base + 2,
            base + 3,
        ]);
    }
}

pub(super) trait TextSink {
    fn content(&self) -> &Content;
    fn set_transform(&mut self, transform: Mat4);
    fn image(
        &mut self,
        name: &str,
        rect: [f32; 4],
        source: [f32; 4],
        color: [f32; 3],
        alpha: f32,
    ) -> Result<()> {
        let texture = self
            .content()
            .texture(name)
            .ok_or_else(|| format!("missing verified GUI texture {name}"))?;
        self.texture(texture, rect, source, color, alpha, 0., 0.);
        Ok(())
    }
    fn text_quad(
        &mut self,
        rect: [f32; 4],
        uv: [f32; 4],
        layer: u32,
        color: [f32; 3],
        alpha: f32,
        italic: f32,
        colored: bool,
        depth: f32,
    );
    fn glyph_depth(&self, _depth: f32) -> f32 {
        0.
    }
    fn outline_pass(&mut self, _outline: bool) {}
    fn solid(&mut self, rect: [f32; 4], color: [f32; 3], alpha: f32) {
        self.text_quad(rect, [-1.; 4], 0, color, alpha, 0., true, 0.);
    }
    fn mask(&mut self, rect: [f32; 4], color: [f32; 3], alpha: f32, italic: f32, depth: f32) {
        self.text_quad(rect, [-1.; 4], 0, color, alpha, italic, false, depth);
    }
    fn background(&mut self, rect: [f32; 4], color: [f32; 3], alpha: f32) {
        self.text_quad(rect, [-1.; 4], 0, color, alpha, 0., true, -0.01);
    }
    fn texture(
        &mut self,
        texture: TextureRegion,
        rect: [f32; 4],
        source: [f32; 4],
        color: [f32; 3],
        alpha: f32,
        italic: f32,
        depth: f32,
    ) {
        let du = texture.uv[2] - texture.uv[0];
        let dv = texture.uv[3] - texture.uv[1];
        self.text_quad(
            rect,
            [
                texture.uv[0] + source[0] * du,
                texture.uv[1] + source[1] * dv,
                texture.uv[0] + source[2] * du,
                texture.uv[1] + source[3] * dv,
            ],
            texture.layer,
            color,
            alpha,
            italic,
            true,
            depth,
        );
    }
}
impl TextSink for Painter<'_> {
    fn content(&self) -> &Content {
        self.content
    }
    fn set_transform(&mut self, transform: Mat4) {
        self.transform = transform;
    }
    fn text_quad(
        &mut self,
        rect: [f32; 4],
        uv: [f32; 4],
        layer: u32,
        color: [f32; 3],
        alpha: f32,
        italic: f32,
        _colored: bool,
        depth: f32,
    ) {
        self.quad(rect, uv, layer, color, alpha, italic, depth);
    }
}
pub(super) struct WorldPainter<'a> {
    content: &'a Content,
    out: &'a mut DynamicGeometry,
    transform: Mat4,
    light: [f32; 2],
    color_layer: EntityMaterial,
    intensity_layer: EntityMaterial,
}
impl<'a> WorldPainter<'a> {
    pub(super) fn new(
        content: &'a Content,
        out: &'a mut DynamicGeometry,
        transform: Mat4,
        light: [f32; 2],
        material: EntityMaterial,
    ) -> Result<Self> {
        let (color_layer, intensity_layer) = text_materials(material)?;
        Ok(Self {
            content,
            out,
            transform,
            light,
            color_layer,
            intensity_layer,
        })
    }
}
fn text_materials(material: EntityMaterial) -> Result<(EntityMaterial, EntityMaterial)> {
    match material {
        EntityMaterial::Text | EntityMaterial::TextIntensity => {
            Ok((EntityMaterial::Text, EntityMaterial::TextIntensity))
        }
        EntityMaterial::TextSeeThrough | EntityMaterial::TextIntensitySeeThrough => Ok((
            EntityMaterial::TextSeeThrough,
            EntityMaterial::TextIntensitySeeThrough,
        )),
        EntityMaterial::TextPolygonOffset | EntityMaterial::TextIntensityPolygonOffset => Ok((
            EntityMaterial::TextPolygonOffset,
            EntityMaterial::TextIntensityPolygonOffset,
        )),
        _ => Err("world_text_material".into()),
    }
}
impl TextSink for WorldPainter<'_> {
    fn content(&self) -> &Content {
        self.content
    }
    fn set_transform(&mut self, transform: Mat4) {
        self.transform = transform;
    }
    fn glyph_depth(&self, depth: f32) -> f32 {
        depth
    }
    fn outline_pass(&mut self, outline: bool) {
        (self.color_layer, self.intensity_layer) = if outline {
            (EntityMaterial::Text, EntityMaterial::TextIntensity)
        } else {
            (
                EntityMaterial::TextPolygonOffset,
                EntityMaterial::TextIntensityPolygonOffset,
            )
        };
    }
    fn background(&mut self, rect: [f32; 4], color: [f32; 3], alpha: f32) {
        // A backdrop is solid, not a glyph sampled at the atlas UV sentinel.
        // Decorations and polygon-offset foreground text keep their material.
        let foreground = self.color_layer;
        self.color_layer = if foreground == EntityMaterial::TextSeeThrough {
            EntityMaterial::TextBackgroundSeeThrough
        } else {
            EntityMaterial::TextBackground
        };
        self.text_quad(rect, [-1.; 4], 0, color, alpha, 0., true, -0.01);
        self.color_layer = foreground;
    }
    fn text_quad(
        &mut self,
        [x, y, w, h]: [f32; 4],
        [u0, v0, u1, v1]: [f32; 4],
        layer: u32,
        color: [f32; 3],
        alpha: f32,
        italic: f32,
        colored: bool,
        depth: f32,
    ) {
        let material = if colored {
            self.color_layer
        } else {
            self.intensity_layer
        };
        let base = self.out.vertices.len() as u32;
        // BakedSheetGlyph.render uses TL, BL, BR, TR. Negative-Y world-text
        // transforms retain an outward front face with source culling enabled.
        for (position, uv) in [
            ([x + italic, y, depth], [u0, v0]),
            ([x - italic, y + h, depth], [u0, v1]),
            ([x + w - italic, y + h, depth], [u1, v1]),
            ([x + w + italic, y, depth], [u1, v0]),
        ] {
            self.out.vertices.push(Vertex {
                position: self
                    .transform
                    .transform_point3(Vec3::from_array(position))
                    .to_array(),
                color,
                uv,
                overlay_uv: [-1.; 2],
                overlay_tint: [1.; 3],
                alpha,
                texture_layer: layer,
                light: self.light,
                normal: [0.; 3],
            });
        }
        self.out.indices[material as usize].extend([
            base,
            base + 1,
            base + 2,
            base,
            base + 2,
            base + 3,
        ]);
    }
}
/// Loaded once and shared across UI, nameplates, signs, and display entities.
pub struct TextRenderer {
    pub language: Language,
    fonts: BTreeMap<String, NamedFont>,
    missing: Option<NamedFont>,
    obfuscation_tick: std::sync::atomic::AtomicU64,
}
struct NamedFont {
    font: Arc<Font>,
    obfuscation: BTreeMap<u32, Vec<char>>,
}
#[derive(Clone, Copy, Debug)]
pub enum TextAlignment {
    Left,
    Center,
    Right,
}
#[derive(Clone, Debug)]
pub struct WorldTextOptions {
    pub line_height: f32,
    pub alignment: TextAlignment,
    pub shadow: bool,
    pub light: [f32; 2],
    pub layer: EntityMaterial,
    pub background: Option<u32>,
    pub outline: Option<u32>,
}
impl Default for WorldTextOptions {
    fn default() -> Self {
        Self {
            line_height: 9.,
            alignment: TextAlignment::Left,
            shadow: false,
            light: [-1.; 2],
            layer: EntityMaterial::Text,
            background: None,
            outline: None,
        }
    }
}
impl TextRenderer {
    pub(crate) fn load_state(_c: &Content) -> Result<Self> {
        Ok(Self {
            language: Language {
                entries: BTreeMap::new(),
            },
            fonts: BTreeMap::new(),
            missing: None,
            obfuscation_tick: std::sync::atomic::AtomicU64::new(0),
        })
    }
    pub(crate) fn can_render(&self) -> bool {
        self.missing.is_some()
    }
    pub fn load(c: &Content) -> Result<Self> {
        Self::load_language(c, "en_us", false, false)
    }
    pub fn load_language(
        c: &Content,
        language: &str,
        uniform: bool,
        japanese: bool,
    ) -> Result<Self> {
        let mut fonts = BTreeMap::new();
        for (id, font) in Font::load_all(c, uniform, japanese)? {
            let mut obfuscation = BTreeMap::<u32, Vec<char>>::new();
            for (&ch, glyph) in &font.glyphs {
                if glyph.image.is_some() && glyph.advance > 0. && glyph.advance <= 32. {
                    obfuscation
                        .entry(glyph.advance.ceil() as u32)
                        .or_default()
                        .push(ch);
                }
            }
            fonts.insert(id, NamedFont { font, obfuscation });
        }
        let font = Arc::clone(
            &fonts
                .get("minecraft:default")
                .ok_or("verified default font definition missing")?
                .font,
        );
        let missing = NamedFont {
            font: Arc::new(font.all_missing()),
            obfuscation: BTreeMap::new(),
        };
        Ok(Self {
            language: Language::load(c, language)?,
            obfuscation_tick: std::sync::atomic::AtomicU64::new(0),
            fonts,
            missing: Some(missing),
        })
    }
    fn named_font_for(&self, id: &str) -> Result<&NamedFont> {
        // Source FontManager.getFontSetRaw uses its AllMissingGlyphProvider set,
        // not the default font, when a valid requested font has no definition.
        self.fonts
            .get(id)
            .or(self.missing.as_ref())
            .ok_or_else(|| "text rendering unavailable in state-only client".into())
    }
    pub(crate) fn font_for(&self, id: &str) -> Result<&Font> {
        Ok(self.named_font_for(id)?.font.as_ref())
    }
    pub fn width(&self, text: &str, bold: bool) -> Result<f32> {
        let layout = self.layout(&serde_json::json!({"text":text,"bold":bold}), f32::MAX)?;
        Ok(layout.width)
    }
    pub fn set_tick(&self, tick: u64) {
        self.obfuscation_tick
            .store(tick, std::sync::atomic::Ordering::Relaxed);
    }
    pub fn append_world_text(
        &self,
        c: &Content,
        component: &Value,
        transform: Mat4,
        width: f32,
        color: u32,
        background: Option<u32>,
        out: &mut DynamicGeometry,
    ) -> Result<()> {
        let layout = self.layout(component, width)?;
        self.append_world_layout(
            c,
            &layout,
            transform,
            color,
            &WorldTextOptions {
                background,
                ..Default::default()
            },
            out,
        )
    }
    pub(crate) fn append_layout(
        &self,
        c: &Content,
        layout: &super::shaping::TextLayout,
        transform: Mat4,
        color: u32,
        background: Option<u32>,
        out: &mut MeshGeometry,
    ) -> Result<()> {
        self.append_text_layout(
            layout,
            color,
            &WorldTextOptions {
                background,
                ..Default::default()
            },
            &mut Painter {
                content: c,
                out,
                transform,
                light: [-1.; 2],
                render_layer: RenderLayer::Translucent,
            },
        )
    }
    pub(crate) fn append_ui_outlined_layout(
        &self,
        c: &Content,
        layout: &super::shaping::TextLayout,
        transform: Mat4,
        color: u32,
        outline: u32,
        out: &mut MeshGeometry,
    ) -> Result<()> {
        self.append_text_layout_with_outline(
            layout,
            color,
            &WorldTextOptions {
                outline: Some(outline),
                ..Default::default()
            },
            &mut Painter {
                content: c,
                out,
                transform,
                light: [-1.; 2],
                render_layer: RenderLayer::Translucent,
            },
            &[[1., 0.], [-1., 0.], [0., 1.], [0., -1.]],
        )
    }
    pub fn append_world_layout(
        &self,
        c: &Content,
        layout: &super::shaping::TextLayout,
        transform: Mat4,
        color: u32,
        options: &WorldTextOptions,
        out: &mut DynamicGeometry,
    ) -> Result<()> {
        self.append_text_layout(
            layout,
            color,
            options,
            &mut WorldPainter::new(c, out, transform, options.light, options.layer)?,
        )
    }
    pub(super) fn append_text_layout<S: TextSink>(
        &self,
        layout: &super::shaping::TextLayout,
        color: u32,
        options: &WorldTextOptions,
        p: &mut S,
    ) -> Result<()> {
        self.append_text_layout_with_outline(
            layout,
            color,
            options,
            p,
            &[
                [-1., -1.],
                [0., -1.],
                [1., -1.],
                [-1., 0.],
                [1., 0.],
                [-1., 1.],
                [0., 1.],
                [1., 1.],
            ],
        )
    }
    fn append_text_layout_with_outline<S: TextSink>(
        &self,
        layout: &super::shaping::TextLayout,
        color: u32,
        options: &WorldTextOptions,
        p: &mut S,
        outline_offsets: &[[f32; 2]],
    ) -> Result<()> {
        if !options.line_height.is_finite()
            || options.line_height <= 0.
            || options.line_height > 1024.
        {
            return Err("world_text_line_height".into());
        }
        if let Some(argb) = options.background {
            let (rgb, alpha) = argb_color(argb);
            p.background(
                [
                    -1.,
                    -1.,
                    layout.width + 2.,
                    layout.line_widths.len() as f32 * options.line_height,
                ],
                rgb,
                alpha,
            );
        }
        let (base_rgb, alpha) = argb_color(color);
        let tick = self
            .obfuscation_tick
            .load(std::sync::atomic::Ordering::Relaxed);
        for (index, ch) in layout.glyphs.iter().enumerate() {
            let style = &layout.runs[ch.style].style;
            let named = self.named_font_for(style.font.as_ref())?;
            let ordinary = named.font.glyph(ch.ch)?;
            let glyph = if style.obfuscated {
                named
                    .obfuscation
                    .get(&(ordinary.advance.ceil() as u32))
                    .and_then(|bucket| {
                        bucket.get(
                            ((tick.wrapping_mul(6364136223846793005) ^ index as u64)
                                % bucket.len() as u64) as usize,
                        )
                    })
                    .map(|ch| named.font.glyph(*ch))
                    .transpose()?
                    .unwrap_or(ordinary)
            } else {
                ordinary
            };
            let line = (ch.position[1] / 9.) as usize;
            let line_width = *layout
                .line_widths
                .get(line)
                .ok_or("world_text_line_index")?;
            let alignment = match options.alignment {
                TextAlignment::Left => 0.,
                TextAlignment::Center => (layout.width - line_width) / 2.,
                TextAlignment::Right => layout.width - line_width,
            };
            let x = ch.position[0] + alignment;
            let y = line as f32 * options.line_height;
            let rgb = if style.color_override {
                style.color
            } else {
                base_rgb
            };
            let text_color = if style.color_override {
                let channels = rgb.map(|v| (v * 255.).round() as u32);
                (color & 0xff000000) | channels[0] << 16 | channels[1] << 8 | channels[2]
            } else {
                color
            };
            let shadow_color = font_shadow_color(
                text_color,
                style.shadow_color,
                options.shadow && options.outline.is_none(),
            );
            let (shadow_rgb, shadow_alpha) = argb_color(shadow_color);
            let has_shadow = shadow_color != 0;
            let bold_depth = p.glyph_depth(0.001);
            let bold_offset = glyph.bold_offset();
            let shadow_offset = glyph.shadow_offset();
            if let Some(outline) = options.outline {
                let (outline_rgb, outline_alpha) = argb_color(outline);
                p.outline_pass(true);
                for &[dx, dy] in outline_offsets {
                    self.glyph(
                        p,
                        glyph,
                        x + dx,
                        y + dy,
                        style,
                        outline_rgb,
                        alpha * outline_alpha,
                        0.,
                    )?;
                    if style.bold {
                        self.glyph(
                            p,
                            glyph,
                            x + dx + bold_offset,
                            y + dy,
                            style,
                            outline_rgb,
                            alpha * outline_alpha,
                            bold_depth,
                        )?;
                    }
                }
                p.outline_pass(false);
            }
            if has_shadow {
                self.glyph(
                    p,
                    glyph,
                    x + shadow_offset,
                    y + shadow_offset,
                    style,
                    shadow_rgb,
                    shadow_alpha,
                    0.,
                )?;
                if style.bold {
                    self.glyph(
                        p,
                        glyph,
                        x + bold_offset + shadow_offset,
                        y + shadow_offset,
                        style,
                        shadow_rgb,
                        shadow_alpha,
                        bold_depth,
                    )?;
                }
            }
            let depth = p.glyph_depth(if has_shadow { 0.03 } else { 0. });
            self.glyph(p, glyph, x, y, style, rgb, alpha, depth)?;
            if style.bold {
                self.glyph(
                    p,
                    glyph,
                    x + bold_offset,
                    y,
                    style,
                    rgb,
                    alpha,
                    depth + bold_depth,
                )?;
            }
            for (enabled, offset) in [(style.underlined, 8.), (style.strikethrough, 3.5)] {
                if !enabled {
                    continue;
                }
                let start = x - if index == 0 { 1. } else { 0. };
                let width = x + ch.advance - start;
                if has_shadow {
                    p.text_quad(
                        [start + shadow_offset, y + offset + shadow_offset, width, 1.],
                        [-1.; 4],
                        0,
                        shadow_rgb,
                        shadow_alpha,
                        0.,
                        true,
                        0.01,
                    );
                }
                p.text_quad(
                    [start, y + offset, width, 1.],
                    [-1.; 4],
                    0,
                    rgb,
                    alpha,
                    0.,
                    true,
                    0.01 + if has_shadow { 0.03 } else { 0. },
                );
            }
        }
        Ok(())
    }
    fn glyph<S: TextSink>(
        &self,
        p: &mut S,
        g: &Glyph,
        x: f32,
        y: f32,
        s: &TextStyle,
        color: [f32; 3],
        alpha: f32,
        depth: f32,
    ) -> Result<()> {
        let (rect, italic) = glyph_region(g, x, y, [0., 0., 1., 1.], s.italic, s.bold);
        match &g.image {
            None => {}
            Some(GlyphImage::Missing) => {
                for [rx, ry, rw, rh] in [
                    [0., 0., 5., 1.],
                    [0., 7., 5., 1.],
                    [0., 1., 1., 6.],
                    [4., 1., 1., 6.],
                ] {
                    let (rect, italic) = glyph_region(
                        g,
                        x,
                        y,
                        [rx / 5., ry / 8., rw / 5., rh / 8.],
                        s.italic,
                        s.bold,
                    );
                    p.text_quad(rect, [-1.; 4], 0, color, alpha, italic, true, depth);
                }
            }
            Some(GlyphImage::Texture { name, uv }) => {
                let t = p
                    .content()
                    .texture(name)
                    .ok_or_else(|| format!("missing verified font texture {name}"))?;
                p.texture(t, rect, *uv, color, alpha, italic, depth);
            }
            Some(GlyphImage::Bits {
                rows,
                left,
                right,
                width,
            }) => {
                for (row, bits) in rows.iter().enumerate() {
                    let mut col = *left;
                    while col <= *right {
                        if bits & (1u32 << (*width - 1 - col)) == 0 {
                            col += 1;
                            continue;
                        }
                        let start = col;
                        col += 1;
                        while col <= *right && bits & (1u32 << (*width - 1 - col)) != 0 {
                            col += 1;
                        }
                        let (rect, italic) = glyph_region(
                            g,
                            x,
                            y,
                            [
                                (start - *left) as f32 / f32::from(*right - *left + 1),
                                row as f32 / 16.,
                                (col - start) as f32 / f32::from(*right - *left + 1),
                                1. / 16.,
                            ],
                            s.italic,
                            s.bold,
                        );
                        p.mask(rect, color, alpha, italic, depth);
                    }
                }
            }
            Some(GlyphImage::Raster(image)) => {
                let raster = image.raster()?;
                let width = f32::from(raster.width);
                let height = f32::from(raster.height);
                for run in &raster.runs {
                    let [rx, ry, rw, rh] = run.bounds.map(f32::from);
                    let (rect, italic) = glyph_region(
                        g,
                        x,
                        y,
                        [rx / width, ry / height, rw / width, rh / height],
                        s.italic,
                        s.bold,
                    );
                    p.mask(
                        rect,
                        color,
                        alpha * f32::from(run.coverage) / 255.,
                        italic,
                        depth,
                    );
                }
            }
        }
        Ok(())
    }
}
fn glyph_region(
    g: &Glyph,
    x: f32,
    y: f32,
    [u, v, du, dv]: [f32; 4],
    italic: bool,
    bold: bool,
) -> ([f32; 4], f32) {
    let thickness = if bold { 0.1 } else { 0. };
    let width = g.width + thickness * 2.;
    let height = g.height + thickness * 2.;
    let up = 7. - g.ascent;
    let shear = if italic {
        1. - (up + (v + dv * 0.5) * g.height) * 0.25
    } else {
        0.
    };
    (
        [
            x + g.bearing_x + u * width - thickness + shear,
            y + up + v * height - thickness,
            du * width,
            dv * height,
        ],
        if italic { dv * g.height * 0.125 } else { 0. },
    )
}
fn font_shadow_color(color: u32, explicit: Option<u32>, draw_shadow: bool) -> u32 {
    if let Some(shadow) = explicit {
        if color >> 24 == 255 {
            shadow
        } else {
            let alpha = (color >> 24) as f32 / 255.;
            let shadow_alpha = (shadow >> 24) as f32 / 255.;
            let product = (alpha * shadow_alpha * 255.).floor() as u32;
            product << 24 | (shadow & 0x00ffffff)
        }
    } else if draw_shadow {
        (color & 0xff000000)
            | (((color >> 16) & 255) / 4) << 16
            | (((color >> 8) & 255) / 4) << 8
            | (color & 255) / 4
    } else {
        0
    }
}
pub fn argb_color(argb: u32) -> ([f32; 3], f32) {
    (
        [
            ((argb >> 16) & 255) as f32 / 255.,
            ((argb >> 8) & 255) as f32 / 255.,
            (argb & 255) as f32 / 255.,
        ],
        (argb >> 24) as f32 / 255.,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn world_backdrops_preserve_foreground_and_polygon_offset_materials() {
        let content = Content::test_cube();
        for material in [EntityMaterial::Text, EntityMaterial::TextPolygonOffset, EntityMaterial::TextSeeThrough] {
            let mut geometry = DynamicGeometry::default();
            let mut painter = WorldPainter::new(&content, &mut geometry, Mat4::IDENTITY, [15., 0.], material).unwrap();
            painter.background([0., 0., 10., 10.], [0.; 3], 0.25);
            // Underlines/strikethroughs retain the foreground's depth/bias.
            painter.solid([0., 8., 10., 1.], [1.; 3], 1.);
            let background = if material == EntityMaterial::TextSeeThrough {
                EntityMaterial::TextBackgroundSeeThrough
            } else {
                EntityMaterial::TextBackground
            };
            assert_eq!(geometry.indices[background as usize], [0, 1, 2, 0, 2, 3]);
            assert_eq!(geometry.indices[material as usize], [4, 5, 6, 4, 6, 7]);
        }
    }
}
