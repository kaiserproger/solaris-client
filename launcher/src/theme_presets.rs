//! Authored Solaris palette catalogue. Ordering and IDs are persistence/API
//! contracts; do not derive colors from an ID, random seed, or catalogue index.

use crate::theme::{Palette, Theme, ThemeCategory, rgb};

struct Preset {
    id: &'static str,
    name: &'static str,
    category: ThemeCategory,
    // background, surface, elevated, input, text, muted, accent (opaque sRGB).
    colors: [u32; 7],
}

macro_rules! preset {
    ($id:literal, $name:literal, $category:ident, $bg:literal, $surface:literal,
     $elevated:literal, $input:literal, $text:literal, $muted:literal, $accent:literal) => {
        Preset {
            id: $id,
            name: $name,
            category: ThemeCategory::$category,
            colors: [$bg, $surface, $elevated, $input, $text, $muted, $accent],
        }
    };
}

#[rustfmt::skip]
const PRESETS: [Preset; 40] = [
    // Neutral darks: warm graphite, violet-black, ink navy and charcoal.
    preset!("graphite", "Graphite", Dark, 0x181817, 0x222220, 0x2B2B28, 0x121211, 0xF3F1EC, 0xB7B4AA, 0xD8B88D),
    preset!("obsidian", "Obsidian", Dark, 0x0C0D10, 0x14161B, 0x1E2128, 0x08090C, 0xF2F3F7, 0xA6ACBA, 0xB8A1ED),
    preset!("midnight", "Midnight", Dark, 0x0D1424, 0x151F33, 0x1D2B43, 0x09101E, 0xEEF3FC, 0xA8B8D0, 0x7FA9F2),
    preset!("slate", "Slate", Dark, 0x1A212B, 0x222C38, 0x2D3948, 0x151C25, 0xEDF2F8, 0xB2BFCE, 0x91B6D5),
    preset!("carbon", "Carbon", Dark, 0x161616, 0x202020, 0x2B2B2B, 0x101010, 0xF5F4F0, 0xB3B1AB, 0xD9BC83),
    preset!("onyx", "Onyx", Dark, 0x101315, 0x181E21, 0x222B2E, 0x0B1012, 0xEFF5F4, 0xA6B9B9, 0x7BC4C3),
    preset!("ash", "Ash", Dark, 0x242426, 0x2B2B2E, 0x343437, 0x1E1E21, 0xF5F2F4, 0xBAB5BE, 0xC1ADB6),
    preset!("deep-space", "Deep Space", Dark, 0x090E19, 0x101827, 0x192338, 0x060B14, 0xEDF2FF, 0xA4B3D0, 0x9DA9FC),
    preset!("soft-black", "Soft Black", Dark, 0x191817, 0x22201E, 0x2D2A27, 0x131211, 0xF5F1EC, 0xB8AFA5, 0xD6B795),
    preset!("storm", "Storm", Dark, 0x1B2029, 0x252D39, 0x303C4B, 0x151A23, 0xF0F4F9, 0xB7C5D7, 0xA4BADD),
    // Saturated darks retain colored surfaces, not just a different button.
    preset!("forest", "Forest", Colorful, 0x101D18, 0x172A22, 0x21372C, 0x0B1712, 0xEDF7EE, 0xB0CBB6, 0x8CC69A),
    preset!("ocean", "Ocean", Colorful, 0x0D1C27, 0x142A39, 0x1C3849, 0x08151F, 0xEDF7FB, 0xAACAD9, 0x70C5E4),
    preset!("lavender", "Lavender", Colorful, 0x1C1729, 0x282038, 0x352B47, 0x15101F, 0xF5EFFD, 0xC8B8DD, 0xC8A7F2),
    preset!("burgundy", "Burgundy", Colorful, 0x25151D, 0x331E29, 0x442A36, 0x1C0F16, 0xFFF0F5, 0xDAB7C5, 0xEC9AB9),
    preset!("espresso", "Espresso", Colorful, 0x211913, 0x2E231B, 0x3B3026, 0x19120D, 0xFCF3E7, 0xD0BDAB, 0xD8AC7A),
    preset!("nord", "Nord", Colorful, 0x2E3440, 0x343C4A, 0x3B4252, 0x282E3A, 0xECEFF4, 0xCAD3E0, 0x88C0D0),
    preset!("sakura-night", "Sakura Night", Colorful, 0x211927, 0x2E2335, 0x3C2E43, 0x19121F, 0xFFF0F9, 0xD8BED3, 0xF1ACD0),
    preset!("emerald", "Emerald", Colorful, 0x0B201B, 0x123027, 0x1A4034, 0x071812, 0xECFFF5, 0xAAD3BE, 0x64D6A4),
    preset!("deep-violet", "Deep Violet", Colorful, 0x17112C, 0x231A40, 0x302451, 0x100B21, 0xF3EEFF, 0xC6B5E3, 0xB99AF6),
    preset!("terracotta", "Terracotta", Colorful, 0x291B17, 0x382720, 0x49342B, 0x211410, 0xFFF1E7, 0xDFC0AF, 0xEBA783),
    // Light collections use deliberately dark, readable accent inks.
    preset!("porcelain", "Porcelain", Light, 0xF4F5F7, 0xFFFFFF, 0xEBEEF3, 0xE8ECF1, 0x202734, 0x525D6C, 0x365F99),
    preset!("snow", "Snow", Light, 0xF8FAFC, 0xFFFFFF, 0xF0F4F8, 0xE9EFF6, 0x1C2938, 0x4C5B70, 0x28628F),
    preset!("ivory", "Ivory", Light, 0xF7F3E9, 0xFFFCF4, 0xEFE9DA, 0xEBE4D4, 0x302C23, 0x665D4D, 0x786030),
    preset!("sand", "Sand", Light, 0xF0E8DB, 0xFBF6ED, 0xEAE0CF, 0xE5DAC6, 0x332C23, 0x665745, 0x85552F),
    preset!("cloud", "Cloud", Light, 0xF0F2F6, 0xFAFBFE, 0xE8ECF3, 0xE4E9F1, 0x263044, 0x535F76, 0x526498),
    preset!("sage", "Sage", Light, 0xEFF3EB, 0xF9FCF6, 0xE5EDDF, 0xE2EADA, 0x293329, 0x52614D, 0x41663D),
    preset!("rosewater", "Rosewater", Light, 0xFAF0F2, 0xFFFAFB, 0xF3E5EA, 0xEFDFE5, 0x3B2932, 0x70515F, 0x96516E),
    preset!("lavender-mist", "Lavender Mist", Light, 0xF3F0FA, 0xFCFAFF, 0xEAE4F3, 0xE5DEEF, 0x302B42, 0x625670, 0x74569A),
    preset!("glacier", "Glacier", Light, 0xECF4F7, 0xF8FDFF, 0xDFEEF3, 0xDBEAF0, 0x21343F, 0x4B626F, 0x2E687D),
    preset!("paper", "Paper", Light, 0xF5F2EC, 0xFFFCF6, 0xECE7DE, 0xE7E1D5, 0x302F2B, 0x625E55, 0x67633D),
    // Solaris amber and fruit/mineral palettes, both light and dark.
    preset!("solaris-original", "Solaris Original", Colorful, 0x0E0906, 0x120C08, 0x22170F, 0x0A0705, 0xFBE9CF, 0xC3A782, 0xFFA32E),
    preset!("sunset", "Sunset", Colorful, 0x281B2B, 0x382535, 0x493344, 0x1F1421, 0xFFF1ED, 0xDEC0CF, 0xF2AA88),
    preset!("matcha", "Matcha", Colorful, 0xEDF2E2, 0xF8FBEF, 0xE3EBD4, 0xDFE8CD, 0x2D3825, 0x536143, 0x53743B),
    preset!("arctic", "Arctic", Colorful, 0x102129, 0x17313C, 0x20414C, 0x0B1921, 0xEDFBFF, 0xAECFD9, 0x83D2DD),
    preset!("peach", "Peach", Colorful, 0xFFF1E8, 0xFFFAF5, 0xF7E4D5, 0xF0DDCD, 0x422F25, 0x795945, 0x9D563A),
    preset!("mocha", "Mocha", Colorful, 0xEDE2D5, 0xF8F0E7, 0xE7D9C9, 0xE4D6C7, 0x342A25, 0x665347, 0x86563E),
    preset!("cherry", "Cherry", Colorful, 0x26121C, 0x361C29, 0x482637, 0x1D0B14, 0xFFF0F5, 0xE0B6C6, 0xF28BAE),
    preset!("mint", "Mint", Colorful, 0xECF7F1, 0xF8FFFB, 0xDFF0E6, 0xD9ECDD, 0x23382E, 0x486352, 0x2F7157),
    preset!("blueberry", "Blueberry", Colorful, 0x171B32, 0x222843, 0x303653, 0x101429, 0xF0F1FF, 0xBCBFDF, 0xA9ADF6),
    preset!("copper", "Copper", Colorful, 0x251B17, 0x33261F, 0x433329, 0x1C130F, 0xFFF2E7, 0xD7BAA4, 0xDFA775),
];

