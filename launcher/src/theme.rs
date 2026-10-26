//! Semantic launcher theme tokens and bounded custom-theme interchange.

use eframe::egui::{self, Color32, CornerRadius, Stroke};
use serde::{Deserialize, Serialize};

/// Catalogue grouping, independent of whether a colorful palette is light.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeCategory {
    Dark,
    Light,
    Colorful,
}

/// Opaque sRGB semantic colors. Use these tokens for custom-painted controls,
/// rather than retaining colors from the previously selected theme.
#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    pub id: String,
    pub name: String,
    pub category: ThemeCategory,
    pub background: Color32,
    pub surface: Color32,
    pub elevated: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub border: Color32,
    pub accent: Color32,
    pub on_accent: Color32,
    pub hover: Color32,
    pub selected: Color32,
    pub focus: Color32,
    /// Disabled foreground over `surface`; egui represents disabled widgets
    /// with opacity, not a separately configurable widget color state.
    pub disabled: Color32,
    pub input: Color32,
    pub success: Color32,
    pub error: Color32,
    pub warning: Color32,
}

pub const MAX_CUSTOM_JSON_BYTES: usize = 4096;
pub const MAX_CUSTOM_ID_BYTES: usize = 64;
pub const MAX_CUSTOM_NAME_CHARS: usize = 64;
const TEXT_CONTRAST: f32 = 4.5;

/// Portable six-field theme format. Missing/unknown fields, duplicate JSON
/// fields, non-hex colors and oversized documents are rejected on import.
/// `Default` is an empty editor draft, deliberately not an accepted theme.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CustomTheme {
    pub id: String,
    pub name: String,
    pub background: String,
    pub surface: String,
    pub text: String,
    pub accent: String,
}

/// The explicitly curated base colors shared by preset construction and
/// validated user themes. Derived colors never modify these authored colors.
pub(crate) struct Palette {
    pub background: Color32,
    pub surface: Color32,
    pub elevated: Color32,
    pub input: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub accent: Color32,
}

impl Theme {
    pub(crate) fn from_palette(
        id: &str,
        name: &str,
        category: ThemeCategory,
        palette: Palette,
    ) -> Self {
        let p = palette;
        let backgrounds = [p.background, p.surface, p.elevated, p.input];
        let hover = tinted_surface(p.surface, p.accent, 0.10, p.text, p.muted);
        let selected = tinted_surface(p.surface, p.accent, 0.18, p.text, p.muted);
        let focus = readable_towards(
            p.accent,
            p.text,
            &[p.background, p.surface, p.elevated, p.input, selected],
            TEXT_CONTRAST,
        );
        let dark = luminance(p.background) < luminance(p.text);
        let (success, error, warning) = if dark {
            (rgb(0x8CCFAD), rgb(0xF29BA2), rgb(0xE9C17A))
        } else {
            (rgb(0x276443), rgb(0xA63246), rgb(0x805817))
        };
        Self {
            id: id.to_owned(),
            name: name.to_owned(),
            category,
            background: p.background,
            surface: p.surface,
            elevated: p.elevated,
            text: p.text,
            muted: p.muted,
            border: readable_towards(mix(p.surface, p.text, 0.28), p.text, &backgrounds, 3.0),
            accent: p.accent,
            on_accent: if contrast(Color32::BLACK, p.accent) >= contrast(Color32::WHITE, p.accent) {
                Color32::BLACK
            } else {
                Color32::WHITE
            },
            hover,
            selected,
            focus,
            disabled: mix(p.surface, p.text, 0.48),
            input: p.input,
            success: readable_towards(success, p.text, &backgrounds, TEXT_CONTRAST),
            error: readable_towards(error, p.text, &backgrounds, TEXT_CONTRAST),
            warning: readable_towards(warning, p.text, &backgrounds, TEXT_CONTRAST),
        }
    }
}

