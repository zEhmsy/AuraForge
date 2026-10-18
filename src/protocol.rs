//! Drevo Tyrfing V2 HID protocol, reverse-engineered from `drevo-gaming-srv.exe`.
//!
//! Every output report is 32 bytes: `[report id][0xBE][command][0x00][payload...]`.
//! The lighting commands carry no checksum.

use crate::color::Rgb;

pub const VENDOR_ID: u16 = 0x0416;
pub const PRODUCT_ID: u16 = 0xA0F8;

/// The command channel is collection `MI_01&COL06` (generic desktop page, usage 0).
pub const INTERFACE: i32 = 1;
pub const USAGE_PAGE: u16 = 0x0001;
pub const USAGE: u16 = 0x0000;

pub const REPORT_ID: u8 = 0x06;
pub const REPORT_LEN: usize = 32;

const MAGIC: u8 = 0xBE;
const CMD_SET_LIGHT_EFFECT: u8 = 0x15;
const CMD_SET_KEY_COLOR: u8 = 0x19;

/// `KB_MODE_STATIC >> 16` in `wkb/kb_def.py`.
pub const MODE_STATIC: u8 = 1;
/// `KB_MODE_CUSTOMIZE >> 16`; per-key colors apply on top of the current effect.
pub const MODE_CUSTOMIZE: u8 = 14;
pub const KEYS_PER_REPORT: usize = 5;
/// Firmware animations, `KB_MODE_* >> 16` in `wkb/kb_def.py`.
pub const THEMES: [(&str, u8); 17] = [
    ("static", 1),
    ("breath", 2),
    ("streamer", 3),
    ("rain", 4),
    ("horse-race", 5),
    ("stars", 6),
    ("reactive", 7),
    ("waterdrop", 8),
    ("cross", 9),
    ("ripple", 10),
    ("aurora", 11),
    ("bomb", 12),
    ("heatmap", 13),
    ("spotlight", 15),
    ("radar", 16),
    ("snaking", 17),
    ("convolution", 18),
];
/// Palette entry flags (4th byte), `KB_F_HUE` / `KB_F_RANDOM | KB_F_REPICK_FG`.
pub const COLOR_HUE: u8 = 1;
pub const COLOR_RANDOM: u8 = 4 | 16;
/// `KB_BRIGHT_MAX` in `wkb/kb_def.py`.
pub const BRIGHTNESS_MAX: u8 = 100;

#[derive(Clone, Copy, Debug)]
pub struct EffectParams {
    pub device_id: u8,
    pub profile_id: u8,
    pub mode: u8,
    pub speed: u8,
    pub brightness: u8,
    pub direction: u8,
    /// Flags of palette entry 0, e.g. `COLOR_HUE` for a rainbow.
    pub color_flags: u8,
}

/// Builds a `KM_IO_SET_LIGHT_EFFECT` report (builder `FUN_14001eb60`).
///
/// Layout: 4 device, 5 profile, 6 mode, 7 speed, 8 brightness, 9 direction,
/// 12..28 palette of four `[R, G, B, X]` entries. Static mode uses slot 0.
pub fn light_effect(params: &EffectParams, color: Rgb) -> [u8; REPORT_LEN] {
    let mut report = [0u8; REPORT_LEN];
    report[0] = REPORT_ID;
    report[1] = MAGIC;
    report[2] = CMD_SET_LIGHT_EFFECT;
    report[4] = params.device_id;
    report[5] = params.profile_id;
    report[6] = params.mode;
    report[7] = params.speed;
    report[8] = params.brightness;
    report[9] = params.direction;
    report[12] = color.r;
    report[13] = color.g;
    report[14] = color.b;
    report[15] = params.color_flags;
    report
}

/// Builds a `KM_IO_SET_KEY_COLOR` report (builder `FUN_14001eee0`).
///
/// Layout: 4 device, 5 profile, 6 mode, 7..12 up to five HID key codes,
/// 12..32 five `[R, G, B, 0]` entries. Unused slots stay zero.
pub fn key_colors(params: &EffectParams, keys: &[(u8, Rgb)]) -> [u8; REPORT_LEN] {
    assert!(keys.len() <= KEYS_PER_REPORT);
    let mut report = [0u8; REPORT_LEN];
    report[0] = REPORT_ID;
    report[1] = MAGIC;
    report[2] = CMD_SET_KEY_COLOR;
    report[4] = params.device_id;
    report[5] = params.profile_id;
    report[6] = MODE_CUSTOMIZE;
    for (i, &(code, color)) in keys.iter().enumerate() {
        report[7 + i] = code;
        report[12 + i * 4..15 + i * 4].copy_from_slice(&[color.r, color.g, color.b]);
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn light_effect_layout() {
        let params = EffectParams {
            device_id: 1,
            profile_id: 2,
            mode: MODE_STATIC,
            speed: 3,
            brightness: 100,
            direction: 0,
            color_flags: 0,
        };
        let report = light_effect(&params, Rgb { r: 0x11, g: 0x22, b: 0x33 });
        assert_eq!(&report[..10], &[0x06, 0xBE, 0x15, 0, 1, 2, 1, 3, 100, 0]);
        assert_eq!(&report[12..16], &[0x11, 0x22, 0x33, 0]);
        assert!(report[16..].iter().all(|&b| b == 0));
    }

    #[test]
    fn key_colors_matches_verified_packet() {
        let params = EffectParams {
            device_id: 1,
            profile_id: 1,
            mode: MODE_STATIC,
            speed: 0,
            brightness: 100,
            direction: 0,
            color_flags: 0,
        };
        // Sent by hand on 2026-10-18: turned Esc red and left other keys alone.
        let report = key_colors(&params, &[(0x29, Rgb { r: 0xFF, g: 0, b: 0 })]);
        let mut expected = [0u8; REPORT_LEN];
        expected[..13].copy_from_slice(&[0x06, 0xBE, 0x19, 0, 1, 1, 0x0E, 0x29, 0, 0, 0, 0, 0xFF]);
        assert_eq!(report, expected);
    }
}