/// All 40 presets in the product's fixed display order. Every call returns
/// owned themes: callers can preview/edit one without mutating the catalogue.
pub fn all() -> Vec<Theme> {
    PRESETS
        .iter()
        .map(|preset| {
            let [background, surface, elevated, input, text, muted, accent] =
                preset.colors.map(rgb);
            Theme::from_palette(
                preset.id,
                preset.name,
                preset.category,
                Palette {
                    background,
                    surface,
                    elevated,
                    input,
                    text,
                    muted,
                    accent,
                },
            )
        })
        .collect()
}

pub(crate) fn contains_id(id: &str) -> bool {
    PRESETS.iter().any(|preset| preset.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::contrast;
    use std::collections::HashSet;

    #[test]
    fn default_graphite_has_the_requested_warm_accent() {
        let theme = &all()[0];
        assert!(
            theme.accent.r() > theme.accent.b(),
            "Default accent must be warm, not blue"
        );
    }

    #[test]
    fn catalogue_has_exact_names_order_categories_and_stable_ids() {
        let expected = [
            "Graphite",
            "Obsidian",
            "Midnight",
            "Slate",
            "Carbon",
            "Onyx",
            "Ash",
            "Deep Space",
            "Soft Black",
            "Storm",
            "Forest",
            "Ocean",
            "Lavender",
            "Burgundy",
            "Espresso",
            "Nord",
            "Sakura Night",
            "Emerald",
            "Deep Violet",
            "Terracotta",
            "Porcelain",
            "Snow",
            "Ivory",
            "Sand",
            "Cloud",
            "Sage",
            "Rosewater",
            "Lavender Mist",
            "Glacier",
            "Paper",
            "Solaris Original",
            "Sunset",
            "Matcha",
            "Arctic",
            "Peach",
            "Mocha",
            "Cherry",
            "Mint",
            "Blueberry",
            "Copper",
        ];
        let themes = all();
        assert_eq!(themes.len(), 40);
        let mut ids = HashSet::new();
        let mut palettes = HashSet::new();
        for (index, (theme, expected)) in themes.iter().zip(expected).enumerate() {
            assert_eq!(theme.name, expected);
            assert_eq!(theme.id, expected.to_ascii_lowercase().replace(' ', "-"));
            assert!(ids.insert(theme.id.clone()), "duplicate ID: {}", theme.id);
            assert!(
                palettes.insert((theme.background, theme.surface, theme.accent)),
                "duplicate palette: {}",
                theme.id
            );
            assert_eq!(
                theme.category,
                match index {
                    0..=9 => ThemeCategory::Dark,
                    20..=29 => ThemeCategory::Light,
                    _ => ThemeCategory::Colorful,
                }
            );
        }
    }

    #[test]
    fn every_preset_passes_all_required_foreground_contrast_pairs() {
        for theme in all() {
            for (role, foreground) in [("text", theme.text), ("muted", theme.muted)] {
                for (surface_name, background) in [
                    ("background", theme.background),
                    ("surface", theme.surface),
                    ("elevated", theme.elevated),
                    ("input", theme.input),
                    ("hover", theme.hover),
                    ("selected", theme.selected),
                ] {
                    let ratio = contrast(foreground, background);
                    assert!(
                        ratio >= 4.5,
                        "{}: {role} vs {surface_name} = {ratio}",
                        theme.id
                    );
                }
            }
            assert!(
                contrast(theme.accent, theme.on_accent) >= 4.5,
                "{} accent",
                theme.id
            );
            assert!(
                contrast(theme.focus, theme.selected) >= 4.5,
                "{} selection",
                theme.id
            );
            for status in [theme.success, theme.error, theme.warning] {
                for background in [theme.background, theme.surface, theme.input, theme.elevated] {
                    assert!(contrast(status, background) >= 4.5, "{} status", theme.id);
                }
            }
        }
    }

    #[test]
    fn semantic_tokens_are_opaque_and_catalogue_results_are_independent() {
        for theme in all() {
            for color in [
                theme.background,
                theme.surface,
                theme.elevated,
                theme.text,
                theme.muted,
                theme.border,
                theme.accent,
                theme.on_accent,
                theme.hover,
                theme.selected,
                theme.focus,
                theme.disabled,
                theme.input,
                theme.success,
                theme.error,
                theme.warning,
            ] {
                assert_eq!(color.a(), 255, "{} has translucent token", theme.id);
            }
        }
        let mut edited = all();
        edited[0].name = "Changed preview".into();
        edited[0].accent = rgb(0xFF0000);
        assert_eq!(all()[0].name, "Graphite");
        assert_eq!(all()[0].accent, rgb(0xD8B88D));
    }
}