impl CustomTheme {
    /// Validate identity, bounded display name and opaque #RRGGBB colors, then
    /// derive readable semantic roles. Supplied text is never silently changed.
    pub fn validate(&self) -> Result<Theme, String> {
        validate_id(&self.id)?;
        if crate::theme_presets::contains_id(&self.id) {
            return Err("Custom theme ID collides with a built-in preset".into());
        }
        if self.name.is_empty()
            || self.name.len() > MAX_CUSTOM_NAME_CHARS * 4
            || self.name.chars().count() > MAX_CUSTOM_NAME_CHARS
            || self.name.trim() != self.name
            || self.name.chars().any(unsafe_name_character)
        {
            return Err(format!(
                "Theme name must contain 1–{MAX_CUSTOM_NAME_CHARS} visible characters, without surrounding whitespace or control characters"
            ));
        }
        let background = parse_hex("background", &self.background)?;
        let surface = parse_hex("surface", &self.surface)?;
        let text = parse_hex("text", &self.text)?;
        let accent = parse_hex("accent", &self.accent)?;
        for (role, color) in [("background", background), ("surface", surface)] {
            if contrast(text, color) < TEXT_CONTRAST {
                return Err(format!(
                    "Text contrast against {role} must be at least 4.5:1 (got {:.2}:1)",
                    contrast(text, color)
                ));
            }
        }
        // Stay close to the authored surface while allowing a little accent
        // character. If tint would harm text contrast, reduce it or use surface.
        let elevated = tinted_surface(surface, accent, 0.05, text, text);
        let input = tinted_surface(background, surface, 0.45, text, text);
        let backgrounds = [background, surface, elevated, input];
        let muted = readable_towards(mix(text, surface, 0.30), text, &backgrounds, TEXT_CONTRAST);
        let theme = Theme::from_palette(
            &self.id,
            &self.name,
            ThemeCategory::Colorful,
            Palette {
                background,
                surface,
                elevated,
                input,
                text,
                muted,
                accent,
            },
        );
        // Keep this check at the trust boundary, not just in catalogue tests.
        for fg in [theme.text, theme.muted] {
            if backgrounds
                .iter()
                .any(|&bg| contrast(fg, bg) < TEXT_CONTRAST)
            {
                return Err("Derived theme text contrast must be at least 4.5:1".into());
            }
        }
        if contrast(theme.accent, theme.on_accent) < TEXT_CONTRAST {
            return Err("Accent text contrast must be at least 4.5:1".into());
        }
        Ok(theme)
    }
}

/// Import one bounded theme, not an array or an arbitrary settings document.
pub fn import_json(json: &str) -> Result<CustomTheme, String> {
    if json.len() > MAX_CUSTOM_JSON_BYTES {
        return Err(format!("Theme JSON exceeds {MAX_CUSTOM_JSON_BYTES} bytes"));
    }
    let custom: CustomTheme =
        serde_json::from_str(json).map_err(|error| format!("Invalid theme JSON: {error}"))?;
    custom.validate()?;
    Ok(custom)
}

/// Export only a valid theme; preserve the authored strings exactly.
pub fn export_json(custom: &CustomTheme) -> Result<String, String> {
    custom.validate()?;
    let json = serde_json::to_string_pretty(custom)
        .map_err(|error| format!("Could not serialize theme: {error}"))?;
    if json.len() > MAX_CUSTOM_JSON_BYTES {
        return Err(format!("Theme JSON exceeds {MAX_CUSTOM_JSON_BYTES} bytes"));
    }
    Ok(json)
}

/// Exact stable ID lookup. Invalid persisted custom themes cannot shadow
/// presets or break startup. Unknown IDs fall back to Graphite, never a draft.
pub fn resolve(id: &str, custom: &[CustomTheme]) -> Theme {
    let presets = crate::theme_presets::all();
    let fallback = presets[0].clone(); // Catalogue order is tested: Graphite first.
    presets
        .into_iter()
        .find(|theme| theme.id == id)
        .or_else(|| {
            custom
                .iter()
                .filter(|theme| theme.id == id)
                .find_map(|theme| theme.validate().ok())
        })
        .unwrap_or(fallback)
}

