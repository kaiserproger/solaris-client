//! One whole-line bidi layout for paint, wrapping, text hit testing and native IME caret.
use super::{render::TextRenderer, text::TextRun};
use crate::protocol::Result;
use std::sync::LazyLock;
#[derive(Clone, Debug)]
pub struct PositionedGlyph {
    pub ch: char,
    pub style: usize,
    pub position: [f32; 2],
    pub advance: f32,
    pub bytes: [usize; 2],
    pub rtl: bool,
}
#[derive(Debug)]
pub struct TextLayout {
    pub runs: Vec<TextRun>,
    pub glyphs: Vec<PositionedGlyph>,
    pub width: f32,
    pub height: f32,
    pub bytes: usize,
    pub end_caret: [f32; 2],
    pub line_widths: Vec<f32>,
}
struct Character {
    ch: char,
    style: usize,
    advance: f32,
    bytes: [usize; 2],
}
impl TextLayout {
    pub fn caret(&self, byte: usize) -> [f32; 2] {
        if byte >= self.bytes {
            return self.end_caret;
        }
        for g in &self.glyphs {
            if byte == g.bytes[0] {
                return [
                    g.position[0] + if g.rtl { g.advance } else { 0. },
                    g.position[1],
                ];
            }
        }
        let mut closest = (usize::MAX, self.end_caret);
        for g in &self.glyphs {
            for (boundary, x) in [
                (
                    g.bytes[0],
                    g.position[0] + if g.rtl { g.advance } else { 0. },
                ),
                (
                    g.bytes[1],
                    g.position[0] + if g.rtl { 0. } else { g.advance },
                ),
            ] {
                let distance = byte.abs_diff(boundary);
                if distance < closest.0 {
                    closest = (distance, [x, g.position[1]]);
                }
            }
        }
        closest.1
    }
    pub fn hit(&self, p: [f32; 2]) -> usize {
        let line = (p[1] / 9.).floor().max(0.);
        let mut best = (f32::INFINITY, self.bytes);
        for g in &self.glyphs {
            if (g.position[1] / 9. - line).abs() > 0.5 {
                continue;
            }
            for (x, b) in [
                (g.position[0], if g.rtl { g.bytes[1] } else { g.bytes[0] }),
                (
                    g.position[0] + g.advance,
                    if g.rtl { g.bytes[0] } else { g.bytes[1] },
                ),
            ] {
                let distance = (x - p[0]).abs();
                if distance < best.0 {
                    best = (distance, b);
                }
            }
        }
        best.1
    }
}
impl TextRenderer {
    pub fn layout(&self, component: &serde_json::Value, width: f32) -> Result<TextLayout> {
        if !width.is_finite() || width <= 0. {
            return Err("text_layout_width".into());
        }
        let runs = self.language.runs(component)?;
        let mut raw = String::new();
        let mut identities = Vec::new();
        for (style, run) in runs.iter().enumerate() {
            for ch in run.text.chars() {
                if identities.len() >= 16384 {
                    return Err("text_layout_scalar_limit".into());
                }
                let start = raw.len();
                raw.push(ch);
                if ch != '\u{200d}' {
                    identities.push((style, [start, raw.len()]));
                }
            }
        }
        static SHAPER: LazyLock<arabic_reshaper::ArabicReshaper<'static>> = LazyLock::new(|| {
            let mut r = arabic_reshaper::ArabicReshaper::new();
            r.configuration.insert("delete_harakat", false);
            r.configuration.insert("shift_harakat_position", false);
            r.configuration.insert("delete_tatweel", false);
            r.configuration.insert("support_ligatures", false);
            r
        });
        let shaped = SHAPER.reshape(&raw);
        if shaped.chars().count() != identities.len() {
            return Err("text_shaping_identity_mismatch".into());
        }
        let mut chars = Vec::with_capacity(identities.len());
        for (ch, (style, bytes)) in shaped.chars().zip(identities) {
            let advance = if ch == '\n' || is_format(ch) {
                0.
            } else {
                self.font_for(runs[style].style.font.as_ref())?
                    .glyph(ch)?
                    .styled_advance(runs[style].style.bold)
            };
            chars.push(Character {
                ch,
                style,
                advance,
                bytes,
            });
        }
        let mut lines = Vec::new();
        let mut start = 0;
        let mut pos = 0;
        let mut line_width = 0.;
        let mut space = None;
        while pos < chars.len() {
            if chars[pos].ch == '\n' {
                lines.push((start, pos, line_width));
                pos += 1;
                start = pos;
                line_width = 0.;
                space = None;
                continue;
            }
            if line_width + chars[pos].advance > width && pos > start {
                if let Some((split, before)) = space {
                    lines.push((start, split, before));
                    start = split + 1;
                    pos = start;
                } else {
                    lines.push((start, pos, line_width));
                    start = pos;
                }
                line_width = 0.;
                space = None;
                continue;
            }
            if chars[pos].ch == ' ' {
                space = Some((pos, line_width));
            }
            line_width += chars[pos].advance;
            pos += 1;
        }
        lines.push((start, pos, line_width));
        if lines.len() > 4096 {
            return Err("text_layout_line_limit".into());
        }
        let actual_width = lines.iter().map(|(_, _, w)| *w).fold(0., f32::max);
        let height = lines.len() as f32 * 9.;
        let line_widths = lines.iter().map(|(_, _, width)| *width).collect();
        let mut glyphs = Vec::with_capacity(chars.len());
        for (line, (start, end, _)) in lines.into_iter().enumerate() {
            let mut text = String::new();
            let mut offsets = Vec::with_capacity(end - start);
            for i in start..end {
                offsets.push((text.len(), i));
                text.push(chars[i].ch);
            }
            let bidi = unicode_bidi::BidiInfo::new(&text, None);
            let mut x = 0.;
            for paragraph in &bidi.paragraphs {
                let (levels, ranges) = bidi.visual_runs(paragraph, paragraph.range.clone());
                for range in ranges {
                    let lo = offsets.partition_point(|(b, _)| *b < range.start);
                    let hi = offsets.partition_point(|(b, _)| *b < range.end);
                    let rtl = levels[range.start].is_rtl();
                    for n in 0..hi - lo {
                        let i = offsets[if rtl { hi - 1 - n } else { lo + n }].1;
                        let c = &chars[i];
                        if !is_format(c.ch) {
                            glyphs.push(PositionedGlyph {
                                ch: c.ch,
                                style: c.style,
                                position: [x, line as f32 * 9.],
                                advance: c.advance,
                                bytes: c.bytes,
                                rtl,
                            });
                        }
                        x += c.advance;
                    }
                }
            }
        }
        let end_caret = if raw.ends_with('\n') {
            [0., height - 9.]
        } else {
            glyphs
                .iter()
                .max_by_key(|glyph| glyph.bytes[1])
                .map_or([0., height - 9.], |g| {
                    [
                        g.position[0] + if g.rtl { 0. } else { g.advance },
                        g.position[1],
                    ]
                })
        };
        Ok(TextLayout {
            runs,
            glyphs,
            width: actual_width,
            height,
            bytes: raw.len(),
            end_caret,
            line_widths,
        })
    }
}
fn is_format(c: char) -> bool {
    matches!(c,'\u{200c}'|'\u{200e}'|'\u{200f}'|'\u{202a}'..='\u{202e}'|'\u{2066}'..='\u{2069}')
}
