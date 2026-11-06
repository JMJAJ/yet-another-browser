//! The four Orrery palettes, values taken verbatim from each theme's
//! `colors.toml` (bg0..bg3, fg, accent_bright, accent_mid, accent_dim).

pub struct Palette {
    pub id: &'static str,
    pub short: &'static str,
    pub dark: bool,
    pub bg0: u32,
    pub bg1: u32,
    pub bg2: u32,
    pub bg3: u32,
    pub fg: u32,
    pub acc: u32,
    pub mid: u32,
    pub dim: u32,
}

pub const THEMES: [Palette; 4] = [
    Palette { id: "eclipse", short: "eclipse", dark: true, bg0: 0x0a0a0a, bg1: 0x141414, bg2: 0x1e1e1e, bg3: 0x282828, fg: 0xe8e8e8, acc: 0xffffff, mid: 0xa0a0a0, dim: 0x606060 },
    Palette { id: "zenith", short: "zenith", dark: false, bg0: 0xf4f4f4, bg1: 0xebebeb, bg2: 0xe2e2e2, bg3: 0xd4d4d4, fg: 0x141414, acc: 0x000000, mid: 0x5e5e5e, dim: 0x7a7a7a },
    Palette { id: "catppuccin-mocha", short: "mocha", dark: true, bg0: 0x11111b, bg1: 0x181825, bg2: 0x1e1e2e, bg3: 0x313244, fg: 0xcdd6f4, acc: 0xb4befe, mid: 0xa6adc8, dim: 0x7f849c },
    Palette { id: "cassini", short: "cassini", dark: true, bg0: 0x110b0b, bg1: 0x181010, bg2: 0x211717, bg3: 0x362826, fg: 0xe4ebe8, acc: 0x9fd8ce, mid: 0x94a29e, dim: 0x676e6b },
];

pub fn index_of(id: &str) -> usize {
    THEMES.iter().position(|t| t.id == id || t.short == id).unwrap_or(0)
}

pub fn color(rgb: u32) -> slint::Color {
    slint::Color::from_rgb_u8((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}