/// WCAG sRGB contrast ratio. Alpha is ignored after unpremultiplication:
/// composite translucent paint over its actual background before calling.
pub fn contrast(a: Color32, b: Color32) -> f32 {
    let a = luminance(a);
    let b = luminance(b);
    ((a.max(b) + 0.05) / (a.min(b) + 0.05)) as f32
}

/// Apply before constructing this frame's panels/windows. All egui visual
/// color slots (including both OS-style variants) are replaced on every call.
/// Explicit widget `.fill()`/`RichText::color()` overrides still belong to the
/// caller and must use `Theme` tokens. Native title bars are OS-owned.
///
/// Egui 0.33 has no global modal-backdrop color or disabled-color slot: set
/// `Modal::backdrop_color` from `theme.background` at the call site; disabled
/// widgets use theme-derived opacity, and custom disabled labels use `disabled`.
pub fn apply(ctx: &egui::Context, theme: &Theme, reduced_motion: bool) {
    let dark = luminance(theme.background) < luminance(theme.text);
    let mut visuals = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    visuals.override_text_color = None;
    visuals.weak_text_color = Some(theme.muted);
    visuals.weak_text_alpha = 1.0;
    visuals.panel_fill = theme.background;
    visuals.window_fill = theme.surface;
    visuals.window_stroke = Stroke::new(1.0, theme.border);
    visuals.extreme_bg_color = theme.input;
    visuals.text_edit_bg_color = Some(theme.input);
    visuals.faint_bg_color = theme.elevated;
    visuals.code_bg_color = theme.elevated;
    visuals.hyperlink_color = readable_towards(
        theme.accent,
        theme.text,
        &[theme.background, theme.surface, theme.elevated, theme.input],
        TEXT_CONTRAST,
    );
    visuals.warn_fg_color = theme.warning;
    visuals.error_fg_color = theme.error;
    visuals.selection.bg_fill = theme.selected;
    // In egui this stroke is BOTH selected-text foreground and the focused
    // TextEdit outline. `focus` is derived to be readable on selected/input.
    visuals.selection.stroke = Stroke::new(1.5, theme.focus);
    visuals.text_cursor.stroke = Stroke::new(2.0, theme.focus);
    visuals.text_cursor.preview = false;
    visuals.text_cursor.blink = !reduced_motion;
    visuals.window_corner_radius = CornerRadius::same(8);
    visuals.menu_corner_radius = CornerRadius::same(8);
    visuals.window_highlight_topmost = false;
    // Tint shadows from the palette, not egui's default hard-coded black.
    let shadow_color = theme.text;
    visuals.window_shadow = egui::epaint::Shadow {
        offset: [0, 4],
        blur: 14,
        spread: 0,
        color: Color32::from_rgba_unmultiplied(
            shadow_color.r(),
            shadow_color.g(),
            shadow_color.b(),
            if dark { 12 } else { 24 },
        ),
    };
    visuals.popup_shadow = visuals.window_shadow;
    visuals.disabled_alpha = disabled_opacity(theme);
    visuals.image_loading_spinners = !reduced_motion;
    visuals.slider_trailing_fill = true;
    visuals.button_frame = true;
    visuals.collapsing_header_frame = true;
    visuals.widgets.noninteractive = widget(theme.surface, theme.border, theme.text, 1.0);
    visuals.widgets.inactive = widget(theme.elevated, theme.border, theme.text, 1.0);
    visuals.widgets.hovered = widget(theme.hover, theme.focus, theme.text, 1.0);
    visuals.widgets.active = widget(theme.selected, theme.focus, theme.text, 1.5);
    visuals.widgets.open = widget(theme.selected, theme.focus, theme.text, 1.0);

    ctx.set_theme(if dark {
        egui::Theme::Dark
    } else {
        egui::Theme::Light
    });
    ctx.all_styles_mut(|style| {
        style.visuals = visuals.clone();
        style.animation_time = if reduced_motion { 0.0 } else { 0.12 };
        style.scroll_animation = if reduced_motion {
            egui::style::ScrollAnimation::none()
        } else {
            egui::style::ScrollAnimation::default()
        };
        // Scrollbars read widget foregrounds for handles and extreme_bg_color
        // for tracks. Nonfloating bars avoid a second opacity/color system.
        style.spacing.scroll = egui::style::ScrollStyle::solid();
        style.spacing.scroll.foreground_color = true;
        style.spacing.scroll.bar_width = 8.0;
        style.spacing.scroll.handle_min_length = 24.0;
        style.spacing.scroll.bar_inner_margin = 4.0;
        style.spacing.scroll.bar_outer_margin = 0.0;
    });
}

