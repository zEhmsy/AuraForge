//! Write-only output to the ASUS Aura USB controller (`0B05:19AF`) that drives
//! the motherboard's addressable RGB headers.
//!
//! Commands follow OpenRGB's `AsusAuraUSBController` / `AsusAuraMainboardController`.
//! Only volatile direct-mode frames are sent: the commit command (`EC 3F`) that
//! saves to the controller's flash is never used, and the config table is not
//! requested because its reply is an input report. The fan splitter mirrors one
//! LED stream to every fan, so the whole header is painted a single color.

use crate::color::Rgb;
use crate::hid::HidWriter;

const VENDOR_ID: u16 = 0x0B05;
const PRODUCT_ID: u16 = 0x19AF;
const USAGE_PAGE: u16 = 0xFF72;
const USAGE: u16 = 0x00A1;

pub const REPORT_LEN: usize = 65;
const REPORT_ID: u8 = 0xEC;
const CMD_SET_GEN1: u8 = 0x52;
const CMD_EFFECT_MODE: u8 = 0x35;
const CMD_DIRECT: u8 = 0x40;
const MODE_DIRECT: u8 = 0xFF;
/// Set on the last frame of a direct update to latch the colors.
const DIRECT_APPLY: u8 = 0x80;
const LEDS_PER_PACKET: usize = 20;

#[derive(Clone, Copy, Debug)]
pub struct FanConfig {
    /// 0-based addressable header.
    pub header: u8,
    /// LEDs painted per update; anything past the real strip length is ignored.
    pub leds: usize,
}

impl FanConfig {
    fn direct_channel(&self) -> u8 {
        self.header
    }

    /// Effect channel 0 is the motherboard's own LEDs, headers follow.
    fn effect_channel(&self) -> u8 {
        self.header + 1
    }
}

fn report(bytes: &[u8]) -> [u8; REPORT_LEN] {
    let mut report = [0u8; REPORT_LEN];
    report[0] = REPORT_ID;
    report[1..=bytes.len()].copy_from_slice(bytes);
    report
}

/// Switches the header to direct mode; nothing is persisted.
pub fn init_reports(config: &FanConfig) -> [[u8; REPORT_LEN]; 2] {
    [
        report(&[CMD_SET_GEN1, 0x53, 0x00, 0x01]),
        report(&[CMD_EFFECT_MODE, config.effect_channel(), 0x00, 0x00, MODE_DIRECT]),
    ]
}

/// Direct frames painting the first LEDs of the header with `colors`.
pub fn direct_reports(config: &FanConfig, colors: &[Rgb]) -> Vec<[u8; REPORT_LEN]> {
    let total = colors.len().max(1);
    (0..total)
        .step_by(LEDS_PER_PACKET)
        .map(|offset| {
            let count = LEDS_PER_PACKET.min(total - offset);
            let last = offset + count == total;
            let mut r = report(&[
                CMD_DIRECT,
                config.direct_channel() | if last { DIRECT_APPLY } else { 0 },
                offset as u8,
                count as u8,
            ]);
            for (led, c) in r[5..5 + 3 * count].chunks_exact_mut(3).zip(&colors[offset..]) {
                led.copy_from_slice(&[c.r, c.g, c.b]);
            }
            r
        })
        .collect()
}

pub struct Fans {
    device: HidWriter,
    config: FanConfig,
}

impl Fans {
    pub fn open(config: FanConfig) -> Result<Self, String> {
        let device = HidWriter::open("ASUS Aura controller", REPORT_LEN, |d| {
            d.vendor_id() == VENDOR_ID
                && d.product_id() == PRODUCT_ID
                && d.usage_page() == USAGE_PAGE
                && d.usage() == USAGE
        })?;
        for r in init_reports(&config) {
            device.write(&r).map_err(|e| format!("Aura init failed: {}", e.message()))?;
        }
        Ok(Fans { device, config })
    }

    /// One color per LED, `config.leds` of them.
    pub fn show(&self, colors: &[Rgb]) -> Result<(), String> {
        for r in direct_reports(&self.config, colors) {
            self.device.write(&r).map_err(|e| e.message())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_frames_split_and_latch_on_last() {
        let config = FanConfig { header: 0, leds: 45 };
        let frames = direct_reports(&config, &[Rgb { r: 1, g: 2, b: 3 }; 45]);
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[0][..5], [0xEC, 0x40, 0x00, 0, 20]);
        assert_eq!(frames[1][..5], [0xEC, 0x40, 0x00, 20, 20]);
        assert_eq!(frames[2][..5], [0xEC, 0x40, 0x80, 40, 5]);
        assert_eq!(frames[2][5..20], [1, 2, 3, 1, 2, 3, 1, 2, 3, 1, 2, 3, 1, 2, 3]);
        assert!(frames[2][20..].iter().all(|&b| b == 0));
    }

    #[test]
    fn init_selects_direct_mode_on_header_channel() {
        let [gen1, mode] = init_reports(&FanConfig { header: 0, leds: 1 });
        assert_eq!(gen1[..5], [0xEC, 0x52, 0x53, 0x00, 0x01]);
        assert_eq!(mode[..6], [0xEC, 0x35, 0x01, 0x00, 0x00, 0xFF]);
    }
}
