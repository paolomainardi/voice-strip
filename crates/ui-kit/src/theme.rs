//! Palette of both views, straight from the design sources.

use crate::painter::Color;

pub mod standard {
    use super::Color;

    pub const FRAME: Color = Color::hex(0x2A1F17);
    pub const FACEPLATE: Color = Color::hex(0xEAE2CE);
    pub const INK: Color = Color::hex(0x2B2620);
    pub const INK_SOFT: Color = Color::hex(0x5E5548);
    pub const SCREW: Color = Color::hex(0xB7AC93);
    pub const VU_FACE: Color = Color::hex(0xF3E7C4);
    pub const RED: Color = Color::hex(0xB4372A);
    pub const AMBER: Color = Color::hex(0xF0A23B);
    pub const LAMP_OFF: Color = Color::hex(0x6B5A3E);
    pub const LED_OFF: Color = Color::hex(0x4A4238);
    pub const LED_LOW: Color = Color::hex(0xE8C25A);
    pub const LED_MID: Color = Color::hex(0xF0A23B);
    pub const LED_HIGH: Color = Color::hex(0xD2553A);
    pub const PEAK_LIT: Color = Color::hex(0xD2553A);
    pub const PEAK_LIT_BRIGHT: Color = Color::hex(0xFFD7C9);
    pub const BUTTON: Color = Color::hex(0xD9CFB6);
    pub const BUTTON_BORDER: Color = Color::hex(0xA89C80);
    pub const SWITCH_BODY: Color = Color::hex(0xC9BEA3);
    pub const SWITCH_PIVOT: Color = Color::hex(0x8C8270);
    pub const MONITOR_LIT: Color = Color::hex(0x7FB3D9);
    pub const KNOB_OUTER: Color = Color::hex(0x2B2620);
    pub const KNOB_INNER: Color = Color::hex(0x3A332B);
    pub const KNOB_RING: Color = Color::hex(0x4A4238);
    pub const RULE: Color = Color::hex(0xC9BEA3);
    pub const LINK_HOVER: Color = Color::hex(0x8A3A22);
}

pub mod advanced {
    use super::Color;

    pub const BG: Color = Color::hex(0x0E1013);
    pub const PANEL: Color = Color::hex(0x161A1F);
    pub const PANEL_OFF_BORDER: Color = Color::hex(0x1F242B);
    pub const CONTROL: Color = Color::hex(0x1D2229);
    pub const BORDER: Color = Color::hex(0x2A3038);
    pub const TEXT: Color = Color::hex(0xE9ECEF);
    pub const TEXT_SOFT: Color = Color::hex(0xC9D1D9);
    pub const MUTED: Color = Color::hex(0xA0A8B3);
    pub const DIM: Color = Color::hex(0x7D8692);
    pub const ACCENT: Color = Color::hex(0xF0A23B);
    pub const ACCENT_HOVER: Color = Color::hex(0xF7C47A);
    pub const ACCENT_INK: Color = Color::hex(0x17120A);
    pub const ACCENT_BG: Color = Color::hex(0x2A2418);
    pub const PHONES: Color = Color::hex(0x6BB0F2);
    pub const PHONES_BG: Color = Color::hex(0x16283A);
    pub const PHONES_SWITCH_BG: Color = Color::hex(0x1E3A55);
    pub const PATH_BG: Color = Color::hex(0x11161C);
    pub const PATH_BORDER: Color = Color::hex(0x22303F);
    pub const GRID: Color = Color::hex(0x232830);
    pub const VOICE_BAND: Color = Color::hex(0x2A2418);
    pub const VOICE_BAND_BORDER: Color = Color::hex(0x4A3A1E);
    /// Mute engaged: red that reads on the dark theme, with light text.
    pub const MUTE_ON: Color = Color::hex(0xD2553A);
}

pub const TRANSPARENT: Color = Color::rgba(0.0, 0.0, 0.0, 0.0);
