//! Built-in palettes and the serializable, independent light/dark preference.
//! Upstream colors and adaptations are recorded in assets/themes/SOURCES.md.
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaletteId {
    Paper,
    Ink,
    NordSnow,
    Nord,
    TokyoDay,
    TokyoNight,
    CatppuccinLatte,
    CatppuccinFrappe,
    CatppuccinMacchiato,
    CatppuccinMocha,
}

pub const ALL: [PaletteId; 10] = [
    PaletteId::Paper,
    PaletteId::NordSnow,
    PaletteId::TokyoDay,
    PaletteId::CatppuccinLatte,
    PaletteId::Ink,
    PaletteId::Nord,
    PaletteId::TokyoNight,
    PaletteId::CatppuccinFrappe,
    PaletteId::CatppuccinMacchiato,
    PaletteId::CatppuccinMocha,
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ThemePreferences {
    pub mode: Appearance,
    pub light: PaletteId,
    pub dark: PaletteId,
}

impl Default for ThemePreferences {
    fn default() -> Self {
        Self {
            mode: Appearance::System,
            light: PaletteId::Paper,
            dark: PaletteId::Ink,
        }
    }
}

impl ThemePreferences {
    pub fn resolve(&self, system_dark: bool) -> PaletteId {
        match self.mode {
            Appearance::Dark => self.dark,
            Appearance::Light => self.light,
            Appearance::System => {
                if system_dark {
                    self.dark
                } else {
                    self.light
                }
            }
        }
    }

    pub fn select(&mut self, palette: PaletteId) {
        if palette.is_dark() {
            self.dark = palette;
        } else {
            self.light = palette;
        }
    }

    pub fn valid(&self) -> bool {
        !self.light.is_dark() && self.dark.is_dark()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub canvas: u32,
    pub surface: u32,
    pub inset: u32,
    pub text: u32,
    pub muted: u32,
    pub subtle: u32,
    pub border: u32,
    pub control: u32,
    pub accent: u32,
    pub on_accent: u32,
    pub success: u32,
    pub warning: u32,
    pub danger: u32,
    pub selection: u32,
}

macro_rules! palette {
    ($canvas:expr,$surface:expr,$inset:expr,$text:expr,$muted:expr,$subtle:expr,$border:expr,$control:expr,$accent:expr,$on:expr,$success:expr,$warning:expr,$danger:expr,$selection:expr) => {
        Palette {
            canvas: $canvas,
            surface: $surface,
            inset: $inset,
            text: $text,
            muted: $muted,
            subtle: $subtle,
            border: $border,
            control: $control,
            accent: $accent,
            on_accent: $on,
            success: $success,
            warning: $warning,
            danger: $danger,
            selection: $selection,
        }
    };
}

impl PaletteId {
    pub fn name(self) -> &'static str {
        match self {
            Self::Paper => "Paper",
            Self::Ink => "Ink",
            Self::NordSnow => "Nord Snow",
            Self::Nord => "Nord",
            Self::TokyoDay => "Tokyo Day",
            Self::TokyoNight => "Tokyo Night",
            Self::CatppuccinLatte => "Catppuccin Latte",
            Self::CatppuccinFrappe => "Catppuccin Frappé",
            Self::CatppuccinMacchiato => "Catppuccin Macchiato",
            Self::CatppuccinMocha => "Catppuccin Mocha",
        }
    }

    pub fn is_dark(self) -> bool {
        !matches!(
            self,
            Self::Paper | Self::NordSnow | Self::TokyoDay | Self::CatppuccinLatte
        )
    }

    pub fn colors(self) -> Palette {
        match self {
            Self::Paper => palette!(
                0xf6f7f9, 0xffffff, 0xeff1f5, 0x202632, 0x626b7b, 0x7e8795, 0xe1e5eb, 0xa2aaba,
                0x3968d7, 0xffffff, 0x297749, 0x966111, 0xc2394c, 0xdde7fd
            ),
            Self::Ink => palette!(
                0x16191f, 0x20242d, 0x1a1e26, 0xe7eaf0, 0xa7afbf, 0x8590a2, 0x333b49, 0x647187,
                0x8baffb, 0x16213a, 0x93cfa4, 0xe3bc77, 0xee9aa8, 0x334769
            ),
            Self::NordSnow => palette!(
                0xeceff4, 0xf8f9fb, 0xe5e9f0, 0x2e3440, 0x4c566a, 0x65738b, 0xd8dee9, 0x8393aa,
                0x5e81ac, 0xffffff, 0x4f7651, 0x8e6937, 0xa94452, 0xd8e4f0
            ),
            Self::Nord => palette!(
                0x2e3440, 0x3b4252, 0x323946, 0xeceff4, 0xd8dee9, 0xa5b1c4, 0x4c566a, 0x6c7a94,
                0x88c0d0, 0x2e3440, 0xa3be8c, 0xebcb8b, 0xbf616a, 0x4c566a
            ),
            Self::TokyoDay => palette!(
                0xe1e2e7, 0xebecef, 0xd5d8e2, 0x343b58, 0x596078, 0x737b96, 0xc4c8da, 0x8990b3,
                0x2e7de9, 0xffffff, 0x587539, 0x8c6c3e, 0xc62454, 0xb7c1e3
            ),
            Self::TokyoNight => palette!(
                0x1a1b26, 0x24283b, 0x16161e, 0xc0caf5, 0xa9b1d6, 0x7982a9, 0x3b4261, 0x565f89,
                0x7aa2f7, 0x16161e, 0x9ece6a, 0xe0af68, 0xf7768e, 0x283457
            ),
            Self::CatppuccinLatte => palette!(
                0xeff1f5, 0xf7f8fa, 0xe6e9ef, 0x4c4f69, 0x6c6f85, 0x7c7f93, 0xccd0da, 0x9ca0b0,
                0x1e66f5, 0xffffff, 0x388a25, 0x996011, 0xd20f39, 0xdce0e8
            ),
            Self::CatppuccinFrappe => palette!(
                0x292c3c, 0x303446, 0x232634, 0xc6d0f5, 0xa5adce, 0x838ba7, 0x414559, 0x626880,
                0x8caaee, 0x232634, 0xa6d189, 0xe5c890, 0xe78284, 0x414559
            ),
            Self::CatppuccinMacchiato => palette!(
                0x1e2030, 0x24273a, 0x181926, 0xcad3f5, 0xa5adcb, 0x8087a2, 0x363a4f, 0x5b6078,
                0x8aadf4, 0x181926, 0xa6da95, 0xeed49f, 0xed8796, 0x363a4f
            ),
            Self::CatppuccinMocha => palette!(
                0x181825, 0x1e1e2e, 0x11111b, 0xcdd6f4, 0xa6adc8, 0x7f849c, 0x313244, 0x585b70,
                0x89b4fa, 0x11111b, 0xa6e3a1, 0xf9e2af, 0xf38ba8, 0x313244
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_appearance_uses_independently_configured_palettes() {
        let mut prefs = ThemePreferences::default();
        prefs.select(PaletteId::CatppuccinLatte);
        prefs.select(PaletteId::TokyoNight);
        assert_eq!(prefs.resolve(false), PaletteId::CatppuccinLatte);
        assert_eq!(prefs.resolve(true), PaletteId::TokyoNight);
        prefs.mode = Appearance::Light;
        assert_eq!(prefs.resolve(true), PaletteId::CatppuccinLatte);
        prefs.mode = Appearance::Dark;
        assert_eq!(prefs.resolve(false), PaletteId::TokyoNight);
    }

    #[test]
    fn every_palette_roundtrips_and_has_distinct_content_layers() {
        for palette in ALL {
            let serialized = serde_json::to_string(&palette).unwrap();
            assert_eq!(
                serde_json::from_str::<PaletteId>(&serialized).unwrap(),
                palette
            );
            let c = palette.colors();
            assert_ne!(c.canvas, c.surface, "{}", palette.name());
            assert_ne!(c.text, c.surface, "{}", palette.name());
            assert_ne!(c.accent, c.on_accent, "{}", palette.name());
        }
        assert!(ThemePreferences::default().valid());
        assert!(
            !ThemePreferences {
                light: PaletteId::Ink,
                ..Default::default()
            }
            .valid()
        );
        assert!(serde_json::from_str::<PaletteId>("\"unknown_theme\"").is_err());
    }
}