fn widget(fill: Color32, border: Color32, text: Color32, width: f32) -> egui::style::WidgetVisuals {
    egui::style::WidgetVisuals {
        bg_fill: fill,
        weak_bg_fill: fill,
        bg_stroke: Stroke::new(width, border),
        corner_radius: CornerRadius::same(8),
        fg_stroke: Stroke::new(1.0, text),
        expansion: 0.0,
    }
}

pub(crate) fn rgb(value: u32) -> Color32 {
    Color32::from_rgb((value >> 16) as u8, (value >> 8) as u8, value as u8)
}

fn parse_hex(role: &str, value: &str) -> Result<Color32, String> {
    if value.len() != 7
        || !value.starts_with('#')
        || !value.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit)
    {
        return Err(format!("{role} must be an opaque #RRGGBB color"));
    }
    u32::from_str_radix(&value[1..], 16)
        .map(rgb)
        .map_err(|_| format!("{role} must be an opaque #RRGGBB color"))
}

fn validate_id(id: &str) -> Result<(), String> {
    if id.is_empty()
        || id.len() > MAX_CUSTOM_ID_BYTES
        || id.starts_with('-')
        || id.ends_with('-')
        || id.contains("--")
        || !id
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
    {
        return Err(format!(
            "Theme ID must be 1–{MAX_CUSTOM_ID_BYTES} lowercase ASCII letters/digits in kebab-case"
        ));
    }
    Ok(())
}

fn unsafe_name_character(c: char) -> bool {
    c.is_control()
        || matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{206F}' | '\u{FEFF}')
}

fn luminance(color: Color32) -> f64 {
    let [r, g, b, _] = color.to_srgba_unmultiplied();
    let linear = |byte: u8| {
        let value = f64::from(byte) / 255.0;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
}

fn mix(a: Color32, b: Color32, amount: f32) -> Color32 {
    let channel =
        |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * amount).round() as u8;
    Color32::from_rgb(
        channel(a.r(), b.r()),
        channel(a.g(), b.g()),
        channel(a.b(), b.b()),
    )
}

fn readable_towards(
    preferred: Color32,
    text: Color32,
    backgrounds: &[Color32],
    minimum: f32,
) -> Color32 {
    for step in 0..=32 {
        let candidate = mix(preferred, text, step as f32 / 32.0);
        if backgrounds
            .iter()
            .all(|&bg| contrast(candidate, bg) >= minimum)
        {
            return candidate;
        }
    }
    text // The validated base text is the guaranteed safe endpoint.
}

fn tinted_surface(
    surface: Color32,
    tint: Color32,
    amount: f32,
    text: Color32,
    muted: Color32,
) -> Color32 {
    for step in (0..=16).rev() {
        let candidate = mix(surface, tint, amount * step as f32 / 16.0);
        if [text, muted]
            .iter()
            .all(|&fg| contrast(fg, candidate) >= TEXT_CONTRAST)
        {
            return candidate;
        }
    }
    surface
}

fn disabled_opacity(theme: &Theme) -> f32 {
    let a = [theme.surface.r(), theme.surface.g(), theme.surface.b()];
    let b = [theme.text.r(), theme.text.g(), theme.text.b()];
    let d = [theme.disabled.r(), theme.disabled.g(), theme.disabled.b()];
    let (mut numerator, mut denominator) = (0.0_f32, 0.0_f32);
    for ((a, b), d) in a.into_iter().zip(b).zip(d) {
        let direction = f32::from(b) - f32::from(a);
        numerator += (f32::from(d) - f32::from(a)) * direction;
        denominator += direction * direction;
    }
    if denominator == 0.0 {
        0.48
    } else {
        (numerator / denominator).clamp(0.30, 0.70)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn custom() -> CustomTheme {
        CustomTheme {
            id: "personal-night".into(),
            name: "Личная ночь".into(),
            background: "#14161a".into(),
            surface: "#1C1f25".into(),
            text: "#F1F3F5".into(),
            accent: "#8EA8DB".into(),
        }
    }

    fn assert_readable(theme: &Theme) {
        for background in [
            theme.background,
            theme.surface,
            theme.elevated,
            theme.input,
            theme.hover,
            theme.selected,
        ] {
            assert!(
                contrast(theme.text, background) >= 4.5,
                "text on {background:?}"
            );
            assert!(
                contrast(theme.muted, background) >= 4.5,
                "muted on {background:?}"
            );
        }
        assert!(contrast(theme.accent, theme.on_accent) >= 4.5);
        assert!(contrast(theme.focus, theme.selected) >= 4.5);
    }

    // Test-first new API contract. Before implementation this references the
    // deliberately missing `resolve` function; the parent runs Cargo after
    // adding the module declarations. This is not a behavioral failure receipt.
    #[test]
    fn unknown_theme_falls_back_to_graphite() {
        let theme = super::resolve("not-a-preset", &[]);
        assert_eq!(theme.id, "graphite");
        assert_eq!(theme.name, "Graphite");
    }

    #[test]
    fn contrast_uses_linear_srgb_and_is_symmetric() {
        assert!((contrast(Color32::BLACK, Color32::WHITE) - 21.0).abs() < 0.0001);
        assert_eq!(contrast(rgb(0x6589AB), rgb(0x6589AB)), 1.0);
        // Known sRGB gray boundary: #767676 passes, #777777 fails on white.
        assert!(contrast(rgb(0x767676), Color32::WHITE) >= 4.5);
        assert!(contrast(rgb(0x777777), Color32::WHITE) < 4.5);
        assert_eq!(
            contrast(rgb(0x142635), rgb(0xFEF1E2)),
            contrast(rgb(0xFEF1E2), rgb(0x142635))
        );
    }

    #[test]
    fn custom_dark_and_light_themes_derive_readable_semantic_tokens() {
        let night = custom();
        let theme = night.validate().unwrap();
        assert_eq!(theme.id, night.id);
        assert_eq!(theme.name, night.name);
        assert_eq!(theme.background, rgb(0x14161A));
        assert_eq!(theme.surface, rgb(0x1C1F25));
        assert_eq!(theme.text, rgb(0xF1F3F5));
        assert_eq!(theme.accent, rgb(0x8EA8DB));
        assert_readable(&theme);

        let light = CustomTheme {
            id: "personal-day".into(),
            name: "Personal day".into(),
            background: "#F4F5F7".into(),
            surface: "#FFFFFF".into(),
            text: "#202734".into(),
            accent: "#365F99".into(),
        };
        assert_readable(&light.validate().unwrap());
        // An accent need not be readable as text: derive an accessible focus
        // and an on-accent pair without altering the supplied brand color.
        for accent in ["#FFFFFF", "#000000", "#777777", "#FF0000"] {
            let mut theme = night.clone();
            theme.accent = accent.into();
            assert_readable(&theme.validate().unwrap());
        }
    }

    #[test]
    fn custom_rejects_insufficient_text_contrast_on_either_authored_surface() {
        let mut bad = custom();
        bad.text = "#777777".into();
        bad.background = "#FFFFFF".into();
        assert!(bad.validate().unwrap_err().contains("background"));
        bad = custom();
        bad.surface = "#EEEEEE".into();
        assert!(bad.validate().unwrap_err().contains("surface"));
    }

    #[test]
    fn custom_id_is_bounded_kebab_case_and_cannot_collide_with_presets() {
        for id in [
            "",
            "Graphite",
            "bad_id",
            "with space",
            "-prefix",
            "suffix-",
            "two--hyphens",
            "../path",
            "тема",
            "abc\n",
        ] {
            let mut theme = custom();
            theme.id = id.into();
            assert!(theme.validate().is_err(), "accepted {id:?}");
        }
        let mut theme = custom();
        theme.id = "x".repeat(MAX_CUSTOM_ID_BYTES);
        assert!(theme.validate().is_ok());
        theme.id.push('x');
        assert!(theme.validate().is_err());
        for preset in crate::theme_presets::all() {
            theme.id = preset.id;
            assert!(theme.validate().unwrap_err().contains("collides"));
        }
    }

    #[test]
    fn custom_name_is_bounded_and_rejects_control_or_bidi_spoofing() {
        for name in [
            "",
            " ",
            " Name",
            "Name ",
            "line\nbreak",
            "nul\0name",
            "tab\tname",
            "invisible\u{200B}",
            "bidi\u{202E}name",
        ] {
            let mut theme = custom();
            theme.name = name.into();
            assert!(theme.validate().is_err(), "accepted {name:?}");
        }
        let mut theme = custom();
        theme.name = "я".repeat(MAX_CUSTOM_NAME_CHARS);
        assert!(theme.validate().is_ok());
        theme.name.push('я');
        assert!(theme.validate().is_err());
    }

    #[test]
    fn custom_colors_require_exact_opaque_rgb_hex_in_every_field() {
        for bad in [
            "",
            "#FFF",
            "112233",
            "#11223344",
            "#GG0000",
            " #112233",
            "#112233 ",
            "#１２３",
            "#12\n456",
        ] {
            for field in 0..4 {
                let mut theme = custom();
                match field {
                    0 => theme.background = bad.into(),
                    1 => theme.surface = bad.into(),
                    2 => theme.text = bad.into(),
                    _ => theme.accent = bad.into(),
                }
                assert!(theme.validate().is_err(), "field {field} accepted {bad:?}");
            }
        }
        assert!(CustomTheme::default().validate().is_err());
    }

    #[test]
    fn json_round_trip_preserves_authored_strings_and_rejects_invalid_exports() {
        let theme = custom();
        let json = export_json(&theme).unwrap();
        assert!(json.len() <= MAX_CUSTOM_JSON_BYTES);
        assert_eq!(import_json(&json).unwrap(), theme);
        assert!(export_json(&CustomTheme::default()).is_err());
    }

    #[test]
    fn json_import_is_strict_and_size_bounded_before_parsing() {
        let json = export_json(&custom()).unwrap();
        for invalid in ["", "{}", "[]", "null", "{", "\"string\""] {
            assert!(import_json(invalid).is_err());
        }
        let unknown = json.replacen('{', "{\"unexpected\": 1,", 1);
        assert!(import_json(&unknown).is_err());
        let duplicate = json.replacen('{', "{\"id\": \"other-id\",", 1);
        assert!(import_json(&duplicate).is_err());
        let mut missing: serde_json::Value = serde_json::from_str(&json).unwrap();
        missing.as_object_mut().unwrap().remove("accent");
        assert!(
            import_json(&missing.to_string())
                .unwrap_err()
                .contains("missing field")
        );
        assert!(import_json(&format!("{json} false")).is_err());
        let at_limit = format!("{json}{}", " ".repeat(MAX_CUSTOM_JSON_BYTES - json.len()));
        assert!(import_json(&at_limit).is_ok());
        assert!(
            import_json(&(at_limit + " "))
                .unwrap_err()
                .contains("exceeds")
        );
    }

    #[test]
    fn resolve_honors_exact_ids_and_ignores_invalid_persisted_custom_data() {
        let valid = custom();
        let mut invalid = valid.clone();
        invalid.accent = "not a color".into();
        assert_eq!(
            resolve(&valid.id, std::slice::from_ref(&valid)),
            valid.validate().unwrap()
        );
        assert_eq!(resolve(&valid.id, &[invalid.clone()]).id, "graphite");
        assert_eq!(
            resolve(&valid.id, &[invalid, valid.clone()]),
            valid.validate().unwrap()
        );
        assert_eq!(resolve("Graphite", &[valid]).id, "graphite");
        for preset in crate::theme_presets::all() {
            let mut collision = custom();
            collision.id = preset.id.clone();
            assert_eq!(resolve(&preset.id, &[collision]), preset);
        }
    }

    #[test]
    fn apply_maps_every_visual_state_for_all_presets_and_both_style_variants() {
        let ctx = egui::Context::default();
        for theme in crate::theme_presets::all() {
            apply(&ctx, &theme, false);
            for variant in [egui::Theme::Dark, egui::Theme::Light] {
                let style = ctx.style_of(variant);
                let v = &style.visuals;
                assert_eq!(v.panel_fill, theme.background);
                assert_eq!(v.window_fill, theme.surface);
                assert_eq!(v.window_stroke.color, theme.border);
                assert_eq!(v.extreme_bg_color, theme.input);
                assert_eq!(v.text_edit_bg_color, Some(theme.input));
                assert_eq!(v.faint_bg_color, theme.elevated);
                assert_eq!(v.code_bg_color, theme.elevated);
                assert_eq!(v.override_text_color, None);
                assert_eq!(v.weak_text_color, Some(theme.muted));
                assert_eq!(v.warn_fg_color, theme.warning);
                assert_eq!(v.error_fg_color, theme.error);
                assert_eq!(v.selection.bg_fill, theme.selected);
                assert_eq!(v.selection.stroke.color, theme.focus);
                assert_eq!(v.text_cursor.stroke.color, theme.focus);
                assert_eq!(v.window_corner_radius, CornerRadius::same(8));
                assert_eq!(v.menu_corner_radius, CornerRadius::same(8));
                assert!((v.disabled_alpha - 0.48).abs() < 0.01);
                assert_eq!(v.widgets.noninteractive.bg_fill, theme.surface);
                assert_eq!(v.widgets.inactive.bg_fill, theme.elevated);
                assert_eq!(v.widgets.hovered.bg_fill, theme.hover);
                assert_eq!(v.widgets.active.bg_fill, theme.selected);
                assert_eq!(v.widgets.open.bg_fill, theme.selected);
                assert_eq!(v.widgets.active.bg_stroke.color, theme.focus);
                for state in [
                    v.widgets.noninteractive,
                    v.widgets.inactive,
                    v.widgets.hovered,
                    v.widgets.active,
                    v.widgets.open,
                ] {
                    assert_eq!(state.corner_radius, CornerRadius::same(8));
                    assert_eq!(state.fg_stroke.color, theme.text);
                    assert_eq!(state.bg_fill, state.weak_bg_fill);
                    assert_eq!(state.bg_fill.a(), 255);
                }
                assert!(!style.spacing.scroll.floating);
                assert!(style.spacing.scroll.foreground_color);
                assert_eq!(style.spacing.scroll.bar_width, 8.0);
                assert!(style.animation_time > 0.0);
                assert!(v.text_cursor.blink);
                for bg in [theme.background, theme.surface, theme.input, theme.elevated] {
                    assert!(contrast(v.hyperlink_color, bg) >= 4.5);
                }
            }
            assert_eq!(
                ctx.style().visuals.dark_mode,
                luminance(theme.background) < luminance(theme.text)
            );
        }
    }

    #[test]
    fn reduced_motion_is_reversible_and_theme_switching_leaves_no_visual_residue() {
        let ctx = egui::Context::default();
        let dark = resolve("solaris-original", &[]);
        let light = resolve("porcelain", &[]);
        apply(&ctx, &dark, true);
        let style = ctx.style();
        assert_eq!(style.animation_time, 0.0);
        assert_eq!(style.scroll_animation.duration.min, 0.0);
        assert_eq!(style.scroll_animation.duration.max, 0.0);
        assert!(!style.visuals.text_cursor.blink);
        assert!(!style.visuals.image_loading_spinners);
        apply(&ctx, &light, false);
        let fresh = egui::Context::default();
        apply(&fresh, &light, false);
        assert_eq!(ctx.style().visuals, fresh.style().visuals);
        assert!(ctx.style().scroll_animation.duration.max > 0.0);
        assert!(ctx.style().animation_time > 0.0);
        assert!(ctx.style().visuals.image_loading_spinners);
    }
}
